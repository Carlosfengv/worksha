use super::{
    protocol::{self, Answer},
    stream::{Decoder, Encoder},
};
use local_agent_core::Provider;
use serde_json::{Value, json};

fn provider() -> Provider {
    let temp = tempfile::tempdir().unwrap();
    let mut p = local_agent_core::Store::open(temp.path())
        .unwrap()
        .providers()
        .unwrap()
        .remove(0);
    p.vendor = "deepseek".into();
    p.protocol = "chat-completions".into();
    p.endpoint = "https://api.deepseek.com".into();
    p.model = "deepseek-flash".into();
    p
}

fn reply(p: &Provider, text: &str, thinking: &str, tools: bool) -> Answer {
    Answer::parse_provider(p, &json!({"choices":[{"message":{"content":text,"reasoning_content":thinking,"tool_calls":if tools {json!([
        {"id":"a","type":"function","function":{"name":"first","arguments":"{}"}},
        {"id":"b","type":"function","function":{"name":"second","arguments":"{}"}}
    ])} else {json!([])}},"finish_reason":if tools {"tool_calls"} else {"stop"}}],"usage":{"prompt_tokens":10,"completion_tokens":7}})).unwrap()
}

fn continuation(protocol: &str, response: &Value) -> Value {
    if protocol == "messages" {
        json!({"thinking":{"type":"enabled","budget_tokens":1024},"max_tokens":4096,"messages":[
            {"role":"user","content":"check both"},
            {"role":"assistant","content":response["content"]},
            {"role":"user","content":[{"type":"tool_result","tool_use_id":"a","content":"one"},{"type":"tool_result","tool_use_id":"b","content":"two"}]}
        ],"tools":[{"name":"first","input_schema":{"type":"object"}},{"name":"second","input_schema":{"type":"object"}}]})
    } else {
        let mut input = vec![json!({"role":"user","content":"check both"})];
        input.extend(response["output"].as_array().unwrap().iter().cloned());
        input.extend([
            json!({"type":"function_call_output","call_id":"a","output":"one"}),
            json!({"type":"function_call_output","call_id":"b","output":"two"}),
        ]);
        json!({"reasoning":{"effort":"high"},"input":input,"tools":[{"type":"function","name":"first","parameters":{"type":"object"}},{"type":"function","name":"second","parameters":{"type":"object"}}]})
    }
}

