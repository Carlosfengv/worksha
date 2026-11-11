use super::hub::Hub;
use local_agent_core::{
    routing::{OpenCodeRouteProof, Turn},
    sessions::NativeSessions,
};
use serde_json::{Value, json};
use std::collections::HashMap;

const OPENCODE_NAMESPACES: [Option<&str>; 4] = [
    Some("opencode"),
    None,
    Some("opencode-cli"),
    Some("opencode-desktop"),
];

fn turn_digest(chain: &[String]) -> Result<String, &'static str> {
    Ok(local_agent_core::providers::files::hash(
        &serde_json::to_vec(chain).map_err(|_| "轮次标识不可用")?,
    ))
}

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
    agent: &str,
    product: Option<&str>,
) -> Result<(Turn, Vec<Turn>), &'static str> {
    validate(body, &Default::default())?;
    service_evidence(hub, body, session, agent, product)
}

/// Project Responses onto Messages only for routing evidence and budget checks.
/// Forward the original protocol to the normal gateway adapter.
pub(super) fn responses_projection(body: &Value) -> Result<Value, &'static str> {
    if body["reasoning"]["effort"]
        .as_str()
        .is_some_and(|effort| effort != "none")
    {
        return Err("模型服务采用文本与标准工具能力，请关闭思考模式");
    }
    if body["store"] == true {
        return Err("模型服务需要完整历史，不支持服务端保存的 Responses 会话");
    }
    if body
        .get("tools")
        .is_some_and(|v| !v.is_null() && !v.is_array())
        || body["input"].as_array().is_some_and(|items| {
            items.iter().any(|item| {
                matches!(item["type"].as_str(), None | Some("message"))
                    && !matches!(
                        item["role"].as_str(),
                        Some("user" | "assistant" | "developer" | "system")
                    )
            })
        })
    {
        return Err("Responses 模型请求的角色或工具格式无效");
    }
    let (_, portable) = crate::gateway::tools::Tools::prepare("responses", body)?;
    let mut messages = crate::gateway::protocol::request("responses", "messages", &portable, "")?;
    if body.get("max_output_tokens").is_none_or(Value::is_null) {
        messages.as_object_mut().unwrap().remove("max_tokens");
    }
    Ok(messages)
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
    agent: &str,
    product: Option<&str>,
) -> Result<(Turn, Vec<Turn>), &'static str> {
    let messages = body["messages"]
        .as_array()
        .ok_or("动态入口需要完整 Messages 历史")?;
    // The OpenCode binding and native session already scope the route.
    // Reuse the pre-split namespace and accept leases written by the split
    // clients, without affecting per-client attribution or policy matching.
    let product_namespace = [product];
    let namespaces: &[Option<&str>] = if agent == "opencode" && session.is_some() {
        &OPENCODE_NAMESPACES
    } else {
        &product_namespace
    };
    let mut chains = vec![Vec::new(); namespaces.len()];
    let mut continuation = false;
    for message in messages {
        if message["role"] != "user" {
            continue;
        }
        let content = &message["content"];
        let tool = content
            .as_array()
            .is_some_and(|parts| parts.iter().any(|p| p["type"] == "tool_result"));
        if tool {
            continuation = true;
            continue;
        }
        let single = json!({"messages":[message]});
        let hash = |namespace| hub.message_identity(&single, Some("dynamic-user-chain"), namespace);
        let Some(primary) = hash(namespaces[0]) else {
            // Codex sends environment and workspace instructions as user-role
            // items. Keep forwarding them, without creating a routing boundary.
            if local_agent_core::sessions::activity_input::preview(content).is_none() {
                continue;
            }
            return Err("用户轮次标识不可用；未选择新的路由");
        };
        continuation = false;
        chains[0].push(primary);
        if chains[0].len() > 256 {
            return Err("动态入口用户轮次超过上限，请新建会话");
        }
        for (chain, &namespace) in chains[1..].iter_mut().zip(&namespaces[1..]) {
            chain.push(hash(namespace).ok_or("用户轮次标识不可用；未选择新的路由")?);
        }
    }
    let portable = portable(&body["messages"]);
    let mut turns = chains
        .into_iter()
        .map(|chain| -> Result<Turn, &'static str> {
            Ok(Turn {
                session: session.clone(),
                digest: turn_digest(&chain)?,
                chain,
                continuation,
                portable,
            })
        });
    let turn = turns.next().expect("primary route identity")?;
    Ok((turn, turns.collect::<Result<_, _>>()?))
}

