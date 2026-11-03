//! Bounded, redacted user-input excerpts for the activity surface.
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct UserInput {
    pub text: String,
    pub at: String,
}

pub fn tool_name(name: &str) -> Option<String> {
    let bounded = name.chars().take(1024).collect::<String>();
    let name = super::parser::safe_text(&bounded, 100).0;
    let name = name.split_whitespace().collect::<Vec<_>>().join(" ");
    (!name.is_empty()).then_some(name)
}

fn question_answers(text: &str) -> Option<String> {
    #[derive(Deserialize)]
    struct Reply {
        answer: String,
    }
    if text.len() > 1024 * 1024 {
        return None;
    }
    let body = text
        .strip_prefix("<send_user_message_question_reply>")?
        .strip_suffix("</send_user_message_question_reply>")?;
    let replies: Vec<Reply> = serde_json::from_str(body).ok()?;
    if replies.is_empty() || replies.len() > 64 {
        return None;
    }
    Some(
        replies
            .iter()
            .map(|reply| reply.answer.as_str())
            .collect::<Vec<_>>()
            .join(" "),
    )
}

pub fn preview(content: &Value) -> Option<String> {
    let mut texts = Vec::new();
    match content {
        Value::String(text) => texts.push(text.as_str()),
        Value::Array(parts) => {
            for part in parts.iter().take(64) {
                if matches!(part["type"].as_str(), Some("text" | "input_text")) {
                    if let Some(text) = part["text"].as_str() {
                        texts.push(text);
                    }
                } else if matches!(
                    part["type"].as_str(),
                    Some("image" | "image_url" | "input_image")
                ) {
                    texts.push("[图片]");
                }
            }
        }
        _ => {}
    }
    let mut excerpt = String::new();
    for text in texts {
        let text = text.trim();
        // Desktop question replies carry the user's answer inside a transport
        // envelope. Question metadata must not displace the actual input.
        let answer = if text.starts_with("<send_user_message_question_reply>") {
            let Some(answer) = question_answers(text) else {
                continue;
            };
            Some(answer)
        } else {
            None
        };
        let text = answer.as_deref().unwrap_or(text);
        let text = text
            .split_once("## My request:")
            .map_or(text, |(_, request)| request)
            .trim();
        if text.is_empty()
            || super::parser::is_context(text)
            || text.starts_with("[Request interrupted")
            || text.starts_with("<local-command-")
            || text.starts_with("<command-name>")
        {
            continue;
        }
        if !excerpt.is_empty() {
            excerpt.push(' ');
        }
        let remaining = 16_384_usize.saturating_sub(excerpt.chars().count());
        excerpt.extend(text.chars().take(remaining));
        if excerpt.chars().count() >= 16_384 {
            break;
        }
    }
    let (text, truncated) = super::parser::safe_text(&excerpt, 240);
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    (!text.is_empty()).then(|| format!("{text}{}", if truncated { "…" } else { "" }))
}

/// Tool results, system messages and serialized arguments are never candidates.
pub fn request(request: &Value) -> Option<String> {
    let messages = request["messages"]
        .as_array()
        .or_else(|| request["input"].as_array());
    if let Some(messages) = messages {
        for item in messages.iter().rev().take(256) {
            if item["role"] != "user" {
                continue;
            }
            if item["content"]
                .as_array()
                .is_some_and(|parts| parts.iter().any(|p| p["type"] == "tool_result"))
            {
                continue;
            }
            if let Some(text) = preview(&item["content"]) {
                return Some(text);
            }
        }
        None
    } else {
        request["input"]
            .is_string()
            .then(|| preview(&request["input"]))
            .flatten()
    }
}

/// Native desktop task inputs use a dedicated envelope, not arbitrary tool text.
/// Callers must additionally bind the native item to its receiving turn.
pub fn codex_task_preview(namespace: &str, name: &str, output: &str) -> Option<String> {
    if namespace != "codex_app" || name != "send_message_to_thread" || output.len() > 1024 * 1024 {
        return None;
    }
    let envelope = output
        .trim()
        .strip_prefix("<codex_delegation>")?
        .strip_suffix("</codex_delegation>")?
        .trim()
        .strip_prefix("<source_thread_id>")?;
    let (source, rest) = envelope.split_once("</source_thread_id>")?;
    uuid::Uuid::parse_str(source.trim()).ok()?;
    let input = rest
        .trim()
        .strip_prefix("<input>")?
        .strip_suffix("</input>")?;
    if input.contains("<input>") || input.contains("</input>") {
        return None;
    }
    // Decode exactly one native escaping layer before the usual privacy filter.
    preview(&Value::String(
        input
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&amp;", "&"),
    ))
}