fn assert_replay(p: &Provider, protocol: &str, response: &Value) {
    let body = continuation(protocol, response);
    let out = protocol::provider_request(protocol, p, &body).unwrap();
    assert_eq!(out["thinking"]["type"], "enabled");
    let messages = out["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 4);
    assert_eq!(messages[1]["reasoning_content"], "检查两项");
    assert_eq!(messages[1]["tool_calls"].as_array().unwrap().len(), 2);
    assert_eq!(messages[2]["tool_call_id"], "a");
    assert_eq!(messages[3]["tool_call_id"], "b");
    assert_eq!(messages[3]["content"], "two");
    let mut other = p.clone();
    other.model = "different-model".into();
    assert!(protocol::provider_request(protocol, &other, &body).is_err());
    other = p.clone();
    other.vendor = "openai".into();
    assert!(protocol::provider_request(protocol, &other, &body).is_err());
}

#[test]
fn json_thinking_tool_rounds_replay_in_both_protocols_and_preserve_prior_turns() {
    let p = provider();
    for protocol in ["messages", "responses"] {
        let response = reply(&p, "checking", "检查两项", true)
            .render(protocol, "first", &p.model)
            .unwrap();
        assert_replay(&p, protocol, &response);
        let mut body = continuation(protocol, &response);
        let final_reply = reply(&p, "done", "两个结果已返回", false)
            .render(protocol, "final", &p.model)
            .unwrap();
        let (key, extra) = if protocol == "messages" {
            (
                "messages",
                vec![
                    json!({"role":"assistant","content":final_reply["content"]}),
                    json!({"role":"user","content":"next task"}),
                ],
            )
        } else {
            let mut extra = final_reply["output"].as_array().unwrap().clone();
            extra.push(json!({"role":"user","content":"next task"}));
            ("input", extra)
        };
        body[key].as_array_mut().unwrap().extend(extra);
        let out = protocol::provider_request(protocol, &p, &body).unwrap();
        let assistant = out["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|m| m["role"] == "assistant")
            .collect::<Vec<_>>();
        assert_eq!(assistant.len(), 2);
        assert_eq!(assistant[0]["reasoning_content"], "检查两项");
        assert_eq!(assistant[1]["reasoning_content"], "两个结果已返回");
    }
}

fn assemble(protocol: &str, wire: &str) -> Value {
    let mut blocks: Vec<Value> = vec![];
    let mut starts = vec![];
    let mut stops = vec![];
    for v in wire
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .map(|raw| serde_json::from_str::<Value>(raw).unwrap())
    {
        match v["type"].as_str() {
            Some("content_block_start") => {
                let i = v["index"].as_u64().unwrap() as usize;
                assert_eq!(i, blocks.len());
                starts.push(i);
                blocks.push(v["content_block"].clone());
            }
            Some("content_block_delta") => {
                let i = v["index"].as_u64().unwrap() as usize;
                let field = match v["delta"]["type"].as_str().unwrap() {
                    "thinking_delta" => "thinking",
                    "signature_delta" => "signature",
                    "text_delta" => "text",
                    "input_json_delta" => "partial_json",
                    _ => panic!("unexpected delta"),
                };
                let text = blocks[i][field].as_str().unwrap_or("").to_string()
                    + v["delta"][field].as_str().unwrap();
                blocks[i][field] = json!(text);
            }
            Some("content_block_stop") => {
                stops.push(v["index"].as_u64().unwrap() as usize);
            }
            Some("response.completed" | "response.incomplete") => return v["response"].clone(),
            _ => {}
        }
    }
    assert_eq!(protocol, "messages");
    assert_eq!(starts, stops);
    for block in &mut blocks {
        if block["type"] == "tool_use" {
            block["input"] = serde_json::from_str(block["partial_json"].as_str().unwrap()).unwrap();
            block.as_object_mut().unwrap().remove("partial_json");
        }
    }
    json!({"content":blocks})
}

#[test]
fn fragmented_streams_emit_thinking_before_text_and_parallel_tools_then_replay() {
    let p = provider();
    let events = [
        json!({"choices":[{"delta":{"reasoning_content":"检查"}}]}),
        json!({"choices":[{"delta":{"reasoning_content":"两项","content":"checking"}}]}),
        json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":"a","function":{"name":"first","arguments":"{"}},{"index":1,"id":"b","function":{"name":"second","arguments":"{}"}}]}}]}),
        json!({"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"}"}}]},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":10,"completion_tokens":7}}),
    ];
    let wire = events
        .iter()
        .map(|e| format!("data: {e}\r\n\r\n"))
        .collect::<String>()
        + "data: [DONE]\r\n\r\n";
    for protocol in ["messages", "responses"] {
        let mut decoder = Decoder::new(&p.protocol).with_provider(&p);
        let mut encoder = Encoder::new(protocol, &p.model).with_provider(&p);
        let mut output = encoder.start();
        for byte in wire.as_bytes() {
            for delta in decoder.feed(&[*byte]).unwrap() {
                output += &encoder.push(delta).unwrap();
            }
        }
        assert!(encoder.is_finished());
        assert_eq!(encoder.end_status(), "success");
        let response = assemble(protocol, &output);
        assert_replay(&p, protocol, &response);
        if protocol == "responses" {
            assert_eq!(response["output"][0]["type"], "reasoning");
            assert_eq!(response["usage"]["total_tokens"], 17);
        }
    }
}

#[test]
fn invalid_reasoning_and_foreign_signatures_still_fail_without_panics() {
    let p = provider();
    for body in [
        json!({}),
        json!({"choices":[]}),
        json!({"choices":[{"message":{"reasoning_content":42}}]}),
    ] {
        assert!(Answer::parse_provider(&p, &body).is_err());
    }
    let body = json!({"messages":[{"role":"assistant","content":[{"type":"thinking","thinking":"foreign","signature":"anthropic-signature"}]}],"thinking":{"type":"enabled"}});
    assert!(protocol::provider_request("messages", &p, &body).is_err());
    let reasoning = b"data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"thinking\"}}]}\n\n";
    assert!(Decoder::new("chat-completions").feed(reasoning).is_err());
    let mut encoder = Encoder::new("messages", &p.model).with_provider(&p);
    encoder
        .push(super::stream::Delta::Text("answer".into()))
        .unwrap();
    assert!(
        encoder
            .push(super::stream::Delta::Reasoning("late".into()))
            .is_err()
    );
    assert!(
        Decoder::new("chat-completions")
            .with_provider(&p)
            .feed(b"data: {\"choices\":[{\"delta\":{\"reasoning_content\":42}}]}\n\n")
            .is_err()
    );
}

#[test]
fn thinking_only_truncated_streams_close_the_block_and_remain_incomplete() {
    let p = provider();
    for protocol in ["messages", "responses"] {
        let mut encoder = Encoder::new(protocol, &p.model).with_provider(&p);
        let mut output = encoder.start();
        for delta in [
            super::stream::Delta::Reasoning("budget".into()),
            super::stream::Delta::Usage(Some(10), Some(7), None),
            super::stream::Delta::Finish("length".into()),
            super::stream::Delta::End,
        ] {
            output += &encoder.push(delta).unwrap();
        }
        assert_eq!(encoder.end_status(), "incomplete");
        let response = assemble(protocol, &output);
        assert_eq!(
            if protocol == "messages" {
                &response["content"][0]["thinking"]
            } else {
                &response["output"][0]["summary"][0]["text"]
            },
            "budget"
        );
    }
}

#[test]
fn deepseek_normalizes_text_history_and_rejects_incompatible_thinking_without_retry() {
    let p = provider();
    let body = json!({"system":[{"type":"text","text":"rules"}],"messages":[
        {"role":"user","content":"hello"},{"role":"assistant","content":[{"type":"text","text":"one"},{"type":"text","text":"two"}]},
        {"role":"user","content":"next"}
    ]});
    let out = protocol::provider_request("messages", &p, &body).unwrap();
    assert_eq!(out["messages"][0]["content"], "rules");
    assert_eq!(out["messages"][2]["content"], "onetwo");
    assert!(out.get("store").is_none());
    let mut body = body;
    body["thinking"] = json!({"type":"enabled"});
    body["tools"] = json!([{"name":"read","input_schema":{"type":"object"}}]);
    assert!(
        protocol::provider_request("messages", &p, &body)
            .unwrap_err()
            .contains("缺少原始思考")
    );
    body["messages"] = json!([{"role":"user","content":"hello"}]);
    for choice in [json!({"type":"any"}), json!({"type":"tool","name":"read"})] {
        body["tool_choice"] = choice;
        assert!(
            protocol::provider_request("messages", &p, &body)
                .unwrap_err()
                .contains("不支持强制工具")
        );
    }
    let same = json!({"messages":[{"role":"user","content":"hello"}],"store":true,"thinking":{"type":"enabled"},"tool_choice":"required"});
    assert_eq!(
        protocol::provider_request("chat-completions", &p, &same).unwrap(),
        {
            let mut expected = same;
            expected["model"] = json!(p.model);
            expected
        }
    );
}

#[test]
fn refusal_json_and_fragmented_stream_preserve_content_usage_and_stop_reason() {
    let p = provider();
    let upstream = json!({"choices":[{"message":{"content":null,"reasoning_content":"检查","refusal":"不能协助"},"finish_reason":"stop"}],"usage":{"prompt_tokens":100,"completion_tokens":7,"prompt_cache_hit_tokens":80}});
    for protocol in ["messages", "responses"] {
        let result = Answer::parse_provider(&p, &upstream)
            .unwrap()
            .render(protocol, "r", &p.model)
            .unwrap();
        if protocol == "messages" {
            assert_eq!(result["content"][1]["text"], "不能协助");
            assert_eq!(result["stop_reason"], "refusal");
            assert_eq!(result["usage"]["input_tokens"], 20);
        } else {
            assert_eq!(result["output"][1]["content"][0]["refusal"], "不能协助");
        }
        let wire = format!(
            "data: {}\r\n\r\ndata: {}\r\n\r\ndata: [DONE]\r\n\r\n",
            json!({"choices":[{"delta":{"reasoning_content":"检查","refusal":"不能协助"}}]}),
            json!({"choices":[{"delta":{},"finish_reason":"stop"}],"usage":upstream["usage"]})
        );
        let mut decoder = Decoder::new(&p.protocol).with_provider(&p);
        let mut encoder = Encoder::new(protocol, &p.model).with_provider(&p);
        let mut output = encoder.start();
        for byte in wire.as_bytes() {
            for delta in decoder.feed(&[*byte]).unwrap() {
                output += &encoder.push(delta).unwrap();
            }
        }
        assert!(encoder.is_finished());
        assert!(output.contains("不能协助"));
        assert!(!output.contains("response.failed"));
        assert!(output.contains(if protocol == "messages" {
            "\"stop_reason\":\"refusal\""
        } else {
            "response.refusal.done"
        }));
        assert!(output.contains("\"output_tokens\":7"));
        assert!(output.contains(if protocol == "messages" {
            "\"cache_read_input_tokens\":80"
        } else {
            "\"cached_tokens\":80"
        }));
    }
}

#[test]
fn content_filter_and_unterminated_eof_keep_explicit_outcomes_and_usage() {
    let p = provider();
    for reason in ["stop", "content_filter"] {
        let wire = format!(
            "data: {}\n\ndata: [DONE]",
            json!({"choices":[{"delta":{"refusal":"不能协助"},"finish_reason":reason}],"usage":{"prompt_tokens":100,"completion_tokens":7,"prompt_cache_hit_tokens":80}})
        );
        for target in ["messages", "responses"] {
            let mut decoder = Decoder::new(&p.protocol).with_provider(&p);
            let mut encoder = Encoder::new(target, &p.model).with_provider(&p);
            for delta in decoder.feed(wire.as_bytes()).unwrap() {
                encoder.push(delta).unwrap();
            }
            assert!(!encoder.is_finished());
            let output: String = decoder
                .end()
                .unwrap()
                .into_iter()
                .map(|d| encoder.push(d).unwrap())
                .collect();
            assert!(encoder.is_finished());
            assert_eq!(
                encoder.end_status(),
                if reason == "stop" {
                    "success"
                } else {
                    "incomplete"
                }
            );
            if target == "responses" && reason == "content_filter" {
                assert!(output.contains("response.incomplete"));
                assert!(output.contains("\"reason\":\"content_filter\""));
            }
        }
    }
}

#[test]
fn explicitly_empty_reasoning_survives_tool_replay_and_is_distinct_from_missing() {
    let p = provider();
    for target in ["messages", "responses"] {
        let response = reply(&p, "checking", "", true)
            .render(target, "empty", &p.model)
            .unwrap();
        let out = protocol::provider_request(target, &p, &continuation(target, &response)).unwrap();
        assert_eq!(out["messages"][1]["reasoning_content"], "");
        let mut decoder = Decoder::new(&p.protocol).with_provider(&p);
        let mut encoder = Encoder::new(target, &p.model).with_provider(&p);
        let wire = format!(
            "data: {}\n\ndata: {}\n\ndata: [DONE]\n\n",
            json!({"choices":[{"delta":{"reasoning_content":"","tool_calls":[{"index":0,"id":"a","function":{"name":"first","arguments":"{}"}},{"index":1,"id":"b","function":{"name":"second","arguments":"{}"}}]}}]}),
            json!({"choices":[{"delta":{},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":10,"completion_tokens":7}})
        );
        let mut output = encoder.start();
        for delta in decoder.feed(wire.as_bytes()).unwrap() {
            output += &encoder.push(delta).unwrap();
        }
        let result = assemble(target, &output);
        let out = protocol::provider_request(target, &p, &continuation(target, &result)).unwrap();
        assert_eq!(out["messages"][1]["reasoning_content"], "");
    }
}
