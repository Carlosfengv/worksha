use crate::AppState;
use local_agent_core::providers::config::NativeConfig;
use serde::Serialize;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Connection {
    agent_id: String,
    mode: &'static str,
    provider_name: String,
    model: Option<String>,
    endpoint: Option<String>,
    reason: Option<String>,
    active_requests: usize,
    active_by_product: std::collections::HashMap<String, usize>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Connections {
    checked_at: String,
    error: Option<&'static str>,
    items: Vec<Connection>,
}

fn declared_mode(config: &NativeConfig) -> &'static str {
    if config.state != "ready" || ["external", "profile"].contains(&config.mode.as_str()) {
        return "unknown";
    }
    // OpenCode's native catalog includes third-party providers and local proxies.
    if config.agent_id == "opencode" {
        return if config.model.is_some() || config.endpoint.is_some() {
            "direct"
        } else {
            "unknown"
        };
    }
    let official = match config.agent_id.as_str() {
        "claude" | "claude-desktop" => "https://api.anthropic.com",
        "codex" => "https://api.openai.com/v1",
        _ => return "unknown",
    };
    if config.mode == "native"
        && config
            .endpoint
            .as_deref()
            .is_none_or(|endpoint| endpoint == official)
    {
        "official_login"
    } else if config.endpoint.as_deref() == Some(official)
        || (config.endpoint.is_none()
            && config.mode == "api"
            && (config.agent_id == "claude"
                || (config.agent_id == "codex" && config.provider_name == "openai")))
    {
        "official_api"
    } else if config.endpoint.is_some() || config.mode == "api" {
        "direct"
    } else {
        "unknown"
    }
}

pub fn snapshot(state: &AppState) -> Connections {
    let checked_at = chrono::Utc::now().to_rfc3339();
    let Ok(manager) = state.providers.try_lock() else {
        return Connections {
            checked_at,
            error: Some("连接配置正在更新，暂时无法确认"),
            items: vec![],
        };
    };
    let configs = manager.inspect_configs();
    drop(manager);
    let active_counts = state.observation.fresh_active_counts();
    let items = configs
        .into_iter()
        .map(|config| {
            let mut mode = declared_mode(&config);
            let mut provider_name = config.provider_name.clone();
            let mut reason = config.error.clone();
            if config.state == "ready"
                && !["external", "profile"].contains(&config.mode.as_str())
                && let Some(endpoint) = &config.endpoint
            {
                match state
                    .gateway
                    .configured_provider(&config.agent_id, endpoint)
                {
                    Ok(Some(provider)) => {
                        mode = "gateway";
                        provider_name = provider.name;
                    }
                    Ok(None) => {
                        match state
                            .store
                            .lock()
                            .ok()
                            .and_then(|s| s.dynamic_bindings().ok())
                        {
                            Some(bindings) => {
                                if let Some(b) = bindings.iter().find(|b| {
                                    b.agent_id == config.agent_id
                                        && b.scope == config.path
                                        && crate::gateway::dynamic::configured_endpoint(state, b)
                                            == *endpoint
                                }) {
                                    mode = "gateway";
                                    provider_name = b.target.provider.name.clone();
                                    if !crate::plugin_host::features::active(state,"builtin.model-services") {
                                        match state.store.lock().ok().and_then(|store|store.model_service_attachment(&b.id).ok()) {
                                            Some(Some(_))=>{mode="unknown";reason=Some("此连接绑定的模型服务插件已停用或卸载，请在插件中心恢复".into());},
                                            None=>{mode="unknown";reason=Some("模型服务绑定状态待核查".into());},
                                            Some(None)=>{},
                                        }
                                    }
                                }
                            }
                            None => {
                                mode = "unknown";
                                reason = Some("动态绑定状态不可读取".into());
                            }
                        }
                    }
                    Err(error) => {
                        mode = "unknown";
                        reason = Some(error.message);
                    }
                }
            }
            if config.state == "absent" {
                reason = Some("未发现用户级连接配置，不能确认启动环境中的实际连接".into());
            }
            if config.mode == "external" {
                reason = Some("检测到云平台或外部网关覆盖，当前实际路由未确认".into());
            }
            if config.mode == "profile" {
                reason = Some("存在默认 Profile，当前配置层的实际连接未确认".into());
            }
            let product = match config.agent_id.as_str() {
                "claude" => "claude-code-cli",
                "claude-desktop" => "claude-code-desktop",
                _ => "",
            };
            let active_requests = if ["codex", "opencode"].contains(&config.agent_id.as_str()) {
                active_counts
                    .iter()
                    .filter(|(id, _)| {
                        local_agent_core::agents::family(id) == Some(config.agent_id.as_str())
                    })
                    .map(|(_, count)| count)
                    .sum()
            } else {
                active_counts.get(product).copied().unwrap_or(0)
            };
            Connection {
                agent_id: config.agent_id,
                mode,
                provider_name,
                model: config.model,
                endpoint: config.endpoint,
                reason,
                active_requests,
                active_by_product: active_counts.clone(),
            }
        })
        .collect();
    Connections {
        checked_at,
        error: None,
        items,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_catalog_missing_config_and_external_gateway_are_not_subscriptions() {
        let home = tempfile::tempdir().unwrap();
        let mut config = local_agent_core::providers::config::inspect(
            &home.path().canonicalize().unwrap(),
            "codex",
        );
        assert_eq!(config.state, "absent");
        assert_eq!(declared_mode(&config), "unknown");
        config.state = "ready".into();
        assert_eq!(declared_mode(&config), "official_login");
        config.mode = "api".into();
        assert_eq!(declared_mode(&config), "official_api");
        config.agent_id = "opencode".into();
        config.model = Some("privatemode-ai/model".into());
        assert_eq!(declared_mode(&config), "direct");
        config.mode = "external".into();
        assert_eq!(declared_mode(&config), "unknown");
        config.mode = "api".into();
        config.agent_id = "claude".into();
        config.endpoint = Some("https://api.anthropic.com".into());
        assert_eq!(declared_mode(&config), "official_api");
        config.endpoint = Some("http://localhost:8080/v1".into());
        assert_eq!(declared_mode(&config), "direct");
        config.state = "unavailable".into();
        assert_eq!(declared_mode(&config), "unknown");
    }
}
