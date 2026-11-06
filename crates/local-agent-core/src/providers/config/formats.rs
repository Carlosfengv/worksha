use super::super::{ConfigError, fail, files};
use crate::{Provider, sessions::sanitize_label};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use toml_edit::{DocumentMut, Item, Table, value};

pub(super) fn preserve_service_selection(
    agent: &str,
    before: Option<&[u8]>,
    after: &mut Vec<u8>,
    catalog: &crate::model_services::ServiceAttachment,
) -> Result<(), ConfigError> {
    if agent == "codex" {
        return preserve_codex_service(before, after, catalog);
    }
    let Some(endpoint) = &catalog.previous_endpoint else {
        return Ok(());
    };
    let Some(before) = before else { return Ok(()) };
    if agent == "claude-desktop" {
        return Ok(());
    }
    let old: Value = serde_json::from_slice(before)
        .map_err(|_| ConfigError::new("invalid_config", "原模型服务配置无法读取"))?;
    let mut next: Value = serde_json::from_slice(after)
        .map_err(|_| ConfigError::new("invalid_config", "模型服务配置无法生成"))?;
    if agent == "opencode" {
        let Some((old_group, old_alias)) = old["model"].as_str().and_then(|m| m.split_once('/'))
        else {
            return Ok(());
        };
        if old["provider"][old_group]["options"]["baseURL"].as_str() != Some(endpoint) {
            return fail("conflict", "原分组入口已改变，未覆盖模型选择");
        }
        let Some((new_group, _)) = next["model"].as_str().and_then(|m| m.split_once('/')) else {
            return fail("invalid_config", "新服务分组缺失");
        };
        let new_group = new_group.to_owned();
        let provider = next["provider"]
            .as_object_mut()
            .and_then(|p| p.remove(&new_group))
            .ok_or_else(|| ConfigError::new("invalid_config", "新服务目录缺失"))?;
        next["provider"][old_group] = provider;
        let alias = if catalog.accepts(old_alias) {
            old_alias
        } else {
            crate::routing::ALIAS
        };
        next["model"] = json!(format!("{old_group}/{alias}"));
        // The adapter also emits a small_model reference. Remap it with the
        // retained group, and keep an existing independent user override.
        let small = match old["small_model"].as_str() {
            Some(value) if !value.starts_with(&format!("{old_group}/")) => value.to_owned(),
            Some(value)
                if value
                    .split_once('/')
                    .is_some_and(|(_, a)| catalog.accepts(a)) =>
            {
                value.to_owned()
            }
            _ => format!("{old_group}/{}", crate::routing::ALIAS),
        };
        next["small_model"] = json!(small);
    } else if agent == "claude"
        && let Some(alias) = old["model"].as_str().filter(|a| catalog.accepts(a))
    {
        next["model"] = json!(alias);
    }
    use zeroize::Zeroize;
    after.zeroize();
    *after = serde_json::to_vec_pretty(&next)
        .map_err(|_| ConfigError::new("invalid_config", "模型服务选择无法生成"))?;
    after.push(b'\n');
    Ok(())
}

fn preserve_codex_service(
    before: Option<&[u8]>,
    after: &mut Vec<u8>,
    catalog: &crate::model_services::ServiceAttachment,
) -> Result<(), ConfigError> {
    let Some(before) = before else { return Ok(()) };
    if catalog.previous_codex_endpoints.is_empty() && catalog.previous_endpoint.is_none() {
        return Ok(());
    }
    let old = parse_toml(Some(before))?;
    let Some(providers) = old.get("model_providers").and_then(Item::as_table) else {
        return Ok(());
    };
    let active = old.get("model_provider").and_then(Item::as_str);
    let retained = catalog
        .previous_endpoint
        .iter()
        .chain(&catalog.previous_codex_endpoints)
        .find_map(|endpoint| {
            let matches = |id: &str, provider: &Item| {
                id.strip_prefix("local_agent_").is_some_and(|digest| {
                    digest.len() == 24 && digest.bytes().all(|b| b.is_ascii_hexdigit())
                }) && provider.get("base_url").and_then(Item::as_str) == Some(endpoint.as_str())
            };
            active
                .filter(|id| {
                    providers
                        .get(id)
                        .is_some_and(|provider| matches(id, provider))
                })
                .or_else(|| {
                    providers
                        .iter()
                        .find_map(|(id, provider)| matches(id, provider).then_some(id))
                })
        });
    let Some(retained) = retained else {
        return Ok(());
    };
    let mut next = parse_toml(Some(after))?;
    let generated = next["model_provider"]
        .as_str()
        .ok_or_else(|| ConfigError::new("invalid_config", "新模型服务的 Codex 供应商缺失"))?
        .to_owned();
    if retained != generated {
        let providers = next["model_providers"]
            .as_table_mut()
            .ok_or_else(|| ConfigError::new("invalid_config", "新模型服务的 Codex 供应商缺失"))?;
        let entry = providers
            .remove(&generated)
            .ok_or_else(|| ConfigError::new("invalid_config", "新模型服务的 Codex 供应商缺失"))?;
        providers.insert(retained, entry);
        next["model_provider"] = value(retained);
    }
    if active == Some(retained)
        && let Some(alias) = old
            .get("model")
            .and_then(Item::as_str)
            .filter(|alias| catalog.accepts(alias))
    {
        next["model"] = value(alias);
    }
    *after = next.to_string().into_bytes();
    Ok(())
}

