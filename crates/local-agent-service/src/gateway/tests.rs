use super::{
    protocol::{self, Answer},
    stream::{Decoder, Encoder},
};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

#[test]
fn route_revision_preserves_the_persisted_52_bit_identity() {
    for id in [
        "00000000-0000-4000-8000-000000000000",
        "ffffffff-ffff-4fff-bfff-ffffffffffff",
        "df8bb13e-fdf5-4e4b-912c-fc34dfd28083",
    ] {
        let historical = u64::from_str_radix(
            &local_agent_core::providers::files::hash(id.as_bytes())[..13],
            16,
        )
        .unwrap();
        assert_eq!(super::route_revision(id), historical);
    }
}

#[test]
fn text_only_encoder_rejects_tool_output_before_emitting_a_tool_block() {
    use super::stream::{Delta, Encoder};
    for protocol in ["messages", "responses"] {
        for delta in [
            Delta::Tool(0, Some("call-one".into()), Some("read".into()), "{}".into()),
            Delta::ToolDone(0, "{}".into()),
            Delta::Finish("tool_calls".into()),
            Delta::Snapshot(super::protocol::Answer {
                tools: vec![json!({"id":"one"})],
                ..Default::default()
            }),
        ] {
            let mut encoder = Encoder::new(protocol, "text-model").with_tool_calls(false);
            let start = encoder.start();
            assert!(!start.contains("tool_use"));
            assert!(
                encoder
                    .push(delta)
                    .unwrap_err()
                    .contains("未发布的工具调用")
            );
        }
        let mut encoder = Encoder::new(protocol, "text-model").with_tool_calls(false);
        encoder.start();
        assert!(
            !encoder
                .push(Delta::Text("hello".into()))
                .unwrap()
                .is_empty()
        );
        encoder.push(Delta::Usage(Some(5), Some(2), None)).unwrap();
        encoder.push(Delta::Finish("stop".into())).unwrap();
        encoder.push(Delta::End).unwrap();
        assert!(encoder.is_finished());
    }
}
#[test]
fn text_only_tool_preparation_does_not_insert_null_definitions() {
    let body =
        json!({"model":"chat","messages":[{"role":"user","content":"hello"}],"max_tokens":30});
    let (_, prepared) = super::tools::Tools::prepare("messages", &body).unwrap();
    assert_eq!(prepared, body);
    for target in ["messages", "chat-completions", "responses"] {
        let converted = protocol::request("messages", target, &prepared, "upstream-chat").unwrap();
        assert!(converted.get("tools").is_none(), "{target}");
        assert!(converted.get("tool_choice").is_none(), "{target}");
    }
}
#[test]
fn ollama_text_service_preserves_explicit_thinking_disable_in_its_wire_format() {
    let mut provider: local_agent_core::Provider = serde_json::from_value(json!({"id":"custom-ollama","vendor":"ollama","name":"Ollama","endpoint":"http://127.0.0.1:11434/v1","protocol":"chat-completions","model":"qwen3.5:0.8b","configured":true})).unwrap();
    let body = json!({"model":"chat","thinking":{"type":"disabled"},"messages":[{"role":"user","content":"hello"}],"max_tokens":30});
    let converted = protocol::provider_request("messages", &provider, &body).unwrap();
    assert_eq!(converted["reasoning_effort"], "none");
    assert_eq!(converted["max_tokens"], 30);
    assert!(converted.get("tools").is_none());
    let native = json!({"messages":[{"role":"user","content":"hello"}],"thinking":{"type":"disabled"},"reasoning":{"effort":"high"}});
    let converted = protocol::provider_request("chat-completions", &provider, &native).unwrap();
    assert_eq!(converted["reasoning_effort"], "none");
    assert!(converted.get("reasoning").is_none());
    provider.vendor = "custom".into();
    assert!(
        protocol::provider_request("messages", &provider, &body)
            .unwrap()
            .get("reasoning_effort")
            .is_none()
    );
    provider.vendor = "ollama".into();
    let mut no_override = body;
    no_override.as_object_mut().unwrap().remove("thinking");
    assert!(
        protocol::provider_request("messages", &provider, &no_override)
            .unwrap()
            .get("reasoning_effort")
            .is_none()
    );
}
#[path = "recovery_tests.rs"]
mod recovery;
fn event(v: Value) -> String {
    format!("data: {v}\r\n\r\n")
}
fn tool() -> Value {
    json!({"id":"call-1","type":"function","function":{"name":"read_file","arguments":"{\"path\":\"你.txt\"}"}})
}

#[test]
fn requests_keep_context_tools_results_images_and_selected_model() {
    let claude = json!({"model":"client-alias","system":[{"type":"text","text":"system"}],"stream":true,"max_tokens":25,"messages":[{"role":"user","content":[{"type":"text","text":"read"},{"type":"image","source":{"type":"base64","media_type":"image/png","data":"AAAA"}}]},{"role":"assistant","content":[{"type":"tool_use","id":"call-1","name":"read_file","input":{"path":"你.txt"}}]},{"role":"user","content":[{"type":"tool_result","tool_use_id":"call-1","content":"result-sentinel"}]}],"tools":[{"name":"read_file","description":"Read file","input_schema":{"type":"object","properties":{"path":{"type":"string"}}}}],"tool_choice":{"type":"tool","name":"read_file"}});
    for target in ["chat-completions", "responses"] {
        let converted = protocol::request("messages", target, &claude, "chosen").unwrap();
        assert_eq!(converted["model"], "chosen");
        assert!(converted.to_string().contains("result-sentinel"));
        assert!(converted.to_string().contains("call-1"));
        assert!(converted.to_string().contains("data:image/png;base64,AAAA"));
        if target == "responses" {
            let back = protocol::request("responses", "messages", &converted, "other").unwrap();
            assert_eq!(back["model"], "other");
            assert!(back.to_string().contains("tool_use_id"));
            assert!(back.to_string().contains("result-sentinel"));
        }
    }
    let responses = json!({"input":[{"role":"developer","content":"rules"},{"type":"function_call","name":"read_file","call_id":"call-1","arguments":"{}"},{"type":"function_call_output","call_id":"call-1","output":"done"}],"tools":[{"type":"function","name":"read_file","parameters":{"type":"object"}}]});
    for target in ["messages", "chat-completions"] {
        let v = protocol::request("responses", target, &responses, "chosen").unwrap();
        assert!(v.to_string().contains("call-1"));
        assert!(v.to_string().contains("rules"));
        assert!(v.to_string().contains("done"));
        if target == "chat-completions" {
            assert_eq!(v["messages"][0]["role"], "system");
            assert_eq!(v["messages"][0]["content"], "rules");
        } else {
            assert!(v["system"].to_string().contains("rules"));
            assert!(!v["messages"].to_string().contains("rules"));
        }
    }
}

