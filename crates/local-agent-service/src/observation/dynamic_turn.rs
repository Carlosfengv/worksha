use super::hub::Hub;
use local_agent_core::routing::Turn;
use serde_json::{Value, json};

fn portable(value: &Value) -> bool {
    match value {
        Value::Object(v) => {
            !matches!(
                v.get("type").and_then(Value::as_str),
                Some("thinking" | "redacted_thinking" | "reasoning")
            ) && v.get("encrypted_content").is_none_or(|v| v.is_null())
                && v.values().all(portable)
        }
        Value::Array(v) => v.iter().all(portable),
        _ => true,
    }
}
pub(super) fn evidence(
    hub: &Hub,
    body: &Value,
    session: Option<String>,
    product: Option<&str>,
) -> Result<Turn, &'static str> {
    validate(body, &Default::default())?;
    service_evidence(hub, body, session, product)
}
fn tool_history(body: &Value) -> bool {
    body["messages"].as_array().is_some_and(|messages| {
        messages.iter().any(|m| {
            m["content"].as_array().is_some_and(|parts| {
                parts
                    .iter()
                    .any(|p| matches!(p["type"].as_str(), Some("tool_use" | "tool_result")))
            })
        })
    })
}
fn standard_tools(body: &Value) -> Result<(), &'static str> {
    if body.get("tools").is_some_and(|tools| {
        !tools.as_array().is_some_and(|tools| {
            tools.iter().all(|tool| {
                tool.get("type").is_none_or(|kind| kind == "custom")
                    && tool["name"].is_string()
                    && tool["input_schema"].is_object()
            })
        })
    }) {
        return Err("此模型入口只发布标准函数工具，不接受托管或未知工具类型");
    }
    Ok(())
}
fn optional_tools(body: &Value) -> bool {
    match body.get("tool_choice") {
        None | Some(Value::Null) => true,
        Some(Value::Object(choice)) => {
            matches!(
                choice.get("type").and_then(Value::as_str),
                Some("auto" | "none")
            ) && choice
                .keys()
                .all(|key| matches!(key.as_str(), "type" | "disable_parallel_tool_use"))
                && choice
                    .get("disable_parallel_tool_use")
                    .is_none_or(Value::is_boolean)
        }
        _ => false,
    }
}
pub(super) fn prepare(
    body: &mut Value,
    caps: &local_agent_core::model_services::Capabilities,
) -> Result<(), &'static str> {
    if !caps.tools {
        if tool_history(body) {
            return Err(
                "此模型入口未发布工具调用能力；当前会话含工具历史，请新建聊天会话或选择支持工具的模型",
            );
        }
        if !optional_tools(body) {
            return Err(
                "此模型入口仅支持文本聊天，不能强制或使用未知方式调用工具；请选择支持工具的模型",
            );
        }
        standard_tools(body)?;
        // Clients such as OpenCode advertise tools even when tool_call=false.
        // An automatic choice permits a text-only response; keep that path
        // usable without publishing or forwarding unsupported tool capability.
        if let Some(body) = body.as_object_mut() {
            for field in ["tools", "tool_choice", "parallel_tool_calls"] {
                body.remove(field);
            }
        }
    }
    validate(body, caps)
}
pub(super) fn validate(
    body: &Value,
    caps: &local_agent_core::model_services::Capabilities,
) -> Result<(), &'static str> {
    if matches!(
        body["thinking"]["type"].as_str(),
        Some("enabled" | "adaptive")
    ) {
        return Err("当前动态槽位采用文本工具能力，请关闭思考模式或使用固定模型连接");
    }
    let _output = body["max_tokens"]
        .as_u64()
        .filter(|v| *v > 0 && *v <= u64::from(caps.max_output_tokens))
        .ok_or("请求输出量超过此模型入口已声明的上限")?;
    if !caps.tools
        && (body["tools"].as_array().is_some_and(|t| !t.is_empty()) || tool_history(body))
    {
        return Err("此模型入口未发布工具调用能力");
    }
    standard_tools(body)?;
    fn text_content(value: &Value) -> bool {
        value.is_string()
            || value.as_array().is_some_and(|parts| {
                parts.iter().all(|part| match part["type"].as_str() {
                    Some("text") => part["text"].is_string(),
                    // Function arguments are arbitrary JSON, not modality declarations.
                    Some("tool_use") => true,
                    Some("tool_result") => part.get("content").is_none_or(text_content),
                    // Historical signed content is retained and pins its old route.
                    Some("thinking" | "redacted_thinking") => true,
                    _ => false,
                })
            })
    }
    if body["messages"]
        .as_array()
        .is_none_or(|messages| !messages.iter().all(|m| text_content(&m["content"])))
        || body
            .get("system")
            .is_some_and(|system| !text_content(system))
    {
        return Err("当前动态槽位只发布文本与标准工具能力，请使用支持此输入的固定模型");
    }
    Ok(())
}
pub(super) fn service_evidence(
    hub: &Hub,
    body: &Value,
    session: Option<String>,
    product: Option<&str>,
) -> Result<Turn, &'static str> {
    let messages = body["messages"]
        .as_array()
        .ok_or("动态入口需要完整 Messages 历史")?;
    let mut chain = vec![];
    let mut continuation = false;
    for message in messages {
        if message["role"] != "user" {
            continue;
        }
        let tool = message["content"]
            .as_array()
            .is_some_and(|parts| parts.iter().any(|p| p["type"] == "tool_result"));
        continuation = tool;
        if tool {
            continue;
        }
        let single = json!({"messages":[message]});
        let hash = hub
            .message_identity(&single, Some("dynamic-user-chain"), product)
            .ok_or("用户轮次标识不可用；未选择新的路由")?;
        chain.push(hash);
        if chain.len() > 256 {
            return Err("动态入口用户轮次超过上限，请新建会话");
        }
    }
    let digest = local_agent_core::providers::files::hash(
        &serde_json::to_vec(&chain).map_err(|_| "轮次标识不可用")?,
    );
    Ok(Turn {
        session,
        digest,
        chain,
        continuation,
        portable: portable(&body["messages"]),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn text_request() -> Value {
        json!({"max_tokens":30,"stream":true,"system":[{"type":"text","text":"Keep this system prompt"}],"tools":[{"name":"read","input_schema":{"type":"object"}}],"messages":[{"role":"user","content":"Keep this user input"}]})
    }
    #[test]
    fn text_only_services_negotiate_optional_tools_without_changing_messages_or_limits() {
        let caps = local_agent_core::model_services::Capabilities {
            tools: false,
            ..Default::default()
        };
        for choice in [
            None,
            Some(Value::Null),
            Some(json!({"type":"auto"})),
            Some(json!({"type":"none"})),
            Some(json!({"type":"auto","disable_parallel_tool_use":true})),
        ] {
            let mut body = text_request();
            if let Some(choice) = choice {
                body["tool_choice"] = choice;
            }
            body["parallel_tool_calls"] = json!(false);
            let mut expected = body.clone();
            for field in ["tools", "tool_choice", "parallel_tool_calls"] {
                expected.as_object_mut().unwrap().remove(field);
            }
            prepare(&mut body, &caps).unwrap();
            assert_eq!(body, expected);
        }
        let mut enabled = text_request();
        enabled["tool_choice"] = json!({"type":"any"});
        let before = enabled.clone();
        prepare(&mut enabled, &Default::default()).unwrap();
        assert_eq!(enabled, before, "Published tools must remain intact");
    }
    #[test]
    fn text_only_services_reject_forced_unknown_and_historical_tools_without_erasing_them() {
        let caps = local_agent_core::model_services::Capabilities {
            tools: false,
            ..Default::default()
        };
        for choice in [
            json!({"type":"any"}),
            json!({"type":"tool","name":"read"}),
            json!("required"),
            json!({"type":"future"}),
            json!({"type":"auto","name":"read"}),
            json!({"type":"auto","disable_parallel_tool_use":"yes"}),
        ] {
            for has_definitions in [false, true] {
                let mut body = text_request();
                if !has_definitions {
                    body.as_object_mut().unwrap().remove("tools");
                }
                body["tool_choice"] = choice.clone();
                let before = body.clone();
                assert!(prepare(&mut body, &caps).is_err());
                assert_eq!(body, before);
            }
        }
        for content in [
            json!({"type":"tool_use","id":"one","name":"read","input":{}}),
            json!({"type":"tool_result","tool_use_id":"one","content":"retained"}),
        ] {
            let mut body = text_request();
            body["messages"][0]["content"] = json!([content]);
            let before = body.clone();
            assert!(
                prepare(&mut body, &caps)
                    .unwrap_err()
                    .contains("新建聊天会话")
            );
            assert_eq!(body, before);
        }
        for tools in [
            json!({}),
            Value::Null,
            json!([{"name":"read"}]),
            json!([{"type":"web_search_20250305","name":"web_search","input_schema":{}}]),
        ] {
            let mut body = text_request();
            body["tools"] = tools;
            let before = body.clone();
            assert!(prepare(&mut body, &caps).is_err());
            assert_eq!(body, before);
        }
        for field in ["thinking", "max_tokens", "messages"] {
            let mut body = text_request();
            body[field] = match field {
                "thinking" => json!({"type":"enabled","budget_tokens":1024}),
                "max_tokens" => json!(caps.max_output_tokens + 1),
                _ => json!([{"role":"user","content":[{"type":"image","source":{}}]}]),
            };
            assert!(prepare(&mut body, &caps).is_err());
        }
    }
    #[test]
    fn service_limits_enforce_tools_context_modalities_and_output() {
        let mut caps = local_agent_core::model_services::Capabilities {
            context_window: 96000,
            max_output_tokens: 4096,
            tools: false,
            ..Default::default()
        };
        let text =
            json!({"max_tokens":30,"messages":[{"role":"user","content":"x".repeat(60000)}]});
        assert!(validate(&text, &caps).is_ok());
        caps.context_window = 32000;
        assert!(
            super::super::context_budget::inspect(&text, &caps, None)
                .unwrap()
                .input_tokens
                > 0
        );
        caps.context_window = 96000;
        let tools = json!({"max_tokens":30,"tools":[{"name":"read","input_schema":{"type":"object"}}],"messages":[{"role":"user","content":"one"}]});
        assert!(validate(&tools, &caps).is_err());
        let history = json!({"max_tokens":30,"messages":[{"role":"user","content":[{"type":"tool_result","tool_use_id":"a","content":"one"}]}]});
        assert!(validate(&history, &caps).is_err());
        caps.tools = true;
        assert!(validate(&history, &caps).is_ok());
        assert!(validate(&json!({"max_tokens":4097,"messages":[]}), &caps).is_err());
        assert!(validate(&json!({"max_tokens":30,"messages":[{"role":"user","content":[{"type":"image","source":{}}]}]}),&caps).is_err());
        assert!(validate(&json!({"max_tokens":30,"messages":[{"role":"user","content":[{"type":"input_audio","data":"fixture"}]}]}),&caps).is_err());
        assert!(validate(&json!({"max_tokens":30,"tools":[{"type":"web_search_20250305","name":"web_search"}],"messages":[]}),&caps).is_err());
        assert!(validate(&json!({"max_tokens":30,"messages":[{"role":"assistant","content":[{"type":"tool_use","id":"a","name":"read","input":{"type":"image"}}]}]}),&caps).is_ok());
    }
    #[tokio::test]
    async fn canonical_user_chain_keeps_tool_results_and_cache_hints_out_of_new_turns() {
        let dir = tempfile::tempdir().unwrap();
        let hub =
            Hub::new(local_agent_core::observation::ObservationStore::open(dir.path()).unwrap())
                .unwrap();
        let a = json!({"max_tokens":30,"messages":[{"role":"user","content":[{"type":"text","text":"repeat","cache_control":{"type":"ephemeral"}}]}]});
        let first = evidence(&hub, &a, Some("s".into()), Some("claude-code-cli")).unwrap();
        let mut tool = a.clone();
        tool["messages"][0]["content"][0]
            .as_object_mut()
            .unwrap()
            .remove("cache_control");
        tool["messages"].as_array_mut().unwrap().push(json!({"role":"user","content":[{"type":"tool_result","tool_use_id":"one","content":"private"}]}));
        let continuation =
            evidence(&hub, &tool, Some("s".into()), Some("claude-code-cli")).unwrap();
        assert_eq!(first.chain, continuation.chain);
        assert!(continuation.continuation);
        tool["messages"]
            .as_array_mut()
            .unwrap()
            .push(json!({"role":"user","content":"repeat"}));
        let next = evidence(&hub, &tool, Some("s".into()), Some("claude-code-cli")).unwrap();
        assert!(next.chain.starts_with(&first.chain));
        assert_eq!(next.chain.len(), 2);
        assert!(!next.continuation);
        assert!(!next.digest.contains("repeat"));
        let mut thinking = tool.clone();
        thinking["messages"].as_array_mut().unwrap().push(json!({"role":"assistant","content":[{"type":"thinking","thinking":"","signature":"opaque"}]}));
        assert!(
            !evidence(&hub, &thinking, Some("s".into()), None)
                .unwrap()
                .portable
        );
        let mut image = a.clone();
        image["messages"][0]["content"] = json!([{"type":"image","source":{}}]);
        assert!(evidence(&hub, &image, Some("s".into()), None).is_err());
    }
}
