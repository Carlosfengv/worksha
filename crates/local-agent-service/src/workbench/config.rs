use local_agent_core::providers::{config as native_config, files};
use serde::Serialize;
use serde_json::{Value, json};
use std::path::Path;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Route {
    pub(super) id: String,
    pub(super) label: String,
    pub(super) is_default: bool,
}

fn config(home: &Path) -> Result<Value, &'static str> {
    let path = native_config::path(home, "opencode")
        .map_err(|_| "任务工作台需要 OpenCode 的用户级 JSON 配置")?;
    let bytes = files::read(&path)
        .map_err(|_| "无法读取 OpenCode 用户配置")?
        .bytes;
    bytes.map_or(Ok(json!({})), |bytes| {
        serde_json::from_slice(&bytes).map_err(|_| "OpenCode 用户配置不是有效的 JSON")
    })
}

fn valid_part(part: &str) -> bool {
    !part.is_empty()
        && part.len() <= 160
        && part
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_./:".contains(&byte))
}

fn route_ids(config: &Value) -> Vec<Route> {
    let default = config["model"].as_str();
    let mut routes = Vec::new();
    if let Some(providers) = config["provider"].as_object() {
        for (provider_id, provider) in providers {
            if !valid_part(provider_id) || provider_id.contains('/') {
                continue;
            }
            let provider_name = provider["name"].as_str().unwrap_or(provider_id);
            if let Some(models) = provider["models"].as_object() {
                for (model_id, model) in models {
                    if !valid_part(model_id) || routes.len() >= 512 {
                        continue;
                    }
                    let id = format!("{provider_id}/{model_id}");
                    let model_name = model["name"].as_str().unwrap_or(model_id);
                    routes.push(Route {
                        is_default: default == Some(id.as_str()),
                        id,
                        label: local_agent_core::sessions::sanitize_label(
                            &format!("{provider_name} · {model_name}"),
                            160,
                        ),
                    });
                }
            }
        }
    }
    if let Some((id, (provider, model))) = default.and_then(|id| Some((id, id.split_once('/')?)))
        && valid_part(provider)
        && valid_part(model)
        && !routes.iter().any(|route| route.id == id)
    {
        routes.push(Route {
            id: id.into(),
            label: id.into(),
            is_default: true,
        });
    }
    routes.sort_by(|a, b| b.is_default.cmp(&a.is_default).then(a.label.cmp(&b.label)));
    routes
}

pub(super) fn routes(home: &Path) -> Result<Vec<Route>, &'static str> {
    config(home).map(|value| route_ids(&value))
}

pub(super) fn runtime(home: &Path, route: &str) -> Result<Value, &'static str> {
    let source = config(home)?;
    if !route_ids(&source).iter().any(|entry| entry.id == route) {
        return Err("所选模型路由已不在 OpenCode 配置中，请核对原路由");
    }
    let (provider_id, _) = route.split_once('/').ok_or("模型路由无效")?;
    let mut provider = source["provider"][provider_id].clone();
    if provider.is_null() {
        let auth = files::read(&home.join(".local/share/opencode/auth.json"))
            .ok()
            .and_then(|file| file.bytes)
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok());
        if let Some(entry) = auth.as_ref().and_then(|auth| auth.get(provider_id)) {
            if entry["type"] != "api" {
                return Err(
                    "该模型路由需要原生登录凭据，任务工作台目前支持 API Key 和已配置的网关路由",
                );
            }
            if let Some(key) = entry["key"].as_str() {
                provider = json!({"options":{"apiKey":key}});
            }
        }
    }
    let permission = json!({
        "*": "deny",
        "read": {"*":"allow", "*.env":"deny", "*.env.*":"deny"},
        "glob":"allow", "grep":"allow", "list":"allow",
        "edit":"ask", "bash":"ask", "external_directory":"deny"
    });
    let mut config = json!({
        "model":route, "small_model":route, "enabled_providers":[provider_id],
        "autoupdate":false, "permission":permission,
        "agent":{"build":{"permission":permission}}
    });
    if let Some(package) = provider["npm"].as_str()
        && !matches!(
            package,
            "@ai-sdk/anthropic"
                | "@ai-sdk/openai"
                | "@ai-sdk/openai-compatible"
                | "@ai-sdk/google"
                | "@ai-sdk/azure"
                | "@ai-sdk/amazon-bedrock"
                | "@ai-sdk/google-vertex"
                | "@ai-sdk/mistral"
                | "@ai-sdk/groq"
                | "@ai-sdk/deepseek"
                | "@ai-sdk/xai"
        )
    {
        return Err("此路由依赖自定义 OpenCode 供应商扩展，任务工作台无法加载");
    }
    if provider.is_object() {
        config["provider"] = json!({(provider_id):provider});
    }
    Ok(config)
}