pub(super) fn service_models(
    agent: &str,
    bytes: &mut Vec<u8>,
    provider: &Provider,
    catalog: &crate::model_services::ServiceAttachment,
) -> Result<(), ConfigError> {
    if agent == "codex" {
        return codex_models(bytes, provider);
    }
    let mut d: Value = serde_json::from_slice(bytes)
        .map_err(|_| ConfigError::new("invalid_config", "模型服务目录配置无法解析"))?;
    let entries = catalog.catalog();
    if agent == "claude" {
        if let Some(allow) = d.get("availableModels") {
            let allow = allow
                .as_array()
                .ok_or_else(|| ConfigError::new("invalid_config", "availableModels 格式无效"))?;
            if entries
                .iter()
                .any(|(id, _, _)| !allow.iter().any(|a| a.as_str() == Some(id)))
            {
                return fail(
                    "unsupported",
                    "已有模型白名单未允许此服务目录，请先在 Claude Code 核对；不会扩大白名单",
                );
            }
        }
        let common = crate::model_services::Capabilities::common(&catalog.installed.models);
        d["env"]["CLAUDE_CODE_MAX_CONTEXT_TOKENS"] = json!(common.context_window.to_string());
        d["env"]["CLAUDE_CODE_MAX_OUTPUT_TOKENS"] = json!(common.max_output_tokens.to_string());
        // Unknown service IDs use a conservative shared window; no Anthropic impersonation.
        d["modelPicker"] = json!({"replaceBuiltInOptions":true,"options":entries.iter().map(|(id,name,c)|json!({"model":id,"label":name,"description":format!("{} Token 上下文 · {} Token 输出 · 文本{} · 不启用思考；Claude 使用 {} Token 共同窗口",c.context_window,c.max_output_tokens,if c.tools {" / 标准工具"}else{""},common.context_window)})).collect::<Vec<_>>()});
        d["env"]["ANTHROPIC_DEFAULT_MODEL"] = json!(crate::routing::ALIAS);
        for family in ["HAIKU", "SONNET", "OPUS", "FABLE"] {
            d["env"][format!("ANTHROPIC_DEFAULT_{family}_MODEL_NAME")] =
                json!(format!("{} · 默认", catalog.installed.name));
        }
    } else if agent == "opencode" {
        let group = format!(
            "local_agent_{}",
            &super::super::files::hash(provider.id.as_bytes())[..16]
        );
        d["provider"][&group]["name"] = json!(format!("Local Agent · {}", catalog.installed.name));
        d["provider"][&group]["models"]=Value::Object(entries.iter().map(|(id,name,c)|(id.clone(),json!({"name":name,"limit":{"context":c.context_window,"output":c.max_output_tokens},"tool_call":c.tools,"reasoning":false,"attachment":false,"modalities":{"input":["text"],"output":["text"]},"options":{"thinking":{"type":"disabled"}}}))).collect());
    } else if agent == "claude-desktop" {
        d["inferenceModels"] = json!(
            entries
                .iter()
                .map(|(alias, label, _)| json!({
                    "name": crate::model_services::client_model_id(agent, alias),
                    "labelOverride": label,
                    "maxEffort": "low"
                }))
                .collect::<Vec<_>>()
        );
    } else {
        return fail("unsupported", "此 Agent 尚未支持模型服务目录");
    }
    use zeroize::Zeroize;
    bytes.zeroize();
    *bytes = serde_json::to_vec_pretty(&d)
        .map_err(|_| ConfigError::new("invalid_config", "模型服务目录无法生成"))?;
    bytes.push(b'\n');
    Ok(())
}

