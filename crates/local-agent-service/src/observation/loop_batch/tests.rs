use super::*;
use serde_json::json;

#[test]
fn parallel_orders_and_json_key_orders_are_canonical_without_guessing_errors_from_text() {
    let mut body = json!({"messages":[
        {"role":"user","content":"work"},
        {"role":"assistant","tool_calls":[
           {"id":"b","function":{"name":"test","arguments":"{\"path\":1, \"depth\":2}"}},
           {"id":"a","function":{"name":"test","arguments":"{\"depth\":2, \"path\":1}"}}
        ]},
        {"role":"tool","tool_call_id":"a","content":"the string says error"},
        {"role":"tool","tool_call_id":"b","content":"{\"ok\":false}"}
    ]});
    let first = extract("chat-completions", &body, "turn").unwrap();
    assert_eq!(first.calls.len(), 2);
    assert_eq!(first.calls[0].action_hash, first.calls[1].action_hash);
    assert!(!first.has_error && first.has_success);
    let messages = body["messages"].as_array_mut().unwrap();
    messages.swap(2, 3);
    assert_eq!(extract("chat-completions", &body, "turn").unwrap(), first);
    body["messages"][2]["exitCode"] = json!(-1);
    let mixed = extract("chat-completions", &body, "turn").unwrap();
    assert!(mixed.has_error && mixed.has_success);
    assert_ne!(mixed.fingerprint, first.fingerprint);
    assert_ne!(
        extract("chat-completions", &body, "next turn")
            .unwrap()
            .fingerprint,
        mixed.fingerprint
    );
}

#[test]
fn only_the_latest_returned_tool_batch_is_used_in_messages_and_responses() {
    let mut messages = json!({"messages":[
        {"role":"user","content":"work"},
        {"role":"assistant","content":[{"type":"tool_use","id":"old","name":"build","input":{}}]},
        {"role":"user","content":[{"type":"tool_result","tool_use_id":"old","is_error":true,"content":"old failure"}]},
        {"role":"assistant","content":[{"type":"tool_use","id":"latest","name":"test","input":{}}]},
        {"role":"user","content":[{"type":"tool_result","tool_use_id":"latest","content":"ok"}]}
    ]});
    let current = extract("messages", &messages, "turn").unwrap();
    assert_eq!(current.calls[0].id_hash, hash(b"latest"));
    assert!(!current.has_error && current.has_success);
    messages["messages"]
        .as_array_mut()
        .unwrap()
        .push(json!({"role":"user","content":"next"}));
    assert!(extract("messages", &messages, "turn").is_none());
    let responses = json!({"input":[
        {"type":"message","role":"user","content":[{"type":"input_text","text":"work"}]},
        {"type":"function_call","call_id":"one","name":"test","arguments":"{}"},
        {"type":"function_call","call_id":"two","name":"test","arguments":"{}"},
        {"type":"function_call_output","call_id":"one","output":"failed","exit_code":2},
        {"type":"function_call_output","call_id":"two","output":"not ok","ok":false}
    ]});
    let current = extract("responses", &responses, "turn").unwrap();
    assert_eq!(current.calls.len(), 2);
    assert!(current.has_error && !current.has_success);
}
