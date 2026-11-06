use super::{
    config::Route,
    gateway::{self, Scope},
};
use local_agent_core::providers::{config, files};
use serde_json::Value;
use std::{collections::BTreeMap, path::Path};

pub(super) struct Config {
    pub env: BTreeMap<String, String>,
    pub credential: Option<String>,
}

impl Config {
    pub(super) fn require_current(&self, host: &str) -> Result<(), &'static str> {
        match gateway::classify(
            self.env.get("ANTHROPIC_BASE_URL").map(String::as_str),
            self.credential.is_some(),
            host,
        ) {
            Scope::Current | Scope::External => Ok(()),
            Scope::Other => Err(
                "Claude Code 连接了另一个 App 网关。请在 Agents 中连接当前 App 后重试；未切换模型或发送消息。",
            ),
            Scope::Unknown => {
                Err("无法确认 Claude Code 模型入口的网关归属，请核对连接后重试；未发送消息。")
            }
        }
    }
}

pub(super) fn scope(home: &Path, host: &str) -> Scope {
    source(home).map_or(Scope::Unknown, |doc| {
        gateway::classify(
            doc["env"]["ANTHROPIC_BASE_URL"].as_str(),
            doc["apiKeyHelper"]
                .as_str()
                .and_then(|helper| gateway_reference(helper, home))
                .is_some(),
            host,
        )
    })
}

const AUTH_REQUIRED: &str = "Claude Code 需要用户级配置中的 API 凭据或 Local Agent 凭据助手";

fn source(home: &Path) -> Result<Value, &'static str> {
    let path = config::path(home, "claude").map_err(|_| "无法定位 Claude Code 用户配置")?;
    let bytes = files::read(&path)
        .map_err(|_| "无法读取 Claude Code 用户配置")?
        .bytes
        .ok_or(AUTH_REQUIRED)?;
    serde_json::from_slice(&bytes).map_err(|_| "Claude Code 用户配置不是有效的 JSON")
}

fn model(doc: &Value) -> Result<&str, &'static str> {
    let env = &doc["env"];
    let missing = "请先在 Claude Code 用户配置中指定模型路由";
    let selected = env["ANTHROPIC_MODEL"]
        .as_str()
        .filter(|value| !value.is_empty())
        .or_else(|| doc["model"].as_str().filter(|value| !value.is_empty()))
        .or_else(|| {
            env["ANTHROPIC_DEFAULT_MODEL"]
                .as_str()
                .filter(|value| !value.is_empty())
        })
        .ok_or(missing)?;
    let resolved = match selected {
        "haiku" => env["ANTHROPIC_DEFAULT_HAIKU_MODEL"].as_str(),
        "sonnet" => env["ANTHROPIC_DEFAULT_SONNET_MODEL"].as_str(),
        "opus" => env["ANTHROPIC_DEFAULT_OPUS_MODEL"].as_str(),
        "fable" => env["ANTHROPIC_DEFAULT_FABLE_MODEL"].as_str(),
        _ => None,
    }
    .filter(|value| !value.is_empty())
    .unwrap_or(selected);
    if resolved.len() <= 160
        && !resolved.starts_with('-')
        && resolved
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_./:[]".contains(&b))
    {
        Ok(resolved)
    } else {
        Err(missing)
    }
}

fn gateway_reference(helper: &str, home: &Path) -> Option<String> {
    let (path, reference) = helper
        .strip_prefix('\'')?
        .strip_suffix('\'')?
        .split_once("' --credential '")?;
    let helper_path = Path::new(path);
    let root = dirs::data_dir().or_else(|| Some(home.join("Library/Application Support")))?;
    let relative = helper_path.strip_prefix(root).ok()?;
    let mut parts = relative.iter();
    if !parts.next()?.to_str()?.starts_with("app.localagent.")
        || parts.next()? != "provider-configs"
        || parts.next()? != "bin"
        || !parts.next()?.to_str()?.starts_with("credential-helper-")
        || parts.next().is_some()
        || uuid::Uuid::parse_str(reference).is_err()
    {
        return None;
    }
    Some(reference.into())
}

pub(super) fn routes(home: &Path) -> Result<Vec<Route>, &'static str> {
    let doc = source(home)?;
    let model = model(&doc)?;
    load(&doc, home, model)?;
    Ok(vec![Route {
        id: model.into(),
        label: model.into(),
        is_default: true,
    }])
}

