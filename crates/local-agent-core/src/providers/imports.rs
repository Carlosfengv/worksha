//! Fixed user-level sources only. Public DTOs never contain credential values or commands.
use super::{ConfigError, ProviderManager, config, fail, files, validate_source};
use crate::{Provider, Store, sessions::sanitize_label};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use zeroize::Zeroizing;
#[cfg(test)]
#[path = "imports/operation_tests.rs"]
mod operation_tests;
pub mod operations;
#[cfg(test)]
#[path = "import_tests.rs"]
mod tests;

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Candidate {
    pub id: String,
    pub agent_id: String,
    pub source_path: String,
    pub source_label: String,
    pub revision: String,
    pub name: String,
    pub vendor: String,
    pub endpoint: Option<String>,
    pub protocol: String,
    pub model: Option<String>,
    pub active: bool,
    pub credential_source: String,
    pub can_import: bool,
    pub reason: Option<String>,
    pub existing_provider_id: Option<String>,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ImportRequest {
    pub candidate_id: String,
    pub expected_revision: String,
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation: Option<operations::Identity>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Imported {
    pub provider: Provider,
    pub reused: bool,
}
struct Material {
    public: Candidate,
    secret: Option<Zeroizing<String>>,
    reference: Option<String>,
    models: Vec<String>,
}
impl Material {
    fn blocked(&mut self, message: &str) {
        self.public.can_import = false;
        self.public.reason = Some(message.into());
    }
    fn credential(&mut self, value: &str, source: &str) {
        if value.is_empty() || value.len() > 8192 || !value.bytes().all(|b| b.is_ascii_graphic()) {
            self.blocked("API Key 格式无效，请在原 Agent 配置中核查。");
        } else {
            self.secret = Some(Zeroizing::new(value.into()));
            self.public.credential_source = source.into();
        }
    }
}
fn vendor(endpoint: &str) -> &'static str {
    let Ok(url) = url::Url::parse(endpoint) else {
        return "custom";
    };
    match url.host_str().unwrap_or("") {
        "api.openai.com" => "openai",
        "api.anthropic.com" => "anthropic",
        "api.deepseek.com" => "deepseek",
        "openrouter.ai" => "openrouter",
        "api.moonshot.cn" | "api.moonshot.ai" => "kimi",
        "localhost" | "127.0.0.1" | "[::1]" if url.port() == Some(11434) => "ollama",
        // A port number cannot reliably identify vLLM or another local engine.
        _ => "custom",
    }
}
fn checked_endpoint(raw: &str) -> Option<String> {
    let url = url::Url::parse(raw).ok()?;
    if !["http", "https"].contains(&url.scheme())
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.scheme() == "http" && !super::is_loopback_url(&url)
    {
        return None;
    }
    Some(url.to_string().trim_end_matches('/').into())
}
fn base(agent: &str, path: &std::path::Path, label: &str, digest: &str, active: bool) -> Material {
    use std::os::unix::ffi::OsStrExt;
    Material {
        public: Candidate {
            id: format!("{agent}-{}", &files::hash(label.as_bytes())[..24]),
            agent_id: agent.into(),
            source_path: path.to_string_lossy().into(),
            source_label: sanitize_label(label, 100),
            revision: files::hash(
                &serde_json::to_vec(&(path.as_os_str().as_bytes(), digest))
                    .expect("source tuple serializes"),
            ),
            name: format!(
                "{} · {}",
                if agent == "claude" {
                    "Claude Code"
                } else {
                    "Codex"
                },
                sanitize_label(label, 70)
            ),
            vendor: "custom".into(),
            endpoint: None,
            protocol: if agent == "claude" {
                "messages"
            } else {
                "responses"
            }
            .into(),
            model: None,
            active,
            credential_source: "未发现可导入的 API Key".into(),
            can_import: true,
            reason: None,
            existing_provider_id: None,
        },
        secret: None,
        reference: None,
        models: vec![],
    }
}
impl ProviderManager {
    fn managed_reference(&self, command: &str, args: &[&str]) -> Option<String> {
        let path = std::path::Path::new(command);
        if path.parent()? != self.directory.join("bin")
            || args.len() != 2
            || args[0] != "--credential"
            || !path
                .file_name()?
                .to_str()?
                .starts_with("credential-helper-")
            || uuid::Uuid::parse_str(args[1]).is_err()
        {
            return None;
        }
        // Never execute the command, even for our own helper. Resolve only our vault reference.
        Some(args[1].into())
    }
    fn claude_material(&self) -> Result<Material, ConfigError> {
        let path = config::path(&self.home, "claude")?;
        let read = files::read(&path)?;
        let raw = read.bytes.map(Zeroizing::new);
        let doc = config::parse_json(raw.as_deref().map(|b| b.as_slice()))?;
        let mut m = base(
            "claude",
            &path,
            "用户配置",
            read.stamp.digest.as_deref().unwrap_or("absent"),
            true,
        );
        let env = &doc["env"];
        for key in [
            "CLAUDE_CODE_USE_BEDROCK",
            "CLAUDE_CODE_USE_VERTEX",
            "CLAUDE_CODE_USE_FOUNDRY",
            "CLAUDE_CODE_USE_GATEWAY",
        ] {
            if env[key]
                .as_str()
                .is_some_and(|s| !["", "0", "false"].contains(&s))
            {
                m.blocked("当前配置使用云平台或登录网关，暂不转换其专用鉴权。");
            }
        }
        m.public.endpoint = checked_endpoint(
            env["ANTHROPIC_BASE_URL"]
                .as_str()
                .unwrap_or("https://api.anthropic.com"),
        );
        m.public.model = env["ANTHROPIC_MODEL"]
            .as_str()
            .or(env["ANTHROPIC_DEFAULT_MODEL"].as_str())
            .or(doc["model"].as_str())
            .filter(|s| !s.trim().is_empty())
            .map(str::to_owned);
        if let Some(alias) = m.public.model.as_deref()
            && ["haiku", "sonnet", "opus", "fable"].contains(&alias)
            && let Some(model) = env[format!("ANTHROPIC_DEFAULT_{}_MODEL", alias.to_uppercase())]
                .as_str()
                .filter(|s| !s.is_empty())
        {
            m.public.model = Some(model.into());
        }
        for key in [
            "ANTHROPIC_DEFAULT_HAIKU_MODEL",
            "ANTHROPIC_DEFAULT_SONNET_MODEL",
            "ANTHROPIC_DEFAULT_OPUS_MODEL",
            "ANTHROPIC_DEFAULT_FABLE_MODEL",
            "CLAUDE_CODE_SUBAGENT_MODEL",
        ] {
            if let Some(model) = env[key].as_str().filter(|s| !s.is_empty()) {
                m.models.push(model.into());
            }
        }
        if env["ANTHROPIC_CUSTOM_HEADERS"]
            .as_str()
            .is_some_and(|s| !s.trim().is_empty())
        {
            m.blocked("配置含自定义请求头，当前导入不能完整保留，请在原 Agent 中核查。");
        }
        if let Some(key) = env["ANTHROPIC_AUTH_TOKEN"]
            .as_str()
            .filter(|s| !s.is_empty())
        {
            m.credential(key, "settings.json · ANTHROPIC_AUTH_TOKEN");
        } else if let Some(key) = env["ANTHROPIC_API_KEY"].as_str().filter(|s| !s.is_empty()) {
            m.credential(key, "settings.json · ANTHROPIC_API_KEY");
        } else if let Some(helper) = doc["apiKeyHelper"].as_str().filter(|s| !s.is_empty()) {
            // Match exactly the quoted form written by config::render; no shell parsing.
            if let Some((command, reference)) = helper
                .strip_prefix('\'')
                .and_then(|s| s.strip_suffix('\''))
                .and_then(|s| s.split_once("' --credential '"))
            {
                m.reference = self.managed_reference(command, &["--credential", reference]);
            }
            m.public.credential_source = "apiKeyHelper · 未执行".into();
            if m.reference.is_none() {
                m.blocked("API Key 来自外部助手；不会自动执行命令，请在 Providers 单独配置。");
            }
        } else if env["CLAUDE_CODE_OAUTH_TOKEN"]
            .as_str()
            .is_some_and(|s| !s.is_empty())
        {
            m.blocked("检测到订阅登录令牌；不导入为 API Key，可在 Agent 详情选择官方原生登录。");
        }
        if raw.is_none() {
            m.blocked("用户配置文件尚未创建；官方登录可在 Agent 详情选择。");
        }
        Ok(m)
    }
    fn codex_materials(
        &self,
        environment: &dyn Fn(&str) -> Option<String>,
    ) -> Result<Vec<Material>, ConfigError> {
        let path = config::path(&self.home, "codex")?;
        let read = files::read(&path)?;
        let raw = read.bytes.map(Zeroizing::new);
        let doc = config::parse_toml(raw.as_deref().map(|b| b.as_slice()))?;
        let active = doc
            .get("model_provider")
            .and_then(|i| i.as_str())
            .unwrap_or("openai");
        let mut names = vec!["openai".to_owned()];
        if let Some(table) = doc.get("model_providers") {
            let table = table.as_table_like().ok_or_else(|| {
                ConfigError::new("invalid_config", "Codex model_providers 格式无效")
            })?;
            if table.len() > 100 {
                return fail("capacity", "Codex Provider 数量超过 100 项，暂不扫描");
            }
            names.extend(
                table
                    .iter()
                    .filter(|(id, _)| *id != "openai")
                    .map(|(id, _)| id.to_owned()),
            );
        }
        if !names.iter().any(|id| id == active) {
            names.push(active.into());
        }
        let mut result = vec![];
        for name in names {
            let p = doc.get("model_providers").and_then(|i| i.get(&name));
            let text = |key| p.and_then(|i| i.get(key)).and_then(|i| i.as_str());
            let mut m = base(
                "codex",
                &path,
                &name,
                read.stamp.digest.as_deref().unwrap_or("absent"),
                name == active,
            );
            m.public.endpoint = checked_endpoint(if name == "openai" {
                doc.get("openai_base_url")
                    .and_then(|i| i.as_str())
                    .or(text("base_url"))
                    .unwrap_or("https://api.openai.com/v1")
            } else {
                text("base_url").unwrap_or("")
            });
            m.public.name = format!(
                "Codex · {}",
                sanitize_label(text("name").unwrap_or(&name), 80)
            );
            m.public.model = (name == active)
                .then(|| {
                    doc.get("model")
                        .and_then(|i| i.as_str())
                        .filter(|s| !s.is_empty())
                        .map(str::to_owned)
                })
                .flatten();
            m.public.protocol = match text("wire_api").unwrap_or("responses") {
                "responses" => "responses",
                "chat" => "chat-completions",
                _ => "unsupported",
            }
            .into();
            if m.public.protocol == "unsupported" {
                m.blocked("不支持此 wire_api，无法确定实际协议。");
            }
            if doc.get("profile").is_some() {
                m.blocked("存在旧版默认 profile，请先在 Codex 中核查有效配置层。");
            }
            if ["http_headers", "env_http_headers", "query_params"]
                .iter()
                .any(|k| {
                    p.and_then(|i| i.get(k))
                        .is_some_and(|i| i.as_table_like().is_none_or(|t| !t.is_empty()))
                })
            {
                m.blocked("配置含请求头或查询参数，当前导入不能完整保留。");
            }
            let auth = p.and_then(|i| i.get("auth"));
            let env_key = text("env_key");
            let bearer = text("experimental_bearer_token");
            let requires_openai = p
                .and_then(|i| i.get("requires_openai_auth"))
                .and_then(|i| i.as_bool())
                .unwrap_or(false);
            if usize::from(auth.is_some())
                + usize::from(env_key.is_some())
                + usize::from(bearer.is_some())
                + usize::from(requires_openai)
                > 1
            {
                m.blocked("存在多种鉴权设置，需先在 Codex 中消除冲突。");
            }
            if let Some(auth) = auth {
                let command = auth.get("command").and_then(|i| i.as_str()).unwrap_or("");
                let args: Vec<_> = auth
                    .get("args")
                    .and_then(|i| i.as_array())
                    .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
                    .unwrap_or_default();
                m.reference = self.managed_reference(command, &args);
                m.public.credential_source = "auth 凭据助手 · 未执行".into();
                if m.reference.is_none() {
                    m.blocked("API Key 来自外部助手；不会自动执行命令，请在 Providers 单独配置。");
                }
            } else if let Some(env_key) = env_key {
                if env_key.len() > 100
                    || env_key.is_empty()
                    || !env_key.bytes().enumerate().all(|(i, b)| {
                        b == b'_' || b.is_ascii_alphabetic() || i > 0 && b.is_ascii_digit()
                    })
                {
                    m.blocked("env_key 变量名称无效。");
                } else if let Some(key) = environment(env_key) {
                    m.credential(&key, &format!("App 进程环境 · {env_key}"));
                    m.public.revision = files::hash(
                        format!("{}:{}", m.public.revision, files::hash(key.as_bytes())).as_bytes(),
                    );
                } else {
                    m.blocked("App 启动环境未提供 env_key 对应的 API Key；不会执行 Shell 或读取启动脚本。");
                }
            } else if let Some(key) = bearer {
                m.credential(key, "config.toml · experimental_bearer_token");
            } else if name == "openai" || requires_openai {
                let auth_path = self.home.join(".codex/auth.json");
                match files::read(&auth_path) {
                    Ok(auth_read) => {
                        m.public.revision = files::hash(
                            format!(
                                "{}:{}",
                                m.public.revision,
                                auth_read.stamp.digest.as_deref().unwrap_or("absent")
                            )
                            .as_bytes(),
                        );
                        let bytes = auth_read.bytes.map(Zeroizing::new);
                        let auth: Option<Value> =
                            bytes.as_ref().and_then(|b| serde_json::from_slice(b).ok());
                        let key = auth
                            .as_ref()
                            .filter(|a| {
                                a["auth_mode"] == "apikey"
                                    || a.get("auth_mode").is_none()
                                        && a.get("tokens").is_none_or(Value::is_null)
                            })
                            .and_then(|a| a["OPENAI_API_KEY"].as_str())
                            .filter(|s| !s.is_empty());
                        if let Some(key) = key {
                            m.credential(key, "auth.json · OPENAI_API_KEY（仅 API Key）");
                        } else {
                            m.blocked("未发现文件中的 API Key；订阅登录由 Codex 管理，不导入 OAuth，可在详情选择官方原生登录。");
                        }
                    }
                    Err(_) => m.blocked("auth.json 无法安全读取，未导入任何登录信息。"),
                }
            }
            result.push(m);
        }
        Ok(result)
    }
    fn import_materials(&self, environment: &dyn Fn(&str) -> Option<String>) -> Vec<Material> {
        let mut items = vec![];
        for agent in ["claude", "codex"] {
            let result = self.adapters.resolve(agent).and_then(|_| {
                if agent == "claude" {
                    self.claude_material().map(|m| vec![m])
                } else {
                    self.codex_materials(environment)
                }
            });
            match result {
                Ok(found) => items.extend(found),
                Err(error) => {
                    let mut m = base(
                        agent,
                        &config::path(&self.home, agent).unwrap(),
                        "用户配置",
                        "unavailable",
                        true,
                    );
                    m.blocked(if error.code == "plugin_disabled" {
                        "配置适配器已停止；本机配置未读取。"
                    } else {
                        "配置缺失、格式无效或无法安全读取。请检查原文件和权限后重新识别。"
                    });
                    items.push(m);
                }
            }
        }
        let journal = self.journal();
        for m in &mut items {
            let was_gateway = m
                .public
                .endpoint
                .as_ref()
                .and_then(|s| url::Url::parse(s).ok())
                .is_some_and(|u| super::is_loopback_url(&u) && u.path().contains("/gateway/"));
            let mut unwrapped = false;
            // Unwrap our saved gateway route to its original upstream, avoiding gateway-to-gateway loops.
            if let Ok(j) = &journal
                && let Some(t) = j.transactions.iter().rev().find(|t| {
                    t.agent_id == m.public.agent_id
                        && t.state == "applied"
                        && t.routing
                            .as_ref()
                            .is_some_and(|r| Some(&r.local_endpoint) == m.public.endpoint.as_ref())
                })
                && let Some(p) = &t.provider
            {
                m.public.endpoint = Some(p.endpoint.clone());
                m.public.protocol = p.protocol.clone();
                m.public.model = Some(p.model.clone());
                m.reference = p.credential_ref.clone();
                m.secret = None;
                m.models = p.models.clone();
                m.public.credential_source = "已保存本机路由的上游钥匙串引用".into();
                unwrapped = true;
            }
            if was_gateway && !unwrapped {
                m.blocked("本机转换路由缺少可确认的上游快照，不将网关入口再次导入为上游。");
            }
            if let Some(endpoint) = &m.public.endpoint {
                m.public.vendor = vendor(endpoint).into();
            } else {
                m.blocked("Endpoint 缺失、包含认证参数或使用不受支持的传输方式，未猜测地址。");
            }
            if let Some(reference) = &m.reference {
                match self.secret(reference).and_then(|b| {
                    String::from_utf8(b.to_vec())
                        .map(Zeroizing::new)
                        .map_err(|_| ConfigError::new("credential_unavailable", "凭据无效"))
                }) {
                    Ok(secret) => m.secret = Some(secret),
                    Err(_) => m.blocked("本机钥匙串凭据不可用，请解锁后重新识别。"),
                }
            }
            if m.secret.is_none() && m.reference.is_none() && m.public.can_import {
                let local = m
                    .public
                    .endpoint
                    .as_ref()
                    .and_then(|s| url::Url::parse(s).ok())
                    .is_some_and(|u| super::is_loopback_url(&u));
                if local {
                    m.public.credential_source = "本机回环连接 · 无鉴权".into();
                } else {
                    m.blocked("未发现可导入的 API Key；官方订阅可在 Agent 详情选择原生登录。");
                }
            }
            if let Some(model) = &m.public.model
                && (model.chars().count() > 200 || model.chars().any(char::is_control))
            {
                m.public.model = None;
                m.blocked("模型字段无效，请先在原配置中核查。");
            }
            // Bind preview to resolved credentials and route snapshots as well as file bytes.
            m.public.revision = files::hash(
                &serde_json::to_vec(&(
                    &m.public.revision,
                    &m.public.endpoint,
                    &m.public.protocol,
                    &m.public.model,
                    m.secret.as_ref().map(|s| files::hash(s.as_bytes())),
                ))
                .unwrap(),
            );
        }
        items
    }
    fn same_import(&self, m: &Material, p: &Provider, model: &str) -> bool {
        if !p.configured
            || Some(&p.endpoint) != m.public.endpoint.as_ref()
            || p.protocol != m.public.protocol
            || p.model != model
        {
            return false;
        }
        match (&m.secret, &p.credential_ref) {
            (None, None) => true,
            (Some(key), Some(reference)) => self
                .secret(reference)
                .is_ok_and(|stored| stored.as_slice() == key.as_bytes()),
            _ => false,
        }
    }
    pub fn discover_imports(&self, saved: &[Provider]) -> Vec<Candidate> {
        self.import_materials(&|name| std::env::var(name).ok())
            .into_iter()
            .map(|mut m| {
                m.public.existing_provider_id = saved
                    .iter()
                    .find(|p| self.same_import(&m, p, m.public.model.as_deref().unwrap_or("")))
                    .map(|p| p.id.clone());
                m.public
            })
            .collect()
    }
    fn checked_import_material(
        &self,
        input: &ImportRequest,
    ) -> Result<(Material, String), ConfigError> {
        let m = self
            .import_materials(&|name| std::env::var(name).ok())
            .into_iter()
            .find(|m| m.public.id == input.candidate_id)
            .ok_or_else(|| ConfigError::new("not_found", "本机配置候选已不存在，请重新识别"))?;
        if m.public.revision != input.expected_revision {
            return fail("conflict", "本机配置或凭据来源已变更，请重新识别后导入");
        }
        if !m.public.can_import {
            return fail(
                "unsupported",
                m.public.reason.as_deref().unwrap_or("此配置无法导入"),
            );
        }
        let model = input
            .model
            .as_deref()
            .or(m.public.model.as_deref())
            .unwrap_or("")
            .trim();
        if model.is_empty() || model.chars().count() > 200 || model.chars().any(char::is_control) {
            return fail("invalid_config", "此连接没有可用的默认模型，请补充模型 ID");
        }
        let model = model.to_owned();
        Ok((m, model))
    }
    pub fn import_provider(
        &self,
        store: &Store,
        mut input: ImportRequest,
    ) -> Result<Imported, ConfigError> {
        let protocol = self.native_import_protocol(store)?;
        if input.operation.is_none() {
            // Preserve legacy not-found/conflict/model errors before assigning
            // the generated operation. No credential write or intent exists yet.
            {
                let _guard = self.provider_edit_guard()?;
                let (_, model) = self.checked_import_material(&input)?;
                // Legacy callers historically accepted a trimmed model and
                // source defaults. Bind that resolved value in their new intent.
                input.model = Some(model);
            }
            input.operation = Some(operations::Identity {
                store_id: protocol.store_id.clone(),
                namespace_id: protocol.namespace_id.clone(),
                operation_id: uuid::Uuid::new_v4().to_string(),
                expected_sequence: protocol.sequence,
            });
        }
        let identity = operations::validate(&input)?.clone();
        let scope = self.native_import_scope(store, &identity)?;
        let existing = store.read_import(&input)?;
        if let Some(existing) = &existing {
            if existing.status != operations::Status::Pending {
                return existing.result();
            }
        } else {
            if let Some(expired) = store.expired_import(&input)? {
                return expired.result();
            }
            // A missing import intent cannot claim an already staged credential.
            // This also fails closed when only one half of a snapshot is restored.
            match self.credential_operation_v3(
                &scope,
                &identity.namespace_id,
                &identity.operation_id,
                false,
            ) {
                Err(error) if error.code == "operation_not_found" => {}
                _ => return Err(operations::unknown()),
            }
        }
        let _guard = self.provider_edit_guard()?;
        let (m, model) = self.checked_import_material(&input)?;
        let saved = store
            .providers()
            .map_err(|_| ConfigError::new("unavailable", "供应商目录不可读取"))?;
        if existing.is_none()
            && let Some(p) = saved.iter().find(|p| self.same_import(&m, p, &model))
        {
            return store
                .begin_import(&input, Some(p.clone()), true, false, false)?
                .result();
        }
        let creates_credential = m.secret.is_some() && m.reference.is_none();
        let label = m.secret.as_ref().map(|_| {
            format!(
                "{} · 本机导入 API Key",
                if m.public.agent_id == "claude" {
                    "Claude Code"
                } else {
                    "Codex"
                }
            )
        });
        let mut p = Provider {
            id: format!("custom-{}", identity.operation_id),
            name: m.public.name,
            vendor: m.public.vendor,
            endpoint: m.public.endpoint.unwrap(),
            protocol: m.public.protocol,
            model,
            configured: true,
            credential_ref: if creates_credential {
                Some(scope.journal.domain(3).reference(&identity.operation_id))
            } else {
                m.reference
            },
            credential_label: label,
            models: m.models,
            disabled_models: vec![],
            model_catalog: None,
            revision: 0,
            category: "custom".into(),
        };
        validate_source(&p)?;
        p = store.normalize_provider(p)?;
        p.revision = 0;
        let operation =
            store.begin_import(&input, Some(p.clone()), false, creates_credential, false)?;
        if operation.status != operations::Status::Pending {
            return operation.result();
        }
        if operation.creates_credential != creates_credential
            || serde_json::to_vec(&operation.provider).ok()
                != serde_json::to_vec(&Some(p.clone())).ok()
        {
            return Err(operations::unknown());
        }
        // v3 takes the recovery lock before any Provider file lock. Do not hold
        // the native configuration lock across it or invert that lock order.
        drop(_guard);
        if creates_credential {
            let receipt = self
                .create_credential_v3(
                    &scope,
                    &identity.namespace_id,
                    &identity.operation_id,
                    p.credential_label
                        .as_deref()
                        .ok_or_else(operations::unknown)?,
                    m.secret.ok_or_else(operations::unknown)?,
                )
                .map_err(|_| operations::unknown())?;
            if receipt
                .operation
                .receipt
                .as_ref()
                .map(|r| &r.credential_ref)
                != p.credential_ref.as_ref()
            {
                return Err(operations::unknown());
            }
        }
        let _guard = self
            .provider_edit_guard()
            .map_err(|_| operations::unknown())?;
        // A changed source never silently completes an earlier intent, even
        // when its credential is already durable and independently recoverable.
        if !self
            .import_materials(&|name| std::env::var(name).ok())
            .iter()
            .any(|m| {
                m.public.id == input.candidate_id
                    && m.public.revision == input.expected_revision
                    && m.public.can_import
            })
        {
            return Err(operations::unknown());
        }
        store
            .finish_import(&input, false)
            .map_err(|_| operations::unknown())?
            .result()
    }
    pub fn native_import_protocol(
        &self,
        store: &Store,
    ) -> Result<operations::Protocol, ConfigError> {
        let scope = store.credential_scope(&store.database_id()?)?;
        let protocol = self.scoped_credential_protocol_v3(&scope)?;
        Ok(operations::Protocol {
            version: 1,
            store_id: protocol.store_id,
            namespace_id: protocol.namespace_id,
            namespace_state: protocol.namespace_state,
            sequence: store.import_sequence()?,
            operation_limit: operations::LIMIT,
            pending_limit: operations::PENDING_LIMIT,
            pending_requests: store.pending_import_requests()?,
        })
    }
    fn native_import_scope(
        &self,
        store: &Store,
        identity: &operations::Identity,
    ) -> Result<super::credential_scope::CredentialScope, ConfigError> {
        let scope = store.credential_scope(&identity.store_id)?;
        self.credential_scoped_manager(&scope, Some(&identity.namespace_id))?;
        Ok(scope)
    }
    pub fn native_import_operation(
        &self,
        store: &Store,
        request: &ImportRequest,
        cancel: bool,
    ) -> Result<operations::Operation, ConfigError> {
        let identity = operations::validate(request)?;
        self.native_import_scope(store, identity)?;
        let _guard = self.provider_edit_guard()?;
        if let Some(existing) = store.read_import(request)? {
            if cancel {
                store.finish_import(request, true)
            } else {
                Ok(existing)
            }
        } else if let Some(expired) = store.expired_import(request)? {
            Ok(expired)
        } else if cancel {
            store.begin_import(request, None, false, false, true)
        } else {
            Err(operations::unknown())
        }
    }
}
