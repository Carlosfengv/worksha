use super::*;
use axum::{Json, http::HeaderMap};
use local_agent_core::providers::upstream_accounts::AccountEdit;
use tokio::sync::Notify;

type MockError = (StatusCode, Option<&'static str>, serde_json::Value);

#[tokio::test]
async fn official_budget_rejections_pause_only_the_proven_key_and_preserve_retry_boundaries() {
    let (listener, upstream) = super::minimax::tls_upstream(
        "openrouter.ai",
        include_bytes!("fixtures/quota-fixture-cert.der"),
    )
    .await;
    let vault = Arc::new(MemoryVault::default());
    let mut fixture = Fixture::with_upstream(vault.clone(), Some(upstream)).await;
    let seen = Arc::new(Mutex::new(Vec::new()));
    let primary_error = Arc::new(Mutex::new(None::<MockError>));
    let gate = Arc::new(Mutex::new(None::<(Arc<Notify>, Arc<Notify>)>));
    let app = axum::Router::new().route("/api/v1/chat/completions", axum::routing::post({
        let (seen, primary_error, gate) = (seen.clone(), primary_error.clone(), gate.clone());
        move |headers: HeaderMap, Json(body): Json<serde_json::Value>| {
            let (seen, primary_error, gate) = (seen.clone(), primary_error.clone(), gate.clone());
            async move {
                let key = match headers["authorization"].to_str().unwrap() {
                    "Bearer fixture-budget-primary" => "primary",
                    "Bearer fixture-budget-backup" => "backup",
                    _ => panic!("unknown fixture key"),
                };
                seen.lock().unwrap().push(key);
                if key == "primary" {
                    let error = primary_error.lock().unwrap().clone();
                    if let Some((status, retry, value)) = error {
                        let mut response = (status, Json(value)).into_response();
                        if let Some(retry) = retry {
                            response.headers_mut().insert("retry-after", HeaderValue::from_static(retry));
                        }
                        return response;
                    }
                    let gate = gate.lock().unwrap().take();
                    if let Some((entered, release)) = gate {
                        entered.notify_one();
                        tokio::time::timeout(Duration::from_secs(4), release.notified()).await.unwrap();
                    }
                }
                Json(json!({"id":"fixture","object":"chat.completion","model":body["model"],
                    "choices":[{"index":0,"message":{"role":"assistant","content":"OK"},"finish_reason":"stop"}],
                    "usage":{"prompt_tokens":10,"completion_tokens":1}})).into_response()
            }
        }
    }));
    fixture.tasks.push(tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap()
    }));
    let credential = |key| {
        let reference = uuid::Uuid::new_v4().to_string();
        vault.set(&reference, key).unwrap();
        reference
    };
    let primary = credential(b"fixture-budget-primary");
    let backup = credential(b"fixture-budget-backup");
    let local = credential(b"fixture-budget-local");
    let (pooled, fixed, account_scope) = {
        let store = fixture.state.store.lock().unwrap();
        let mut provider = store.provider_by_id("openrouter").unwrap().unwrap();
        provider.configured = true;
        provider.model = "fixture-budget".into();
        provider.models = vec![provider.model.clone()];
        provider.credential_ref = Some(primary.clone());
        store.save_provider(provider).unwrap();
        let provider = store.provider_by_id("openrouter").unwrap().unwrap();
        let accounts = [primary, backup]
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
                            label: format!("Fixture {}", index + 1),
                            credential_ref,
                        },
                    )
                    .unwrap()
                    .reference()
            })
            .collect::<Vec<_>>();
        let connect = |enabled| {
            let model = uuid::Uuid::new_v4().to_string();
            let policy = if enabled {
                json!({"enabled":true,"maxAttempts":2,"fallbackOnRateLimit":true,
                    "accountPools":[{"modelId":model,"accounts":accounts}]})
            } else {
                json!({"enabled":false})
            };
            let service = store.save_model_service(serde_json::from_value(json!({
                "id":uuid::Uuid::new_v4().to_string(),"revision":0,"name":"Budget fixture","defaultModelId":model,
                "models":[{"id":model,"name":"Fixture","providerId":provider.id,"model":provider.model,
                    "capabilities":{"contextWindow":32000,"maxOutputTokens":4096,"tools":true,"thinking":false,"inputModalities":["text"]}}],
                "routingPolicy":policy,
            })).unwrap()).unwrap();
            let binding = store
                .create_model_service_binding("opencode", "fixture", &local, &service)
                .unwrap()
                .0;
            format!(
                "{}/messages",
                crate::gateway::dynamic::configured_endpoint(&fixture.state, &binding)
            )
        };
        (connect(true), connect(false), scopes(&provider).0)
    };
    let request = |endpoint: &str, session: &str| {
        fixture.client.post(endpoint).bearer_auth("fixture-budget-local").header("x-opencode-session", session)
            .json(&json!({"model":local_agent_core::routing::ALIAS,"max_tokens":30,"messages":[{"role":"user","content":"Quota fixture"}]}))
            .timeout(Duration::from_secs(6))
    };
    let play = async |endpoint: &str, session: &str, expected: StatusCode, keys: &[&str]| {
        let before = seen.lock().unwrap().len();
        let response = request(endpoint, session).send().await.unwrap();
        let status = response.status();
        let body = response.text().await.unwrap();
        assert_eq!(status, expected, "{session}: {body}");
        assert_eq!(&seen.lock().unwrap()[before..], keys, "{session}");
    };
    let set_error = |status, retry, error| {
        *primary_error.lock().unwrap() = Some((status, retry, json!({"error":error})));
    };
    let hold = || {
        fixture
            .state
            .store
            .lock()
            .unwrap()
            .openrouter_reply_holds(&account_scope)
            .unwrap()
    };

    let reset = chrono::Utc::now() + chrono::Duration::seconds(4);
    let exhausted = json!({"code":403,"metadata":{"limit_source":"openrouter_guardrail_budget",
        "scope":"workspace_default_member","limit_usd":50,"spend_usd":52,"resets_at":reset.to_rfc3339()}});
    set_error(StatusCode::FORBIDDEN, None, exhausted.clone());
    play(
        &pooled,
        "ses_replybudgetFirst",
        StatusCode::OK,
        &["primary", "backup"],
    )
    .await;
    let detail = fixture
        .detail_when("ses_replybudgetFirst", |detail| {
            detail["request"]["attempts"][1].is_object()
        })
        .await;
    assert_eq!(detail["request"]["attempts"][0]["httpStatus"], 403);
    assert_eq!(
        detail["request"]["attempts"][1]["routing"]["service"]["fallbackReason"],
        "account_quota_exhausted"
    );
    assert_eq!(
        hold(),
        [("workspace_default_member".into(), Some(reset.timestamp()))]
    );
    *fixture.state.store.lock().unwrap() = Store::open(&fixture.root.path().join("data")).unwrap();
    play(
        &fixed,
        "ses_replybudgetCold",
        StatusCode::TOO_MANY_REQUESTS,
        &[],
    )
    .await;
    play(
        &pooled,
        "ses_replybudgetBackup",
        StatusCode::OK,
        &["backup"],
    )
    .await;

    let delay = reset.timestamp_millis() - chrono::Utc::now().timestamp_millis();
    tokio::time::sleep(Duration::from_millis(delay.max(0) as u64)).await;
    *primary_error.lock().unwrap() = None;
    let (entered, release) = (Arc::new(Notify::new()), Arc::new(Notify::new()));
    *gate.lock().unwrap() = Some((entered.clone(), release.clone()));
    let probe = tokio::spawn(request(&fixed, "ses_replybudgetProbe").send());
    tokio::time::timeout(Duration::from_secs(3), entered.notified())
        .await
        .unwrap();
    play(
        &pooled,
        "ses_replybudgetParallel",
        StatusCode::OK,
        &["backup"],
    )
    .await;
    release.notify_one();
    assert_eq!(probe.await.unwrap().unwrap().status(), StatusCode::OK);
    play(
        &pooled,
        "ses_replybudgetRecovered",
        StatusCode::OK,
        &["primary"],
    )
    .await;

    let held = hold();
    for (session, status, error) in [
        (
            "ses_replybudgetGuardrail",
            StatusCode::FORBIDDEN,
            json!({"code":403,"metadata":{"patterns":["fixture"]}}),
        ),
        (
            "ses_replybudgetCredits",
            StatusCode::PAYMENT_REQUIRED,
            json!({"code":402,"metadata":{"limit_source":"openrouter_credits","reason":"weight_exceeds_budget"}}),
        ),
        ("ses_replybudgetEmbedded", StatusCode::OK, exhausted),
    ] {
        set_error(status, None, error);
        let expected = if status.is_success() {
            StatusCode::FORBIDDEN
        } else {
            status
        };
        play(&pooled, session, expected, &["primary"]).await;
        assert_eq!(hold(), held);
    }

    let in_flight = json!({"code":402,"metadata":{"limit_source":"openrouter_in_flight_budget","reason":"in_flight_budget_exhausted"}});
    set_error(StatusCode::PAYMENT_REQUIRED, None, in_flight.clone());
    play(
        &pooled,
        "ses_replybudgetUnconfirmedRetry",
        StatusCode::PAYMENT_REQUIRED,
        &["primary"],
    )
    .await;
    set_error(StatusCode::PAYMENT_REQUIRED, Some("2"), in_flight);
    play(
        &pooled,
        "ses_replybudgetInflight",
        StatusCode::OK,
        &["primary", "backup"],
    )
    .await;
    play(
        &pooled,
        "ses_replybudgetCooling",
        StatusCode::OK,
        &["backup"],
    )
    .await;
    assert_eq!(
        hold(),
        held,
        "temporary in-flight capacity must not become a durable quota hold"
    );
    let until = fixture
        .state
        .store
        .lock()
        .unwrap()
        .service_cooldown(&account_scope)
        .unwrap()
        .unwrap();
    assert_eq!(until.1, "upstream_rate_limit");
    let delay = until.0 - chrono::Utc::now().timestamp_millis();
    tokio::time::sleep(Duration::from_millis(delay.max(0) as u64)).await;

    set_error(
        StatusCode::PAYMENT_REQUIRED,
        None,
        json!({"code":402,"metadata":{"limit_source":"openrouter_key_limit"}}),
    );
    play(
        &fixed,
        "ses_replybudgetFixedFailure",
        StatusCode::PAYMENT_REQUIRED,
        &["primary"],
    )
    .await;
    assert!(
        hold()
            .iter()
            .any(|(scope, reset)| scope == "key_limit" && reset.is_none())
    );
    play(
        &fixed,
        "ses_replybudgetFixedHold",
        StatusCode::TOO_MANY_REQUESTS,
        &[],
    )
    .await;
}