#[test]
fn responses_parallel_calls_remain_a_single_assistant_tool_round() {
    let input = json!({"input":[
        {"role":"developer","content":"project instructions"},
        {"role":"user","content":"check both"},
        {"type":"function_call","name":"first","call_id":"a","arguments":"{}"},
        {"type":"function_call","name":"second","call_id":"b","arguments":"{}"},
        {"type":"function_call_output","call_id":"a","output":"one"},
        {"type":"function_call_output","call_id":"b","output":"two"}
    ]});
    let out = protocol::request("responses", "chat-completions", &input, "chosen").unwrap();
    let messages = out["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 5);
    assert_eq!(messages[0]["role"], "system");
    assert_eq!(messages[2]["tool_calls"].as_array().unwrap().len(), 2);
    assert_eq!(messages[2]["tool_calls"][0]["id"], "a");
    assert_eq!(messages[2]["tool_calls"][1]["id"], "b");
    assert_eq!(messages[3]["tool_call_id"], "a");
    assert_eq!(messages[4]["tool_call_id"], "b");
    let same = protocol::request("responses", "responses", &input, "chosen").unwrap();
    assert_eq!(same["input"], input["input"]);
}

#[test]
fn unsupported_state_and_content_never_disappear() {
    for extra in [
        json!({"previous_response_id":"previous"}),
        json!({"input":[{"type":"reasoning","encrypted_content":"opaque"}]}),
        json!({"tools":[{"type":"web_search"}]}),
    ] {
        let mut v = json!({"input":"hello"});
        for (k, value) in extra.as_object().unwrap() {
            v[k] = value.clone();
        }
        assert!(protocol::request("responses", "chat-completions", &v, "model").is_err());
    }
    assert!(protocol::request("messages","responses",&json!({"messages":[{"role":"assistant","content":[{"type":"thinking","thinking":"signed"}]}]}),"model").is_err());
    assert!(
        protocol::request(
            "responses",
            "messages",
            &json!({"input":[{"type":"input_file","file_id":"file"}]}),
            "model"
        )
        .is_err()
    );
}

#[test]
fn deepseek_cross_protocol_respects_explicit_thinking_and_keeps_default_disabled() {
    let temp = tempfile::tempdir().unwrap();
    let mut provider = local_agent_core::Store::open(temp.path())
        .unwrap()
        .providers()
        .unwrap()
        .remove(0);
    provider.vendor = "deepseek".into();
    provider.protocol = "chat-completions".into();
    provider.model = "deepseek-flash".into();
    for thinking in [None, Some(json!({"type":"disabled"}))] {
        let mut body = json!({"messages":[{"role":"user","content":"hello"}],"stream":true});
        if let Some(thinking) = thinking {
            body["thinking"] = thinking;
        }
        let out = protocol::provider_request("messages", &provider, &body).unwrap();
        assert_eq!(out["thinking"]["type"], "disabled");
        assert_eq!(out["model"], "deepseek-flash");
        assert!(out.get("reasoning_effort").is_none());
        body["thinking"] = json!({"type":"adaptive"});
        let out = protocol::provider_request("messages", &provider, &body).unwrap();
        assert_eq!(out["thinking"]["type"], "enabled");
        assert_eq!(out["reasoning_effort"], "high");
    }
    let body = json!({"input":"hello","reasoning":{"effort":"none"}});
    assert_eq!(
        protocol::provider_request("responses", &provider, &body).unwrap()["thinking"]["type"],
        "disabled"
    );
    assert_eq!(
        protocol::provider_request(
            "responses",
            &provider,
            &json!({"input":"hello","reasoning":{"effort":"high"}})
        )
        .unwrap()["thinking"]["type"],
        "enabled"
    );
    let same_protocol = json!({"messages":[],"thinking":{"type":"enabled"}});
    assert_eq!(
        protocol::provider_request("chat-completions", &provider, &same_protocol).unwrap()["thinking"]
            ["type"],
        "enabled"
    );
    provider.vendor = "openai".into();
    assert!(
        protocol::provider_request("responses", &provider, &body)
            .unwrap()
            .get("thinking")
            .is_none()
    );
}

#[test]
fn json_responses_preserve_calls_and_cache_without_double_counting() {
    let chat = json!({"object":"chat.completion","choices":[{"message":{"content":"你","tool_calls":[tool()]},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":10,"completion_tokens":2,"prompt_tokens_details":{"cached_tokens":4}}});
    let a = Answer::parse("chat-completions", &chat).unwrap();
    let c = a.render("messages", "id", "model").unwrap();
    assert_eq!(c["usage"]["input_tokens"], 6);
    assert_eq!(c["usage"]["cache_read_input_tokens"], 4);
    assert_eq!(c["content"][1]["input"]["path"], "你.txt");
    let r = a.render("responses", "id", "model").unwrap();
    assert_eq!(r["usage"]["total_tokens"], 12);
    assert_eq!(r["output"][1]["call_id"], "call-1");
    let roundtrip = Answer::parse("messages", &c).unwrap();
    assert_eq!(roundtrip.input, Some(10));
}

#[test]
fn all_cross_protocol_streams_preserve_utf8_tool_ids_arguments_and_usage() {
    let chat = event(json!({"choices":[{"delta":{"content":"你"},"finish_reason":null}]}))
        + &event(
            json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call-1","function":{"name":"read_file","arguments":"{\"path\":"}}]}}]}),
        )
        + &event(
            json!({"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"你.txt\"}"}}]},"finish_reason":"tool_calls"}]}),
        )
        + &event(json!({"choices":[],"usage":{"prompt_tokens":10,"completion_tokens":2}}))
        + "data: [DONE]\r\n\r\n";
    let messages = event(json!({"type":"message_start","message":{"usage":{"input_tokens":10}}}))
        + &event(
            json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
        )
        + &event(
            json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"你"}}),
        )
        + &event(
            json!({"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"call-1","name":"read_file","input":{}}}),
        )
        + &event(
            json!({"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"path\":\"你.txt\"}"}}),
        )
        + &event(
            json!({"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":2}}),
        )
        + &event(json!({"type":"message_stop"}));
    let final_response = Answer {
        text: "你".into(),
        tools: vec![tool()],
        input: Some(10),
        output: Some(2),
        finish: "tool_calls".into(),
        ..Answer::default()
    }
    .render("responses", "upstream", "model")
    .unwrap();
    let responses = event(
        json!({"type":"response.output_text.delta","output_index":0,"delta":"你"}),
    ) + &event(
        json!({"type":"response.output_item.added","output_index":1,"item":{"type":"function_call","call_id":"call-1","name":"read_file","arguments":""}}),
    ) + &event(
        json!({"type":"response.function_call_arguments.delta","output_index":1,"delta":"{\"path\":\"你.txt\"}"}),
    ) + &event(json!({"type":"response.completed","response":final_response}));
    for (source, target, body) in [
        ("chat-completions", "messages", &chat),
        ("chat-completions", "responses", &chat),
        ("messages", "responses", &messages),
        ("responses", "messages", &responses),
    ] {
        let mut decoder = Decoder::new(source);
        let mut encoder = Encoder::new(target, "model");
        let mut result = encoder.start();
        for byte in body.as_bytes() {
            for delta in decoder.feed(&[*byte]).unwrap() {
                result += &encoder.push(delta).unwrap();
            }
        }
        assert!(encoder.is_finished(), "{source} -> {target}");
        assert!(result.contains("你"));
        assert!(result.contains("call-1"));
        assert!(result.contains("read_file"));
        assert!(result.contains("output_tokens\":2"));
        assert!(result.contains(if target == "messages" {
            "event: message_stop"
        } else {
            "event: response.completed"
        }));
    }
}

