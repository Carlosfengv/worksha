use super::*;
use std::cell::Cell;

#[test]
fn history_filter_removes_only_issued_assistant_status_and_preserves_user_tools_and_signatures() {
    let notice = "【路由提示】已恢复使用 Source / model。";
    let issued = vec![local_agent_core::providers::files::hash(notice.as_bytes())];
    let user = json!({"role":"user","content":notice});
    let thinking = json!({"type":"thinking","thinking":notice,"signature":"signed-keep"});
    let tool = json!({"type":"tool_use","id":"call1","name":"Read","input":{"path":notice}});
    let result = json!({"role":"user","content":[{"type":"tool_result","tool_use_id":"call1","content":notice}]});
    let mut body = json!({"messages":[user.clone(),{"role":"assistant","content":[{"type":"text","text":format!("{notice}\n\n")},thinking.clone(),tool.clone()]},result.clone(),{"role":"assistant","content":format!("{notice}\n\nactual answer")},{"role":"assistant","content":[{"type":"text","text":"【路由提示】unissued text"}]},{"role":"assistant","content":notice}]});
    strip_history(&mut body, &issued);
    let m = body["messages"].as_array().unwrap();
    assert_eq!(m.len(), 5);
    assert_eq!(m[0], user);
    assert_eq!(m[1]["content"], json!([thinking, tool]));
    assert_eq!(m[2], result);
    assert_eq!(m[3]["content"], "actual answer");
    assert_eq!(m[4]["content"][0]["text"], "【路由提示】unissued text");
}

