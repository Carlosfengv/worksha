use super::*;
use axum::{Json, extract::OriginalUri, http::HeaderMap, serve::Listener};
use local_agent_core::providers::upstream_accounts::AccountEdit;
use std::net::SocketAddr;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Notify;
use tokio_rustls::{
    TlsAcceptor,
    rustls::{ServerConfig, pki_types::PrivatePkcs8KeyDer},
    server::TlsStream,
};

pub(super) struct LoopbackTls(TcpListener, TlsAcceptor);
impl Listener for LoopbackTls {
    type Io = TlsStream<TcpStream>;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        loop {
            let (stream, addr) = self.0.accept().await.unwrap();
            if let Ok(Ok(tls)) =
                tokio::time::timeout(Duration::from_secs(2), self.1.accept(stream)).await
            {
                return (tls, addr);
            }
        }
    }

    fn local_addr(&self) -> std::io::Result<Self::Addr> {
        self.0.local_addr()
    }
}

pub(super) async fn tls_upstream(host: &str, certificate: &[u8]) -> (LoopbackTls, reqwest::Client) {
    tls_upstream_hosts(&[host], certificate).await
}

pub(super) async fn tls_upstream_hosts(
    hosts: &[&str],
    certificate: &[u8],
) -> (LoopbackTls, reqwest::Client) {
    let key = include_bytes!("fixtures/minimax-fixture-key.pk8");
    let tls = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(
            vec![certificate.to_vec().into()],
            PrivatePkcs8KeyDer::from(key.as_slice()).into(),
        )
        .unwrap();
    let listener = LoopbackTls(
        TcpListener::bind("127.0.0.1:0").await.unwrap(),
        TlsAcceptor::from(Arc::new(tls)),
    );
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .tls_built_in_root_certs(false)
        .add_root_certificate(reqwest::Certificate::from_der(certificate).unwrap());
    let address = listener.local_addr().unwrap();
    let client = hosts
        .iter()
        .fold(client, |builder, host| builder.resolve(host, address))
        .timeout(Duration::from_secs(6))
        .build()
        .unwrap();
    (listener, client)
}

