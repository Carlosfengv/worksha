pub(super) use super::super::gateway::Scope;
use super::{Error, Server, conflict};
use crate::AppState;
use serde_json::Value;

pub(super) fn classify(provider: &Value, host: &str) -> Scope {
    let managed = provider["id"]
        .as_str()
        .is_some_and(|id| id.starts_with("local_agent_"));
    super::super::gateway::classify(provider["settings"]["baseURL"].as_str(), managed, host)
}

pub(super) async fn inspect(state: &AppState, server: &Server, route: &str) -> Scope {
    let Some((provider, _)) = route.split_once('/') else {
        return Scope::Unknown;
    };
    if !provider
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(&byte))
    {
        return Scope::Unknown;
    }
    match server.session_data(&format!("/provider/{provider}")).await {
        Ok(provider) => classify(&provider, &state.gateway.host),
        Err(_) => Scope::Unknown,
    }
}

pub(super) async fn require_current(
    state: &AppState,
    server: &Server,
    route: &str,
) -> Result<(), Error> {
    match inspect(state, server, route).await {
        Scope::Current | Scope::External => Ok(()),
        Scope::Other => Err(conflict(
            "OpenCode 的模型入口连接了另一个 App 网关。请在 Agents 中连接当前 App 后重试；未切换模型或发送消息。",
        )),
        Scope::Unknown => Err(conflict(
            "无法确认 OpenCode 模型入口的网关归属，请核对连接后重试；未发送消息。",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn gateway_scope_requires_the_exact_current_listener_and_a_valid_binding_path() {
        let mut provider = json!({"id":"local_agent_fixture","settings":{
            "baseURL":"http://127.0.0.1:4000/gateway/00000000-0000-4000-8000-000000000001/v1"
        }});
        assert_eq!(classify(&provider, "127.0.0.1:4000"), Scope::Current);
        assert_eq!(classify(&provider, "127.0.0.1:4001"), Scope::Other);
        for url in [
            "http://127.0.0.1:4000/gateway/not-a-binding/v1",
            "http://user:secret@127.0.0.1:4000/gateway/00000000-0000-4000-8000-000000000001/v1",
            "http://127.0.0.1:4000/gateway/00000000-0000-4000-8000-000000000001/v1?other=1",
        ] {
            provider["settings"]["baseURL"] = json!(url);
            assert_eq!(classify(&provider, "127.0.0.1:4000"), Scope::Unknown);
        }
        assert_eq!(
            classify(&json!({"id":"local_agent_fixture"}), "127.0.0.1:4000"),
            Scope::Unknown
        );
        assert_eq!(
            classify(
                &json!({"id":"vendor","settings":{"baseURL":"https://example.com/v1"}}),
                "127.0.0.1:4000"
            ),
            Scope::External
        );
    }
}