pub(super) fn opencode_text_groups(body: &Value, turn: &Turn) -> Option<Vec<Vec<String>>> {
    if !turn.portable || turn.continuation || !turn.session.as_deref()?.starts_with("opencode:") {
        return None;
    }
    let mut groups = Vec::new();
    let mut bytes = 0;
    for message in body["messages"]
        .as_array()?
        .iter()
        .filter(|m| m["role"] == "user")
    {
        let content = &message["content"];
        if content
            .as_array()
            .is_some_and(|parts| parts.iter().any(|p| p["type"] == "tool_result"))
        {
            continue;
        }
        if local_agent_core::sessions::activity_input::preview(content).is_none() {
            continue;
        }
        let text = if let Some(text) = content.as_str() {
            vec![text]
        } else {
            let parts = content.as_array()?;
            if parts.len() > 64 {
                return None;
            }
            parts
                .iter()
                .map(|part| {
                    (part["type"] == "text")
                        .then(|| part["text"].as_str())
                        .flatten()
                })
                .collect::<Option<Vec<_>>>()?
        };
        bytes += text.iter().map(|part| part.len()).sum::<usize>();
        if bytes > 512 * 1024 {
            return None;
        }
        groups.push(text.into_iter().map(str::to_owned).collect::<Vec<_>>());
    }
    (!groups.is_empty() && groups.len() == turn.chain.len()).then_some(groups)
}

/// The Anthropic client merges adjacent user messages when an assistant reply
/// is empty. Verify each user boundary against the authorized native history,
/// and keep the original wire digest for exact retries and persisted leases.
pub(super) struct OpenCodeHistory {
    pub route: OpenCodeRouteProof,
    pub intent: Option<String>,
    pub authority: (u64, u64),
}

pub(super) fn opencode_history(
    hub: &Hub,
    sessions: &NativeSessions,
    groups: &[Vec<String>],
    turn: &Turn,
    compatible: &[Turn],
) -> Option<OpenCodeHistory> {
    let native_id = turn.session.as_deref()?.strip_prefix("opencode:")?;
    let verified = sessions
        .opencode_routing_boundaries(native_id, groups)
        .ok()??;
    let boundaries = verified.ends;
    if !verified.v2 && !boundaries.iter().any(|group| group.len() > 1) {
        return None;
    }
    let total: usize = boundaries.iter().map(Vec::len).sum();
    if total < 2 {
        return None;
    }
    let mut prefixes = HashMap::new();
    for (namespace, identity) in OPENCODE_NAMESPACES
        .into_iter()
        .zip(std::iter::once(turn).chain(compatible))
    {
        let mut position = 0;
        for (index, (group, ends)) in groups.iter().zip(&boundaries).enumerate() {
            let mut chain = identity.chain[..=index].to_vec();
            for &end in ends {
                position += 1;
                if position == total {
                    break;
                }
                if end < group.len() {
                    let body = opencode_text_message(&group[..end]);
                    chain[index] =
                        hub.message_identity(&body, Some("dynamic-user-chain"), namespace)?;
                } else {
                    chain[index].clone_from(&identity.chain[index]);
                }
                prefixes.insert(turn_digest(&chain).ok()?, position);
            }
        }
    }
    let last = boundaries.last()?;
    let start = last.iter().rev().nth(1).copied().unwrap_or(0);
    let intent = local_agent_core::sessions::activity_input::intent_excerpt(
        &opencode_text_message(&groups.last()?[start..]),
    );
    let legacy_title_turns = if verified.v2 {
        let first = groups.first()?.first()?;
        let title = format!(
            "Here is the first message of the conversation:\n\n<message>\n{first}\n</message>\n\nReply with only the title."
        );
        let body = opencode_text_message(&[title]);
        OPENCODE_NAMESPACES
            .into_iter()
            .map(|namespace| {
                let identity =
                    hub.message_identity(&body, Some("dynamic-user-chain"), namespace)?;
                turn_digest(&[identity]).ok()
            })
            .collect::<Option<Vec<_>>>()?
    } else {
        Vec::new()
    };
    Some(OpenCodeHistory {
        route: OpenCodeRouteProof {
            prefixes,
            legacy_title_turns,
        },
        intent,
        authority: verified.authority,
    })
}

