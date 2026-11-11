mod api;
mod codex;
mod object_summary;
mod refresh;
mod upstream;
pub(crate) use object_summary::upstream_summary;
pub(crate) use upstream::balance::{BalanceProvider, target as balance_target};
#[cfg(test)]
pub(crate) use upstream::upstream_exhausted;
pub(crate) use upstream::{
    BudgetReset, UpstreamBudget, openrouter_scope, route_budget_supported, routing_quota_status,
};
pub(crate) mod sources;
pub(crate) use refresh::refresh;

use crate::{ApiResult, AppState, json_value};
use api::ReadError;
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use local_agent_core::{
    Provider, Store,
    billing::AccountSnapshot,
    providers::{config::NativeConfig, files, fingerprint, upstream_accounts::AccountRef},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    path::{Path as FsPath, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use zeroize::Zeroizing;

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Account {
    #[serde(skip)]
    source_kind: Option<&'static str>,
    #[serde(skip)]
    read_scope: Option<String>,
    id: String,
    name: String,
    account_type: String,
    scope: String,
    provider_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    upstream_account: Option<AccountRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    routing_quota: Option<&'static str>,
    declared_for: Vec<String>,
    source_revision: String,
    readable: bool,
    official_url: Option<String>,
    state: String,
    error: Option<String>,
    snapshot: Option<AccountSnapshot>,
}
struct Cached {
    revision: String,
    source_epoch: Option<u64>,
    current_read_scope: Option<String>,
    snapshot: AccountSnapshot,
    error: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct RefreshKey {
    account: String,
    revision: String,
    epoch: Option<u64>,
    read_scope: Option<String>,
}
impl RefreshKey {
    fn new(account: &Account, epoch: Option<u64>) -> Self {
        Self {
            account: account.id.clone(),
            revision: account.source_revision.clone(),
            epoch,
            read_scope: account.read_scope.clone(),
        }
    }

    fn same_read(&self, other: &Self) -> bool {
        self == other
            || (self.epoch == other.epoch
                && self.read_scope.is_some()
                && self.read_scope == other.read_scope)
    }
}

fn read_scope(provider: &Provider, kind: &str) -> Option<String> {
    let reference = provider.credential_ref.as_deref()?;
    Some(files::hash(format!("{kind}:{reference}").as_bytes()))
}
pub struct Billing {
    pub(crate) sources: Arc<sources::AccountSources>,
    home: PathBuf,
    directory: PathBuf,
    cache: HashMap<String, Cached>,
    openrouter_admission: HashMap<String, upstream::OpenRouterAdmission>,
    shared_budgets: HashMap<String, upstream::SharedBudget>,
    unavailable_budget_epoch: Option<u64>,
    in_flight: HashMap<RefreshKey, Arc<AtomicBool>>,
    attempted: HashMap<RefreshKey, Instant>,
    failures: HashMap<String, (RefreshKey, ReadError)>,
}
impl Billing {
    #[cfg(test)]
    pub fn new(home: PathBuf, data: &FsPath) -> Self {
        Self::with_sources(
            home,
            data,
            Arc::new(sources::AccountSources::builtin().expect("valid built-in billing sources")),
        )
    }
    pub(crate) fn with_sources(
        home: PathBuf,
        data: &FsPath,
        sources: Arc<sources::AccountSources>,
    ) -> Self {
        Self {
            sources,
            home,
            directory: data.join("billing"),
            cache: HashMap::new(),
            openrouter_admission: HashMap::new(),
            shared_budgets: HashMap::new(),
            unavailable_budget_epoch: None,
            in_flight: HashMap::new(),
            attempted: HashMap::new(),
            failures: HashMap::new(),
        }
    }
    fn catalog(
        &self,
        route_store: Option<&Store>,
        providers: &[Provider],
        configs: &[NativeConfig],
        upstream: Vec<Account>,
    ) -> Vec<Account> {
        let mut accounts = Vec::new();
        let now = chrono::Utc::now();
        for (agent, name, url) in [
            (
                "codex",
                "Codex · 本机订阅登录",
                "https://chatgpt.com/settings/usage?tab=overview",
            ),
            (
                "claude",
                "Claude Code · 额度采集入口",
                "https://claude.ai/settings/usage",
            ),
        ] {
            let native = configs.iter().find(|c| c.agent_id == agent);
            let declared = native.is_some_and(|c| c.state == "ready" && c.mode == "native");
            accounts.push(Account {
                source_kind: Some(agent),
                read_scope: None,
                id: format!("native-{agent}"),
                name: name.into(),
                account_type: "subscription".into(),
                scope: if agent == "codex" {
                    "本机 Codex 官方登录；跨客户端共享账户额度"
                } else {
                    "使用独立入口运行的 Claude 会话；官方未提供身份时不合并账户"
                }
                .into(),
                provider_id: None,
                upstream_account: None,
                routing_quota: None,
                declared_for: if declared { vec![agent.into()] } else { vec![] },
                source_revision: native.and_then(|c| c.revision.clone()).unwrap_or_default(),
                readable: agent == "codex",
                official_url: Some(url.into()),
                state: "not_loaded".into(),
                error: None,
                snapshot: None,
            });
        }
        for p in providers
            .iter()
            .filter(|p| p.configured && p.category != "local")
        {
            let url = official(p);
            let subscription = local_agent_core::providers::subscription::minimax_plan(p);
            let source_kind = api::adapter(p).map(|(kind, _)| kind);
            let declared_for = vec![];
            accounts.push(Account {
                source_kind,
                read_scope: source_kind.and_then(|kind| read_scope(p, kind)),
                id: format!("provider-{}", p.id),
                name: p.name.clone(),
                account_type: if subscription { "subscription" } else { "api" }.into(),
                scope: format!(
                    "已保存连接 · {}；密钥 / 组织范围独立，不汇总未知账户",
                    p.credential_label.as_deref().unwrap_or("未命名凭据")
                ),
                provider_id: Some(p.id.clone()),
                upstream_account: None,
                routing_quota: route_store
                    .and_then(|store| routing_quota_status(Some(self), store, p, now)),
                declared_for,
                source_revision: fingerprint(p),
                readable: source_kind.is_some() && p.credential_ref.is_some(),
                official_url: url.map(str::to_owned),
                state: "not_loaded".into(),
                error: None,
                snapshot: None,
            });
        }
        // A native inline key is used only for a pinned provider origin, on explicit refresh.
        if let Some(c) = configs
            .iter()
            .find(|c| c.agent_id == "claude" && c.mode != "native")
            && let Some(endpoint) = &c.endpoint
        {
            let p = native_provider(endpoint);
            accounts.push(Account {
                source_kind: api::adapter(&p).map(|(kind, _)| kind),
                read_scope: None,
                id: "native-api-claude".into(),
                name: "Claude Code · 当前文件中的 API 账户".into(),
                account_type: "api".into(),
                scope: "~/.claude/settings.json 内的显式 API 凭据；不运行凭据助手，不读取订阅令牌"
                    .into(),
                provider_id: None,
                upstream_account: None,
                routing_quota: None,
                declared_for: vec!["claude".into()],
                source_revision: c.revision.clone().unwrap_or_default(),
                readable: api::adapter(&p).is_some(),
                official_url: official(&p).map(str::to_owned),
                state: "not_loaded".into(),
                error: None,
                snapshot: None,
            });
        }
        accounts.extend(upstream);
        for a in &mut accounts {
            let epoch = a
                .source_kind
                .and_then(|kind| self.sources.lease(kind).ok().map(|lease| lease.epoch));
            if a.source_kind.is_some() && epoch.is_none() {
                a.readable = false;
                a.state = "not_available".into();
                a.error = Some("只读额度插件已停止".into());
                continue;
            }
            if let Some(c) = self.cached(a, epoch) {
                a.snapshot = Some(c.snapshot.clone());
                a.error = c.error.clone();
                let age = chrono::DateTime::parse_from_rfc3339(&c.snapshot.as_of)
                    .map(|d| chrono::Utc::now().signed_duration_since(d).num_seconds())
                    .unwrap_or(i64::MAX);
                a.state = if c.error.is_some() {
                    "refresh_failed"
                } else if (a.read_scope.is_some() && c.current_read_scope.is_none())
                    || age < 0
                    || age > c.snapshot.valid_for_seconds as i64
                {
                    "stale"
                } else {
                    "ready"
                }
                .into();
            } else if a.id == "native-claude"
                && let Ok(Some(s)) = self.sources.collected(&self.directory)
            {
                a.state = if is_stale(&s) {
                    "stale"
                } else if s.windows.is_empty() {
                    "not_available"
                } else {
                    "ready"
                }
                .into();
                a.snapshot = Some(s);
            }
            if a.snapshot.is_none() && !a.readable && a.id != "native-claude" {
                a.state = "unsupported".into();
                a.error = Some(
                    "此连接没有已接入的账户查询接口或独立读取凭据；请在官方账单页面核查。".into(),
                );
            }
            let key = RefreshKey::new(a, epoch);
            if self
                .in_flight
                .iter()
                .any(|(other, active)| key.same_read(other) && active.load(Ordering::Acquire))
            {
                a.state = "refreshing".into();
            } else if let Some((_, reason)) = self
                .failures
                .get(&a.id)
                .filter(|(failed, _)| failed == &key)
            {
                a.state = reason
                    .state()
                    .unwrap_or(if a.snapshot.is_some() {
                        "refresh_failed"
                    } else {
                        "failed"
                    })
                    .into();
                a.error = Some(reason.message().into());
            }
        }
        accounts
    }

    fn cached(&self, account: &Account, epoch: Option<u64>) -> Option<&Cached> {
        let cached = self.cache.get(&account.id)?;
        if cached.revision != account.source_revision || cached.source_epoch != epoch {
            return None;
        }
        Some(cached)
    }
}
fn is_stale(s: &AccountSnapshot) -> bool {
    chrono::DateTime::parse_from_rfc3339(&s.as_of)
        .map(|d| chrono::Utc::now().signed_duration_since(d).num_seconds())
        .map_or(true, |age| age < 0 || age > s.valid_for_seconds as i64)
}
fn native_provider(endpoint: &str) -> Provider {
    Provider {
        id: "native-api-claude".into(),
        name: "原生 API".into(),
        vendor: String::new(),
        endpoint: endpoint.into(),
        protocol: "messages".into(),
        model: String::new(),
        configured: true,
        credential_ref: None,
        credential_label: None,
        models: vec![],
        disabled_models: vec![],
        model_catalog: None,
        revision: 0,
        category: "cloud".into(),
    }
}
fn official(p: &Provider) -> Option<&'static str> {
    match api::adapter(p).map(|a| a.0) {
        Some("deepseek") => Some("https://platform.deepseek.com/usage"),
        Some("openrouter") => Some("https://openrouter.ai/settings/credits"),
        Some("kimi-cn") => Some("https://platform.kimi.com/console/account"),
        Some("kimi-global") => Some("https://platform.kimi.ai/console/account"),
        Some(local_agent_core::providers::subscription::MINIMAX_PLAN) => {
            Some("https://platform.minimax.io/console/plan")
        }
        _ if p.endpoint.trim_end_matches('/') == "https://api.openai.com/v1" => {
            Some("https://platform.openai.com/usage")
        }
        _ if p.endpoint.trim_end_matches('/') == "https://api.anthropic.com" => {
            Some("https://platform.claude.com/settings/billing")
        }
        _ => None,
    }
}
fn err(message: &str) -> (StatusCode, Json<Value>) {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({"code":"account_unavailable","message":message})),
    )
}
fn catalog(state: &AppState) -> Result<Vec<Account>, (StatusCode, Json<Value>)> {
    let configs = state
        .providers
        .lock()
        .map_err(|_| err("配置读取暂不可用"))?
        .status()
        .map_err(|_| err("无法读取原生配置范围"))?
        .configs;
    let store = state.store.lock().map_err(|_| err("账户目录暂不可用"))?;
    let providers = store.providers().map_err(crate::error)?;
    let billing = state.billing.lock().map_err(|_| err("账户缓存暂不可用"))?;
    let upstream = upstream::rows(&store, &providers, &billing).map_err(crate::providers::error)?;
    Ok(billing.catalog(Some(&store), &providers, &configs, upstream))
}
pub async fn list(State(state): State<AppState>) -> ApiResult {
    crate::blocking::query(state.query_capacity.clone(), move || {
        json_value(json!({"accounts":catalog(&state)?,"asOf":chrono::Utc::now().to_rfc3339()}))
    })
    .await
}
fn api_credentials(
    state: &AppState,
    account: &Account,
    home: &FsPath,
) -> Result<(Provider, Zeroizing<Vec<u8>>), ReadError> {
    let (provider, secret) = if account.id == "native-api-claude" {
        let snapshot =
            files::read(&home.join(".claude/settings.json")).map_err(|_| "原生配置不可安全读取")?;
        if snapshot.stamp.digest.as_deref() != Some(&account.source_revision) {
            return Err("原生配置已改变".into());
        }
        let bytes = Zeroizing::new(snapshot.bytes.ok_or("原生配置不存在")?);
        let value: Value = serde_json::from_slice(&bytes).map_err(|_| "原生配置格式不支持")?;
        let endpoint = value["env"]["ANTHROPIC_BASE_URL"]
            .as_str()
            .ok_or("没有显式 API 端点")?;
        let api_key = value["env"]["ANTHROPIC_API_KEY"]
            .as_str()
            .filter(|s| !s.is_empty());
        let auth_token = value["env"]["ANTHROPIC_AUTH_TOKEN"]
            .as_str()
            .filter(|s| !s.is_empty());
        if matches!((api_key, auth_token), (Some(a), Some(b)) if a != b) {
            return Err(
                "原配置存在不同的 API Key 与 Auth Token，无法确认计费凭据；请在 Providers 单独配置"
                    .into(),
            );
        }
        let token = value["env"]["ANTHROPIC_API_KEY"]
            .as_str()
            .filter(|s| !s.is_empty())
            .or_else(|| {
                value["env"]["ANTHROPIC_AUTH_TOKEN"]
                    .as_str()
                    .filter(|s| !s.is_empty())
            })
            .ok_or("没有可直接读取的 API 凭据；请在 Providers 单独配置，凭据助手不会被执行")?;
        (
            native_provider(endpoint),
            Zeroizing::new(token.as_bytes().to_vec()),
        )
    } else {
        let p = {
            let store = state.store.lock().map_err(|_| "服务商目录不可用")?;
            let id = account.provider_id.as_deref().ok_or("服务商已移除")?;
            let provider = store
                .provider_by_id(id)
                .map_err(|_| "服务商目录不可用")?
                .ok_or("服务商已移除")?;
            upstream::target(&store, provider, account)?
        };
        let reference = p.credential_ref.as_deref().ok_or("未配置账户读取凭据")?;
        uuid::Uuid::parse_str(reference).map_err(|_| "钥匙串凭据不可用，请在 Providers 核查")?;
        let secret = state
            .credentials
            .get(reference)
            .map_err(|_| "钥匙串凭据不可用，请在 Providers 核查")?;
        (p, secret)
    };
    local_agent_core::providers::subscription::validate_key(&provider, Some(&secret))
        .map_err(|_| ReadError::SubscriptionKeyRequired)?;
    Ok((provider, secret))
}
async fn read_api(
    state: &AppState,
    account: &Account,
    home: &FsPath,
    sources: &sources::AccountSources,
    expected_epoch: Option<u64>,
) -> Result<AccountSnapshot, ReadError> {
    let state_copy = state.clone();
    let account = account.clone();
    let home = home.to_path_buf();
    let (provider, secret) = crate::blocking::run_with(state.query_capacity.clone(), move || {
        api_credentials(&state_copy, &account, &home)
    })
    .await
    .map_err(|_| "账户凭据读取繁忙，请稍后重试")??;
    let (adapter, url) = api::adapter(&provider)
        .ok_or("只支持已经核对的官方账户接口；自定义网关不发送凭据到推测的账单地址")?;
    let token = std::str::from_utf8(&secret)
        .ok()
        .filter(|s| !s.is_empty() && s.len() <= 8192 && s.bytes().all(|b| b.is_ascii_graphic()))
        .ok_or("凭据格式不支持")?;
    sources.api(adapter, url, token, expected_epoch).await
}