pub(super) fn runtime(home: &Path, route: &str) -> Result<Config, &'static str> {
    load(&source(home)?, home, route)
}

fn load(doc: &Value, home: &Path, route: &str) -> Result<Config, &'static str> {
    if model(doc)? != route {
        return Err("所选 Claude Code 模型已从当前配置移除，原任务不会切换路由");
    }
    let native = &doc["env"];
    for flag in [
        "CLAUDE_CODE_USE_BEDROCK",
        "CLAUDE_CODE_USE_VERTEX",
        "CLAUDE_CODE_USE_FOUNDRY",
        "CLAUDE_CODE_USE_GATEWAY",
        "CLAUDE_CODE_OAUTH_TOKEN",
    ] {
        if native[flag]
            .as_str()
            .is_some_and(|value| !["", "0", "false"].contains(&value))
        {
            return Err("任务工作台当前支持 Claude Code 的 API Key 和本机网关凭据");
        }
    }
    let mut env = BTreeMap::<String, String>::new();
    for key in [
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_AUTH_TOKEN",
        "ANTHROPIC_BASE_URL",
        "ANTHROPIC_CUSTOM_HEADERS",
        "CLAUDE_CODE_MAX_CONTEXT_TOKENS",
        "CLAUDE_CODE_MAX_OUTPUT_TOKENS",
    ] {
        if let Some(value) = native[key]
            .as_str()
            .filter(|value| !value.is_empty() && !value.contains('\0'))
        {
            env.insert(key.into(), value.into());
        }
    }
    let endpoint = env.get("ANTHROPIC_BASE_URL").map_or(Ok(None), |url| {
        url::Url::parse(url)
            .map(Some)
            .map_err(|_| "Claude Code 路由地址无效")
    })?;
    let local = endpoint
        .as_ref()
        .is_some_and(|url| matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]")));
    if endpoint.as_ref().is_some_and(|url| {
        !(url.scheme() == "https" || (url.scheme() == "http" && local))
            || !url.username().is_empty()
            || url.password().is_some()
    }) {
        return Err("Claude Code 路由必须使用 HTTPS 或本机 HTTP");
    }
    let credential =
        if env.contains_key("ANTHROPIC_API_KEY") || env.contains_key("ANTHROPIC_AUTH_TOKEN") {
            None
        } else {
            let helper = doc["apiKeyHelper"].as_str().ok_or(AUTH_REQUIRED)?;
            Some(
                gateway_reference(helper, home)
                    .filter(|_| local)
                    .ok_or(AUTH_REQUIRED)?,
            )
        };
    if local {
        env.insert("CLAUDE_CODE_DISABLE_THINKING".into(), "1".into());
        env.insert("MAX_THINKING_TOKENS".into(), "0".into());
    }
    Ok(Config { env, credential })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_gateway_checks_reject_other_and_malformed_listener_before_execution() {
        let mut source = serde_json::json!({"model":"fixture","env":{"ANTHROPIC_API_KEY":"fixture","ANTHROPIC_BASE_URL":"http://127.0.0.1:4000/gateway/00000000-0000-4000-8000-000000000001"}});
        let config = load(&source, Path::new("/"), "fixture").unwrap();
        assert!(config.require_current("127.0.0.1:4000").is_ok());
        assert!(config.require_current("127.0.0.1:4001").is_err());
        source["env"]["ANTHROPIC_BASE_URL"] = "http://127.0.0.1:4000/gateway/invalid".into();
        assert!(
            load(&source, Path::new("/"), "fixture")
                .unwrap()
                .require_current("127.0.0.1:4000")
                .is_err()
        );
        assert_eq!(source["model"], "fixture");
    }

    #[test]
    fn claude_alias_pins_the_resolved_model_and_rejects_a_changed_alias() {
        let mut source = serde_json::json!({
            "model":"sonnet",
            "env":{
                "ANTHROPIC_DEFAULT_SONNET_MODEL":"fixture-old-model",
                "ANTHROPIC_DEFAULT_MODEL":"unused-default",
                "ANTHROPIC_API_KEY":"fixture"
            }
        });
        assert_eq!(model(&source), Ok("fixture-old-model"));
        assert!(load(&source, Path::new("/"), "fixture-old-model").is_ok());
        source["env"]["ANTHROPIC_DEFAULT_SONNET_MODEL"] = "fixture-new-model".into();
        assert!(load(&source, Path::new("/"), "fixture-old-model").is_err());
    }
}