#[test]
fn stream_missing_finish_invalid_arguments_and_explicit_errors_are_not_success() {
    let mut decoder = Decoder::new("chat-completions");
    let mut encoder = Encoder::new("responses", "model");
    for d in decoder
        .feed(event(json!({"choices":[{"delta":{"content":"partial"}}]})).as_bytes())
        .unwrap()
    {
        encoder.push(d).unwrap();
    }
    assert!(!encoder.is_finished());
    assert!(encoder.error().contains("response.failed"));
    assert!(
        decoder
            .feed(event(json!({"error":{"message":"secret-must-not-leak"}})).as_bytes())
            .is_err()
    );
    let mut encoder = Encoder::new("messages", "model");
    encoder
        .push(super::stream::Delta::Tool(
            0,
            Some("id".into()),
            Some("tool".into()),
            "{".into(),
        ))
        .unwrap();
    encoder
        .push(super::stream::Delta::Finish("tool_calls".into()))
        .unwrap();
    assert!(encoder.push(super::stream::Delta::End).is_err());
}

#[tokio::test]
async fn gateway_port_survives_restart_and_conflicts_never_reassign_it() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let (gateway, listener) = super::Gateway::bind(&root).await.unwrap();
    let address = listener.local_addr().unwrap();
    // Keep the real listener as the conflict instead of releasing and racing
    // to reserve its port again.
    let occupied = listener;
    drop(gateway);
    let error = super::Gateway::bind(&root).await.err().unwrap();
    assert!(error.contains("固定端口被占用"));
    drop(occupied);
    let (gateway, listener) = super::Gateway::bind(&root).await.unwrap();
    assert_eq!(listener.local_addr().unwrap(), address);
    drop(listener);
    drop(gateway);
    std::fs::write(root.join("gateway/listener.json"), b"0").unwrap();
    assert!(super::Gateway::bind(&root).await.is_err());
}

#[test]
fn namespace_and_freeform_tools_roundtrip_without_losing_names_or_input() {
    let value = json!({"input":[{"type":"custom_tool_call","call_id":"patch-1","name":"apply_patch","input":"patch\n你"},{"type":"custom_tool_call_output","call_id":"patch-1","output":"ok"},{"type":"function_call","call_id":"exec-1","namespace":"functions","name":"exec_command","arguments":"{}"}],"tools":[{"type":"namespace","name":"functions","tools":[{"type":"function","name":"exec_command","parameters":{"type":"object"}}]},{"type":"custom","name":"apply_patch","description":"Apply a patch","format":{"type":"grammar","definition":"start: /.+/"}}]});
    let (tools, prepared) = super::tools::Tools::prepare("responses", &value).unwrap();
    let converted = protocol::request("responses", "chat-completions", &prepared, "model").unwrap();
    assert_eq!(converted["tools"].as_array().unwrap().len(), 2);
    let custom = prepared["input"][0]["name"].as_str().unwrap().to_string();
    let function = prepared["input"][2]["name"].as_str().unwrap().to_string();
    assert_eq!(converted["tools"][0]["function"]["name"], function);
    let restored = tools
        .response_item(
            json!({"type":"function_call","name":function,"call_id":"exec-1","arguments":"{}"}),
        )
        .unwrap();
    assert_eq!(restored["namespace"], "functions");
    assert_eq!(restored["name"], "exec_command");
    let mut encoder = Encoder::new("responses", "model").with_tools(tools);
    let mut result = encoder.start();
    result += &encoder
        .push(super::stream::Delta::Tool(
            0,
            Some("patch-1".into()),
            Some(custom),
            json!({"input":"patch\n你"}).to_string(),
        ))
        .unwrap();
    result += &encoder
        .push(super::stream::Delta::Finish("tool_calls".into()))
        .unwrap();
    result += &encoder.push(super::stream::Delta::End).unwrap();
    assert!(result.contains("response.custom_tool_call_input.delta"));
    assert!(result.contains("custom_tool_call"));
    assert!(result.contains("apply_patch"));
    assert!(result.contains("patch\\n你"));
}