#[tokio::test]
async fn subscription_allowance_and_recovery_probe_are_shared_across_all_wires() {
    use local_agent_core::{
        billing::MINIMAX_PLAN_WINDOWS,
        model_services::policy::{QuotaProbeSource, QuotaProbeState},
    };

    let (listener, client) = tls_upstream(
        "api.minimax.io",
        include_bytes!("fixtures/minimax-fixture-cert.der"),
    )
    .await;
    let vault = Arc::new(MemoryVault::default());
    let mut fixture = Fixture::with_upstream(vault.clone(), Some(client)).await;
    let wires = ["messages", "chat-completions", "responses"];
    let seen = Arc::new(Mutex::new(Vec::new()));
    let probe = Arc::new(Mutex::new(None::<(Arc<Notify>, Arc<Notify>, bool)>));
    let infer = {
        let (seen, probe) = (seen.clone(), probe.clone());
        move |OriginalUri(uri): OriginalUri,
              headers: HeaderMap,
              Json(body): Json<serde_json::Value>| {
            let (seen, probe) = (seen.clone(), probe.clone());
            async move {
                let primary = match headers["authorization"].to_str().unwrap() {
                    "Bearer sk-cp-shared-plan-fixture" => true,
                    "Bearer sk-cp-independent-plan-fixture" => false,
                    _ => panic!("unexpected synthetic credential"),
                };
                assert!(headers.get("x-api-key").is_none());
                let model = body["model"].as_str().unwrap();
                assert_eq!(model, "MiniMax-M3.1-Flash");
                let (wire, effort, reply) = match uri.path() {
                    "/anthropic/v1/messages" => (
                        "messages",
                        &body["output_config"]["effort"],
                        json!({
                            "id":"msg_fixture","type":"message","role":"assistant","model":model,
                            "content":[{"type":"text","text":"Quota fixture OK"}],"stop_reason":"end_turn",
                            "usage":{"input_tokens":5,"output_tokens":4}
                        }),
                    ),
                    "/v1/chat/completions" => (
                        "chat-completions",
                        &body["reasoning_effort"],
                        json!({
                            "id":"chatcmpl_fixture","object":"chat.completion","model":model,
                            "choices":[{"index":0,"message":{"role":"assistant","content":"Quota fixture OK"},"finish_reason":"stop"}],
                            "usage":{"prompt_tokens":5,"completion_tokens":4,"total_tokens":9}
                        }),
                    ),
                    "/v1/responses" => (
                        "responses",
                        &body["reasoning"]["effort"],
                        json!({
                            "id":"resp_fixture","object":"response","model":model,"status":"completed",
                            "output":[{"id":"msg_fixture","type":"message","status":"completed","role":"assistant",
                            "content":[{"type":"output_text","text":"Quota fixture OK","annotations":[]}]}],
                            "usage":{"input_tokens":5,"output_tokens":4,"total_tokens":9}
                        }),
                    ),
                    _ => panic!("unexpected fixture endpoint"),
                };
                assert_eq!(effort, "low");
                seen.lock().unwrap().push((wire, primary));
                let gate = primary.then(|| probe.lock().unwrap().take()).flatten();
                if let Some((entered, release, fail)) = gate {
                    entered.notify_one();
                    tokio::time::timeout(Duration::from_secs(4), release.notified())
                        .await
                        .unwrap();
                    if fail {
                        return (StatusCode::SERVICE_UNAVAILABLE, Json(json!({
                            "error":{"type":"server_error","message":"Synthetic recovery failure"}
                        }))).into_response();
                    }
                }
                Json(reply).into_response()
            }
        }
    };
    let app = axum::Router::new()
        .route("/anthropic/v1/messages", axum::routing::post(infer.clone()))
        .route("/v1/chat/completions", axum::routing::post(infer.clone()))
        .route("/v1/responses", axum::routing::post(infer));
    fixture.tasks.push(tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    }));

    let credential = |secret: &[u8]| {
        let reference = uuid::Uuid::new_v4().to_string();
        vault.set(&reference, secret).unwrap();
        reference
    };
    let primary = credential(b"sk-cp-shared-plan-fixture");
    let independent = credential(b"sk-cp-independent-plan-fixture");
    let local = credential(b"local-plan-quota-wire-fixture");
    let template = fixture.provider();
    let (routes, scope) = {
        let store = fixture.state.store.lock().unwrap();
        let mut routes = Vec::new();
        let mut scope = None;
        for (wire, key) in wires
            .iter()
            .map(|wire| (*wire, &primary))
            .chain([("messages", &independent)])
        {
            let mut provider = template.clone();
            provider.id = format!("custom-{}", uuid::Uuid::new_v4());
            provider.vendor = "minimax-plan".into();
            provider.endpoint = if wire == "messages" {
                "https://api.minimax.io/anthropic"
            } else {
                "https://api.minimax.io/v1"
            }
            .into();
            provider.protocol = wire.into();
            provider.model = "MiniMax-M3.1-Flash".into();
            provider.models = vec![provider.model.clone()];
            provider.credential_ref = Some(key.clone());
            provider.revision = 0;
            store.save_provider(provider.clone()).unwrap();
            let model_id = uuid::Uuid::new_v4().to_string();
            let service = store.save_model_service(serde_json::from_value(json!({
                "id":uuid::Uuid::new_v4().to_string(),"revision":0,"name":"Quota wire fixture","defaultModelId":model_id,
                "models":[{"id":model_id,"name":"Flash","providerId":provider.id,"model":provider.model,
                    "capabilities":{"contextWindow":96000,"maxOutputTokens":4096,"tools":true,"thinking":false,"inputModalities":["text"]}}]
            })).unwrap()).unwrap();
            let binding = store
                .create_model_service_binding("opencode", "quota wire fixture", &local, &service)
                .unwrap()
                .0;
            routes.push(format!(
                "{}/messages",
                crate::gateway::dynamic::configured_endpoint(&fixture.state, &binding)
            ));
            if scope.is_none() {
                scope = Some(scopes(&provider).0);
            }
        }
        (routes, scope.unwrap())
    };
    let call = |index: usize| {
        fixture
            .client
            .post(&routes[index])
            .timeout(Duration::from_secs(5))
            .bearer_auth("local-plan-quota-wire-fixture")
            .header(
                "x-opencode-session",
                format!("ses_{}", uuid::Uuid::new_v4().simple()),
            )
            .json(
                &json!({"model":local_agent_core::routing::ALIAS,"max_tokens":256,
                "messages":[{"role":"user","content":"Quota fixture request"}]}),
            )
    };
    let expect_ok = async |response: reqwest::Response| {
        let answer: serde_json::Value = response.error_for_status().unwrap().json().await.unwrap();
        assert_eq!(answer["content"][0]["text"], "Quota fixture OK");
    };
    for (probe_index, fail) in [(0, false), (1, false), (2, false), (2, true)] {
        let wire = wires[probe_index];
        seen.lock().unwrap().clear();
        let observed = chrono::Utc::now();
        let reset = observed.timestamp() + 2;
        let snapshot = local_agent_core::billing::api_at("minimax-plan", &json!({
            "base_resp":{"status_code":0},"model_remains":[{"model_name":"general",
                "current_interval_status":2,"end_time":reset,"current_weekly_status":2,"weekly_end_time":reset}]
        }), observed).unwrap();
        fixture
            .state
            .store
            .lock()
            .unwrap()
            .record_minimax_allowance(&scope, &snapshot.windows, observed.fixed_offset())
            .unwrap();
        for index in 0..wires.len() {
            let blocked = call(index).send().await.unwrap();
            assert_eq!(blocked.status(), StatusCode::TOO_MANY_REQUESTS);
            assert!(blocked.text().await.unwrap().contains("额度"));
        }
        assert!(
            seen.lock().unwrap().is_empty(),
            "depleted aliases must not send"
        );
        let wait = (reset * 1000 - chrono::Utc::now().timestamp_millis()).max(0) as u64;
        tokio::time::sleep(Duration::from_millis(wait + 20)).await;

        let entered = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        *probe.lock().unwrap() = Some((entered.clone(), release.clone(), fail));
        let pending = tokio::spawn(call(probe_index).send());
        tokio::time::timeout(Duration::from_secs(2), entered.notified())
            .await
            .unwrap();
        for index in (0..wires.len()).filter(|index| *index != probe_index) {
            let waiting = call(index).send().await.unwrap();
            assert_eq!(waiting.status(), StatusCode::TOO_MANY_REQUESTS);
            assert!(waiting.text().await.unwrap().contains("正在探测恢复"));
        }
        assert_eq!(*seen.lock().unwrap(), [(wire, true)]);
        expect_ok(call(wires.len()).send().await.unwrap()).await;
        assert_eq!(*seen.lock().unwrap(), [(wire, true), ("messages", false)]);
        release.notify_one();
        let response = pending.await.unwrap().unwrap();
        if fail {
            assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
            let _ = response.bytes().await.unwrap();
            for index in 0..wires.len() {
                let cooling = call(index).send().await.unwrap();
                assert_eq!(cooling.status(), StatusCode::TOO_MANY_REQUESTS);
                assert!(cooling.text().await.unwrap().contains("正在冷却"));
            }
            assert_eq!(*seen.lock().unwrap(), [(wire, true), ("messages", false)]);
            let windows = MINIMAX_PLAN_WINDOWS.map(|window| format!("{scope}:{window}"));
            let guard = format!("minimax-probe:{scope}");
            let states = fixture
                .state
                .store
                .lock()
                .unwrap()
                .service_quota_probes(
                    QuotaProbeSource::MiniMax,
                    windows.iter().map(String::as_str).chain([guard.as_str()]),
                    chrono::Utc::now().timestamp_millis(),
                )
                .unwrap();
            assert_eq!(
                states,
                [Some(QuotaProbeState::Cooling); 3],
                "both quota windows and the shared guard should cool after the failed probe"
            );
            continue;
        }
        expect_ok(response).await;
        for index in 0..wires.len() {
            expect_ok(call(index).send().await.unwrap()).await;
        }
        assert_eq!(
            &seen.lock().unwrap()[2..],
            &[
                ("messages", true),
                ("chat-completions", true),
                ("responses", true)
            ]
        );
    }
}

