use super::*;

#[tokio::test]
async fn stream_preflight_respects_event_order_across_transport_chunks() {
    let output = "data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"partial\"}}\n\n";
    let rejection = "data: {\"type\":\"error\",\"error\":{\"type\":\"rate_limit_error\"}}\n\n";
    let ordered = [output, rejection].concat();
    let make = |chunks: Vec<Bytes>| {
        let body = Body::from_stream(futures_util::stream::iter(
            chunks.into_iter().map(Ok::<_, std::convert::Infallible>),
        ));
        ([("content-type", "text/event-stream")], body).into_response()
    };
    for chunks in [
        vec![Bytes::copy_from_slice(ordered.as_bytes())],
        vec![
            Bytes::from_static(output.as_bytes()),
            Bytes::from_static(rejection.as_bytes()),
        ],
    ] {
        let response = preflight(make(chunks), "messages", true).await.unwrap();
        let bytes = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        assert_eq!(bytes, ordered.as_bytes());
    }
    let reversed = [rejection, output].concat();
    let error = preflight(make(vec![Bytes::from(reversed)]), "messages", true)
        .await
        .unwrap_err();
    assert_eq!(error.status(), StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn mismatched_stream_format_preserves_a_complete_upstream_rejection() {
    for (retry, expected) in [
        (["12"].as_slice(), 12),
        (["12", "3600"].as_slice(), 60),
        (["12, 3600"].as_slice(), 60),
        ([].as_slice(), 60),
    ] {
        let mut response = axum::Json(serde_json::json!({
            "type": "error",
            "error": {"type": "rate_limit_error", "message": "fixture rate limit"}
        }))
        .into_response();
        for value in retry {
            response
                .headers_mut()
                .append("retry-after", HeaderValue::from_static(value));
        }
        assert_eq!(
            retry_seconds(&response, 60),
            expected,
            "Retry-After {retry:?}"
        );
        let error = preflight(response, "messages", true).await.unwrap_err();
        assert_eq!(error.status(), StatusCode::TOO_MANY_REQUESTS);
        assert!(matches!(failure(error.status(), &error), Failure::Rate));
        assert_eq!(retry_seconds(&error, 60), expected, "Retry-After {retry:?}");
    }
}

#[test]
fn rate_reservations_release_before_send_and_count_once_after_send() {
    let runtime = Arc::new(Runtime::default());
    let limit = ModelLimit {
        max_concurrent: 0,
        requests_per_minute: 2,
        ..Default::default()
    };
    let reserve = || runtime.slot("shared model".into(), &limit);
    let first = reserve().unwrap();
    let cancelled = reserve().unwrap();
    assert!(reserve().is_err());
    drop(cancelled);
    let other = reserve().unwrap();
    let send = SendSlots(vec![first]);
    send.mark_sent().unwrap();
    send.mark_sent().unwrap();
    drop(send);
    assert!(reserve().is_err());
    drop(other);
    let last = SendSlots(vec![reserve().unwrap()]);
    last.mark_sent().unwrap();
    drop(last);
    assert!(reserve().is_err());
}

#[test]
fn access_rejection_requires_unambiguous_wire_status_and_code() {
    for (status, bytes, expected) in [
        (
            401,
            br#"{"error":{"type":"authentication_error","code":"invalid_api_key"}}"#.as_slice(),
            Some("upstream_credential_rejected"),
        ),
        (
            403,
            br#"{"error":{"type":"invalid_request_error","code":"model_not_found"}}"#.as_slice(),
            Some("upstream_model_access_denied"),
        ),
        (
            401,
            br#"{"error":{"type":"authentication_error","message":"invalid_api_key"}}"#.as_slice(),
            None,
        ),
        (
            403,
            br#"{"error":{"type":"permission_error","code":"permission_denied"}}"#.as_slice(),
            None,
        ),
        (
            403,
            br#"{"error":{"code":"permission_denied","code":"model_access_denied"}}"#.as_slice(),
            None,
        ),
        (
            401,
            br#"{"error":{"type":"permission_error","code":"invalid_api_key"}}"#.as_slice(),
            None,
        ),
        (
            403,
            br#"{"error":{"type":"authentication_error","code":"model_access_denied"}}"#.as_slice(),
            None,
        ),
        (
            401,
            br#"{"error":{"type":3,"code":"invalid_api_key"}}"#.as_slice(),
            None,
        ),
        (
            404,
            br#"{"error":{"code":"model_not_found"}}"#.as_slice(),
            None,
        ),
    ] {
        let mut extensions = Extensions::new();
        mark_upstream_rejection(
            StatusCode::from_u16(status).unwrap(),
            bytes,
            &mut extensions,
        );
        assert_eq!(
            extensions
                .get::<AccessRejection>()
                .copied()
                .map(AccessRejection::reason),
            expected
        );
    }
    for error in [
        r#"{"type":"error","code":"invalid_api_key"}"#,
        r#"{"type":"error","error":{"type":"content_policy_violation","code":"provider_error"}}"#,
    ] {
        let response = reply_rejection(&serde_json::from_str(error).unwrap(), false, None).unwrap();
        assert!(matches!(
            failure(response.status(), &response),
            Failure::Stop
        ));
    }
}

#[test]
fn official_openrouter_reply_uses_the_error_shape_of_each_wire_protocol() {
    for (value, status, retryable) in [
        (
            json!({"error":{"code":429,"message":"throttled"}}),
            429,
            true,
        ),
        (
            json!({"error":{"code":402,"message":"credits"}}),
            402,
            false,
        ),
        (
            json!({"error":{"code":403,"metadata":{"error_type":"rate_limit_exceeded"}}}),
            403,
            false,
        ),
        (
            json!({"type":"error","error":{"type":"api_error","error_type":"payment_required"}}),
            402,
            false,
        ),
        (
            json!({"type":"error","error":{"type":"permission_error","error_type":"rate_limit_exceeded"}}),
            403,
            false,
        ),
        (
            json!({"type":"response.failed","response":{"error":{"code":"server_error"},"error_type":"authentication"}}),
            401,
            false,
        ),
        (
            json!({"type":"response.failed","response":{"error":{"code":"invalid_prompt"},"error_type":"context_length_exceeded"}}),
            400,
            false,
        ),
        (
            json!({"object":"response","status":"failed","error":{"code":"server_error"},"error_type":"provider_overloaded"}),
            503,
            true,
        ),
    ] {
        let response = reply_rejection(&value, true, None).unwrap();
        assert_eq!(response.status().as_u16(), status, "{value}");
        assert_eq!(
            matches!(
                failure(response.status(), &response),
                Failure::Rate | Failure::Transient
            ),
            retryable,
            "{value}"
        );
    }
    let empty = json!({"error":{}});
    assert!(reply_rejection(&empty, true, None).is_none());
    let foreign = json!({"error":{"code":429,"message":"foreign error code"}});
    assert_eq!(
        reply_rejection(&foreign, false, None).unwrap().status(),
        StatusCode::BAD_GATEWAY,
    );
}

#[tokio::test]
async fn access_cooldowns_distinguish_models_and_connection_identity() {
    let fixture = Fixture::new(Arc::new(MemoryVault::default())).await;
    let provider = fixture.provider();
    let (account, model) = access_scopes(&provider);
    let mut sibling_model = provider.clone();
    sibling_model.model = "another-model".into();
    let (same_account, sibling) = access_scopes(&sibling_model);
    assert_eq!(account, same_account);
    let mut sibling_path = provider.clone();
    sibling_path.endpoint.push_str("/tenant");
    let (other_account, other_model) = access_scopes(&sibling_path);
    let mut rotated = provider;
    rotated.credential_ref = Some(uuid::Uuid::new_v4().to_string());
    let (new_key, new_model) = access_scopes(&rotated);
    let store = fixture.state.store.lock().unwrap();
    let active = |account, model| AccessRejection::active(&store, account, model).unwrap();
    let now = chrono::Utc::now().timestamp_millis();
    store
        .service_set_cooldown(&model, now + 60_000, AccessRejection::Model.reason())
        .unwrap();
    assert!(matches!(
        active(&account, &model),
        Some(AccessRejection::Model)
    ));
    assert!(active(&account, &sibling).is_none());
    store
        .service_set_cooldown(&account, now + 60_000, AccessRejection::Credential.reason())
        .unwrap();
    assert!(matches!(
        active(&account, &sibling),
        Some(AccessRejection::Credential)
    ));
    assert!(active(&other_account, &other_model).is_none());
    assert!(active(&new_key, &new_model).is_none());
    store
        .service_set_cooldown(&new_key, now - 1, AccessRejection::Credential.reason())
        .unwrap();
    assert!(active(&new_key, &new_model).is_none());
    assert!(store.service_cooldown_probe(&new_key).unwrap());
    store.service_recovered(&new_key).unwrap();
    assert!(!store.service_cooldown_probe(&new_key).unwrap());
}
use local_agent_core::{
    Store,
    providers::{
        ConfigError, ProviderManager,
        vault::{MemoryVault, Vault},
    },
};
use serde_json::json;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    mpsc,
};
use zeroize::Zeroizing;

mod accounting;
mod deadline;
mod minimax;
mod openrouter_rejections;
mod provider_balance;
mod quota_reset;

/// Every gate has a timeout and releases on unwind; a regression cannot strand
/// a worker or hold a test database lock indefinitely.
struct Gate {
    release: Option<mpsc::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl Drop for Gate {
    fn drop(&mut self) {
        if let Some(release) = self.release.take() {
            let _ = release.send(());
        }
        if let Some(thread) = self.thread.take() {
            thread.join().unwrap();
        }
    }
}
fn hold(
    work: impl FnOnce(tokio::sync::oneshot::Sender<()>, mpsc::Receiver<()>) + Send + 'static,
) -> (Gate, tokio::sync::oneshot::Receiver<()>) {
    let (ready, entered) = tokio::sync::oneshot::channel();
    let (release, wait) = mpsc::channel();
    (
        Gate {
            release: Some(release),
            thread: Some(std::thread::spawn(move || work(ready, wait))),
        },
        entered,
    )
}

#[derive(Default)]
struct SlowVault {
    inner: MemoryVault,
    gate: Mutex<Option<VaultGate>>,
    reads: AtomicUsize,
}
struct VaultGate {
    account: Option<String>,
    ready: tokio::sync::oneshot::Sender<()>,
    wait: mpsc::Receiver<()>,
}
impl SlowVault {
    fn pause_next(&self) -> (Gate, tokio::sync::oneshot::Receiver<()>) {
        self.pause(None)
    }

    fn pause(&self, account: Option<String>) -> (Gate, tokio::sync::oneshot::Receiver<()>) {
        let (ready, entered) = tokio::sync::oneshot::channel();
        let (release, wait) = mpsc::channel();
        *self.gate.lock().unwrap() = Some(VaultGate {
            account,
            ready,
            wait,
        });
        (
            Gate {
                release: Some(release),
                thread: None,
            },
            entered,
        )
    }
}
impl Vault for SlowVault {
    fn get(&self, account: &str) -> Result<Zeroizing<Vec<u8>>, ConfigError> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        let gate = {
            let mut gate = self.gate.lock().unwrap();
            if gate
                .as_ref()
                .is_some_and(|gate| gate.account.as_deref().is_none_or(|id| id == account))
            {
                gate.take()
            } else {
                None
            }
        };
        if let Some(VaultGate { ready, wait, .. }) = gate {
            let _ = ready.send(());
            wait.recv_timeout(Duration::from_secs(9)).unwrap();
        }
        self.inner.get(account)
    }
    fn set(&self, account: &str, value: &[u8]) -> Result<(), ConfigError> {
        self.inner.set(account, value)
    }
    fn delete(&self, account: &str) -> Result<(), ConfigError> {
        self.inner.delete(account)
    }
}

struct Fixture {
    state: AppState,
    runtime: crate::runtime::Runtime,
    client: reqwest::Client,
    tasks: Vec<tokio::task::JoinHandle<()>>,
    root: tempfile::TempDir,
}

#[tokio::test]
async fn desktop_and_codex_service_catalogs_route_authenticated_requests() {
    let vault = Arc::new(MemoryVault::default());
    let mut fixture = Fixture::new(vault.clone()).await;
    let upstream = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = upstream.local_addr().unwrap();
    fixture.tasks.push(tokio::spawn(async move {
        let router = axum::Router::new().route("/v1/messages", axum::routing::post(
            |axum::Json(body): axum::Json<serde_json::Value>| async move {
                assert_eq!(body["thinking"]["type"], "disabled");
                let model = body["model"].as_str().unwrap();
                if model == "one" && body["messages"].to_string().contains("fallback fixture") {
                    return (StatusCode::SERVICE_UNAVAILABLE, "offline upstream busy").into_response();
                }
                if body["messages"].to_string().contains("empty gateway fixture") {
                    return StatusCode::BAD_GATEWAY.into_response();
                }
                if body["messages"].to_string().contains("empty namespace fixture") {
                    assert!(body.get("tools").is_none());
                    assert!(body.get("tool_choice").is_none());
                }
                if body["system"].as_str() == Some("Offline test instructions") {
                    assert!(body["messages"].to_string().contains("<environment_context>"));
                }
                let result = json!({"id":"msg-service-fixture","type":"message","role":"assistant","model":model,"content":[{"type":"text","text":model}],"stop_reason":"end_turn","usage":{"input_tokens":10,"output_tokens":2}});
                if body["stream"] == true {
                    let frames = [
                        json!({"type":"message_start","message":{"id":"msg-service-fixture","type":"message","role":"assistant","model":model,"content":[],"usage":{"input_tokens":10}}}),
                        json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
                        json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":model}}),
                        json!({"type":"content_block_stop","index":0}),
                        json!({"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":2}}),
                        json!({"type":"message_stop"}),
                    ].map(|event| format!("data: {event}\n\n")).join("");
                    return ([("content-type", "text/event-stream")], frames).into_response();
                }
                axum::Json(result).into_response()
            }
        ));
        axum::serve(upstream, router).await.unwrap();
    }));
    let mut provider = fixture.provider();
    provider.endpoint = format!("http://{address}");
    provider.model = "one".into();
    provider.models = vec!["one".into(), "two".into()];
    let first = uuid::Uuid::new_v4().to_string();
    let second = uuid::Uuid::new_v4().to_string();
    let reference = uuid::Uuid::new_v4().to_string();
    vault.set(&reference, b"model-service-fixture-key").unwrap();
    let (codex, desktop) = {
        let store = fixture.state.store.lock().unwrap();
        store.save_provider(provider.clone()).unwrap();
        let caps = json!({"contextWindow":96000,"maxOutputTokens":4096,"tools":true,"thinking":false,"inputModalities":["text"]});
        let service = store.save_model_service(serde_json::from_value(json!({
            "id":uuid::Uuid::new_v4().to_string(), "revision":0,"name":"Fixture service","defaultModelId":first,
            "models":[{"id":first,"name":"One","providerId":provider.id,"model":"one","capabilities":caps},
                      {"id":second,"name":"Two","providerId":provider.id,"model":"two","capabilities":caps}],
            "routingPolicy":{"enabled":true,"cooldownSeconds":1}
        })).unwrap()).unwrap();
        let make = |agent| {
            store
                .create_model_service_binding(agent, "fixture", &reference, &service)
                .unwrap()
                .0
        };
        (make("codex"), make("claude-desktop"))
    };
    let codex = crate::gateway::dynamic::configured_endpoint(&fixture.state, &codex);
    let desktop = format!(
        "{}/v1",
        crate::gateway::dynamic::configured_endpoint(&fixture.state, &desktop)
    );
    let codex_catalog = format!("{codex}/codex/models.json?client_version=0.166.0-alpha.6");
    assert_eq!(
        fixture
            .client
            .get(&codex_catalog)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let catalog: serde_json::Value = fixture
        .client
        .get(codex_catalog)
        .bearer_auth("model-service-fixture-key")
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        catalog["models"][0]["slug"],
        local_agent_core::routing::ALIAS
    );
    assert_eq!(
        catalog["models"][2]["slug"],
        format!("local-agent-model-{second}")
    );
    assert_eq!(catalog["models"][2]["context_window"], 96000);
    for stream in [false, true] {
        let response = fixture.client.post(format!("{codex}/responses"))
            .bearer_auth("model-service-fixture-key")
            .header("session_id", uuid::Uuid::new_v4().to_string())
            .json(&json!({
                "model":catalog["models"][2]["slug"],
                "input":[{"type":"message","role":"developer","content":[{"type":"input_text","text":"Offline test instructions"}]},
                    {"type":"message","role":"user","content":[{"type":"input_text","text":"<environment_context>\n<cwd>/offline/fixture</cwd>\n</environment_context>"}]},
                    {"type":"message","role":"user","content":[{"type":"input_text","text":"offline fixture"}]}],
                "tools":[{"type":"custom","name":"apply_patch","description":"Apply patch","format":{"type":"text"}}],
                "reasoning":{"effort":"none"},"tool_choice":"auto","parallel_tool_calls":false,
                "include":["reasoning.encrypted_content"],"store":false,"stream":stream
            }))
            .send().await.unwrap();
        let status = response.status();
        let response = response.text().await.unwrap();
        assert_eq!(status, StatusCode::OK, "{response}");
        if stream {
            assert!(
                response.contains("response.created")
                    && response.contains("response.output_item.done")
                    && response.contains("response.completed")
                    && response.contains("\"delta\":\"two\""),
                "{response}"
            );
        } else {
            let body: serde_json::Value = serde_json::from_str(&response).unwrap();
            assert_eq!(body["output"][0]["content"][0]["text"], "two");
        }
    }
    let response = fixture
        .client
        .post(format!("{codex}/responses"))
        .bearer_auth("model-service-fixture-key")
        .json(&json!({
            "model":catalog["models"][0]["slug"],
            "input":"empty namespace fixture",
            "tools":[{"type":"namespace","name":"functions","description":"","tools":[]}],
            "tool_choice":"auto","parallel_tool_calls":false,"store":false,"stream":true
        }))
        .send()
        .await
        .unwrap();
    let status = response.status();
    let body = response.text().await.unwrap();
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        body.contains("response.completed") && body.contains("\"delta\":\"one\""),
        "{body}"
    );
    let rejected = fixture.client.post(format!("{codex}/responses"))
        .bearer_auth("model-service-fixture-key")
        .json(&json!({"model":catalog["models"][2]["slug"],"input":"empty gateway fixture","stream":true}))
        .send().await.unwrap();
    assert_eq!(rejected.status(), StatusCode::BAD_GATEWAY);
    let error: serde_json::Value = rejected.json().await.unwrap();
    assert!(
        error["error"]["message"]
            .as_str()
            .unwrap()
            .contains("上游模型接口返回 502")
    );
    assert!(
        fixture
            .state
            .observation
            .hub_rows_for_test()
            .iter()
            .any(|row| row.agent_id == "codex"
                && row.status == "error"
                && row.http_status == Some(502))
    );
    let catalog: serde_json::Value = fixture
        .client
        .get(format!("{desktop}/models"))
        .bearer_auth("model-service-fixture-key")
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let desktop_default = local_agent_core::model_services::client_model_id(
        "claude-desktop",
        local_agent_core::routing::ALIAS,
    );
    assert_eq!(catalog["data"][0]["id"], desktop_default);
    let call_desktop = async || {
        fixture.client.post(format!("{desktop}/messages"))
            .bearer_auth("model-service-fixture-key")
            .header("session_id", uuid::Uuid::new_v4().to_string())
            .json(&json!({"model":desktop_default,"max_tokens":30,"messages":[{"role":"user","content":"offline fixture"}]}))
            .send().await.unwrap().error_for_status().unwrap().json::<serde_json::Value>().await.unwrap()
    };
    let (until, reason) = fixture
        .state
        .store
        .lock()
        .unwrap()
        .service_cooldown(&scopes(&provider).1)
        .unwrap()
        .unwrap();
    assert_eq!(reason, "upstream_failure", "The fixed Codex 502 is shared");
    let response = call_desktop().await;
    assert_eq!(
        response["content"].as_array().unwrap().last().unwrap()["text"],
        "two"
    );
    tokio::time::sleep(Duration::from_millis(
        (until - chrono::Utc::now().timestamp_millis()).max(0) as u64 + 25,
    ))
    .await;
    let response = call_desktop().await;
    assert_eq!(response["content"][0]["text"], "one");

    let session = uuid::Uuid::new_v4().to_string();
    let context = json!({"type":"message","role":"user","content":"# AGENTS.md instructions for /offline/fixture\nUse fixture data only"});
    let user = json!({"type":"message","role":"user","content":"fallback fixture"});
    let request = |input| {
        fixture
            .client
            .post(format!("{codex}/responses"))
            .bearer_auth("model-service-fixture-key")
            .header("thread-id", &session)
            .header("session-id", "cache-affinity-fixture")
            .json(&json!({"model":local_agent_core::routing::ALIAS,"input":input}))
    };
    let fallback: serde_json::Value = request(json!([context, user]))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(fallback["output"][0]["content"][0]["text"], "two");
    let continuation: serde_json::Value = request(json!([context,user,
        {"type":"function_call","call_id":"fixture-call","name":"read","arguments":"{}"},
        {"type":"function_call_output","call_id":"fixture-call","output":"fixture output"},
        {"type":"message","role":"user","content":"<environment_context>\n<cwd>/offline/fixture</cwd>\n</environment_context>"}
    ]))
    .send()
    .await
    .unwrap()
    .error_for_status()
    .unwrap()
    .json()
    .await
    .unwrap();
    assert_eq!(continuation["output"][0]["content"][0]["text"], "two");

    fixture.state.observation.flush().unwrap();
    let session_id = format!("codex:{session}");
    let read_sessions = || {
        fixture
            .client
            .get(format!(
                "http://{}/api/sessions?includeGateway=true&query={session}",
                fixture.state.host
            ))
            .bearer_auth(fixture.state.token.as_str())
    };
    let page: serde_json::Value = read_sessions()
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(page["coverage"], "gateway_history");
    assert_eq!(page["total"], 1);
    assert_eq!(page["items"][0]["id"], session_id);
    assert_eq!(page["items"][0]["status"], "observed");
    assert_eq!(page["items"][0]["title"], "fallback fixture");
    assert_eq!(page["items"][0]["titleSource"], "first_user_message");
    assert!(page["items"][0]["modelCalls"].as_u64().unwrap() >= 1);
    assert!(page["items"][0]["projectPath"].is_null());
    let requests: serde_json::Value = fixture
        .client
        .get(format!(
            "http://{}/api/request-dashboard",
            fixture.state.host
        ))
        .query(&[("source", "gateway"), ("session", session_id.as_str())])
        .bearer_auth(fixture.state.token.as_str())
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let request_rows = requests["items"].as_array().unwrap();
    assert!(!request_rows.is_empty());
    assert!(
        request_rows
            .iter()
            .all(|row| row["sessionId"] == session_id)
    );

    let sessions_dir = fixture.root.path().join("home/.codex/sessions");
    std::fs::create_dir_all(&sessions_dir).unwrap();
    let timestamp = chrono::Utc::now().to_rfc3339();
    let meta = json!({"type":"session_meta","timestamp":timestamp,"payload":{"id":session}});
    let message = json!({"type":"response_item","timestamp":timestamp,"payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"Authorized fixture conversation"}]}});
    std::fs::write(
        sessions_dir.join("rollout-fixture.jsonl"),
        format!("{meta}\n{message}\n"),
    )
    .unwrap();
    fixture.state.sessions.set_enabled("codex", true).unwrap();
    fixture.state.sessions.refresh().unwrap();
    let page: serde_json::Value = read_sessions()
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        page["total"], 1,
        "the native and observed identities are the same"
    );
    assert_eq!(page["items"][0]["id"], session_id);
    assert_eq!(page["items"][0]["status"], "imported");
    assert_eq!(page["items"][0]["title"], "Authorized fixture conversation");

    fixture.state.sessions.set_enabled("codex", false).unwrap();
    let page: serde_json::Value = read_sessions()
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(page["total"], 1);
    assert_eq!(page["items"][0]["status"], "observed");
    assert_eq!(page["items"][0]["title"], "fallback fixture");
}
impl Drop for Fixture {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}
impl Fixture {
    async fn new(vault: Arc<dyn Vault>) -> Self {
        Self::with_upstream(vault, None).await
    }

