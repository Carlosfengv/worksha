use super::*;
use std::sync::atomic::AtomicBool;
use tokio::sync::Notify;

#[tokio::test]
async fn replaced_policy_cannot_send_a_backup_waiting_for_credentials() {
    let vault = Arc::new(SlowVault::default());
    let mut fixture = Fixture::new(vault.clone()).await;
    let upstream = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", upstream.local_addr().unwrap());
    let sends = Arc::new(Mutex::new(Vec::new()));
    let router = axum::Router::new().route("/v1/messages", axum::routing::post({
        let sends = sends.clone();
        move |headers: axum::http::HeaderMap| {
            let sends = sends.clone();
            async move {
                let key = headers["x-api-key"].to_str().unwrap();
                sends.lock().unwrap().push(key.to_owned());
                if key == "primary-fixture" {
                    return (StatusCode::TOO_MANY_REQUESTS, axum::Json(json!({"error":{"type":"rate_limit_error","message":"isolated throttle"}}))).into_response();
                }
                axum::Json(json!({
                    "id":"policy-fence-fixture","type":"message","role":"assistant","model":"fixture-model",
                    "content":[{"type":"text","text":"OK"}],"stop_reason":"end_turn",
                    "usage":{"input_tokens":5,"output_tokens":2}
                })).into_response()
            }
        }
    }));
    fixture.tasks.push(tokio::spawn(async move {
        axum::serve(upstream, router).await.unwrap();
    }));
    let local_ref = uuid::Uuid::new_v4().to_string();
    let primary_ref = uuid::Uuid::new_v4().to_string();
    let backup_ref = uuid::Uuid::new_v4().to_string();
    vault.set(&local_ref, b"isolated-local-key").unwrap();
    vault.set(&primary_ref, b"primary-fixture").unwrap();
    vault.set(&backup_ref, b"backup-fixture").unwrap();
    let mut primary = fixture.provider();
    primary.endpoint = endpoint;
    primary.credential_ref = Some(primary_ref);
    let mut backup = primary.clone();
    backup.id = format!("custom-{}", uuid::Uuid::new_v4());
    backup.credential_ref = Some(backup_ref.clone());
    let (binding, service, store_id) = {
        let store = fixture.state.store.lock().unwrap();
        store.save_provider(primary.clone()).unwrap();
        store.save_provider(backup.clone()).unwrap();
        let primary_id = uuid::Uuid::new_v4().to_string();
        let backup_id = uuid::Uuid::new_v4().to_string();
        let models = [(primary_id.clone(), primary.id), (backup_id, backup.id)]
            .map(|(id, provider)| json!({
                "id":id,"name":id,"providerId":provider,"model":"fixture-model",
                "capabilities":{"contextWindow":32000,"maxOutputTokens":4096,"tools":true,"thinking":false,"inputModalities":["text"]}
            }));
        let service = store.save_model_service(serde_json::from_value(json!({
            "id":uuid::Uuid::new_v4().to_string(), "revision":0,"name":"Policy fence fixture","defaultModelId":primary_id,
            "models":models,
            "routingPolicy":{"enabled":true,"deadlineSeconds":10}
        })).unwrap()).unwrap();
        let binding = store
            .create_model_service_binding("opencode", "policy-fence-fixture", &local_ref, &service)
            .unwrap()
            .0;
        (
            binding,
            service,
            store.model_service_operation_store_id().unwrap(),
        )
    };
    let revision = fixture.state.plugins.bind_unavailable_policy_fixture(
        &store_id,
        &service.id,
        &service.version_id,
    );
    let url = format!(
        "{}/messages",
        crate::gateway::dynamic::configured_endpoint(&fixture.state, &binding)
    );
    let session = "ses_policyLateRevoke";
    let call = || {
        let request = fixture.client.post(&url).bearer_auth("isolated-local-key")
            .header("x-opencode-session", session)
            .json(&json!({"model":local_agent_core::routing::ALIAS,"max_tokens":30,"messages":[{"role":"user","content":"Policy send fence"}]}))
            .timeout(Duration::from_secs(12));
        async move {
            let response = request.send().await.unwrap();
            (response.status(), response.text().await.unwrap())
        }
    };
    let (gate, entered) = vault.pause(Some(backup_ref));
    let preparing = tokio::spawn(call());
    tokio::time::timeout(Duration::from_secs(4), entered)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(sends.lock().unwrap().as_slice(), ["primary-fixture"]);
    fixture
        .state
        .plugins
        .unbind_policy_fixture(&store_id, &service.id, &revision);
    drop(gate);
    let (status, body) = preparing.await.unwrap();
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{body}");
    assert_eq!(sends.lock().unwrap().as_slice(), ["primary-fixture"]);
    let detail = fixture
        .detail_when(session, |detail| detail["routeTrace"]["httpStatus"] == 429)
        .await;
    assert_eq!(detail["request"]["attempts"].as_array().unwrap().len(), 1);
    assert!(
        detail["routeTrace"]["decisions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|decision| decision["reason"] == "policy-unavailable")
    );
    assert_eq!(call().await.0, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(sends.lock().unwrap().as_slice(), ["primary-fixture"]);
}

#[tokio::test]
async fn deadline_before_send_can_retry_but_an_unknown_upstream_result_stays_pinned() {
    let vault = Arc::new(SlowVault::default());
    let mut fixture = Fixture::new(vault.clone()).await;
    let upstream = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut provider = fixture.provider();
    provider.endpoint = format!("http://{}", upstream.local_addr().unwrap());
    let credential = uuid::Uuid::new_v4().to_string();
    let local_token = uuid::Uuid::new_v4().to_string();
    vault.set(&credential, b"isolated-upstream-key").unwrap();
    vault.set(&local_token, b"isolated-local-key").unwrap();
    provider.credential_ref = Some(credential.clone());
    let count = Arc::new(AtomicUsize::new(0));
    let block = Arc::new(AtomicBool::new(false));
    let received = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let router = axum::Router::new().route(
        "/v1/messages",
        axum::routing::post({
            let (count, block, received, release) =
                (count.clone(), block.clone(), received.clone(), release.clone());
            move || {
                let (count, block, received, release) =
                    (count.clone(), block.clone(), received.clone(), release.clone());
                async move {
                    count.fetch_add(1, Ordering::AcqRel);
                    if block.load(Ordering::Acquire) {
                        received.notify_one();
                        let _ = tokio::time::timeout(Duration::from_secs(9), release.notified()).await;
                    }
                    axum::Json(json!({
                        "id":"deadline-fixture","type":"message","role":"assistant","model":"fixture-model",
                        "content":[{"type":"text","text":"OK"}],"stop_reason":"end_turn",
                        "usage":{"input_tokens":5,"output_tokens":2}
                    }))
                }
            }
        }),
    );
    fixture.tasks.push(tokio::spawn(async move {
        axum::serve(upstream, router).await.unwrap();
    }));
    let binding = {
        let store = fixture.state.store.lock().unwrap();
        store.save_provider(provider.clone()).unwrap();
        let model_id = uuid::Uuid::new_v4().to_string();
        let service = store.save_model_service(serde_json::from_value(json!({
            "id":uuid::Uuid::new_v4().to_string(), "revision":0,"name":"Deadline fixture","defaultModelId":model_id,
            "models":[{"id":model_id,"name":"Deadline model","providerId":provider.id,"model":provider.model,
                "capabilities":{"contextWindow":32000,"maxOutputTokens":4096,"tools":true,"thinking":false,"inputModalities":["text"]}}],
            "routingPolicy":{"enabled":true,"deadlineSeconds":5,"cooldownSeconds":1}
        })).unwrap()).unwrap();
        store
            .create_model_service_binding("opencode", "deadline-fixture", &local_token, &service)
            .unwrap()
            .0
    };
    let endpoint = format!(
        "{}/messages",
        crate::gateway::dynamic::configured_endpoint(&fixture.state, &binding)
    );
    let call = |session| {
        let request = fixture
            .client
            .post(&endpoint)
            .bearer_auth("isolated-local-key")
            .header("x-opencode-session", session)
            .json(
                &json!({"model":local_agent_core::routing::ALIAS,"max_tokens":30,
                "messages":[{"role":"user","content":"Deadline fixture"}]}),
            )
            .timeout(Duration::from_secs(8));
        async move {
            let response = request.send().await.unwrap();
            (response.status(), response.text().await.unwrap())
        }
    };
    let latest_outcome = || {
        rusqlite::Connection::open(fixture.root.path().join("data/local-agent.sqlite3"))
            .unwrap().query_row(
                "SELECT d.state,a.outcome FROM service_route_attempts a JOIN service_route_dispatch d
                 ON d.binding=a.binding AND d.session=a.session AND d.turn=a.turn
                 WHERE a.binding=?1 ORDER BY a.id DESC LIMIT 1",
                [&binding.id], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            ).unwrap()
    };
    // Hold input preparation after selection, before any dispatch row exists.
    let budget_gate = tokio::time::timeout(
        Duration::from_secs(3),
        crate::observation::context_budget::hold_budget_for_test(),
    )
    .await
    .unwrap();
    let preparing = tokio::spawn(call("ses_duringPreparation"));
    let path = fixture.root.path().join("data/local-agent.sqlite3");
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let connection = rusqlite::Connection::open(&path).unwrap();
            let lease: bool = connection.query_row(
                "SELECT EXISTS(SELECT 1 FROM gateway_route_leases WHERE binding=?1 AND session='opencode:ses_duringPreparation')",
                [&binding.id], |row| row.get(0),
            ).unwrap();
            if lease {
                let plans: u32 = connection.query_row(
                    "SELECT count(*) FROM service_route_dispatch WHERE binding=?1",
                    [&binding.id], |row| row.get(0),
                ).unwrap();
                assert_eq!(plans, 0);
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }).await.unwrap();
    let (status, body) =
        tokio::time::timeout(Duration::from_secs(2), call("ses_duringPreparation"))
            .await
            .unwrap();
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(count.load(Ordering::Acquire), 0);
    let blocked = fixture
        .detail_when("ses_duringPreparation", |detail| {
            detail["routeTrace"]["httpStatus"] == 409
        })
        .await;
    assert_eq!(blocked["routeTrace"]["sentUpstream"], false);
    assert_eq!(
        blocked["routeTrace"]["decisions"][0]["reason"],
        "outcome-unknown"
    );
    drop(budget_gate);
    assert_eq!(preparing.await.unwrap().0, StatusCode::OK);
    assert_eq!(count.swap(0, Ordering::AcqRel), 1);

    let (gate, entered) = vault.pause(Some(credential));
    let waiting = tokio::spawn(call("ses_deadlinebefore"));
    tokio::time::timeout(Duration::from_secs(3), entered)
        .await
        .unwrap()
        .unwrap();
    let (status, body) = waiting.await.unwrap();
    drop(gate);
    assert_eq!(status, StatusCode::GATEWAY_TIMEOUT, "{body}");
    assert!(body.contains("本次候选未发送"), "{body}");
    assert_eq!(count.load(Ordering::Acquire), 0);
    assert_eq!(
        latest_outcome(),
        ("failed".into(), "deadline_before_send".into())
    );
    let has_trace = |detail: &serde_json::Value| detail["routeTrace"].is_object();
    let local = fixture.detail_when("ses_deadlinebefore", has_trace).await;
    assert_eq!(local["routeTrace"]["sentUpstream"], false);
    assert_eq!(local["routeTrace"]["decisions"][0]["reason"], "deadline");
    assert!(local["request"]["attempts"].as_array().unwrap().is_empty());

    tokio::time::sleep(Duration::from_millis(1100)).await;
    assert_eq!(call("ses_deadlinebefore").await.0, StatusCode::OK);
    assert_eq!(count.load(Ordering::Acquire), 1);

    block.store(true, Ordering::Release);
    let waiting = tokio::spawn(call("ses_deadlineafter"));
    tokio::time::timeout(Duration::from_secs(3), received.notified())
        .await
        .unwrap();
    let (status, body) = waiting.await.unwrap();
    release.notify_one();
    assert_eq!(status, StatusCode::GATEWAY_TIMEOUT, "{body}");
    assert!(body.contains("上游结果未确认"), "{body}");
    assert_eq!(
        latest_outcome(),
        ("pending".into(), "outcome_unknown".into())
    );
    let remote = fixture.detail_when("ses_deadlineafter", has_trace).await;
    assert_eq!(remote["routeTrace"]["sentUpstream"], true);
    assert_eq!(
        remote["routeTrace"]["decisions"][0]["reason"],
        "upstream-outcome-unconfirmed"
    );
    assert_eq!(remote["request"]["attempts"].as_array().unwrap().len(), 1);
    block.store(false, Ordering::Release);
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection
        .execute_batch(
            "CREATE TRIGGER fixture_send_write BEFORE UPDATE ON service_route_attempts
         WHEN NEW.session='opencode:ses_recoverUnsent'
         BEGIN SELECT RAISE(FAIL,'fixture send write interrupted'); END;",
        )
        .unwrap();
    assert_eq!(
        call("ses_recoverUnsent").await.0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(latest_outcome(), ("pending".into(), "preparing".into()));
    assert_eq!(count.load(Ordering::Acquire), 2);
    connection
        .execute_batch("DROP TRIGGER fixture_send_write")
        .unwrap();
    drop(connection);
    *fixture.state.store.lock().unwrap() = Store::open(path.parent().unwrap()).unwrap();
    assert_eq!(call("ses_deadlineafter").await.0, StatusCode::CONFLICT);
    assert_eq!(count.load(Ordering::Acquire), 2);
    assert_eq!(call("ses_recoverUnsent").await.0, StatusCode::OK);
    assert_eq!(count.load(Ordering::Acquire), 3);
    assert_eq!(latest_outcome(), ("committed".into(), "committed".into()));
}
