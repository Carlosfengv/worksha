use super::*;
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
    gate: Mutex<Option<(tokio::sync::oneshot::Sender<()>, mpsc::Receiver<()>)>>,
    reads: AtomicUsize,
}
impl SlowVault {
    fn pause_next(&self) -> (Gate, tokio::sync::oneshot::Receiver<()>) {
        let (ready, entered) = tokio::sync::oneshot::channel();
        let (release, wait) = mpsc::channel();
        *self.gate.lock().unwrap() = Some((ready, wait));
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
        let gate = self.gate.lock().unwrap().take();
        if let Some((ready, wait)) = gate {
            let _ = ready.send(());
            wait.recv_timeout(Duration::from_secs(4)).unwrap();
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
            "routingPolicy":{"enabled":true}
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
    let response: serde_json::Value = fixture.client.post(format!("{desktop}/messages"))
        .bearer_auth("model-service-fixture-key")
        .header("session_id", uuid::Uuid::new_v4().to_string())
        .json(&json!({"model":desktop_default,"max_tokens":30,"messages":[{"role":"user","content":"offline fixture"}]}))
        .send().await.unwrap().error_for_status().unwrap().json().await.unwrap();
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
    assert_ne!(page["items"][0]["title"], "Authorized fixture conversation");
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
        let state = AppState {
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
            observation: Arc::new(crate::observation::Observation::new(&data, &host).unwrap()),
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
async fn cancelled_dispatch_keeps_its_durable_pending_owner_while_the_store_is_locked() {
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
    let task = tokio::spawn(dispatch(
        fixture.state.clone(),
        forward,
        parts,
        Bytes::from_static(b"{}"),
        selected.clone(),
        turn.clone(),
    ));
    tokio::time::sleep(Duration::from_millis(5)).await;
    fixture.health_samples("store_mutex_locked_dispatch").await;
    assert!(!task.is_finished());
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
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
    let store = fixture.state.store.clone();
    tokio::time::timeout(
        Duration::from_secs(1),
        crate::blocking::store(store, move |store| {
            // The cancelled caller cannot cancel an already admitted SQLite write.
            // Its persisted pending owner prevents a second upstream attempt.
            let error = store
                .service_dispatch(&binding.id, &turn, &selected)
                .unwrap_err();
            assert_eq!(error.code, "dispatch_uncertain");
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
        "fixture-model-scope".into(),
        30,
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