pub fn codex_models(rendered: &mut Vec<u8>, provider: &Provider) -> Result<(), ConfigError> {
    let mut doc = parse_toml(Some(rendered))?;
    if doc.get("model_catalog_json").is_some() {
        return fail(
            "unsupported",
            "Codex 已配置本地模型目录，会覆盖供应商目录；请先在 Codex 核查该配置层，原文件未修改",
        );
    }
    let id = doc["model_provider"]
        .as_str()
        .ok_or_else(|| ConfigError::new("invalid_config", "Codex 供应商缺失"))?
        .to_owned();
    doc["model_providers"][&id]["model_catalog_url"] = value(format!(
        "{}/codex/models.json",
        provider.endpoint.trim_end_matches('/')
    ));
    // Cross-protocol routes expose standard functions, not OpenAI-hosted web tools.
    doc["web_search"] = value("disabled");
    doc["model_reasoning_effort"] = value("none");
    *rendered = doc.to_string().into_bytes();
    Ok(())
}

const CLAUDE_ENV: &[&str] = &[
    "ANTHROPIC_BASE_URL",
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "ANTHROPIC_MODEL",
    "ANTHROPIC_DEFAULT_MODEL",
    "ANTHROPIC_DEFAULT_HAIKU_MODEL",
    "ANTHROPIC_DEFAULT_SONNET_MODEL",
    "ANTHROPIC_DEFAULT_OPUS_MODEL",
    "ANTHROPIC_DEFAULT_FABLE_MODEL",
    "CLAUDE_CODE_SUBAGENT_MODEL",
    "ANTHROPIC_SMALL_FAST_MODEL",
    "ANTHROPIC_CUSTOM_HEADERS",
    "CLAUDE_CODE_OAUTH_TOKEN",
    "CLAUDE_CODE_USE_BEDROCK",
    "CLAUDE_CODE_USE_VERTEX",
    "CLAUDE_CODE_USE_FOUNDRY",
    "CLAUDE_CODE_USE_GATEWAY",
];
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeConfig {
    pub agent_id: String,
    pub path: String,
    pub state: String,
    pub revision: Option<String>,
    pub mode: String,
    pub endpoint: Option<String>,
    #[serde(skip)]
    pub catalog_endpoints: Vec<String>,
    pub model: Option<String>,
    pub provider_name: String,
    pub credential: String,
    pub error: Option<String>,
    pub warnings: Vec<String>,
}
pub fn path(home: &Path, agent: &str) -> Result<PathBuf, ConfigError> {
    match agent {
        "opencode" | "claude-desktop" => super::super::desktop_clients::path(home, agent),
        "claude" => Ok(home.join(".claude/settings.json")),
        "codex" => Ok(home.join(".codex/config.toml")),
        _ => fail("unsupported", "当前仅支持 Claude Code 和 Codex"),
    }
}
pub(crate) fn parse_json(bytes: Option<&[u8]>) -> Result<Value, ConfigError> {
    let value = match bytes {
        Some(b) => serde_json::from_slice(b).map_err(|_| {
            ConfigError::new(
                "invalid_config",
                "Claude settings.json 无法解析，保留原文件",
            )
        })?,
        None => json!({}),
    };
    if !value.is_object() || value.get("env").is_some_and(|e| !e.is_object()) {
        return fail(
            "invalid_config",
            "Claude 配置或 env 必须是对象，原文件未覆盖",
        );
    }
    if ["model", "apiKeyHelper"]
        .iter()
        .any(|k| value.get(k).is_some_and(|v| !v.is_string()))
    {
        return fail("invalid_config", "Claude 路由字段类型无效，原文件未覆盖");
    }
    Ok(value)
}
pub(crate) fn parse_toml(bytes: Option<&[u8]>) -> Result<DocumentMut, ConfigError> {
    let doc = std::str::from_utf8(bytes.unwrap_or_default())
        .ok()
        .and_then(|s| s.parse::<DocumentMut>().ok())
        .ok_or_else(|| {
            ConfigError::new("invalid_config", "Codex config.toml 无法解析，保留原文件")
        })?;
    if ["model", "model_provider", "profile", "openai_base_url"]
        .iter()
        .any(|k| doc.get(k).is_some_and(|v| !v.is_str()))
    {
        return fail("invalid_config", "Codex 路由字段类型无效，原文件未覆盖");
    }
    Ok(doc)
}
pub fn endpoint(raw: &str) -> Option<String> {
    let mut url = url::Url::parse(raw).ok()?;
    if !["http", "https"].contains(&url.scheme()) {
        return None;
    }
    url.set_username("").ok()?;
    url.set_password(None).ok()?;
    url.set_query(None);
    url.set_fragment(None);
    Some(url.to_string().trim_end_matches('/').to_owned())
}
pub(crate) fn routing_endpoint(raw: &str) -> Result<String, ConfigError> {
    let url = url::Url::parse(raw).map_err(|_| {
        ConfigError::new("invalid_config", "连接 Endpoint 无法识别，实际路由待确认")
    })?;
    if !["http", "https"].contains(&url.scheme())
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return fail(
            "invalid_config",
            "连接 Endpoint 包含不支持的协议、凭据或参数，实际路由待确认",
        );
    }
    endpoint(raw).ok_or_else(|| ConfigError::new("invalid_config", "连接 Endpoint 无法识别"))
}
pub fn inspect(home: &Path, agent: &str) -> NativeConfig {
    if ["opencode", "claude-desktop"].contains(&agent) {
        return super::super::desktop_clients::inspect(home, agent);
    }
    let location = path(home, agent).unwrap();
    let mut result=NativeConfig { agent_id:agent.into(),path:location.to_string_lossy().into(),state:"ready".into(),revision:None,
        mode:"native".into(),endpoint:None,catalog_endpoints:vec![],model:None,provider_name:"官方原生登录 · 登录状态由 Agent 管理".into(),credential:"未读取原生登录凭据".into(),error:None,
        warnings:vec!["仅展示当前用户级文件。项目、命令行、启动环境与组织策略可能覆盖；不代表运行中实例的实际路由。".into()] };
    let read = files::read(&location).and_then(|s| {
        if agent == "codex" { result.warnings.push("此配置由 Codex CLI 与 Codex 桌面端共用；切换与恢复影响两端，应用前必须退出两端。历史调用来源不会随配置修改。".into()); }
        result.revision = Some(s.stamp.digest.clone().unwrap_or_else(|| "absent".into()));
        if s.bytes.is_none() {
            result.state = "absent".into();
        }
        if agent == "claude" {
            let d = parse_json(s.bytes.as_deref())?;
            let env = &d["env"];
            result.endpoint = env["ANTHROPIC_BASE_URL"]
                .as_str()
                .map(routing_endpoint)
                .transpose()?;
            result.model = env["ANTHROPIC_MODEL"]
                .as_str()
                .or(d["model"].as_str())
                .map(|s| sanitize_label(s, 200));
            let has_key = ["ANTHROPIC_API_KEY", "ANTHROPIC_AUTH_TOKEN"]
                .iter()
                .any(|k| env[k].as_str().is_some_and(|s| !s.is_empty()));
            let helper = d["apiKeyHelper"].as_str().is_some_and(|s| !s.is_empty());
            if has_key
                || helper
                || d["forceLoginMethod"].as_str() == Some("console")
                || result
                    .endpoint
                    .as_deref()
                    .is_some_and(|s| s != "https://api.anthropic.com")
            {
                result.mode = "api".into();
                result.provider_name = "文件中的 API / 网关配置".into();
                result.credential = if has_key {
                    "原配置中存在凭据 · 不展示"
                } else if helper {
                    "原配置中存在凭据助手 · 未执行"
                } else {
                    "需要在 Agent 中确认鉴权"
                }
                .into();
            }
            if [
                "CLAUDE_CODE_USE_BEDROCK",
                "CLAUDE_CODE_USE_VERTEX",
                "CLAUDE_CODE_USE_FOUNDRY",
                "CLAUDE_CODE_USE_GATEWAY",
            ]
            .iter()
            .any(|k| {
                env[k]
                    .as_str()
                    .is_some_and(|s| !s.is_empty() && s != "0" && s != "false")
            }) {
                result.mode = "external".into();
                result.provider_name = "其他云平台 / 网关".into();
            }
        } else {
            let d = parse_toml(s.bytes.as_deref())?;
            let name = d
                .get("model_provider")
                .and_then(Item::as_str)
                .unwrap_or("openai");
            let provider = d.get("model_providers").and_then(|p| p.get(name));
            result.provider_name = sanitize_label(name, 100);
            result.endpoint = provider
                .and_then(|p| p.get("base_url"))
                .and_then(Item::as_str)
                .or(d.get("openai_base_url").and_then(Item::as_str))
                .map(routing_endpoint)
                .transpose()?;
            result.model = d
                .get("model")
                .and_then(Item::as_str)
                .map(|s| sanitize_label(s, 200));
            if name != "openai"
                || result
                    .endpoint
                    .as_deref()
                    .is_some_and(|s| s != "https://api.openai.com/v1")
            {
                result.mode = "api".into();
            }
            if d.get("profile").is_some() {
                result.warnings.push(
                    "存在默认 profile，切换预览将阻止覆盖；请先在 Codex 中确认实际配置层。".into(),
                );
            }
            if provider.is_some_and(|p| {
                ["auth", "env_key", "experimental_bearer_token"]
                    .iter()
                    .any(|k| p.get(k).is_some())
            }) {
                result.credential = "原配置包含凭据引用 / 助手 · 未执行".into();
                result.mode = "api".into();
            }
            if d.get("forced_login_method").and_then(Item::as_str) == Some("api") {
                result.mode = "api".into();
            }
            if d.get("profile").is_some() {
                result.mode = "profile".into();
            }
        }
        Ok(())
    });
    if let Err(e) = read {
        result.state = "unavailable".into();
        result.error = Some(e.message);
    }
    result
}