// A separate launch entry consumes documented statusLine JSON; native settings stay untouched.
pub async fn claude_launcher(State(state): State<AppState>) -> ApiResult {
    crate::blocking::management(move || {
        let directory = state
            .billing
            .lock()
            .map_err(|_| err("账户服务不可用"))?
            .directory
            .clone();
        json_value(create_launcher(&directory).map_err(err)?)
    })
    .await
}
fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\"'\"'"))
}
fn create_launcher(directory: &FsPath) -> Result<Value, &'static str> {
    files::private_dir(directory).map_err(|_| "额度目录不可写")?;
    let binary = crate::providers::binary("claude").ok_or("请先安装 Claude Code")?;
    let source = std::env::current_exe().map_err(|_| "采集程序不可用")?;
    let bytes = std::fs::read(source).map_err(|_| "采集程序不可读取")?;
    let helper = directory.join(format!("usage-reader-{}", &files::hash(&bytes)[..24]));
    if let Some(existing) = files::read_executable(&helper)
        .map_err(|_| "采集程序不可安全读取")?
        .bytes
    {
        if existing != bytes {
            return Err("已有采集程序被修改，请核查数据目录");
        }
    } else {
        files::atomic_write(&helper, &bytes, 0o700).map_err(|_| "采集程序未保存")?;
    }
    let settings = directory.join("claude-usage-settings.json");
    let command = format!(
        "{} --claude-quota-stdin {}",
        quote(&helper.to_string_lossy()),
        quote(&directory.to_string_lossy())
    );
    files::atomic_write(
        &settings,
        &serde_json::to_vec(&json!({"statusLine":{"type":"command","command":command}}))
            .map_err(|_| "入口格式错误")?,
        0o600,
    )
    .map_err(|_| "额度入口设置未保存")?;
    let launcher = directory.join("claude-with-usage");
    let script = format!(
        "#!/bin/sh\nexec {} --settings {} \"$@\"\n",
        quote(&binary.to_string_lossy()),
        quote(&settings.to_string_lossy())
    );
    files::atomic_write(&launcher, script.as_bytes(), 0o700).map_err(|_| "额度启动入口未保存")?;
    Ok(json!({"launcherPath":launcher.to_string_lossy(),"settingsPath":settings.to_string_lossy()}))
}
#[derive(Serialize, Deserialize)]
struct Collected {
    snapshot: AccountSnapshot,
}
fn read_collected(directory: &FsPath) -> Result<Option<AccountSnapshot>, &'static str> {
    let bytes = files::read(&directory.join("claude-quota.json"))
        .map_err(|_| "采集快照不可安全读取")?
        .bytes;
    bytes
        .map(|b| {
            serde_json::from_slice::<Collected>(&b)
                .map(|v| v.snapshot)
                .map_err(|_| "采集快照格式不支持")
        })
        .transpose()
}
pub fn capture_stdin(directory: &FsPath) -> Result<(), &'static str> {
    use std::io::Read;
    let mut bytes = Zeroizing::new(Vec::new());
    std::io::stdin()
        .take(128 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "采集输入不可读")?;
    if bytes.len() > 128 * 1024 {
        return Err("采集输入超限");
    }
    println!("{}", capture_bytes(directory, &bytes)?);
    Ok(())
}
fn capture_bytes(directory: &FsPath, bytes: &[u8]) -> Result<String, &'static str> {
    let value: Value = serde_json::from_slice(bytes).map_err(|_| "采集格式无效")?;
    let snapshot = local_agent_core::billing::claude_statusline(&value).unwrap_or_else(|reason| {
        let mut snapshot =
            AccountSnapshot::new("subscription", "Claude Code 官方 statusLine · rate_limits");
        snapshot.notes.push(reason.into());
        snapshot
    });
    // Store only the normalized allowlisted quota fields, never transcript paths or input payload.
    files::private_dir(directory).map_err(|_| "额度目录不可写")?;
    let text = snapshot
        .windows
        .iter()
        .filter_map(|w| w.used_percent.map(|p| format!("{} {p:.0}%", w.label)))
        .collect::<Vec<_>>()
        .join(" · ");
    let bytes = serde_json::to_vec(&Collected { snapshot }).map_err(|_| "采集格式无效")?;
    files::atomic_write(&directory.join("claude-quota.json"), &bytes, 0o600)
        .map_err(|_| "采集快照未保存")?;
    Ok(if text.is_empty() {
        "Claude · 额度未提供".into()
    } else {
        text
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    #[test]
    fn stopping_quota_sources_hides_cached_and_collected_snapshots_without_changing_cache() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().canonicalize().unwrap();
        let mut billing = Billing::new(home.clone(), &home);
        capture_bytes(
            &billing.directory,
            br#"{"rate_limits":{"five_hour":{"used_percentage":10}}}"#,
        )
        .unwrap();
        billing.cache.insert(
            "native-codex".into(),
            Cached {
                source_epoch: Some(billing.sources.lease("codex").unwrap().epoch),
                current_read_scope: None,
                revision: String::new(),
                snapshot: AccountSnapshot::new("subscription", "fixture"),
                error: None,
            },
        );
        assert!(
            billing
                .catalog(None, &[], &[], vec![])
                .iter()
                .all(|a| a.snapshot.is_some())
        );
        billing.sources.stop_all();
        for account in billing.catalog(None, &[], &[], vec![]) {
            assert_eq!(account.state, "not_available");
            assert!(!account.readable);
            assert!(account.snapshot.is_none());
        }
        assert_eq!(billing.cache.len(), 1);
        assert!(billing.directory.join("claude-quota.json").exists());
    }
    #[test]
    fn statusline_snapshot_keeps_only_quota_and_marks_missing_instead_of_reusing_old_account() {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().canonicalize().unwrap().join("billing");
        capture_bytes(&directory,br#"{"apiKey":"secret-sentinel","transcript_path":"private-sentinel","rate_limits":{"five_hour":{"used_percentage":10,"resets_at":2000000000}}}"#).unwrap();
        let bytes = std::fs::read(directory.join("claude-quota.json")).unwrap();
        let text = String::from_utf8(bytes).unwrap();
        assert!(!text.contains("sentinel"));
        assert_eq!(
            std::fs::metadata(directory.join("claude-quota.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert_eq!(
            read_collected(&directory).unwrap().unwrap().windows[0].remaining_percent,
            Some(90.)
        );
        capture_bytes(&directory, br#"{"model":{"display_name":"API"}}"#).unwrap();
        assert!(
            read_collected(&directory)
                .unwrap()
                .unwrap()
                .windows
                .is_empty()
        );
    }
    #[test]
    fn shared_cache_exposes_expiry_errors_and_drops_old_configuration() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().canonicalize().unwrap();
        let mut b = Billing::new(home.clone(), &home);
        let mut s = AccountSnapshot::new("subscription", "fixture");
        s.plan = Some("pro".into());
        b.cache.insert(
            "native-codex".into(),
            Cached {
                source_epoch: Some(b.sources.lease("codex").unwrap().epoch),
                current_read_scope: None,
                revision: String::new(),
                snapshot: s.clone(),
                error: None,
            },
        );
        for _ in 0..2 {
            let rows = b.catalog(None, &[], &[], vec![]);
            assert_eq!(rows[0].snapshot.as_ref().unwrap().as_of, s.as_of);
            assert_eq!(rows[0].state, "ready");
        }
        b.cache.get_mut("native-codex").unwrap().snapshot.as_of =
            (chrono::Utc::now() - chrono::Duration::minutes(6)).to_rfc3339();
        assert_eq!(b.catalog(None, &[], &[], vec![])[0].state, "stale");
        let current = b.catalog(None, &[], &[], vec![]).remove(0);
        let key = RefreshKey::new(&current, Some(b.sources.lease("codex").unwrap().epoch));
        b.failures.insert(current.id, (key, "read denied".into()));
        assert_eq!(b.catalog(None, &[], &[], vec![])[0].state, "refresh_failed");
        b.cache.get_mut("native-codex").unwrap().revision = "changed".into();
        assert!(b.catalog(None, &[], &[], vec![])[0].snapshot.is_none());
    }
}