#[tokio::test]
async fn http_gateway_converts_all_paths_authenticates_observes_and_restores_temp_configs() {
    use axum::{
        Json, Router,
        extract::State,
        http::HeaderMap,
        response::{IntoResponse, Response},
        routing::post,
    };
    use local_agent_core::{
        Store,
        providers::{BindingDraft, ProviderManager, vault::MemoryVault},
        sessions::NativeSessions,
    };
    type Seen = Arc<Mutex<Vec<(String, Value)>>>;
    async fn upstream(
        State(seen): State<Seen>,
        headers: HeaderMap,
        request: axum::extract::Request,
    ) -> Response {
        let path = request.uri().path().to_string();
        assert_eq!(
            headers.get("authorization").unwrap(),
            "Bearer upstream-secret-fixture"
        );
        let body: Value = serde_json::from_slice(
            &axum::body::to_bytes(request.into_body(), 32 * 1024 * 1024)
                .await
                .unwrap(),
        )
        .unwrap();
        seen.lock().unwrap().push((path.clone(), body.clone()));
        if let Some(response) = recovery::upstream(&body) {
            return response;
        }
        let a = Answer {
            text: "response-private-sentinel".into(),
            tools: vec![],
            input: Some(10),
            output: Some(2),
            finish: "stop".into(),
            ..Answer::default()
        };
        if body["stream"] == true {
            let parts = if path.ends_with("/chat/completions") {
                let mut parts = vec![];
                if body["thinking"]["type"] == "enabled" {
                    parts.push(event(json!({"choices":[{"delta":{"reasoning_content":"reasoning-private-sentinel"}}]})));
                }
                parts.extend([
                    event(
                        json!({"model":body["model"],"choices":[{"delta":{"content":"response-private-sentinel"}}]}),
                    ),
                    event(
                        json!({"choices":[{"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":2}}),
                    ),
                    "data: [DONE]\n\n".into(),
                ]);
                parts
            } else if path.ends_with("/messages") {
                vec![
                    event(
                        json!({"type":"message_start","message":{"model":body["model"],"usage":{"input_tokens":10}}}),
                    ),
                    event(
                        json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"response-private-sentinel"}}),
                    ),
                    event(
                        json!({"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":2}}),
                    ),
                    event(json!({"type":"message_stop"})),
                ]
            } else {
                vec![
                    event(
                        json!({"type":"response.output_text.delta","output_index":0,"delta":"response-private-sentinel"}),
                    ),
                    event(
                        json!({"type":"response.completed","response":a.render("responses","upstream",body["model"].as_str().unwrap()).unwrap()}),
                    ),
                ]
            };
            let slow = body["max_tokens"] == 777;
            let linger = body["max_tokens"] == 778 || body["max_output_tokens"] == 778;
            let stream = async_stream::stream! {for part in parts {yield Ok::<_,std::io::Error>(axum::body::Bytes::from(part));tokio::time::sleep(std::time::Duration::from_millis(if slow{5000}else{15})).await;}if linger {tokio::time::sleep(std::time::Duration::from_secs(5)).await;}};
            return (
                [("content-type", "text/event-stream")],
                axum::body::Body::from_stream(stream),
            )
                .into_response();
        }
        Json(if path.ends_with("/chat/completions"){json!({"id":"chat-fixture","object":"chat.completion","model":body["model"],"choices":[{"message":{"role":"assistant","content":a.text,"reasoning_content":if body["thinking"]["type"]=="enabled" {json!("reasoning-private-sentinel")} else {Value::Null}},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":2}})}else{a.render(if path.ends_with("/messages"){"messages"}else{"responses"},"upstream",body["model"].as_str().unwrap()).unwrap()}).into_response()
    }
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let data = root.join("data");
    let home = root.join("home");
    std::fs::create_dir_all(home.join(".claude")).unwrap();
    std::fs::create_dir_all(home.join(".codex")).unwrap();
    let original = b"{\"env\":{\"KEEP\":\"yes\"},\"permissions\":{\"deny\":[\"Read(.env)\"]}}\n";
    std::fs::write(home.join(".claude/settings.json"), original).unwrap();
    std::fs::write(
        home.join(".codex/config.toml"),
        "# keep\nmodel='original'\n",
    )
    .unwrap();
    let desktop = root.join("home/Library/Application Support/Claude-3p/configLibrary");
    std::fs::create_dir_all(&desktop).unwrap();
    let desktop_id = uuid::Uuid::new_v4().to_string();
    std::fs::write(
        desktop.join("_meta.json"),
        json!({"appliedId":desktop_id}).to_string(),
    )
    .unwrap();
    std::fs::write(desktop.join(format!("{desktop_id}.json")), b"{}\n").unwrap();
    let helper = root.join("fixture-helper");
    std::fs::write(&helper, b"isolated fixture helper").unwrap();
    let vault = Arc::new(MemoryVault::default());
    let manager = ProviderManager::new(home.clone(), &data, helper, vault.clone()).unwrap();
    let key = manager
        .credential(
            "upstream",
            zeroize::Zeroizing::new("upstream-secret-fixture".into()),
        )
        .unwrap();
    let seen: Seen = Arc::new(Mutex::new(vec![]));
    let upstream_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream_host = upstream_listener.local_addr().unwrap();
    let seen_clone = seen.clone();
    let upstream_task = tokio::spawn(async move {
        axum::serve(
            upstream_listener,
            Router::new()
                .route("/{*path}", post(upstream))
                .with_state(seen_clone),
        )
        .await
        .unwrap()
    });
    let store = Store::open(&data).unwrap();
    let sessions = Arc::new(NativeSessions::new(home.clone(), &[]));
    let (gateway, listener) = super::Gateway::bind(&data).await.unwrap();
    let state = crate::AppState {
        workbench: None,
        provider_adapters: crate::provider_models::adapters::builtin(),
        provider_client: crate::state::provider_client().unwrap(),
        credentials: vault,
        plugins: Arc::new(
            crate::plugin_host::external::Manager::open(
                &data,
                Arc::new(local_agent_core::providers::vault::MemoryVault::default()),
            )
            .unwrap(),
        ),
        readonly: Arc::new(crate::plugin_host::ReadonlyRuntime::builtin().unwrap()),
        store: Arc::new(Mutex::new(store)),
        token: Arc::new("test-token".into()),
        host: Arc::new("127.0.0.1:9".into()),
        sessions,
        session_work: Arc::new(crate::sessions::work::SessionWork::default()),
        observation: Arc::new(crate::observation::Observation::new(&data, "127.0.0.1:9").unwrap()),
        listeners: Arc::new(Mutex::new(Default::default())),
        providers: Arc::new(Mutex::new(manager)),
        verification_capacity: Arc::new(tokio::sync::Semaphore::new(2)),
        query_capacity: crate::blocking::query_capacity(),
        billing: Arc::new(Mutex::new(crate::billing::Billing::new(
            home.clone(),
            &data,
        ))),
        gateway: Arc::new(gateway),
    };
    let server_state = state.clone();
    let gateway_task = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new()
                .route(
                    "/gateway/{id}/{*path}",
                    post(super::capture).get(super::capture),
                )
                .with_state(server_state),
        )
        .await
        .unwrap()
    });
    let client = reqwest::Client::new();
    for (agent, protocol) in [
        ("claude", "messages"),
        ("claude", "chat-completions"),
        ("codex", "chat-completions"),
        ("claude", "responses"),
        ("codex", "messages"),
        ("opencode", "chat-completions"),
        ("claude-desktop", "chat-completions"),
    ] {
        let mut p = state.store.lock().unwrap().providers().unwrap().remove(0);
        p.vendor = "deepseek".into();
        p.protocol = protocol.into();
        p.endpoint = format!("http://{upstream_host}/v1");
        p.model = "provider-default".into();
        p.models = vec!["chosen-model".into()];
        if ["opencode", "claude-desktop", "codex"].contains(&agent) {
            p.models
                .extend(["another-model".into(), "disabled-model".into()]);
            p.disabled_models = vec!["disabled-model".into()];
        }
        p.configured = true;
        p.credential_ref = Some(key.credential_ref.clone());
        let client_model = |model: &str| {
            if agent == "claude-desktop" {
                local_agent_core::providers::claude_desktop_model_id(&p.id, model)
            } else {
                model.to_string()
            }
        };
        let selected = local_agent_core::providers::select_model(&p, Some("chosen-model")).unwrap();
        crate::providers::probe(
            &state.provider_client,
            &selected,
            Some(zeroize::Zeroizing::new(b"upstream-secret-fixture".to_vec())),
        )
        .await
        .unwrap();
        let (rendered, routing, local, transaction) = {
            let m = state.providers.lock().unwrap();
            let (rendered, routing) = state.gateway.prepare(&m, agent, &selected).unwrap();
            let local = String::from_utf8(
                m.secret(rendered.credential_ref.as_ref().unwrap())
                    .unwrap()
                    .to_vec(),
            )
            .unwrap();
            let before = m
                .status()
                .unwrap()
                .configs
                .into_iter()
                .find(|c| c.agent_id == agent)
                .unwrap();
            let transaction = m
                .prepare_routed(
                    BindingDraft {
                        operation: None,
                        agent_id: agent.into(),
                        expected_revision: before.revision.unwrap(),
                        provider_id: Some(p.id.clone()),
                        expected_provider_revision: Some(p.revision),
                        model: Some("chosen-model".into()),
                    },
                    Some(&p),
                    Some((&rendered, routing.clone())),
                )
                .unwrap();
            (rendered, routing, local, transaction)
        };
        let evidence = state.gateway.usage_evidence().unwrap();
        let route_evidence = evidence
            .iter()
            .find(|e| {
                e.provider_id == p.id
                    && e.revision
                        == u64::from_str_radix(
                            &local_agent_core::providers::files::hash(routing.id.as_bytes())[..13],
                            16,
                        )
                        .unwrap()
            })
            .unwrap();
        if agent == "codex" {
            assert_eq!(
                route_evidence.native_provider.as_deref(),
                Some(
                    format!(
                        "local_agent_{}",
                        &local_agent_core::providers::fingerprint(&rendered)[..24]
                    )
                    .as_str()
                )
            );
        }
        if agent == "claude-desktop" {
            assert!(
                route_evidence
                    .model_aliases
                    .iter()
                    .any(|(alias, model)| alias == &client_model("another-model")
                        && model == "another-model")
            );
        }
        let configured_endpoint = if agent == "opencode" {
            format!("{}/v1", rendered.endpoint)
        } else {
            rendered.endpoint.clone()
        };
        assert_eq!(
            state
                .gateway
                .configured_provider(agent, &configured_endpoint)
                .unwrap()
                .unwrap()
                .model,
            "chosen-model"
        );
        for endpoint in [
            format!("http://{}/v1", state.gateway.host),
            format!("{configured_endpoint}/wrong"),
            configured_endpoint.replace("127.0.0.1", "localhost"),
        ] {
            assert!(
                state
                    .gateway
                    .configured_provider(agent, &endpoint)
                    .unwrap()
                    .is_none()
            );
        }
        let other_agent = if agent == "codex" { "claude" } else { "codex" };
        assert!(
            state
                .gateway
                .configured_provider(other_agent, &configured_endpoint)
                .unwrap()
                .is_none()
        );
        assert_eq!(
            transaction.provider.as_ref().unwrap().model,
            "provider-default"
        );
        assert_eq!(transaction.model.as_deref(), Some("chosen-model"));
        let (release, gate) = std::sync::mpsc::channel();
        let entered = Arc::new(tokio::sync::Notify::new());
        let configuration = state.providers.clone();
        let held = entered.clone();
        let owner = std::thread::spawn(move || {
            let _configuration_guard = configuration.lock().unwrap();
            held.notify_one();
            gate.recv_timeout(std::time::Duration::from_secs(3))
                .unwrap();
        });
        entered.notified().await;
        let catalog_while_configuration_busy = client
            .get(format!(
                "http://{}/gateway/{}/v1/models",
                state.gateway.host, routing.id
            ))
            .bearer_auth(&local)
            .timeout(std::time::Duration::from_millis(500))
            .send()
            .await;
        release.send(()).unwrap();
        owner.join().unwrap();
        assert_eq!(
            catalog_while_configuration_busy.unwrap().status(),
            200,
            "authenticated gateway reads must not wait for the configuration transaction mutex"
        );
        let path = if agent != "codex" {
            "/v1/messages"
        } else {
            "/responses"
        };
        let url = format!(
            "{}{path}{}",
            rendered.endpoint,
            if agent == "claude-desktop" {
                "?beta=true"
            } else {
                ""
            }
        );
        let response = client
            .post(&url)
            .bearer_auth("wrong")
            .json(&json!({}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 401);
        let response = client
            .post(&url)
            .bearer_auth(&local)
            .header("origin", "https://evil.invalid")
            .json(&json!({}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 401);
        for query in ["beta=false", "beta=true&extra=1", "api_key=secret"] {
            let response = client
                .post(format!("{}{path}?{query}", rendered.endpoint))
                .bearer_auth(&local)
                .json(&json!({}))
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), 401);
        }
        if agent != "codex" {
            let catalog_url = format!("{}/v1/models", rendered.endpoint);
            assert_eq!(client.get(&catalog_url).send().await.unwrap().status(), 401);
            let catalog: serde_json::Value = client
                .get(&catalog_url)
                .bearer_auth(&local)
                .send()
                .await
                .unwrap()
                .error_for_status()
                .unwrap()
                .json()
                .await
                .unwrap();
            if ["opencode", "claude-desktop"].contains(&agent) {
                assert_eq!(catalog["data"].as_array().unwrap().len(), 3);
                let ids: Vec<_> = catalog["data"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|m| m["id"].as_str().unwrap())
                    .collect();
                let expected = if agent == "claude-desktop" {
                    vec![
                        client_model("chosen-model"),
                        client_model("another-model"),
                        client_model("provider-default"),
                    ]
                } else {
                    vec![
                        client_model("another-model"),
                        client_model("chosen-model"),
                        client_model("provider-default"),
                    ]
                };
                assert_eq!(ids, expected);
                assert!(catalog["data"].as_array().unwrap().iter().all(|m| {
                    m["display_name"]
                        .as_str()
                        .unwrap()
                        .starts_with(&format!("{} · ", p.name))
                }));
            } else {
                assert_eq!(
                    catalog["data"][0]["id"],
                    if agent == "claude-desktop" {
                        "claude-sonnet-5"
                    } else {
                        "chosen-model"
                    }
                );
                assert!(
                    catalog["data"][0]["display_name"]
                        .as_str()
                        .unwrap()
                        .contains("chosen-model")
                );
            }
            assert_eq!(
                client
                    .get(format!("{catalog_url}?beta=true"))
                    .bearer_auth(&local)
                    .send()
                    .await
                    .unwrap()
                    .status(),
                401
            );
        }
        let requests = if agent == "codex" {
            vec![
                (false, Some("codex_cli_rs"), None),
                (true, Some("codex_work_desktop"), None),
                (false, None, None),
                (false, Some("custom_client"), Some("codex_cli_rs/1.0")),
                (false, Some("codex_work_desktop"), Some("codex_cli_rs/1.0")),
            ]
        } else {
            vec![(false, None, None), (true, None, None)]
        };
        for (streamed, originator, user_agent) in requests {
            let mut body = if agent != "codex" {
                json!({"model":if ["opencode", "claude-desktop"].contains(&agent) {client_model("chosen-model")}else{"ignored-client-model".into()},"messages":[{"role":"user","content":"request-private-sentinel"}],"max_tokens":64,"stream":streamed})
            } else {
                json!({"model":"chosen-model","input":"request-private-sentinel","stream":streamed})
            };
            if protocol == "chat-completions" {
                if agent == "codex" {
                    body["reasoning"] = json!({"effort":"high"});
                } else {
                    body["thinking"] = json!({"type":"enabled","budget_tokens":1024});
                }
            }
            if agent == "claude" && protocol == "messages" {
                body["thinking"] = json!({"type":"enabled","budget_tokens":1024});
                body["metadata"] = json!({"user_id":json!({"session_id":"81c97c12-77da-4b7f-b0eb-e60908402875"}).to_string()});
                body["tools"] = json!([{"name":"read_fixture","input_schema":{"type":"object","properties":{"path":{"type":"string"}}}}]);
                body["messages"] = json!([
                    {"role":"user","content":"request-private-sentinel"},
                    {"role":"assistant","content":[{"type":"tool_use","id":"fixture-call","name":"read_fixture","input":{"path":"README.md"}}]},
                    {"role":"user","content":[{"type":"tool_result","tool_use_id":"fixture-call","content":"fixture-result"}]}
                ]);
            }
            let mut request = client.post(&url).bearer_auth(&local).json(&body);
            if let Some(value) = originator {
                request = request.header("originator", value);
            }
            if let Some(value) = user_agent {
                request = request.header("user-agent", value);
            }
            let response = request.send().await.unwrap();
            assert_eq!(response.status(), 200);
            let text = response.text().await.unwrap();
            if agent == "claude" && protocol == "messages" {
                let seen = seen.lock().unwrap();
                let upstream = &seen.last().unwrap().1;
                for key in ["thinking", "metadata", "tools", "messages"] {
                    assert_eq!(upstream[key], body[key], "same-protocol {key} must survive");
                }
            }
            assert!(
                text.contains("response-private-sentinel"),
                "{agent} {protocol}: {text}"
            );
            assert!(!text.contains("upstream-secret-fixture"));
            if protocol == "chat-completions" {
                assert!(
                    text.contains("reasoning-private-sentinel"),
                    "{agent}: {text}"
                );
                assert_eq!(
                    seen.lock().unwrap().last().unwrap().1["thinking"]["type"],
                    "enabled"
                );
            }
            let client_protocol = if agent == "codex" {
                "responses"
            } else {
                "messages"
            };
            if protocol != client_protocol {
                let client_id = if streamed {
                    text.lines()
                        .filter_map(|line| line.strip_prefix("data: "))
                        .filter_map(|raw| serde_json::from_str::<Value>(raw).ok())
                        .find_map(|v| {
                            v["message"]["id"]
                                .as_str()
                                .or_else(|| v["response"]["id"].as_str())
                                .map(str::to_owned)
                        })
                        .unwrap()
                } else {
                    serde_json::from_str::<Value>(&text).unwrap()["id"]
                        .as_str()
                        .unwrap()
                        .to_string()
                };
                let rows = state.observation.hub_rows_for_test();
                let recorded = rows
                    .iter()
                    .find(|r| r.client_response_id.as_deref() == Some(&client_id))
                    .unwrap();
                assert_eq!(recorded.native_response_id(), Some(client_id.as_str()));
                if !streamed && protocol == "chat-completions" {
                    assert_eq!(
                        recorded.upstream_response_id.as_deref(),
                        Some("chat-fixture")
                    );
                }
            }
            if streamed {
                assert!(
                    text.contains(if agent == "claude" && protocol == "messages" {
                        // This fixture emits data-only SSE. Same-protocol forwarding
                        // retains it rather than rewriting events through the encoder.
                        "\"type\":\"message_stop\""
                    } else if agent != "codex" {
                        "event: message_stop"
                    } else {
                        "event: response.completed"
                    })
                );
            }
        }
        if agent == "codex" {
            let catalog_url = format!("{}/codex/models.json", rendered.endpoint);
            assert_eq!(client.get(&catalog_url).send().await.unwrap().status(), 401);
            let catalog: Value = client
                .get(format!("{catalog_url}?client_version=0.163.0-alpha.10.2"))
                .bearer_auth(&local)
                .send()
                .await
                .unwrap()
                .error_for_status()
                .unwrap()
                .json()
                .await
                .unwrap();
            let models = catalog["models"].as_array().unwrap();
            assert_eq!(
                models
                    .iter()
                    .map(|m| m["slug"].as_str().unwrap())
                    .collect::<Vec<_>>(),
                vec!["another-model", "chosen-model", "provider-default"]
            );
            assert!(models.iter().all(|m| {
                m["input_modalities"] == json!(["text"])
                    && m["default_reasoning_level"] == "none"
                    && m["display_name"]
                        .as_str()
                        .unwrap()
                        .starts_with(&format!("{} · ", p.name))
            }));
            for (query, status) in [
                ("client_version=0.162.0", 200),
                ("client_version=0.164.0", 200),
                ("client_version=0.165.0", 200),
                ("client_version=0.166.0-alpha.5.1", 200),
                ("client_version=0.166.0-alpha.6", 200),
                ("client_version=0.166.0", 200),
                ("client_version=0.167.0", 200),
                ("client_version=invalid", 400),
                ("client_version=0.163.1&extra=x", 401),
                ("client_version=0.163.1&client_version=0.163.2", 401),
                ("beta=true", 401),
            ] {
                assert_eq!(
                    client
                        .get(format!("{catalog_url}?{query}"))
                        .bearer_auth(&local)
                        .send()
                        .await
                        .unwrap()
                        .status()
                        .as_u16(),
                    status
                );
            }
            assert_eq!(client.post(&url).bearer_auth(&local)
                .header("originator", "codex_cli_rs")
                .header("originator", "codex_work_desktop")
                .json(&json!({"model":"chosen-model", "input":"request-private-sentinel", "stream":false}))
                .send().await.unwrap().status(), 200);
        }
        assert_eq!(
            seen.lock().unwrap().last().unwrap().1["model"],
            "chosen-model"
        );
        if protocol == "chat-completions" {
            assert_eq!(
                seen.lock().unwrap().last().unwrap().1["thinking"]["type"],
                if agent == "codex" {
                    "disabled"
                } else {
                    "enabled"
                }
            );
        }
        if ["opencode", "claude-desktop", "codex"].contains(&agent) {
            for streamed in [false, true] {
                let body = if agent == "codex" {
                    json!({"model":"another-model","input":"request-private-sentinel","stream":streamed})
                } else {
                    json!({"model":client_model("another-model"),"messages":[{"role":"user","content":"request-private-sentinel"}],"stream":streamed})
                };
                let response = client
                    .post(&url)
                    .bearer_auth(&local)
                    .json(&body)
                    .send()
                    .await
                    .unwrap();
                assert_eq!(response.status(), 200);
                assert!(
                    response
                        .text()
                        .await
                        .unwrap()
                        .contains("response-private-sentinel")
                );
                assert_eq!(
                    seen.lock().unwrap().last().unwrap().1["model"],
                    "another-model"
                );
            }
            let before = seen.lock().unwrap().len();
            let mut rejected = vec![
                json!("unknown"),
                json!(client_model("disabled-model")),
                Value::Null,
            ];
            if agent == "claude-desktop" {
                rejected.extend([
                    json!("claude-sonnet-5"),
                    json!("chosen-model"),
                    json!(local_agent_core::providers::claude_desktop_model_id(
                        "other-provider",
                        "chosen-model"
                    )),
                ]);
            }
            for model in rejected {
                assert_eq!(
                    client
                        .post(&url)
                        .bearer_auth(&local)
                        .json(
                            &json!({"model":model,"messages":[{"role":"user","content":"hello"}]})
                        )
                        .send()
                        .await
                        .unwrap()
                        .status(),
                    400
                );
            }
            assert_eq!(
                seen.lock().unwrap().len(),
                before,
                "unpublished models must never reach upstream"
            );
        }
        {
            let m = state.providers.lock().unwrap();
            state.gateway.ensure(&routing, &selected).unwrap();
            m.apply(&transaction.id, Some(&p), true).unwrap();
            let bytes =
                std::fs::read(local_agent_core::providers::config::path(&home, agent).unwrap())
                    .unwrap();
            assert!(String::from_utf8_lossy(&bytes).contains(&state.gateway.host));
            let restore = m.prepare_restore(&transaction.id).unwrap();
            m.apply(&restore.id, None, true).unwrap();
        }
    }
    assert_eq!(
        std::fs::read(home.join(".claude/settings.json")).unwrap(),
        original
    );
    let rows = state.observation.hub_rows_for_test();
    assert_eq!(rows.len(), 30);
    assert_eq!(
        rows.iter()
            .filter(|r| r.session_id.as_deref()
                == Some("claude:81c97c12-77da-4b7f-b0eb-e60908402875")
                && r.agent_product_id.as_deref() == Some("claude-code-cli"))
            .count(),
        2
    );
    assert!(
        rows.iter().all(|r| r.provider_id.is_some()),
        "every captured request retains its connection identity"
    );
    assert_eq!(
        rows.iter()
            .filter(|r| r.agent_id == "opencode" && r.model.as_deref() == Some("another-model"))
            .count(),
        2
    );
    assert_eq!(
        rows.iter()
            .filter(
                |r| r.agent_product_id.as_deref() == Some("claude-code-desktop")
                    && r.model.as_deref() == Some("another-model")
            )
            .count(),
        2
    );
    for (product, expected) in [
        (Some("codex-cli"), 2),
        (Some("codex-desktop"), 2),
        (None, 12),
    ] {
        assert_eq!(
            rows.iter()
                .filter(|r| r.agent_id == "codex" && r.agent_product_id.as_deref() == product)
                .count(),
            expected
        );
    }
    assert!(
        rows.iter()
            .all(|r| r.client_protocol.is_some() && r.status == "success"),
        "{:?}",
        rows.iter()
            .filter(|r| r.client_protocol.is_none() || r.status != "success")
            .map(|r| (&r.agent_id, &r.protocol, &r.status, &r.timing_issue))
            .collect::<Vec<_>>()
    );
    assert!(rows.iter().all(|r| r.usage.total_tokens == Some(12)));
    assert_eq!(rows.iter().filter(|r| r.ttft_ms.is_some()).count(), 11);
    let mut p = state.store.lock().unwrap().providers().unwrap().remove(0);
    p.protocol = "chat-completions".into();
    p.endpoint = format!("http://{upstream_host}/v1");
    p.model = "chosen-model".into();
    p.configured = true;
    p.credential_ref = Some(key.credential_ref.clone());
    let (rendered, local) = {
        let m = state.providers.lock().unwrap();
        let (rendered, _) = state.gateway.prepare(&m, "claude", &p).unwrap();
        let local = String::from_utf8(
            m.secret(rendered.credential_ref.as_ref().unwrap())
                .unwrap()
                .to_vec(),
        )
        .unwrap();
        (rendered, local)
    };
    let mut cancelled=client.post(format!("{}/v1/messages",rendered.endpoint)).bearer_auth(local).json(&json!({"messages":[{"role":"user","content":"request-private-sentinel"}],"stream":true,"max_tokens":777})).send().await.unwrap();
    assert!(cancelled.chunk().await.unwrap().is_some());
    drop(cancelled);
    for _ in 0..50 {
        if state.observation.active_count("claude") == 0 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert_eq!(state.observation.active_count("claude"), 0);
    assert!(
        state
            .observation
            .hub_rows_for_test()
            .iter()
            .any(|r| r.status == "cancelled")
    );
    // Closing after a complete terminal packet must not turn a successful request
    // into cancellation, even while the upstream HTTP connection remains open.
    for protocol in ["chat-completions", "responses"] {
        p.protocol = protocol.into();
        let (rendered, local) = {
            let manager = state.providers.lock().unwrap();
            let (rendered, _) = state.gateway.prepare(&manager, "codex", &p).unwrap();
            let local = String::from_utf8(
                manager
                    .secret(rendered.credential_ref.as_ref().unwrap())
                    .unwrap()
                    .to_vec(),
            )
            .unwrap();
            (rendered, local)
        };
        let before: std::collections::HashSet<_> = state
            .observation
            .hub_rows_for_test()
            .into_iter()
            .map(|r| r.id)
            .collect();
        let mut response=client.post(format!("{}/responses",rendered.endpoint)).bearer_auth(local).json(&json!({"model":"chosen-model","input":"request-private-sentinel","stream":true,"max_output_tokens":778})).send().await.unwrap();
        assert_eq!(response.status(), 200);
        let mut received = String::new();
        while !received.contains("response.completed") {
            let chunk = tokio::time::timeout(std::time::Duration::from_secs(2), response.chunk())
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            received.push_str(&String::from_utf8_lossy(&chunk));
        }
        drop(response);
        let rows: Vec<_> = state
            .observation
            .hub_rows_for_test()
            .into_iter()
            .filter(|r| !before.contains(&r.id))
            .collect();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].status, "success", "{protocol}");
        assert_eq!(state.observation.active_count("codex"), 0);
    }
    recovery::verify(&state, &client, &p).await;
    fn scan(path: &std::path::Path) {
        for p in std::fs::read_dir(path).unwrap() {
            let p = p.unwrap().path();
            if p.is_dir() {
                scan(&p)
            } else {
                let mut b = std::fs::read(&p)
                    .unwrap_or_else(|error| panic!("privacy audit {}: {error}", p.display()));
                if p.file_name()
                    .is_some_and(|name| name == "gateway-activity-inputs.json")
                {
                    let mut summaries: Value = serde_json::from_slice(&b).unwrap();
                    assert_eq!(summaries["version"], 1);
                    let rows = summaries["rows"].as_array_mut().unwrap();
                    assert!(rows.len() <= 512);
                    for row in rows {
                        assert!(row["input"]["text"].as_str().unwrap().chars().count() <= 241);
                        // Only the bounded user-summary field is allowed to retain input.
                        row["input"]["text"] = Value::Null;
                    }
                    b = serde_json::to_vec(&summaries).unwrap();
                }
                for secret in [
                    "request-private-sentinel",
                    "response-private-sentinel",
                    "upstream-secret-fixture",
                    "fixture-result",
                    "reasoning-private-sentinel",
                    "refusal-private-sentinel",
                ] {
                    assert!(
                        !b.windows(secret.len()).any(|w| w == secret.as_bytes()),
                        "{}",
                        p.display()
                    );
                }
            }
        }
    }
    assert_eq!(state.observation.capacity.available_permits(), 64);
    assert!(
        state
            .observation
            .hub_rows_for_test()
            .iter()
            .all(|row| row.status != "in_progress")
    );
    // A fixed sleep does not fence atomic replacement / SQLite writer files.
    // Audit the same complete committed data after the real persistence barrier.
    state.observation.flush().unwrap();
    scan(&data);
    gateway_task.abort();
    upstream_task.abort();
    let _ = gateway_task.await;
    let _ = upstream_task.await;
}
