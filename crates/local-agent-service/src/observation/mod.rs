mod codex_desktop;
mod context_budget;
#[cfg(test)]
mod cost_tests;
mod delivery;
mod dynamic_turn;
mod hub;
mod message;
mod native_publication;
mod persistence;
pub(crate) mod policy;
pub(crate) mod proxy;
mod query_queue;
pub(crate) mod request_export;
pub(crate) mod request_query;
mod route_notices;
pub(crate) mod stream;
use crate::{ApiResult, AppState, error, json_value};
use axum::{
    Json,
    extract::{Query, State},
};
pub(crate) use hub::GatewayActivity;
use hub::Hub;
#[cfg(test)]
use local_agent_core::observation::ModelObservation;
use local_agent_core::{
    Provider,
    observation::OverviewQuery,
    sessions::{live::NativeWatch, sanitize_label},
};
pub(crate) use native_publication::NativePublication;
pub use proxy::capture;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

pub fn label(value: &str) -> String {
    sanitize_label(value, 200)
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Binding {
    agent_id: String,
    provider: Provider,
    revision: u64,
    enabled: bool,
    secret: String,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BindingStatus {
    pub(crate) agent_id: String,
    provider: Provider,
    revision: u64,
    pub enabled: bool,
    launcher_path: String,
    pub(crate) active_requests: usize,
}
pub struct Observation {
    native_collecting: std::sync::atomic::AtomicBool,
    hub: Arc<Hub>,
    bindings: Mutex<HashMap<String, Binding>>,
    directory: PathBuf,
    host: String,
    client: reqwest::Client,
    pub native_watch: Mutex<NativeWatch>,
    native: Mutex<NativePublication>,
    codex_desktop: Mutex<codex_desktop::Observer>,
    pub capacity: Arc<tokio::sync::Semaphore>,
    policy: Arc<policy::Runtime>,
}
impl Observation {
    /// Metadata for the compiled host gateway read facade; no native source grant.
    pub(crate) fn domain_activity_capability(&self) -> local_agent_core::domain::CapabilityEntry {
        use local_agent_core::domain as d;
        d::CapabilityEntry {
            provider: d::Provider {
                provider_key: "builtin:domain.activity-source@1:gateway".into(),
                plugin_id: "observation.gateway".into(),
                implementation_version: env!("CARGO_PKG_VERSION").into(),
                implementation_identity: format!(
                    "builtin:observation.gateway:{}",
                    env!("CARGO_PKG_VERSION")
                ),
                capability: d::Capability {
                    name: "domain.activity-source".into(),
                    major: 1,
                    instance_id: "gateway".into(),
                },
                execution_kind: d::ExecutionKind::Builtin,
            },
            status: d::ProviderStatus {
                enabled: true,
                authorization: d::Authorization::Authorized,
                grant_revision: 1u64.into(),
                runtime_active: true,
                freshness: d::Freshness::Current,
            },
            operations: vec![d::ReadOperation::CollectActivity],
        }
    }
    pub(crate) fn native_activity_collecting(&self) -> bool {
        self.native_collecting
            .load(std::sync::atomic::Ordering::Acquire)
    }
    pub(crate) fn published_native_activity(&self) -> Result<NativePublication, String> {
        self.native
            .lock()
            .map(|p| p.clone())
            .map_err(|_| "原生活动快照不可用".into())
    }
    pub(crate) fn published_gateway_activity(&self) -> Result<Vec<hub::GatewayActivity>, String> {
        self.hub.try_activities()
    }
    pub(crate) fn retention_impact(
        &self,
        days: u16,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<
        local_agent_core::observation::retention::impact::GatewayImpact,
        local_agent_core::observation::retention::impact::Error,
    > {
        local_agent_core::observation::retention::impact::gateway(&self.directory, days, now)
    }
    pub(crate) fn retention_writer_health(&self) -> persistence::Health {
        self.hub.health()
    }
    pub(crate) fn retention_activity_impact(
        &self,
        source: local_agent_core::observation::retention::impact::ActivitySource,
        days: u16,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<
        local_agent_core::observation::retention::impact::ActivityImpact,
        local_agent_core::observation::retention::impact::Error,
    > {
        local_agent_core::observation::retention::impact::activity(
            &self.directory,
            source,
            days,
            now,
        )
    }
    pub(crate) fn dynamic_route_observations(
        &self,
    ) -> Vec<local_agent_core::observation::ModelObservation> {
        self.hub
            .rows()
            .into_iter()
            .filter(|row| row.routing.is_some())
            .collect()
    }
    pub(crate) fn release_native_snapshots(&self) {
        self.hub.release_native_snapshots();
    }
    pub(crate) fn release_request_views(&self) -> Result<(), String> {
        self.hub.release_request_views()
    }
    pub(crate) fn forget_native_inputs(&self, agent: &str) {
        if let Ok(mut watch) = self.native_watch.lock() {
            watch.forget_inputs(agent);
        }
        if agent == "codex"
            && let Ok(mut observer) = self.codex_desktop.lock()
        {
            observer.forget();
        }
    }
    pub(crate) fn sql_query<T>(
        &self,
        read: impl FnOnce(&mut local_agent_core::observation::sql::History) -> Result<T, String>,
    ) -> Result<T, String> {
        self.hub.query(read)
    }
    pub(crate) fn cached_sql_query<T>(
        &self,
        read: impl FnOnce(&mut local_agent_core::observation::sql::History) -> Result<T, String>,
    ) -> Result<T, String> {
        self.hub.cached_query(read)
    }
    pub(crate) fn history_query_partial(&self) -> bool {
        self.hub.error().is_some()
    }
    pub(crate) fn gateway_sessions(
        &self,
    ) -> Result<(Vec<local_agent_core::sessions::SessionSummary>, bool), String> {
        self.sql_query(|history| {
            let inputs = local_agent_core::sessions::activity_input_cache::InputCache::open(
                self.directory.join("gateway-activity-inputs.json"),
            );
            history.gateway_sessions(&inputs)
        })
    }
    pub(crate) fn sql_query_now<T>(
        &self,
        read: impl FnOnce(
            &mut local_agent_core::observation::sql::History,
            chrono::DateTime<chrono::Utc>,
        ) -> Result<T, String>,
    ) -> Result<T, String> {
        self.hub.query_now(read)
    }
    pub(crate) fn persistence_error(&self) -> bool {
        self.hub.error().is_some()
    }
    pub fn flush(&self) -> Result<(), String> {
        let native = self
            .codex_desktop
            .lock()
            .map_err(|_| "原生 TTFT 存储不可用".to_owned())
            .and_then(|mut observer| observer.flush());
        let gateway = self.hub.flush(std::time::Duration::from_secs(5));
        native.and(gateway)
    }
    pub fn interrupt_active(&self) {
        self.hub.interrupt_active();
    }
    pub fn usage_evidence(
        &self,
    ) -> Result<Vec<local_agent_core::provider_usage::Evidence>, String> {
        let bindings = self.bindings.lock().map_err(|_| "观测归属暂不可用")?;
        Ok(bindings
            .values()
            .map(|b| local_agent_core::provider_usage::Evidence {
                agent: b.agent_id.clone(),
                provider_id: b.provider.id.clone(),
                provider_name: b.provider.name.clone(),
                endpoint: b.provider.endpoint.clone(),
                protocol: b.provider.protocol.clone(),
                revision: b.revision,
                native_provider: None,
                model_aliases: vec![],
            })
            .collect())
    }
    pub fn fresh_active_counts(&self) -> HashMap<String, usize> {
        let mut counts = HashMap::new();
        for activity in self.hub.activities() {
            if activity.request.status == "in_progress" && !activity.stale {
                *counts
                    .entry(local_agent_core::agents::group(
                        &activity.request.agent_id,
                        activity.request.agent_product_id.as_deref(),
                    ))
                    .or_insert(0) += 1;
            }
        }
        counts
    }
    pub fn active_count(&self, agent: &str) -> usize {
        self.hub.active_count(agent)
    }
    #[cfg(test)]
    pub fn hub_rows_for_test(&self) -> Vec<ModelObservation> {
        self.hub.rows()
    }
    pub fn project_activities(
        &self,
        catalog: &local_agent_core::projects::ProjectCatalog,
        id: &str,
        agent: Option<&str>,
    ) -> serde_json::Value {
        let ids: std::collections::HashSet<_> = catalog
            .sessions
            .iter()
            .filter(|s| local_agent_core::projects::session_identity(s).is_some_and(|p| p.id == id))
            .map(|s| s.id.as_str())
            .collect();
        let native = self
            .native
            .lock()
            .map(|items| {
                items
                    .rows
                    .iter()
                    .filter(|a| {
                        self.native_collecting
                            .load(std::sync::atomic::Ordering::Acquire)
                            && ids.contains(a.session_id.as_str())
                            && catalog.calls.directory().matches(
                                agent,
                                &a.agent_id,
                                a.agent_product_id.as_deref(),
                            )
                    })
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let gateway = self
            .hub
            .activities()
            .into_iter()
            .filter(|a| {
                catalog.calls.directory().matches(
                    agent,
                    &a.request.agent_id,
                    a.request.agent_product_id.as_deref(),
                ) && a
                    .request
                    .session_id
                    .as_deref()
                    .is_some_and(|session| ids.contains(session))
            })
            .collect::<Vec<_>>();
        serde_json::json!({"native":native,"gateway":gateway})
    }
    pub fn new(directory: &Path, host: &str) -> Result<Self, String> {
        let directory = directory.join("observation");
        if std::fs::symlink_metadata(&directory).is_ok_and(|m| m.file_type().is_symlink()) {
            return Err("观测目录不能为符号链接".into());
        }
        std::fs::create_dir_all(&directory).map_err(|_| "无法创建观测目录")?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))
                .map_err(|_| "无法保护观测目录")?;
        }
        let path = directory.join("bindings.json");
        if std::fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink()) {
            return Err("观测配置不能为符号链接".into());
        }
        let bindings: HashMap<String, Binding> = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|_| "无法读取观测入口配置")?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => HashMap::new(),
            Err(_) => return Err("无法读取观测入口配置".into()),
        };
        let result = Self {
            native_collecting: std::sync::atomic::AtomicBool::new(true),
            hub: Arc::new(Hub::open(&directory)?),
            bindings: Mutex::new(bindings),
            directory: directory.clone(),
            host: host.into(),
            client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(std::time::Duration::from_secs(15))
                .build()
                .map_err(|_| "无法建立流式转发服务")?,
            native_watch: Mutex::new(NativeWatch::with_cache_directory(directory.clone())),
            native: Mutex::new(NativePublication::default()),
            codex_desktop: Mutex::new(codex_desktop::Observer::new(&directory)),
            capacity: Arc::new(tokio::sync::Semaphore::new(64)),
            policy: Arc::new(policy::Runtime::default()),
        };
        for binding in result
            .bindings
            .lock()
            .map_err(|_| "观测配置不可用")?
            .values()
            .filter(|b| b.enabled)
        {
            validate_provider(&binding.agent_id, &binding.provider)?;
            result.write_launcher(binding)?;
        }
        result.hub.warm();
        Ok(result)
    }
    fn launcher(&self, agent: &str, revision: u64) -> PathBuf {
        self.directory.join(format!("{agent}-observed-{revision}"))
    }
    fn write_launcher(&self, b: &Binding) -> Result<(), String> {
        let base = format!("http://{}/capture/{}/{}", self.host, b.agent_id, b.secret);
        let quote = |v: &str| format!("'{}'", v.replace('\'', "'\\''"));
        let body = if b.agent_id == "claude" {
            format!(
                "#!/bin/sh\n# Local Agent managed launcher; existing Agent configuration is not edited.\nif [ -z \"$ANTHROPIC_API_KEY\" ]; then printf '%s\\n' '请先在当前终端设置 ANTHROPIC_API_KEY；订阅登录继续使用原生 claude。' >&2; exit 1; fi\nexport ANTHROPIC_BASE_URL={}\nexec claude \"$@\"\n",
                quote(&base)
            )
        } else {
            let fields = [
                "model_provider=\"local_agent_observed\"".to_string(),
                "model_providers.local_agent_observed.name=\"Local Agent\"".into(),
                format!("model_providers.local_agent_observed.base_url=\"{base}/v1\""),
                "model_providers.local_agent_observed.wire_api=\"responses\"".into(),
                "model_providers.local_agent_observed.env_key=\"OPENAI_API_KEY\"".into(),
                "model_providers.local_agent_observed.requires_openai_auth=false".into(),
                "model_providers.local_agent_observed.supports_websockets=false".into(),
            ];
            format!(
                "#!/bin/sh\n# Local Agent managed launcher; existing Agent configuration is not edited.\nif [ -z \"$OPENAI_API_KEY\" ]; then printf '%s\\n' '请先在当前终端设置 OPENAI_API_KEY；订阅登录继续使用原生 codex。' >&2; exit 1; fi\nexec codex {} \"$@\"\n",
                fields
                    .iter()
                    .map(|v| format!("-c {}", quote(v)))
                    .collect::<Vec<_>>()
                    .join(" ")
            )
        };
        private_file(
            &self.launcher(&b.agent_id, b.revision),
            body.as_bytes(),
            0o700,
        )
    }
    pub fn statuses(&self) -> Vec<BindingStatus> {
        self.bindings
            .lock()
            .map(|b| {
                b.values()
                    .map(|b| BindingStatus {
                        agent_id: b.agent_id.clone(),
                        provider: b.provider.clone(),
                        revision: b.revision,
                        enabled: b.enabled,
                        launcher_path: self
                            .launcher(&b.agent_id, b.revision)
                            .to_string_lossy()
                            .into(),
                        active_requests: self.hub.active_count(&b.agent_id),
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
    pub(crate) fn pause_native(&self) {
        self.native_collecting
            .store(false, std::sync::atomic::Ordering::Release);
        if let Ok(mut observer) = self.codex_desktop.lock() {
            observer.stop();
        }
    }
    pub(crate) fn resume_native(&self) {
        self.native_collecting
            .store(true, std::sync::atomic::Ordering::Release);
    }
    pub(crate) fn poll_native_guarded(
        &self,
        sessions: &local_agent_core::sessions::NativeSessions,
        proceed: &dyn Fn() -> bool,
    ) {
        if !proceed() {
            return;
        }
        let authorizations = sessions
            .domain_activity_authorizations()
            .unwrap_or_default();
        if let Ok(mut watch) = self.native_watch.lock() {
            let mut rows = watch.poll(sessions);
            if let Ok(mut observer) = self.codex_desktop.lock() {
                if sessions.sources().is_ok_and(|sources| {
                    sources.iter().all(|s| s.agent_id != "codex" || !s.enabled)
                }) {
                    observer.forget();
                }
                if proceed()
                    && self
                        .native_collecting
                        .load(std::sync::atomic::Ordering::Acquire)
                {
                    observer.sync(sessions.home(), &mut rows);
                } else {
                    observer.stop();
                }
                if let Some(error) = observer.error() {
                    watch.error = Some(error);
                }
            }
            if let Ok(mut native) = self.native.lock() {
                if proceed()
                    && self
                        .native_collecting
                        .load(std::sync::atomic::Ordering::Acquire)
                {
                    *native = NativePublication {
                        rows,
                        checked_at: watch.checked_at.clone(),
                        error: watch.error.clone(),
                        revision: native.revision.wrapping_add(1),
                        authorizations,
                    };
                } else {
                    *native = NativePublication::default();
                    *watch = NativeWatch::default();
                    watch.error = Some("原生活动采集器已停止".into());
                }
            }
        }
    }
}
fn validate_provider(agent: &str, p: &Provider) -> Result<(), String> {
    if !["claude", "codex"].contains(&agent)
        || p.protocol
            != if agent == "claude" {
                "messages"
            } else {
                "responses"
            }
    {
        return Err(
            "此 Agent 与 Provider 协议不兼容：Claude 需要 Messages，Codex 需要 Responses".into(),
        );
    }
    let url = url::Url::parse(&p.endpoint).map_err(|_| "Provider 端点无效")?;
    let local = url.host_str().is_some_and(|h| {
        h == "localhost"
            || h.parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback())
    });
    if (url.scheme() != "https" && !(url.scheme() == "http" && local))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("观测目标必须使用 HTTPS；仅本机服务允许 HTTP，端点不能包含凭据或参数".into());
    }
    Ok(())
}
fn private_file(path: &Path, bytes: &[u8], mode: u32) -> Result<(), String> {
    use std::io::Write;
    if std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err("受管入口不能为符号链接".into());
    }
    let temporary = path.with_extension(uuid::Uuid::new_v4().to_string());
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(mode);
    }
    let mut file = options
        .open(&temporary)
        .map_err(|_| "无法写入受管观测入口")?;
    let result = file
        .write_all(bytes)
        .and_then(|_| file.sync_all())
        .and_then(|_| std::fs::rename(&temporary, path));
    if result.is_err() {
        let _ = std::fs::remove_file(temporary);
        return Err("观测入口未能保存，原配置保持不变".into());
    }
    Ok(())
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BindingChange {
    agent_id: String,
    provider_id: String,
    expected_revision: u64,
    enabled: bool,
}
pub async fn save_binding(
    State(state): State<AppState>,
    Json(change): Json<BindingChange>,
) -> ApiResult {
    crate::blocking::management(move||{
        let provider=state.store.lock().map_err(|_|error("数据服务不可用".into()))?.providers().map_err(error)?.into_iter().find(|p|p.id==change.provider_id).ok_or_else(||error("Provider 不存在".into()))?;
        validate_provider(&change.agent_id,&provider).map_err(error)?;
        if change.enabled { local_agent_core::providers::select_model(&provider, None).map_err(crate::providers::error)?; }
        let mut bindings=state.observation.bindings.lock().map_err(|_|error("观测配置不可用".into()))?;
        if bindings.get(&change.agent_id).map_or(0,|b|b.revision)!=change.expected_revision {return Err((axum::http::StatusCode::CONFLICT,Json(serde_json::json!({"code":"stale_snapshot","message":"观测入口已变化，请重新读取后再提交"}))));}
        if state.observation.hub.active_count(&change.agent_id)>0 && change.enabled {return Err(error("此 Agent 仍有请求进行中，完成后可切换观测目标".into()));}
        let binding=Binding{agent_id:change.agent_id.clone(),provider,revision:change.expected_revision+1,enabled:change.enabled,secret:uuid::Uuid::new_v4().simple().to_string()};
        let mut next=bindings.clone();next.insert(change.agent_id.clone(),binding.clone());
        // Prepare the app-owned launcher before committing metadata. It cannot affect existing Agent processes.
        if binding.enabled {state.observation.write_launcher(&binding).map_err(error)?;}
        private_file(&state.observation.directory.join("bindings.json"),&serde_json::to_vec(&next).map_err(|_|error("配置无效".into()))?,0o600).map_err(error)?;
        *bindings=next;drop(bindings);
        json_value(state.observation.statuses())
    }).await
}
pub async fn overview(
    State(state): State<AppState>,
    Query(query): Query<OverviewQuery>,
) -> ApiResult {
    query
        .validate_with_directory(state.sessions.directory())
        .map_err(error)?;
    crate::blocking::query(state.query_capacity.clone(), move ||{
        let native_authorization=if query.source=="native"{Some(crate::sessions::authorizations(&state)?)}else{None};
        let (analysis,stats,now)=state.observation.hub.query(|history|{
            history.use_directory(state.sessions.directory())?;
            if query.source=="native" {
                let revision=state.sessions.revision().map_err(|e|e.message)?;
                if !history.native_analysis_matches(revision){history.sync_native_analysis(&state.sessions.project_catalog().map_err(|e|e.message)?)?;}
            }
            let now=chrono::Utc::now();
            Ok((history.analysis(&query,now)?,history.timing_stats(&query,now)?,now))
        }).map_err(error)?;
        let sources=state.sessions.sources().map_err(crate::native_error)?;
        let native=state.observation.native.lock().map_err(|_|error("原生活动不可用".into()))?.rows.iter().filter(|a|state.sessions.directory().matches(query.agent.as_deref(),&a.agent_id,a.agent_product_id.as_deref())&&state.observation.native_collecting.load(std::sync::atomic::Ordering::Acquire)&&state.sessions.source_active(&a.agent_id)&&sources.iter().any(|s|s.agent_id==a.agent_id&&s.enabled)).cloned().collect::<Vec<_>>();
        let activities=state.observation.hub.activities().into_iter().filter(|a|state.sessions.directory().matches(query.agent.as_deref(),&a.request.agent_id,a.request.agent_product_id.as_deref())).collect::<Vec<_>>();
        let watch=state.observation.native_watch.lock().map_err(|_|error("采集器状态不可用".into()))?;
        let context_ids = native.iter().filter(|a| !a.stale && ["processing", "thinking", "tool_requested", "tool_returned"].contains(&a.phase.as_str())).map(|a| a.session_id.clone()).chain(activities.iter().filter(|a| !a.stale && a.request.status == "in_progress").filter_map(|a| a.request.session_id.clone()));
        let contexts = if state.observation.native_collecting.load(std::sync::atomic::Ordering::Acquire) { watch.context_samples(&state.sessions, context_ids).map_err(crate::native_error)? } else { Vec::new() };
        if let Some(authorization)=native_authorization && authorization!=crate::sessions::authorizations(&state)?{return Err(error("原生来源已更新，请刷新统计".into()));}
        Ok(Json(serde_json::json!({"analysis":analysis,"timings":stats,"nativeActivities":native,"gatewayActivities":activities,"sessionContexts":contexts,"collector":{"checkedAt":watch.checked_at,"error":if state.observation.native_collecting.load(std::sync::atomic::Ordering::Acquire){watch.error.clone()}else{Some("原生活动采集器已停止".to_string())},"persistenceError":state.observation.hub.error(),"persistence":state.observation.hub.health()},"bindings":state.observation.statuses(),"asOf":now.to_rfc3339()})))
    }).await
}
#[cfg(test)]
fn gateway_rows(records: &[ModelObservation]) -> Vec<local_agent_core::observation::AnalysisRow> {
    let mut rows: HashMap<String, local_agent_core::observation::AnalysisRow> = HashMap::new();
    let mut seen = std::collections::HashSet::new();
    for r in records {
        if !seen.insert(&r.id) {
            continue;
        }
        let key = local_agent_core::requests::logical_request_key(r);
        let item = rows
            .entry(key)
            .or_insert_with(|| local_agent_core::observation::AnalysisRow {
                cost: Default::default(),
                agent: local_agent_core::agents::group(&r.agent_id, r.agent_product_id.as_deref()),
                model: r.model.clone(),
                time: Some(r.started_at.clone()),
                tokens: None,
                provisional: false,
                partial: false,
            });
        if item.time.as_ref().is_some_and(|t| t > &r.started_at) {
            item.time = Some(r.started_at.clone());
            item.model = r.model.clone();
        }
        item.tokens = match (item.tokens, r.usage.total_tokens) {
            (None, None) => None,
            (a, b) => Some(a.unwrap_or(0).saturating_add(b.unwrap_or(0))),
        };
        item.cost.add(&local_agent_core::provider_cost::calculate(
            Some(&r.endpoint),
            r.model.as_deref(),
            Some(&r.started_at),
            &r.status,
            &r.usage,
            r.reported_cost.as_ref(),
            r.ended_at.as_deref(),
        ));
        item.partial |= r.usage.total_tokens.is_none() || r.status != "success";
        item.provisional |= r.status == "in_progress";
    }
    rows.into_values().collect()
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RequestQuery {
    agent: Option<String>,
    offset: Option<usize>,
    id: Option<String>,
}
pub async fn requests(
    State(state): State<AppState>,
    Query(query): Query<RequestQuery>,
) -> ApiResult {
    if query
        .agent
        .as_deref()
        .is_some_and(|a| !local_agent_core::agents::valid_filter(a))
    {
        return Err(error("请求筛选无效".into()));
    }
    crate::blocking::query(state.query_capacity.clone(), move || {
        state
            .observation
            .hub
            .query(|history| {
                history.observation_page(
                    query.agent.as_deref(),
                    query.id.as_deref(),
                    query.offset.unwrap_or(0),
                )
            })
            .map(Json)
            .map_err(error)
    })
    .await
}
