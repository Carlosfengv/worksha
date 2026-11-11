use super::*;
use serde_json::json;

fn provider(protocol: &str) -> Provider {
    serde_json::from_value(json!({"id":"owned","name":"Owned","endpoint":"http://127.0.0.1:1/v1","protocol":protocol,"model":"owned-model","configured":true})).unwrap()
}
fn request(source: &str, stream: bool) -> Open {
    Open {
        stream,
        request: if source == "messages" {
            json!({"model":"client","max_tokens":100,"stream":stream,"messages":[{"role":"user","content":"你好"}]})
        } else {
            json!({"model":"client","stream":stream,"input":"你好"})
        },
    }
}
fn event(value: Value) -> String {
    format!("data: {value}\r\n\r\n")
}
fn fixtures() -> Vec<(&'static str, String)> {
    let chat = event(json!({"choices":[{"delta":{"content":"你好"}}]}))
        + &event(
            json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call-owned","function":{"name":"read","arguments":"{\"path\":"}}]}}]}),
        )
        + &event(
            json!({"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"你.txt\"}"}}]},"finish_reason":"tool_calls"}]}),
        )
        + &event(
            json!({"choices":[],"usage":{"prompt_tokens":10,"completion_tokens":3,"prompt_tokens_details":{"cached_tokens":2}}}),
        )
        + "data: [DONE]\r\n\r\n";
    let messages = event(
        json!({"type":"message_start","message":{"usage":{"input_tokens":8,"cache_read_input_tokens":2}}}),
    ) + &event(
        json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"你好"}}),
    ) + &event(
        json!({"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"call-owned","name":"read","input":{}}}),
    ) + &event(
        json!({"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"path\":\"你.txt\"}"}}),
    ) + &event(
        json!({"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":3}}),
    ) + &event(json!({"type":"message_stop"}));
    let answer = Answer {
        text: "你好".into(),
        tools: vec![
            json!({"id":"call-owned","type":"function","function":{"name":"read","arguments":"{\"path\":\"你.txt\"}"}}),
        ],
        input: Some(10),
        output: Some(3),
        cached: Some(2),
        finish: "tool_calls".into(),
        ..Default::default()
    };
    let responses = event(
        json!({"type":"response.output_text.delta","output_index":0,"delta":"你好"}),
    ) + &event(
        json!({"type":"response.output_item.added","output_index":1,"item":{"type":"function_call","call_id":"call-owned","name":"read","arguments":""}}),
    ) + &event(
        json!({"type":"response.function_call_arguments.delta","output_index":1,"delta":"{\"path\":\"你.txt\"}"}),
    ) + &event(
        json!({"type":"response.completed","response":answer.render("responses","upstream","owned-model").unwrap()}),
    );
    vec![
        ("chat-completions", chat),
        ("messages", messages),
        ("responses", responses),
    ]
}