// Only routing fields are changed. Permissions, MCP, skills, hooks and unknown fields survive.
pub fn render(
    agent: &str,
    before: Option<&[u8]>,
    provider: Option<&Provider>,
    helper: &Path,
) -> Result<Vec<u8>, ConfigError> {
    if ["opencode", "claude-desktop"].contains(&agent) {
        return super::super::desktop_clients::render(agent, before, provider, helper);
    }
    if agent == "claude" {
        let mut d = parse_json(before)?;
        if provider.is_none()
            && d.get("forceLoginMethod").and_then(Value::as_str) == Some("console")
        {
            return fail(
                "unsupported",
                "原配置限制为 Console/API 登录，本次不会移除登录限制",
            );
        }
        if provider.is_some()
            && ["forceLoginMethod", "forceLoginOrgUUID"]
                .iter()
                .any(|k| d.get(k).is_some())
        {
            return fail(
                "unsupported",
                "原配置要求原生登录，本次不会移除登录限制；请先在 Agent 中核查",
            );
        }
        if d.get("env").is_none() {
            d["env"] = json!({});
        }
        let own_service_picker = d["env"]["ANTHROPIC_DEFAULT_MODEL"] == crate::routing::ALIAS
            && d["env"]["ANTHROPIC_BASE_URL"]
                .as_str()
                .and_then(|url| url::Url::parse(url).ok())
                .is_some_and(|url| {
                    super::super::is_loopback_url(&url) && url.path().starts_with("/gateway/")
                })
            && d["modelPicker"]["replaceBuiltInOptions"] == true
            && d["modelPicker"]["options"]
                .as_array()
                .is_some_and(|entries| {
                    !entries.is_empty()
                        && entries.iter().all(|entry| {
                            entry["model"].as_str().is_some_and(|id| {
                                id == crate::routing::ALIAS
                                    || id
                                        .strip_prefix("local-agent-model-")
                                        .is_some_and(|id| uuid::Uuid::parse_str(id).is_ok())
                            })
                        })
                });
        if own_service_picker {
            d.as_object_mut().unwrap().remove("modelPicker");
            if d["alwaysThinkingEnabled"] == false {
                d.as_object_mut().unwrap().remove("alwaysThinkingEnabled");
            }
        }
        let env = d["env"].as_object_mut().unwrap();
        if own_service_picker {
            for key in [
                "CLAUDE_CODE_MAX_CONTEXT_TOKENS",
                "CLAUDE_CODE_MAX_OUTPUT_TOKENS",
                "ANTHROPIC_DEFAULT_HAIKU_MODEL_NAME",
                "ANTHROPIC_DEFAULT_SONNET_MODEL_NAME",
                "ANTHROPIC_DEFAULT_OPUS_MODEL_NAME",
                "ANTHROPIC_DEFAULT_FABLE_MODEL_NAME",
            ] {
                env.remove(key);
            }
        }
        for key in CLAUDE_ENV {
            env.remove(*key);
        }
        d.as_object_mut().unwrap().remove("apiKeyHelper");
        d.as_object_mut().unwrap().remove("model");
        if let Some(p) = provider {
            d["env"]["ANTHROPIC_BASE_URL"] = json!(p.endpoint);
            d["model"] = json!(p.model);
            if p.id.starts_with("dynamic-") && p.model == crate::routing::ALIAS {
                d["env"]["CLAUDE_CODE_MAX_OUTPUT_TOKENS"] = json!("8192");
                d["env"]["CLAUDE_CODE_MAX_CONTEXT_TOKENS"] = json!("32000");
                d["alwaysThinkingEnabled"] = json!(false);
            }
            for key in [
                "ANTHROPIC_DEFAULT_HAIKU_MODEL",
                "ANTHROPIC_DEFAULT_SONNET_MODEL",
                "ANTHROPIC_DEFAULT_OPUS_MODEL",
                "ANTHROPIC_DEFAULT_FABLE_MODEL",
                "CLAUDE_CODE_SUBAGENT_MODEL",
            ] {
                d["env"][key] = json!(p.model);
            }
            // Empty overrides avoid leaking an inherited API credential to a new destination.
            for key in [
                "ANTHROPIC_API_KEY",
                "ANTHROPIC_AUTH_TOKEN",
                "CLAUDE_CODE_OAUTH_TOKEN",
                "ANTHROPIC_CUSTOM_HEADERS",
            ] {
                d["env"][key] = json!("");
            }
            if let Some(reference) = &p.credential_ref {
                let quote = |s: &str| format!("'{}'", s.replace('\'', "'\\''"));
                d["apiKeyHelper"] = json!(format!(
                    "{} --credential {}",
                    quote(&helper.to_string_lossy()),
                    quote(reference)
                ));
            }
        }
        let mut bytes = serde_json::to_vec_pretty(&d)
            .map_err(|_| ConfigError::new("invalid_config", "无法生成 Claude 配置"))?;
        bytes.push(b'\n');
        Ok(bytes)
    } else if agent == "codex" {
        let mut d = parse_toml(before)?;
        if d.get("profile").is_some() {
            return fail(
                "unsupported",
                "Codex 默认 profile 会覆盖用户级路由，请先在 Codex 中处理该配置层；原文件未修改",
            );
        }
        if d.get("model_providers").is_some_and(|p| !p.is_table()) {
            return fail(
                "unsupported",
                "暂不改写内联 model_providers，请先使用普通 TOML 表；原文件未修改",
            );
        }
        if provider.is_some()
            && d.get("forced_login_method").and_then(Item::as_str) == Some("chatgpt")
        {
            return fail("unsupported", "原配置限制为 ChatGPT 登录，不会移除该限制");
        }
        for key in ["openai_base_url", "review_model"] {
            d.remove(key);
        }
        if provider.is_none() {
            if d.get("forced_login_method").and_then(Item::as_str) == Some("api")
                || d.get("chatgpt_base_url").is_some()
            {
                return fail(
                    "unsupported",
                    "原配置限制了登录方式或原生登录端点，请先在 Codex 中核查；不会移除该限制",
                );
            }
            d.remove("model");
            if let Some(providers) = d.get_mut("model_providers").and_then(Item::as_table_mut) {
                providers.remove("openai");
            }
        }
        if let Some(p) = provider {
            let id = format!("local_agent_{}", &super::super::fingerprint(p)[..24]);
            if d.get("model_providers").is_none() {
                d["model_providers"] = Item::Table(Table::new());
            }
            let mut t = Table::new();
            t["name"] = value(&p.name);
            t["base_url"] = value(&p.endpoint);
            t["wire_api"] = value("responses");
            t["supports_websockets"] = value(false);
            if let Some(reference) = &p.credential_ref {
                let mut auth = Table::new();
                auth["command"] = value(helper.to_string_lossy().as_ref());
                let mut args = toml_edit::Array::new();
                args.push("--credential");
                args.push(reference.as_str());
                auth["args"] = value(args);
                auth["timeout_ms"] = value(5000);
                t["auth"] = Item::Table(auth);
            }
            d["model_providers"][&id] = Item::Table(t);
            d["model_provider"] = value(id);
            d["model"] = value(&p.model);
        } else {
            d["model_provider"] = value("openai");
        }
        Ok(d.to_string().into_bytes())
    } else {
        fail("unsupported", "不支持此 Agent")
    }
}