#[tokio::test]
async fn subscription_keys_route_over_official_wire_and_keep_tool_round_account() {
    let (listener, client) = tls_upstream(
        "api.minimax.io",
        include_bytes!("fixtures/minimax-fixture-cert.der"),
    )
    .await;
    let vault = Arc::new(MemoryVault::default());
    let mut fixture = Fixture::with_upstream(vault.clone(), Some(client.clone())).await;
    let seen = Arc::new(Mutex::new(vec![]));
    let probe = Arc::new(Mutex::new(None::<Arc<Notify>>));
    let router = axum::Router::new().route("/anthropic/v1/messages", axum::routing::post({
        let seen = seen.clone();
        let probe = probe.clone();
        move |headers: HeaderMap, Json(body): Json<serde_json::Value>| {
            let seen = seen.clone();
            let probe = probe.clone();
            async move {
                assert!(headers.get("x-api-key").is_none());
                let key = headers["authorization"].to_str().unwrap();
                let name = match key {
                    "Bearer sk-cp-fixture-a" => "a",
                    "Bearer sk-cp-fixture-b" => "b",
                    _ => panic!("unexpected fixture key"),
                };
                seen.lock().unwrap().push(name);
                assert!(body.get("thinking").is_none(), "Flash rejects explicit disabled thinking");
                assert_eq!(body["output_config"]["effort"], "low");
                assert_eq!(body["model"], "MiniMax-M3.1-Flash");
                if body["stream"] == true {
                    assert_eq!(body["max_tokens"], 1024);
                    let gate = if name == "a" { probe.lock().unwrap().take() } else { None };
                    if let Some(release) = gate {
                        let prefix = [
                            json!({"type":"message_start","message":{"id":"msg_plan_probe","type":"message","role":"assistant","model":body["model"],"content":[],"usage":{"input_tokens":10}}}),
                            json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
                            json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"probe fixture output"}}),
                        ].map(|event| format!("data: {event}\n\n")).join("");
                        let tail = [
                            json!({"type":"content_block_stop","index":0}),
                            json!({"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":5}}),
                            json!({"type":"message_stop"}),
                        ].map(|event| format!("data: {event}\n\n")).join("");
                        let stream = async_stream::stream! {
                            yield Ok::<_, std::convert::Infallible>(prefix);
                            release.notified().await;
                            yield Ok(tail);
                        };
                        return ([("content-type", "text/event-stream")], axum::body::Body::from_stream(stream)).into_response();
                    }
                    if name == "a" {
                        return (StatusCode::TOO_MANY_REQUESTS, [("retry-after", "60")], Json(json!({"type":"error","error":{"type":"rate_limit_error","message":"Fixture plan rate limit"}}))).into_response();
                    }
                    let frames = [
                        json!({"type":"message_start","message":{"id":"msg_plan_tool","type":"message","role":"assistant","model":body["model"],"content":[],"usage":{"input_tokens":10}}}),
                        json!({"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}),
                        json!({"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"internal-fixture"}}),
                        json!({"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"private-fixture"}}),
                        json!({"type":"content_block_stop","index":0}),
                        json!({"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"tool_plan","name":"inspect","input":{}}}),
                        json!({"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{}"}}),
                        json!({"type":"content_block_stop","index":1}),
                        json!({"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":19}}),
                        json!({"type":"message_stop"}),
                    ].map(|event| format!("data: {event}\n\n")).join("");
                    return ([("content-type", "text/event-stream")], frames).into_response();
                }
                Json(json!({"id":"msg_plan_fixture","type":"message","role":"assistant","model":body["model"],"content":[{"type":"thinking","thinking":"internal-fixture","signature":"private-fixture"},{"type":"text","text":"OK"}],"stop_reason":"end_turn","usage":{"input_tokens":10,"output_tokens":16}})).into_response()
            }
        }
    }));
    fixture.tasks.push(tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap()
    }));

    let mut provider = fixture.provider();
    provider.id = "minimax-plan".into();
    provider.vendor = "minimax-plan".into();
    provider.endpoint = "https://api.minimax.io/anthropic".into();
    provider.model = "MiniMax-M3.1-Flash".into();
    provider.models = vec![provider.model.clone()];
    let rejected = crate::providers::probe_detailed(
        &client,
        &provider,
        Some(Zeroizing::new(b"sk-payg-fixture".to_vec())),
    )
    .await
    .unwrap_err();
    assert_eq!(rejected.code, "invalid_credential");
    assert!(seen.lock().unwrap().is_empty());
    let primary_key = uuid::Uuid::new_v4().to_string();
    let secondary_key = uuid::Uuid::new_v4().to_string();
    let local_key = uuid::Uuid::new_v4().to_string();
    vault.set(&primary_key, b"sk-cp-fixture-a").unwrap();
    vault.set(&secondary_key, b"sk-cp-fixture-b").unwrap();
    vault.set(&local_key, b"local-plan-fixture").unwrap();
    provider.credential_ref = Some(primary_key.clone());
    let (binding, default_binding, targets, pool, draft, store_id) = {
        let store = fixture.state.store.lock().unwrap();
        store.save_provider(provider.clone()).unwrap();
        let provider = store.provider_by_id(&provider.id).unwrap().unwrap();
        let pool = [primary_key, secondary_key]
            .into_iter()
            .enumerate()
            .map(|(index, credential_ref)| {
                store
                    .save_upstream_account(
                        &provider,
                        &AccountEdit {
                            id: uuid::Uuid::new_v4().to_string(),
                            expected_revision: 0,
                            provider_revision: provider.revision,
                            label: format!("Plan account {}", index + 1),
                            credential_ref,
                        },
                    )
                    .unwrap()
                    .reference()
            })
            .collect::<Vec<_>>();
        let targets = pool
            .iter()
            .map(|account| store.upstream_account_target(&provider, account).unwrap().0)
            .collect::<Vec<_>>();
        let model = uuid::Uuid::new_v4().to_string();
        let mut draft = json!({
            "id":uuid::Uuid::new_v4().to_string(), "revision":0, "name":"Plan fixture", "defaultModelId":model,
            "models":[{"id":model,"name":"Flash","providerId":provider.id,"model":provider.model,"capabilities":{"contextWindow":96000,"maxOutputTokens":4096,"tools":true,"thinking":false,"inputModalities":["text"]}}],
            "routingPolicy":{"enabled":true,"accountPools":[{"modelId":model,"accounts":pool,"strategy":"quota-pace"}]}
        });
        let service = store
            .save_model_service(serde_json::from_value(draft.clone()).unwrap())
            .unwrap();
        draft["revision"] = json!(service.revision);
        let binding = store
            .create_model_service_binding("opencode", "fixture", &local_key, &service)
            .unwrap()
            .0;
        let mut default_draft = draft.clone();
        default_draft["id"] = json!(uuid::Uuid::new_v4().to_string());
        default_draft["revision"] = json!(0);
        default_draft["routingPolicy"] = json!({"enabled":false});
        let default_service = store
            .save_model_service(serde_json::from_value(default_draft).unwrap())
            .unwrap();
        let default_binding = store
            .create_model_service_binding(
                "opencode",
                "default fixture",
                &local_key,
                &default_service,
            )
            .unwrap()
            .0;
        (
            binding,
            default_binding,
            targets,
            pool,
            draft,
            store.database_id().unwrap(),
        )
    };
    for target in &targets {
        let secret = vault
            .get(target.credential_ref.as_deref().unwrap())
            .unwrap();
        crate::providers::probe_detailed(&client, target, Some(secret))
            .await
            .unwrap();
    }
    assert_eq!(*seen.lock().unwrap(), ["a", "b"]);
    let endpoint = format!(
        "{}/messages",
        crate::gateway::dynamic::configured_endpoint(&fixture.state, &binding)
    );
    let default_endpoint = format!(
        "{}/messages",
        crate::gateway::dynamic::configured_endpoint(&fixture.state, &default_binding)
    );
    let default_call = |session, stream| {
        fixture
            .client
            .post(&default_endpoint)
            .timeout(Duration::from_secs(5))
            .bearer_auth("local-plan-fixture")
            .header("x-opencode-session", session)
            .json(&json!({
                "model":local_agent_core::routing::ALIAS, "max_tokens":1024, "stream":stream,
                "messages":[{"role":"user","content":"Default account quota fixture"}]
            }))
    };
    let call =
        |session, stream, history| {
            fixture.client.post(&endpoint).bearer_auth("local-plan-fixture")
        .header("x-opencode-session", session).json(&json!({
            "model":local_agent_core::routing::ALIAS, "max_tokens":1024, "stream":stream,
            "messages":history, "tools":[{"name":"inspect","input_schema":{"type":"object"}}]
        }))
        };
    let now = chrono::Utc::now();
    let snapshot = |interval, weekly| {
        json!({
            "base_resp":{"status_code":0},
            "model_remains":[{
                "model_name":"general",
                "current_interval_remaining_percent":interval,
                "start_time":now.timestamp()-3600, "end_time":now.timestamp()+4*3600,
                "current_weekly_remaining_percent":weekly,
                "weekly_start_time":now.timestamp()-160*3600, "weekly_end_time":now.timestamp()+8*3600
            }]
        })
    };
    let seed = |index: usize, value| {
        let now = chrono::Utc::now();
        let snapshot = local_agent_core::billing::api_at("minimax-plan", &value, now).unwrap();
        fixture
            .state
            .store
            .lock()
            .unwrap()
            .record_minimax_allowance(
                &scopes(&targets[index]).0,
                &snapshot.windows,
                now.fixed_offset(),
            )
            .unwrap();
        fixture.state.billing.lock().unwrap().seed_upstream_quota(
            "minimax-plan",
            &targets[index],
            &pool[index],
            now,
            &value,
        );
    };
    let assert_route = async |session: &'static str, account: &str, reason: &str| {
        let before = seen.lock().unwrap().len();
        call(
            session,
            false,
            json!([{"role":"user","content":"Quota fixture"}]),
        )
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .text()
        .await
        .unwrap();
        assert_eq!(&seen.lock().unwrap()[before..], &[account]);
        let detail = fixture
            .detail_when(session, |detail| {
                detail["request"]["attempts"][0]["routing"]["service"].is_object()
            })
            .await;
        assert_eq!(
            detail["request"]["attempts"][0]["routing"]["service"]["accountSelectionReason"],
            reason
        );
    };
    seed(0, snapshot(json!(5), serde_json::Value::Null));
    seed(1, snapshot(json!(50), json!(30)));
    assert_route("ses_planPartialQuota", "a", "quota_unknown").await;
    let preview = fixture
        .client
        .post(format!(
            "http://{}/api/model-services/preview",
            fixture.state.host
        ))
        .bearer_auth(fixture.state.token.as_str())
        .json(&json!({"storeId":store_id,"draft":draft,"entry":{"kind":"default"}}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json::<serde_json::Value>()
        .await
        .unwrap();
    assert_eq!(preview["models"][0]["accounts"][0]["quota"], "unknown");
    assert_eq!(preview["models"][0]["accounts"][1]["quota"], "reported");
    let mut expired = snapshot(json!(5), json!(5));
    expired["model_remains"][0]["end_time"] = json!(now.timestamp() - 1);
    seed(0, expired);
    assert_route("ses_planExpiredQuota", "a", "quota_unknown").await;
    let mut without_timing = snapshot(json!(5), json!(5));
    without_timing["model_remains"][0]["end_time"] = serde_json::Value::Null;
    seed(0, without_timing);
    assert_route("ses_planQuotaWithoutTiming", "a", "quota_unknown").await;
    seed(0, snapshot(json!(5), json!(5)));
    assert_route("ses_planCompleteQuota", "b", "quota_pace").await;
    seed(0, snapshot(json!(0), serde_json::Value::Null));
    assert_route("ses_planPartialExhaustion", "b", "quota_pace").await;
    seed(0, snapshot(serde_json::Value::Null, json!(90)));
    seed(1, snapshot(json!(50), json!(0)));
    let before = seen.lock().unwrap().len();
    let both_exhausted = call(
        "ses_planBothAccountsExhausted",
        false,
        json!([{"role":"user","content":"Quota fixture"}]),
    )
    .send()
    .await
    .unwrap();
    assert_eq!(both_exhausted.status(), StatusCode::TOO_MANY_REQUESTS);
    let default_exhausted = default_call("ses_planDefaultExhausted", false)
        .send()
        .await
        .unwrap();
    assert_eq!(default_exhausted.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(
        seen.lock().unwrap().len(),
        before,
        "no depleted account was sent a request"
    );
    let preview = fixture
        .client
        .post(format!(
            "http://{}/api/model-services/preview",
            fixture.state.host
        ))
        .bearer_auth(fixture.state.token.as_str())
        .json(&json!({"storeId":store_id,"draft":draft,"entry":{"kind":"default"}}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json::<serde_json::Value>()
        .await
        .unwrap();
    assert_eq!(preview["models"][0]["accounts"][0]["quota"], "exhausted");
    assert_eq!(preview["models"][0]["accounts"][1]["quota"], "exhausted");

    seed(1, snapshot(json!(50), json!(30)));
    let first_reset = chrono::Utc::now().timestamp() + 1;
    let second_reset = first_reset + 2;
    let mut near_reset = snapshot(json!(0), json!(0));
    near_reset["model_remains"][0]["end_time"] = json!(first_reset);
    near_reset["model_remains"][0]["weekly_end_time"] = json!(second_reset);
    seed(0, near_reset);
    let wait_for = async |reset: i64| {
        let delay = reset * 1000 - chrono::Utc::now().timestamp_millis();
        tokio::time::sleep(Duration::from_millis(delay.max(0) as u64)).await;
    };
    wait_for(first_reset).await;
    assert_route("ses_planWeeklyStillExhausted", "b", "quota_pace").await;
    wait_for(second_reset).await;
    let verify_held_probe = async |first: &mut reqwest::Response, session: &'static str| {
        let mut prefix = Vec::new();
        while !String::from_utf8_lossy(&prefix).contains("probe fixture output") {
            prefix.extend_from_slice(&first.chunk().await.unwrap().expect("probe text"));
        }
        // Advance just the persisted hold while the same upstream stream is live.
        // Billing snapshots and the stream's monotonic delivery timer stay intact.
        let db = rusqlite::Connection::open(fixture.root.path().join("data/local-agent.sqlite3"))
            .unwrap();
        assert!(
            db.execute(
                "UPDATE service_minimax_probes SET until_ms=until_ms-360000 WHERE state='active'",
                [],
            )
            .unwrap()
                > 0
        );
        drop(db);
        let before = seen.lock().unwrap().len();
        let waiting = default_call(session, false).send().await.unwrap();
        assert_eq!(waiting.status(), StatusCode::TOO_MANY_REQUESTS);
        assert!(waiting.text().await.unwrap().contains("正在探测恢复"));
        assert_eq!(seen.lock().unwrap().len(), before);
    };
    let release = Arc::new(Notify::new());
    *probe.lock().unwrap() = Some(release.clone());
    let mut first = default_call("ses_planDefaultRecoveryProbe", true)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    verify_held_probe(&mut first, "ses_planFixedDuringLongProbe").await;
    assert_route("ses_planProbeUsesBackup", "b", "quota_pace").await;
    release.notify_one();
    assert!(first.text().await.unwrap().contains("message_stop"));
    let before = seen.lock().unwrap().len();
    default_call("ses_planDefaultRecovered", false)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .text()
        .await
        .unwrap();
    assert_eq!(&seen.lock().unwrap()[before..], &["a"]);

    let next_reset = chrono::Utc::now().timestamp() + 2;
    let mut next_window = snapshot(json!(0), json!(0));
    next_window["model_remains"][0]["end_time"] = json!(next_reset - 1);
    next_window["model_remains"][0]["weekly_end_time"] = json!(next_reset);
    seed(0, next_window);
    seed(1, snapshot(json!(0), json!(0)));
    wait_for(next_reset).await;
    let release = Arc::new(Notify::new());
    *probe.lock().unwrap() = Some(release.clone());
    let mut managed = call(
        "ses_planManagedLongRecovery",
        true,
        json!([{"role":"user","content":"Managed long recovery fixture"}]),
    )
    .timeout(Duration::from_secs(5))
    .send()
    .await
    .unwrap()
    .error_for_status()
    .unwrap();
    verify_held_probe(&mut managed, "ses_planFixedDuringManagedLongProbe").await;
    seed(1, snapshot(json!(50), json!(30)));
    assert_route("ses_planManagedLongProbeBackup", "b", "quota_pace").await;
    release.notify_one();
    assert!(managed.text().await.unwrap().contains("message_stop"));
    let before = seen.lock().unwrap().len();
    default_call("ses_planAfterManagedLongProbe", false)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .text()
        .await
        .unwrap();
    assert_eq!(&seen.lock().unwrap()[before..], &["a"]);

    // Preserve the original rate-limit and tool-continuation scenario.
    seed(0, snapshot(json!(90), json!(90)));
    seen.lock().unwrap().clear();
    let result = call(
        "ses_planfixture",
        true,
        json!([{"role":"user","content":"Check the fixture"}]),
    )
    .send()
    .await
    .unwrap()
    .error_for_status()
    .unwrap()
    .text()
    .await
    .unwrap();
    assert!(result.contains("tool_plan"));
    assert!(result.contains("\"output_tokens\":19"));
    assert!(!result.contains("internal-fixture") && !result.contains("private-fixture"));
    let result: serde_json::Value = call("ses_planfixture", false, json!([
        {"role":"user","content":"Check the fixture"},
        {"role":"assistant","content":[{"type":"tool_use","id":"tool_plan","name":"inspect","input":{}}]},
        {"role":"user","content":[{"type":"tool_result","tool_use_id":"tool_plan","content":"fixture result"}]}
    ])).send().await.unwrap().error_for_status().unwrap().json().await.unwrap();
    assert_eq!(result["content"], json!([{"type":"text","text":"OK"}]));
    assert_eq!(*seen.lock().unwrap(), ["a", "b", "b"]);
}