fn start() -> Value {
    json!({"type":"message_start","message":{"type":"message","role":"assistant","model":"actual","usage":{"input_tokens":7}}})
}
fn text_start(index: u64) -> Value {
    json!({"type":"content_block_start","index":index,"content_block":{"type":"text","text":""}})
}
fn text(index: u64, s: &str) -> Value {
    json!({"type":"content_block_delta","index":index,"delta":{"type":"text_delta","text":s}})
}
fn output(frames: &[Value], allow: bool) -> (Vec<Value>, usize) {
    let mut r = Rewriter::default();
    let calls = Cell::new(0);
    let input = frames.iter().flat_map(event).collect::<Vec<_>>();
    let mut raw = vec![];
    for byte in input {
        for chunk in r.feed(&[byte], &|plain| {
            calls.set(calls.get() + 1);
            (allow && plain).then(|| "【路由提示】切换\n\n".into())
        }) {
            raw.extend(chunk);
        }
    }
    raw.extend(r.end());
    let values = raw
        .split(|b| *b == b'\n')
        .filter_map(|l| l.strip_prefix(b"data: "))
        .map(|d| serde_json::from_slice(d).unwrap())
        .collect();
    (values, calls.get())
}
#[test]
fn fragmented_stream_inserts_once_after_thinking_and_shifts_tool_indexes_without_touching_usage() {
    let frames = vec![
        start(),
        json!({"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}),
        json!({"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"keep-signed-content"}}),
        json!({"type":"content_block_stop","index":0}),
        text_start(1),
        text(1, "正常正文"),
        text(1, "后续"),
        json!({"type":"content_block_stop","index":1}),
        json!({"type":"content_block_start","index":2,"content_block":{"type":"tool_use","id":"call1","name":"Read","input":{}}}),
        json!({"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":"{\"path\":\"x\"}"}}),
        json!({"type":"content_block_stop","index":2}),
        json!({"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":11}}),
        json!({"type":"message_stop"}),
    ];
    let (actual, calls) = output(&frames, true);
    assert_eq!(calls, 1);
    assert_eq!(actual.len(), frames.len() + 3);
    assert_eq!(&actual[..4], &frames[..4]);
    assert_eq!(actual[4]["index"], 1);
    assert!(
        actual[5]["delta"]["text"]
            .as_str()
            .unwrap()
            .contains("路由提示")
    );
    assert_eq!(actual[7]["index"], 2);
    assert_eq!(actual[11]["index"], 3);
    assert_eq!(actual[12]["delta"], frames[9]["delta"]);
    assert_eq!(&actual[actual.len() - 2..], &frames[frames.len() - 2..]);
}
#[test]
fn json_prefixes_and_scalar_values_remain_byte_equivalent() {
    for prefix in [
        "{\"ok\":true}",
        " [1]",
        "\"value\"",
        "42",
        "-1",
        "true",
        "false",
        "null",
        "```json\n{\"ok\":true}\n```",
    ] {
        let mut frames = vec![start(), text_start(0)];
        frames.extend(prefix.chars().map(|c| text(0, &c.to_string())));
        frames.push(json!({"type":"content_block_stop","index":0}));
        frames.push(json!({"type":"message_stop"}));
        let (out, calls) = output(&frames, true);
        assert_eq!(out, frames, "{prefix}");
        assert_eq!(calls, 1);
    }
}
#[test]
fn explicit_contracts_skip_and_normal_tool_definitions_do_not_disable_notices() {
    assert!(output_contract(
        &json!({"messages":[{"role":"user","content":"请读取文件，只返回 JSON"}]})
    ));
    assert!(output_contract(
        &json!({"tools":[{"name":"StructuredOutput","input_schema":{"type":"object"}}]})
    ));
    for body in [
        json!({"response_format":{"type":"json_object"}}),
        json!({"output_config":{"format":{"type":"json_schema","schema":{}}}}),
        json!({"text":{"format":{"type":"json_schema"}}}),
        json!({"tool_choice":{"type":"tool","name":"StructuredOutput"}}),
    ] {
        assert!(output_contract(&body));
    }
    assert!(!output_contract(
        &json!({"response_format":{"type":"text"},"tools":[{"name":"Read","input_schema":{"type":"object"}}],"tool_choice":{"type":"auto"}})
    ));
    let frames = vec![
        start(),
        text_start(0),
        text(0, "普通内容"),
        json!({"type":"message_stop"}),
    ];
    assert_eq!(output(&frames, false).0, frames);
}
#[test]
fn tool_first_notifies_once_and_preserves_arguments_while_structured_tool_is_silent() {
    for name in ["Read", "StructuredOutput"] {
        let frames = vec![
            start(),
            json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"call-keep","name":name,"input":{}}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{}"}}),
            json!({"type":"content_block_stop","index":0}),
            json!({"type":"message_stop"}),
        ];
        let (out, calls) = output(&frames, true);
        assert_eq!(calls, 1);
        if name == "Read" {
            assert_eq!(out[4]["content_block"], frames[1]["content_block"]);
            assert_eq!(out[5]["delta"], frames[2]["delta"]);
        } else {
            assert_eq!(out, frames);
        }
    }
}
#[test]
fn non_streaming_json_guard_and_insertion_preserve_model_usage_and_tools() {
    for (answer, allowed) in [("hello", true), ("{\"ok\":true}", false)] {
        let value = json!({"type":"message","role":"assistant","model":"actual","content":[{"type":"text","text":answer}],"usage":{"input_tokens":7,"output_tokens":4},"stop_reason":"end_turn"});
        let raw = serde_json::to_vec(&value).unwrap();
        let out: Value = serde_json::from_slice(&rewrite_json(raw.clone(), &|plain| {
            plain.then(|| "NOTICE".into())
        }))
        .unwrap();
        assert_eq!(out["usage"], value["usage"]);
        assert_eq!(out["model"], value["model"]);
        assert_eq!(
            out["content"].as_array().unwrap().len(),
            if allowed { 2 } else { 1 }
        );
        if !allowed {
            assert_eq!(serde_json::to_vec(&out).unwrap(), raw);
        }
    }
}
#[test]
fn interrupted_prefix_is_flushed_without_claiming_and_sse_metadata_survives() {
    let mut r = Rewriter::default();
    let calls = Cell::new(0);
    let claim = |_| {
        calls.set(calls.get() + 1);
        Some("NOTICE".into())
    };
    let mut input = event(&start());
    input.extend(event(&text_start(0)));
    input.extend(event(&text(0, "t")));
    let mut out = r.feed(&input, &claim).concat();
    out.extend(r.end());
    assert_eq!(out, input);
    assert_eq!(calls.get(), 0);
    let mut r = Rewriter::default();
    r.feed(&event(&start()), &claim);
    r.feed(&event(&text_start(0)), &claim);
    let frame=b"id: keep-id\r\nretry: 20\r\nevent: content_block_delta\r\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"hello\"}}\r\n\r\n";
    let out = String::from_utf8(r.feed(frame, &claim).concat()).unwrap();
    assert!(out.contains("id: keep-id\nretry: 20\n"));
}