#[tokio::test]
async fn flash_adapts_intrinsic_reasoning_across_all_supported_streams() {
    for (target, stream) in fixtures() {
        let mut p = provider(target);
        p.vendor = "minimax-plan".into();
        p.model = "MiniMax-M3.1-Flash".into();
        let mut adapter = Native::new(&p, "messages", Tools::default(), true, true)
            .hide_intrinsic_reasoning(true);
        let mut input = request("messages", true);
        input.request["thinking"] = json!({"type":"disabled"});
        let Opened::Json(body) = adapter.open(input).await.unwrap() else {
            panic!("JSON request")
        };
        assert!(body.get("thinking").is_none());
        let effort = match target {
            "messages" => &body["output_config"]["effort"],
            "chat-completions" => &body["reasoning_effort"],
            _ => &body["reasoning"]["effort"],
        };
        assert_eq!(effort, "low");
        let upstream = super::super::capabilities::builtin()
            .resolve(target)
            .unwrap();
        let client = super::super::capabilities::builtin()
            .resolve("messages")
            .unwrap();
        let mut output = adapter.start_stream(upstream, client).unwrap();
        let private = match target {
            "messages" => {
                json!({"type":"content_block_delta","index":2,"delta":{"type":"thinking_delta","thinking":"private-sample"}})
            }
            "chat-completions" => {
                json!({"choices":[{"delta":{"reasoning_content":"private-sample"}}]})
            }
            _ => json!({"type":"response.reasoning_text.delta","delta":"private-sample"}),
        };
        for chunk in adapter
            .feed((event(private) + &stream).as_bytes())
            .await
            .unwrap()
            .chunks
        {
            output.extend(chunk);
        }
        let Finished::Stream(end) = adapter.finish().await.unwrap() else {
            panic!("stream result")
        };
        for chunk in end.chunks {
            output.extend(chunk);
        }
        let output = String::from_utf8(output).unwrap();
        assert!(!output.contains("private-sample"));
        assert!(
            output.contains("你好")
                && output.contains("call-owned")
                && output.contains("message_stop")
        );
    }
}
fn normalized(parts: Vec<Vec<u8>>, id: &str, earliest: i64) -> Vec<String> {
    assert!(id.starts_with("resp_"));
    uuid::Uuid::parse_str(id.strip_prefix("resp_").unwrap()).unwrap();
    parts
        .into_iter()
        .map(|p| {
            let mut text = String::from_utf8(p).unwrap().replace(id, "RESPONSE_ID");
            let clocks = text
                .lines()
                .filter_map(|line| line.strip_prefix("data: "))
                .map(|line| serde_json::from_str::<Value>(line).unwrap())
                .filter_map(|value| {
                    value
                        .pointer("/response/created_at")
                        .or_else(|| value.get("created_at"))
                        .and_then(Value::as_i64)
                })
                .collect::<Vec<_>>();
            for clock in clocks {
                assert!((earliest..=chrono::Utc::now().timestamp()).contains(&clock));
                text = text.replace(&format!("\"created_at\":{clock}"), "\"created_at\":0");
            }
            text
        })
        .collect()
}
#[tokio::test]
async fn native_open_and_json_finish_keep_original_conversion_and_raw_scope() {
    for target in ["messages", "responses", "chat-completions"] {
        for source in ["messages", "responses"] {
            let p = provider(target);
            let input = request(source, false);
            let expected = protocol::provider_request(source, &p, &input.request).unwrap();
            let mut adapter = Native::new(&p, source, Tools::default(), true, true);
            let Opened::Json(actual) = adapter.open(input).await.unwrap() else {
                panic!("native JSON request")
            };
            assert_eq!(actual, expected);
            let answer = Answer {
                text: "你好".into(),
                input: Some(10),
                output: Some(3),
                finish: "stop".into(),
                ..Default::default()
            };
            let upstream = if target == "chat-completions" {
                json!({"id":"upstream","object":"chat.completion","model":"owned-model","choices":[{"message":{"role":"assistant","content":"你好"},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":3}})
            } else {
                answer.render(target, "upstream", "owned-model").unwrap()
            };
            let bytes = serde_json::to_vec(&upstream).unwrap();
            for part in bytes.chunks(1) {
                assert!(adapter.feed(part).await.unwrap().chunks.is_empty());
            }
            assert_eq!(adapter.json_bytes(), bytes);
            let earliest = chrono::Utc::now().timestamp();
            let Finished::Json(result) = adapter.finish().await.unwrap() else {
                panic!("native JSON finish")
            };
            let mut expected = Answer::parse_provider(&p, &upstream)
                .unwrap()
                .render(source, adapter.response_id().unwrap(), &p.model)
                .unwrap();
            if expected.get("created_at").is_some() {
                let clock = result["created_at"].as_i64().unwrap();
                assert!((earliest..=chrono::Utc::now().timestamp()).contains(&clock));
                expected["created_at"] = result["created_at"].clone();
            }
            assert_eq!(result, expected);
            assert!(adapter.finish().await.is_err());
            assert!(adapter.feed(b"extra").await.is_err());
        }
    }
    let mut adapter = Native::new(
        &provider("messages"),
        "responses",
        Tools::default(),
        true,
        false,
    );
    let input = Open {
        stream: false,
        request: json!({"custom":"raw-marker","model":"keep","unknown":[1,2]}),
    };
    let expected = input.request.clone();
    let Opened::Json(actual) = adapter.open(input).await.unwrap() else {
        panic!("raw request")
    };
    assert_eq!(actual, expected);
}
#[tokio::test]
async fn native_stream_frames_match_original_engine_across_all_byte_boundaries() {
    for (upstream, wire) in fixtures() {
        for client in ["messages", "responses"] {
            for width in [1, 2, 3, 7, 13, wire.len()] {
                let earliest = chrono::Utc::now().timestamp();
                let p = provider(upstream);
                let mut adapter = Native::new(&p, client, Tools::default(), true, true);
                adapter.open(request(client, true)).await.unwrap();
                let registry = super::super::capabilities::builtin();
                let mut actual = vec![
                    adapter
                        .start_stream(
                            registry.resolve(upstream).unwrap(),
                            registry.resolve(client).unwrap(),
                        )
                        .unwrap(),
                ];
                let mut decoder = Decoder::new(upstream).with_provider(&p);
                let mut encoder = Encoder::new(client, &p.model).with_provider(&p);
                let mut expected = vec![encoder.start().into_bytes()];
                for part in wire.as_bytes().chunks(width) {
                    for delta in decoder.feed(part).unwrap() {
                        expected.push(encoder.push(delta).unwrap().into_bytes());
                    }
                    let output = adapter.feed(part).await.unwrap();
                    assert_eq!(output.complete, encoder.is_finished());
                    actual.extend(output.chunks);
                }
                for delta in decoder.end().unwrap() {
                    expected.push(encoder.push(delta).unwrap().into_bytes());
                }
                let Finished::Stream(end) = adapter.finish().await.unwrap() else {
                    panic!("native stream finish")
                };
                actual.extend(end.chunks);
                assert!(end.complete && encoder.is_finished());
                assert_eq!(adapter.end_status(), encoder.end_status());
                assert_eq!(
                    normalized(actual, adapter.response_id().unwrap(), earliest),
                    normalized(expected, encoder.response_id(), earliest),
                    "{upstream} to {client}, width {width}"
                );
            }
        }
    }
}
#[tokio::test]
async fn native_stream_preserves_a_valid_prefix_before_a_coalesced_error() {
    let provider = provider("chat-completions");
    let registry = super::super::capabilities::builtin();
    let prefix = event(json!({"choices":[{"delta":{"content":"partial"}}]}));
    let rejected = event(json!({"error":{"type":"rate_limit_error"}}));
    for chunks in [vec![prefix.clone() + &rejected], vec![prefix, rejected]] {
        let mut adapter = Native::new(&provider, "messages", Tools::default(), true, true);
        adapter.open(request("messages", true)).await.unwrap();
        adapter
            .start_stream(
                registry.resolve(&provider.protocol).unwrap(),
                registry.resolve("messages").unwrap(),
            )
            .unwrap();
        let mut output = Output::default();
        let (last, earlier) = chunks.split_last().unwrap();
        for chunk in earlier {
            adapter.feed_stream(chunk.as_bytes(), &mut output).unwrap();
        }
        assert!(adapter.feed_stream(last.as_bytes(), &mut output).is_err());
        assert!(!output.complete);
        let text = String::from_utf8(output.chunks.concat()).unwrap();
        assert!(text.contains("partial"), "{text}");
        assert!(!text.contains("message_stop"), "{text}");
        assert_eq!(
            adapter.take_rejection().unwrap()["error"]["type"],
            "rate_limit_error"
        );
    }
}

#[tokio::test]
async fn native_partial_error_and_text_only_tool_output_never_claim_completion() {
    let p = provider("chat-completions");
    let registry = super::super::capabilities::builtin();
    for client in ["messages", "responses"] {
        for wire in [
            event(json!({"choices":[{"delta":{"content":"partial"}}]})),
            "data: {\"broken\":\r\n\r\n".into(),
        ] {
            let mut adapter = Native::new(&p, client, Tools::default(), true, true);
            adapter.open(request(client, true)).await.unwrap();
            adapter
                .start_stream(
                    registry.resolve(&p.protocol).unwrap(),
                    registry.resolve(client).unwrap(),
                )
                .unwrap();
            let result = adapter.feed(wire.as_bytes()).await;
            if let Ok(output) = result {
                assert!(!output.complete);
                let Finished::Stream(end) = adapter.finish().await.unwrap() else {
                    panic!("partial stream finish")
                };
                assert!(!end.complete);
            }
            let error = String::from_utf8(adapter.error()).unwrap();
            assert!(!error.contains("response.completed") && !error.contains("message_stop"));
        }
        let mut adapter = Native::new(&p, client, Tools::default(), false, true);
        adapter.open(request(client, true)).await.unwrap();
        adapter
            .start_stream(
                registry.resolve(&p.protocol).unwrap(),
                registry.resolve(client).unwrap(),
            )
            .unwrap();
        assert!(adapter.feed(event(json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":"one","function":{"name":"read","arguments":"{}"}}]}}]})).as_bytes()).await.err().unwrap().contains("未发布的工具调用"));
    }
}

#[tokio::test]
async fn native_common_lifecycle_keeps_reasoning_scope_and_unknown_usage_errors() {
    let registry = super::super::capabilities::builtin();
    let mut p = provider("chat-completions");
    p.vendor = "deepseek".into();
    let wire = event(json!({"choices":[{"delta":{"reasoning_content":"思考内容"}}]}))
        + &event(
            json!({"choices":[{"delta":{"content":"你好"},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":3}}),
        )
        + "data: [DONE]\r\n\r\n";
    for client in ["messages", "responses"] {
        let mut adapter = Native::new(&p, client, Tools::default(), true, true);
        adapter.open(request(client, true)).await.unwrap();
        let mut output = adapter
            .start_stream(
                registry.resolve(&p.protocol).unwrap(),
                registry.resolve(client).unwrap(),
            )
            .unwrap();
        let mut complete = false;
        for byte in wire.as_bytes() {
            let part = adapter.feed(&[*byte]).await.unwrap();
            complete |= part.complete;
            for chunk in part.chunks {
                output.extend(chunk);
            }
        }
        assert!(complete);
        let output = String::from_utf8(output).unwrap();
        assert!(output.contains("思考内容") && output.contains("local_agent_deepseek_v1_"));
    }
    p.vendor = "openai".into();
    let mut adapter = Native::new(&p, "messages", Tools::default(), true, true);
    adapter.open(request("messages", true)).await.unwrap();
    adapter
        .start_stream(
            registry.resolve(&p.protocol).unwrap(),
            registry.resolve("messages").unwrap(),
        )
        .unwrap();
    assert!(
        adapter
            .feed(wire.as_bytes())
            .await
            .err()
            .unwrap()
            .contains("思考内容")
    );
    let mut adapter = Native::new(&p, "messages", Tools::default(), true, true);
    adapter.open(request("messages", false)).await.unwrap();
    let reply = json!({"choices":[{"message":{"content":"reply"},"finish_reason":"stop"}]});
    adapter
        .feed(&serde_json::to_vec(&reply).unwrap())
        .await
        .unwrap();
    let expected = Answer::parse_provider(&p, &reply)
        .unwrap()
        .render("messages", "unused", &p.model)
        .err()
        .unwrap();
    assert_eq!(adapter.finish().await.err().unwrap(), expected);
}