fn opencode_text_message(group: &[String]) -> Value {
    let parts: Vec<_> = group
        .iter()
        .map(|text| json!({"type":"text","text":text}))
        .collect();
    json!({"messages":[{"role":"user","content":parts}]})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opencode_native_turns_prove_prefixes_inside_merged_wire_messages() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let database = home.join(".local/share/opencode/opencode.db");
        std::fs::create_dir_all(database.parent().unwrap()).unwrap();
        let db = rusqlite::Connection::open(database).unwrap();
        db.execute_batch("CREATE TABLE session (id TEXT); CREATE TABLE message (id TEXT, session_id TEXT, time_created INTEGER, data TEXT); CREATE TABLE part (id TEXT, message_id TEXT, session_id TEXT, time_created INTEGER, data TEXT);").unwrap();
        for (i, texts) in
            (0_i64..).zip([&["history"][..], &["earlier", "more details"], &["latest"]])
        {
            db.execute(
                "INSERT INTO message VALUES (?1,'ses_test',?1,'{\"role\":\"user\"}')",
                [i],
            )
            .unwrap();
            for (part, text) in (0_i64..).zip(texts) {
                db.execute(
                    "INSERT INTO part VALUES (?1,?2,'ses_test',?1,?3)",
                    rusqlite::params![part, i, json!({"type":"text","text":text}).to_string()],
                )
                .unwrap();
            }
        }
        let hub =
            Hub::new(local_agent_core::observation::ObservationStore::open(dir.path()).unwrap())
                .unwrap();
        let session = Some("opencode:ses_test".into());
        let make = |texts: &[&str]| json!({"messages":[{"role":"user","content":"history"},{"role":"user","content":texts.iter().map(|text| json!({"type":"text","text":text})).collect::<Vec<_>>() }]});
        let body = make(&["earlier", "more details", "latest"]);
        let (current, compatible) =
            service_evidence(&hub, &body, session.clone(), "opencode", None).unwrap();
        let groups = opencode_text_groups(&body, &current).unwrap();
        let sessions = NativeSessions::new(home.clone(), &["opencode".into()]);
        let proof = opencode_history(&hub, &sessions, &groups, &current, &compatible).unwrap();
        let (older, variants) = service_evidence(
            &hub,
            &make(&["earlier", "more details"]),
            session.clone(),
            "opencode",
            None,
        )
        .unwrap();
        for previous in std::iter::once(older).chain(variants) {
            assert_eq!(proof.route.prefixes.get(&previous.digest), Some(&2));
        }
        let (cut, _) =
            service_evidence(&hub, &make(&["earlier"]), session, "opencode", None).unwrap();
        assert!(!proof.route.prefixes.contains_key(&cut.digest));
        assert!(!proof.route.prefixes.contains_key(&current.digest));
        assert_eq!(proof.intent.as_deref(), Some("latest"));

        let mut edited = groups.clone();
        edited[1][0] = "rewritten".into();
        assert!(opencode_history(&hub, &sessions, &edited, &current, &compatible).is_none());
        let disabled = NativeSessions::new(home, &[]);
        assert!(opencode_history(&hub, &disabled, &groups, &current, &compatible).is_none());
    }

    #[test]
    fn opencode_v2_title_recovery_requires_the_current_authorized_text_history() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let path = home.join(".local/share/opencode/opencode.db");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let db = rusqlite::Connection::open(path).unwrap();
        db.execute_batch("CREATE TABLE session_v2(id TEXT PRIMARY KEY); INSERT INTO session_v2 VALUES('ses_test'); CREATE TABLE session_message(session_id TEXT, type TEXT, seq INTEGER, data TEXT);").unwrap();
        for (seq, text) in [(1, "first user"), (3, "latest user")] {
            db.execute(
                "INSERT INTO session_message VALUES('ses_test','user',?1,?2)",
                rusqlite::params![
                    seq,
                    json!({"text":text,"files":[],"agents":[],"skills":[]}).to_string()
                ],
            )
            .unwrap();
        }
        let hub =
            Hub::new(local_agent_core::observation::ObservationStore::open(dir.path()).unwrap())
                .unwrap();
        let session = Some("opencode:ses_test".into());
        let first = opencode_text_message(&["first user".into()]);
        let mut body = first.clone();
        body["messages"]
            .as_array_mut()
            .unwrap()
            .push(json!({"role":"user","content":"latest user"}));
        let evidence = |body| {
            service_evidence(
                &hub,
                body,
                session.clone(),
                "opencode",
                Some("opencode-cli"),
            )
            .unwrap()
        };
        let (current, compatible) = evidence(&body);
        let groups = opencode_text_groups(&body, &current).unwrap();
        let sessions = NativeSessions::new(home, &["opencode".into()]);
        let verify = |groups: &[Vec<String>]| {
            opencode_history(&hub, &sessions, groups, &current, &compatible)
        };
        let proof = verify(&groups).unwrap();
        let title = opencode_text_message(&["Here is the first message of the conversation:\n\n<message>\nfirst user\n</message>\n\nReply with only the title.".into()]);
        assert_eq!(
            proof.route.prefixes.get(&evidence(&first).0.digest),
            Some(&1)
        );
        assert!(
            proof
                .route
                .legacy_title_turns
                .contains(&evidence(&title).0.digest)
        );
        assert!(sessions.opencode_routing_authorized(proof.authority));
        assert!(verify(&[vec!["first user".into()], vec!["forged".into()]]).is_none());
        sessions.set_enabled("opencode", false).unwrap();
        assert!(verify(&groups).is_none());
        sessions.set_enabled("opencode", true).unwrap();
        assert!(!sessions.opencode_routing_authorized(proof.authority));
        db.execute(
            "UPDATE session_message SET data=?1 WHERE seq=3",
            [json!({"text":"latest user","files":[{"name":"attachment"}]}).to_string()],
        )
        .unwrap();
        assert!(verify(&groups).is_none());
    }

    #[test]
    fn opencode_route_evidence_keeps_legacy_namespace_and_exposes_split_identities() {
        let dir = tempfile::tempdir().unwrap();
        let hub =
            Hub::new(local_agent_core::observation::ObservationStore::open(dir.path()).unwrap())
                .unwrap();
        let body = text_request();
        let (primary, compatible) = service_evidence(
            &hub,
            &body,
            Some("opencode:ses_test".into()),
            "opencode",
            Some("opencode-desktop"),
        )
        .unwrap();
        let old_identity = |product| {
            hub.message_identity(&body, Some("dynamic-user-chain"), product)
                .unwrap()
        };
        assert_eq!(primary.chain, [old_identity(Some("opencode"))]);
        let saved = compatible
            .iter()
            .map(|turn| turn.chain[0].clone())
            .collect::<Vec<_>>();
        assert_eq!(
            saved,
            [
                old_identity(None),
                old_identity(Some("opencode-cli")),
                old_identity(Some("opencode-desktop"))
            ]
        );
    }

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
        let turn = |body: &Value, product| {
            evidence(&hub, body, Some("s".into()), "claude", product).map(|(turn, _)| turn)
        };
        let a = json!({"max_tokens":30,"messages":[{"role":"user","content":[{"type":"text","text":"repeat","cache_control":{"type":"ephemeral"}}]}]});
        let first = turn(&a, Some("claude-code-cli")).unwrap();
        let mut tool = a.clone();
        tool["messages"][0]["content"][0]
            .as_object_mut()
            .unwrap()
            .remove("cache_control");
        tool["messages"].as_array_mut().unwrap().push(json!({"role":"user","content":[{"type":"tool_result","tool_use_id":"one","content":"private"}]}));
        let continuation = turn(&tool, Some("claude-code-cli")).unwrap();
        assert_eq!(first.chain, continuation.chain);
        assert!(continuation.continuation);
        tool["messages"]
            .as_array_mut()
            .unwrap()
            .push(json!({"role":"user","content":"repeat"}));
        let next = turn(&tool, Some("claude-code-cli")).unwrap();
        assert!(next.chain.starts_with(&first.chain));
        assert_eq!(next.chain.len(), 2);
        assert!(!next.continuation);
        assert!(!next.digest.contains("repeat"));
        let mut thinking = tool.clone();
        thinking["messages"].as_array_mut().unwrap().push(json!({"role":"assistant","content":[{"type":"thinking","thinking":"","signature":"opaque"}]}));
        assert!(!turn(&thinking, None).unwrap().portable);
        let mut image = a.clone();
        image["messages"][0]["content"] = json!([{"type":"image","source":{}}]);
        assert!(turn(&image, None).is_err());
    }
}