    async fn detail_when(
        &self,
        session: &str,
        ready: impl Fn(&serde_json::Value) -> bool,
    ) -> serde_json::Value {
        tokio::time::timeout(Duration::from_secs(3), async {
            let api = format!("http://{}", self.state.host);
            let session_id = format!("opencode:{session}");
            loop {
                let page: serde_json::Value = self
                    .client
                    .get(format!("{api}/api/request-dashboard"))
                    .bearer_auth(self.state.token.as_str())
                    .query(&[("agent", "opencode"), ("session", session_id.as_str())])
                    .send()
                    .await
                    .unwrap()
                    .error_for_status()
                    .unwrap()
                    .json()
                    .await
                    .unwrap();
                if let Some(id) = page["items"]
                    .as_array()
                    .unwrap()
                    .first()
                    .and_then(|row| row["id"].as_str())
                {
                    let detail: serde_json::Value = self
                        .client
                        .get(format!("{api}/api/request-detail"))
                        .bearer_auth(self.state.token.as_str())
                        .query(&[("id", id)])
                        .send()
                        .await
                        .unwrap()
                        .error_for_status()
                        .unwrap()
                        .json()
                        .await
                        .unwrap();
                    if ready(&detail) {
                        return detail;
                    }
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap()
    }

    async fn with_upstream(vault: Arc<dyn Vault>, client: Option<reqwest::Client>) -> Self {
        let root = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
        let home = root.path().join("home");
        let data = root.path().join("data");
        std::fs::create_dir_all(&home).unwrap();
        let store = Store::open(&data).unwrap();
        let helper = root.path().join("fixture-helper");
        std::fs::write(&helper, b"offline fixture").unwrap();
        let manager = ProviderManager::new(home.clone(), &data, helper, vault.clone()).unwrap();
        let (gateway, gateway_listener) = crate::gateway::Gateway::bind(&data).await.unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let host = listener.local_addr().unwrap().to_string();
        let mut observation = crate::observation::Observation::new(&data, &host).unwrap();
        if let Some(client) = client {
            observation.client = client;
        }
        let state = AppState {
            iam: Default::default(),
            workbench: None,
            provider_adapters: crate::provider_models::adapters::builtin(),
            provider_client: crate::state::provider_client().unwrap(),
            credentials: vault,
            plugins: Arc::new(
                crate::plugin_host::external::Manager::open(
                    &data,
                    Arc::new(MemoryVault::default()),
                )
                .unwrap(),
            ),
            readonly: Arc::new(crate::plugin_host::ReadonlyRuntime::builtin().unwrap()),
            store: Arc::new(Mutex::new(store)),
            token: Arc::new("isolated-responsiveness-token-00000000".into()),
            host: Arc::new(host.clone()),
            sessions: Arc::new(local_agent_core::sessions::NativeSessions::new(
                home.clone(),
                &[],
            )),
            observation: Arc::new(observation),
            listeners: Arc::new(Mutex::new(Default::default())),
            providers: Arc::new(Mutex::new(manager)),
            verification_capacity: Arc::new(tokio::sync::Semaphore::new(2)),
            query_capacity: crate::blocking::query_capacity(),
            session_work: Arc::new(crate::sessions::work::SessionWork::default()),
            billing: Arc::new(Mutex::new(crate::billing::Billing::new(home, &data))),
            gateway: Arc::new(gateway),
        };
        let runtime = crate::runtime::Runtime::new(local_agent_core::runtime::Identity {
            protocol: 1,
            version: "fixture".into(),
            instance_id: uuid::Uuid::new_v4().to_string(),
            pid: std::process::id(),
            persistent: false,
        });
        let app = crate::routes::management(state.clone()).merge(runtime.routes(state.clone()));
        let gateway_app = crate::routes::gateway(state.clone());
        let tasks = vec![
            tokio::spawn(async move {
                axum::serve(listener, app).await.unwrap();
            }),
            tokio::spawn(async move {
                axum::serve(gateway_listener, gateway_app).await.unwrap();
            }),
        ];
        Self {
            state,
            runtime,
            client: reqwest::Client::new(),
            tasks,
            root,
        }
    }
    fn provider(&self) -> Provider {
        let mut p = self
            .state
            .store
            .lock()
            .unwrap()
            .providers()
            .unwrap()
            .remove(0);
        p.name = "Offline fixture".into();
        p.endpoint = "http://127.0.0.1:1".into();
        p.protocol = "messages".into();
        p.model = "fixture-model".into();
        p.models = vec![p.model.clone()];
        p.disabled_models.clear();
        p.configured = true;
        p.credential_ref = None;
        p.model_catalog = None;
        p
    }
    async fn health_samples(&self, name: &str) {
        let mut samples = Vec::new();
        for _ in 0..40 {
            let started = Instant::now();
            let response = tokio::time::timeout(
                Duration::from_millis(250),
                self.client
                    .get(format!("http://{}/api/runtime", self.state.host))
                    .bearer_auth(self.state.token.as_str())
                    .send(),
            )
            .await
            .unwrap()
            .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(
                response.json::<serde_json::Value>().await.unwrap()["version"],
                "fixture"
            );
            samples.push(started.elapsed().as_secs_f64() * 1000.0);
        }
        samples.sort_by(f64::total_cmp);
        eprintln!(
            "RESPONSIVENESS {}",
            json!({"case":name,"samples":samples.len(),"p95Ms":samples[37],"p99Ms":samples[39],"maxMs":samples[39],"targetMs":250,"runtimeThreads":1})
        );
    }
    async fn writer(&self) -> Gate {
        let path = self.root.path().join("data/local-agent.sqlite3");
        let (gate, entered) = hold(move |ready, wait| {
            let db = rusqlite::Connection::open(path).unwrap();
            db.execute_batch("BEGIN IMMEDIATE").unwrap();
            ready.send(()).unwrap();
            wait.recv_timeout(Duration::from_secs(4)).unwrap();
            db.execute_batch("ROLLBACK").unwrap();
        });
        entered.await.unwrap();
        gate
    }
    async fn wait_for_store_worker(&self) {
        tokio::time::timeout(Duration::from_millis(250), async {
            loop {
                if matches!(
                    self.state.store.try_lock(),
                    Err(std::sync::TryLockError::WouldBlock)
                ) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .unwrap();
    }
}

#[tokio::test(flavor = "current_thread")]
async fn dynamic_authentication_waits_off_runtime_and_does_not_take_the_configuration_lock() {
    let vault = Arc::new(SlowVault::default());
    let fixture = Fixture::new(vault.clone()).await;
    let reference = uuid::Uuid::new_v4().to_string();
    vault.set(&reference, b"local-fixture-key").unwrap();
    let provider = fixture.provider();
    let binding = fixture
        .state
        .store
        .lock()
        .unwrap()
        .dynamic_create("claude", "fixture", &reference, &provider)
        .unwrap();
    let manager = fixture.state.providers.clone();
    let (configuration_gate, entered) = hold(move |ready, wait| {
        let _manager = manager.lock().unwrap();
        ready.send(()).unwrap();
        wait.recv_timeout(Duration::from_secs(4)).unwrap();
    });
    entered.await.unwrap();
    let (vault_gate, entered) = vault.pause_next();
    let url = format!(
        "http://{}/gateway/{}/v1/models",
        fixture.state.gateway.host, binding.id
    );
    let client = fixture.client.clone();
    let request_url = url.clone();
    let request = tokio::spawn(async move {
        client
            .get(request_url)
            .header("x-api-key", "local-fixture-key")
            .send()
            .await
            .unwrap()
    });
    tokio::time::timeout(Duration::from_millis(250), entered)
        .await
        .unwrap()
        .unwrap();
    assert!(!request.is_finished());
    fixture
        .health_samples("slow_vault_with_configuration_lock")
        .await;
    drop(vault_gate);
    assert_eq!(request.await.unwrap().status(), StatusCode::OK);
    vault.delete(&reference).unwrap();
    assert_eq!(
        fixture
            .client
            .get(&url)
            .header("x-api-key", "local-fixture-key")
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert!(vault.reads.load(Ordering::SeqCst) >= 2);
    let response = fixture
        .client
        .post(format!(
            "http://{}/api/runtime/shutdown",
            fixture.state.host
        ))
        .bearer_auth(fixture.state.token.as_str())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    tokio::time::timeout(Duration::from_millis(250), fixture.runtime.stopped())
        .await
        .unwrap();
    drop(configuration_gate);
}

#[tokio::test(flavor = "current_thread")]
async fn cancelled_dispatch_retains_turn_guard_until_its_unfinished_write_returns() {
    let fixture = Fixture::new(Arc::new(MemoryVault::default())).await;
    let provider = fixture.provider();
    let turn = Turn {
        session: Some("fixture-session".into()),
        digest: "fixture-turn".into(),
        chain: vec!["fixture-turn".into()],
        continuation: false,
        portable: true,
    };
    let (binding, attachment, selected) = {
        let store = fixture.state.store.lock().unwrap();
        store.save_provider(provider.clone()).unwrap();
        let model = uuid::Uuid::new_v4().to_string();
        let service = store.save_model_service(serde_json::from_value(json!({
                "id":uuid::Uuid::new_v4().to_string(), "revision":0,"name":"Fixture service", "defaultModelId":model,
            "models":[{"id":model,"name":"Fixture model","providerId":provider.id,"model":provider.model,"capabilities":{"contextWindow":96000,"maxOutputTokens":4096,"tools":true,"thinking":false,"inputModalities":["text"]}}],
            "routingPolicy":{"enabled":true}
        })).unwrap()).unwrap();
        let binding = store
            .dynamic_create(
                "claude",
                "fixture",
                &uuid::Uuid::new_v4().to_string(),
                &service.models[0].provider,
            )
            .unwrap();
        let attachment = store.attach_model_service(&binding.id, &service).unwrap();
        let selected = store
            .model_service_admit(&binding.id, &turn, local_agent_core::routing::ALIAS)
            .unwrap();
        (binding, attachment, selected)
    };
    let mut request = Request::new(Body::empty());
    request
        .extensions_mut()
        .insert(crate::gateway::dynamic::Context {
            binding: binding.clone(),
            service: Some(attachment),
        });
    let (parts, _) = request.into_parts();
    let forward = crate::observation::proxy::Forward {
        provider,
        agent: "claude".into(),
        agent_product_id: Some("claude-code-cli".into()),
        revision: 1,
        client_protocol: "messages".into(),
        credential: None,
        managed: true,
        models: vec![],
    };
    let locked_store = fixture.state.store.clone();
    let (writer, entered) = hold(move |ready, wait| {
        let _store = locked_store.lock().unwrap();
        ready.send(()).unwrap();
        wait.recv_timeout(Duration::from_secs(4)).unwrap();
    });
    entered.await.unwrap();
    let state = fixture.state.clone();
    let candidate = selected.clone();
    let dispatch_turn = turn.clone();
    let turn_guard = state
        .observation
        .policy
        .reserve_turn(&binding.id, &turn)
        .unwrap();
    let task = tokio::spawn(async move {
        let (mut journal, attempt) = trace::Journal::new(
            state.observation.clone(),
            local_agent_core::observation::ModelObservation::default(),
        );
        let mut parts = parts;
        parts.extensions.insert(attempt);
        dispatch(
            state,
            forward,
            parts,
            Bytes::from_static(b"{}"),
            candidate,
            DispatchInputs {
                turn: dispatch_turn,
                turn_guard,
                facts: RuleFacts::default(),
                journal: &mut journal,
            },
        )
        .await
    });
    tokio::time::sleep(Duration::from_millis(5)).await;
    fixture.health_samples("store_mutex_locked_dispatch").await;
    assert!(!task.is_finished());
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert!(
        fixture
            .state
            .observation
            .policy
            .reserve_turn(&binding.id, &turn)
            .is_err()
    );
    drop(writer);
    let path = fixture.root.path().join("data/local-agent.sqlite3");
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let path = path.clone();
            let pending = crate::blocking::run(move || {
                rusqlite::Connection::open(path)
                    .unwrap()
                    .query_row(
                        "SELECT count(*) FROM service_route_dispatch WHERE state='pending'",
                        [],
                        |row| row.get::<_, i64>(0),
                    )
                    .unwrap()
            })
            .await
            .unwrap();
            if pending == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .unwrap();
    let _retry_guard = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if let Ok(guard) = fixture
                .state
                .observation
                .policy
                .reserve_turn(&binding.id, &turn)
            {
                break guard;
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .unwrap();
    let store = fixture.state.store.clone();
    tokio::time::timeout(
        Duration::from_secs(1),
        crate::blocking::store(store, move |store| {
            // After the worker finishes, explicit retry can resume the saved plan.
            let retry = store
                .service_dispatch(
                    &binding.id,
                    &turn,
                    &selected,
                    |_| Default::default(),
                    |_, _, _| Default::default(),
                )
                .unwrap()
                .unwrap();
            assert_eq!(retry.candidates.len(), 1);
            Ok(())
        }),
    )
    .await
    .unwrap()
    .unwrap();
}

#[tokio::test(flavor = "current_thread")]
async fn interrupted_stream_cooldown_waits_off_runtime_and_preserves_the_body_error() {
    let fixture = Fixture::new(Arc::new(MemoryVault::default())).await;
    let writer = fixture.writer().await;
    let response = Response::new(Body::from_stream(futures_util::stream::once(async {
        Err::<Bytes, _>(std::io::Error::other("fixture interrupted stream"))
    })));
    let response = monitor(
        response,
        fixture.state.clone(),
        "messages",
        "fixture-model-scope".into(),
        30,
        None,
        None,
    );
    let reader = tokio::spawn(axum::body::to_bytes(response.into_body(), 1024));
    fixture.wait_for_store_worker().await;
    fixture
        .health_samples("sqlite_locked_stream_cooldown")
        .await;
    assert!(!reader.is_finished());
    drop(writer);
    assert!(reader.await.unwrap().is_err());
    let saved = crate::blocking::store(fixture.state.store.clone(), |store| {
        store.service_cooldown("fixture-model-scope")
    })
    .await
    .unwrap()
    .unwrap();
    assert_eq!(saved.1, "stream_interrupted_after_commit");
}

#[tokio::test]
async fn committed_stream_cooldown_requires_an_upstream_failure_or_missing_terminal() {
    let fixture = Fixture::new(Arc::new(MemoryVault::default())).await;
    let messages = "data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"partial\"}}\n\n";
    let responses = "data: {\"type\":\"response.output_text.delta\",\"delta\":\"partial\"}\n\n";
    let cases = [
        ("messages-eof", "messages", messages, "", true),
        ("responses-eof", "responses", responses, "", true),
        (
            "messages-complete",
            "messages",
            messages,
            "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"max_tokens\"}}\n\ndata: {\"type\":\"message_stop\"}",
            false,
        ),
        (
            "responses-complete",
            "responses",
            responses,
            "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\"}}",
            false,
        ),
        (
            "responses-error",
            "responses",
            responses,
            "data: {\"type\":\"response.failed\",\"response\":{\"status\":\"failed\"}}\n\n",
            true,
        ),
    ];
    for (scope, protocol, prefix, suffix, expected) in cases {
        let body = [prefix, suffix].concat();
        let response = ([("content-type", "text/event-stream")], body.clone()).into_response();
        let response = monitor(
            response,
            fixture.state.clone(),
            protocol,
            scope.into(),
            30,
            None,
            None,
        );
        let bytes = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        assert_eq!(bytes, body.as_bytes());
        let saved = fixture
            .state
            .store
            .lock()
            .unwrap()
            .service_cooldown(scope)
            .unwrap();
        assert_eq!(
            saved.as_ref().map(|(_, reason)| reason.as_str()),
            expected.then_some("stream_interrupted_after_commit"),
            "{scope}"
        );
    }

    let prefix = futures_util::stream::once(async {
        Ok::<_, std::io::Error>(Bytes::from_static(messages.as_bytes()))
    });
    let upstream = Body::from_stream(prefix.chain(futures_util::stream::pending()));
    let response = ([("content-type", "text/event-stream")], upstream).into_response();
    let response = monitor(
        response,
        fixture.state.clone(),
        "messages",
        "client-cancelled".into(),
        30,
        None,
        None,
    );
    let mut body = response.into_body().into_data_stream();
    assert_eq!(body.next().await.unwrap().unwrap(), messages.as_bytes());
    drop(body);
    assert!(
        fixture
            .state
            .store
            .lock()
            .unwrap()
            .service_cooldown("client-cancelled")
            .unwrap()
            .is_none()
    );
}