/// A turn-bound input projection shared by the live tail and historical index.
pub fn codex(value: &Value, turn: Option<&str>) -> Option<String> {
    let p = &value["payload"];
    let native_turn = p["internal_chat_message_metadata_passthrough"]["turn_id"].as_str();
    if turn.is_some_and(|turn| {
        [p["turn_id"].as_str(), native_turn]
            .into_iter()
            .flatten()
            .any(|id| id != turn)
    }) {
        return None;
    }
    match (value["type"].as_str()?, p["type"].as_str()?) {
        ("response_item", "message") if p["role"] == "user" => preview(&p["content"]),
        ("event_msg", "user_message") => preview(&p["message"]),
        ("event_msg", "item_completed") if p["item"]["type"] == "UserMessage" => {
            preview(&p["item"]["content"])
        }
        ("response_item", "function_call_output") => {
            let turn = turn.filter(|turn| uuid::Uuid::parse_str(turn).is_ok())?;
            let id = p["id"].as_str()?;
            uuid::Uuid::parse_str(id.strip_prefix("fco_")?).ok()?;
            if native_turn != Some(turn) || !p["call_id"].is_null() {
                return None;
            }
            let sender = &value["metadata"]["sender_user_messages"];
            if !sender.is_null()
                && (sender["receiver_turn_id"] != turn || sender["receiver_message_id"] != id)
            {
                return None;
            }
            codex_task_preview(
                p["namespace"].as_str()?,
                p["name"].as_str()?,
                p["output"].as_str()?,
            )
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn latest_human_input_excludes_context_results_and_credentials_and_is_bounded() {
        let r = json!({"messages":[
            {"role":"system","content":"system sentinel"},
            {"role":"user","content":"old input"},
            {"role":"user","content":[{"type":"text","text":"修改按钮 Bearer abcdefghijklmnop"}]},
            {"role":"user","content":[{"type":"tool_result","content":"tool result sentinel"}]},
            {"role":"user","content":"# AGENTS.md instructions context sentinel"}
        ]});
        assert_eq!(request(&r).as_deref(), Some("修改按钮 [已隐藏凭据]"));
        assert_eq!(
            preview(&json!([{"type":"input_image","image_url":"secret-data"}])).as_deref(),
            Some("[图片]")
        );
        assert!(preview(&json!({"arguments":"not user input"})).is_none());
        assert!(preview(&json!("<environment_context>context")).is_none());
        assert_eq!(
            preview(&json!("# AGENTS.md instructions\n## My request:\n最新请求")).as_deref(),
            Some("最新请求")
        );
        let text = preview(&json!("你好🙂".repeat(2000))).unwrap();
        assert!(text.chars().count() <= 241);
        assert!(text.ends_with('…'));
    }

    fn task() -> Value {
        json!({"type":"response_item","payload":{
            "type":"function_call_output","id":"fco_33333333-3333-4333-8333-333333333333",
            "namespace":"codex_app","name":"send_message_to_thread",
            "internal_chat_message_metadata_passthrough":{"turn_id":"11111111-1111-4111-8111-111111111111"},
            "output":"<codex_delegation>\n<source_thread_id>22222222-2222-4222-8222-222222222222</source_thread_id>\n<input>核对 &lt;button&gt; &amp; 状态 Bearer abcdefghijklmnop</input>\n</codex_delegation>"
        },"metadata":{"sender_user_messages":{
            "receiver_turn_id":"11111111-1111-4111-8111-111111111111",
            "receiver_message_id":"fco_33333333-3333-4333-8333-333333333333",
            "text":"OLD SENDER CONTEXT MUST NOT BECOME THE INPUT"
        }}})
    }
    #[test]
    fn native_task_inputs_are_redacted_and_bound_to_the_receiving_message() {
        let t = "11111111-1111-4111-8111-111111111111";
        assert_eq!(
            codex(&task(), Some(t)).as_deref(),
            Some("核对 <button> & 状态 [已隐藏凭据]")
        );
        assert!(codex(&task(), None).is_none());
        assert!(codex(&task(), Some("22222222-2222-4222-8222-222222222222")).is_none());
        for (path, value) in [
            ("/payload/namespace", json!("shell")),
            ("/payload/name", json!("Read")),
            ("/payload/call_id", json!("actual-tool-result")),
            ("/payload/id", json!("invalid")),
            (
                "/metadata/sender_user_messages/receiver_message_id",
                json!("another-message"),
            ),
            (
                "/metadata/sender_user_messages/receiver_turn_id",
                json!("another-turn"),
            ),
            (
                "/payload/internal_chat_message_metadata_passthrough/turn_id",
                json!("another-turn"),
            ),
            ("/payload/output", json!({"text":"not a native envelope"})),
        ] {
            let mut v = task();
            if path == "/payload/call_id" {
                v["payload"]["call_id"] = value;
            } else {
                *v.pointer_mut(path).unwrap() = value;
            }
            assert!(codex(&v, Some(t)).is_none(), "{path}");
        }
    }
    #[test]
    fn native_envelopes_require_a_valid_source_and_never_decode_twice_or_leak_context() {
        let output = |text: &str| {
            format!(
                "<codex_delegation><source_thread_id>22222222-2222-4222-8222-222222222222</source_thread_id><input>{text}</input></codex_delegation>"
            )
        };
        let preview = |s: &str| codex_task_preview("codex_app", "send_message_to_thread", s);
        assert_eq!(
            preview(&output("保留 &amp;lt;input&amp;gt;")).as_deref(),
            Some("保留 &lt;input&gt;")
        );
        assert!(preview(&output("<input>ambiguous</input>")).is_none());
        assert!(preview(&output("# AGENTS.md instructions context-only")).is_none());
        assert!(
            preview(&output("hello").replace("22222222-2222-4222-8222-222222222222", "invalid"))
                .is_none()
        );
        assert!(preview(&format!("tool result {}", output("hello"))).is_none());
        assert!(preview(&output(&"x".repeat(1024 * 1024))).is_none());
        assert!(
            preview(&output(&"你好".repeat(1000)))
                .unwrap()
                .chars()
                .count()
                <= 241
        );
    }
    #[test]
    fn native_user_messages_and_completed_user_items_respect_explicit_turn_ids() {
        let value = json!({"type":"event_msg","payload":{"type":"item_completed","turn_id":"one","item":{"type":"UserMessage","content":[{"type":"text","text":"真实输入"}]}}});
        assert_eq!(codex(&value, Some("one")).as_deref(), Some("真实输入"));
        assert!(codex(&value, Some("two")).is_none());
        let value = json!({"type":"response_item","payload":{"type":"message","role":"user","internal_chat_message_metadata_passthrough":{"turn_id":"two"},"content":[{"type":"input_text","text":"新消息"}]}});
        assert!(codex(&value, Some("one")).is_none());
        assert_eq!(codex(&value, Some("two")).as_deref(), Some("新消息"));
    }

    #[test]
    fn desktop_question_replies_use_answers_and_redact_before_bounding() {
        let envelope = |answers: Value| {
            format!(
                "<send_user_message_question_reply>\n{answers}\n</send_user_message_question_reply>"
            )
        };
        let text = envelope(json!([
            {"questionItemId":"internal-id", "question":"## My request: metadata must not be retained".repeat(100), "answer":"已解锁，继续核对"},
            {"questionItemId":"another-id", "question":"credential", "answer":"Bearer abcdefghijklmnop"}
        ]));
        assert_eq!(
            preview(&json!(text)).as_deref(),
            Some("已解锁，继续核对 [已隐藏凭据]")
        );
        let value = json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":text}]}});
        assert_eq!(
            codex(&value, Some("one")).as_deref(),
            Some("已解锁，继续核对 [已隐藏凭据]")
        );
        let long = preview(&json!(envelope(json!([{"answer":"你好".repeat(500)}])))).unwrap();
        assert!(long.ends_with('…') && long.chars().count() <= 241);
        for text in [
            envelope(json!([])),
            envelope(json!([{"question":"not an answer"}])),
            envelope(json!([{"answer":{"text":"not a string"}}])),
            envelope(json!([{"answer":"  "}])),
            "<send_user_message_question_reply>invalid</send_user_message_question_reply>".into(),
            format!("{} trailing data", envelope(json!([{"answer":"valid"}]))),
        ] {
            assert!(preview(&json!(text)).is_none());
        }
    }
}
