use super::*;
use axum::{
    Json,
    extract::{Path, State},
    http::HeaderMap,
};
use local_agent_core::{
    model_services::policy::BalanceState, providers::upstream_accounts::AccountEdit,
};
use tokio::sync::Notify;

type Rejection = (StatusCode, Option<&'static str>, serde_json::Value);

#[tokio::test]
async fn official_provider_balance_rejections_fall_back_and_probe_after_refresh() {
    let (listener, upstream) = super::minimax::tls_upstream_hosts(
        &["api.deepseek.com", "api.moonshot.cn"],
        include_bytes!("fixtures/provider-balance-fixture-cert.der"),
    )
    .await;
    let vault = Arc::new(SlowVault::default());
    let mut fixture = Fixture::with_upstream(vault.clone(), Some(upstream)).await;
    let seen = Arc::new(Mutex::new(Vec::new()));
    let errors = Arc::new(Mutex::new([None::<Rejection>, None]));
    let gate = Arc::new(Mutex::new(None::<(usize, Arc<Notify>, Arc<Notify>)>));
    let app = axum::Router::new().route("/v1/chat/completions", axum::routing::post({
        let (seen, errors, gate) = (seen.clone(), errors.clone(), gate.clone());
        move |headers: HeaderMap, Json(body): Json<serde_json::Value>| {
            let (seen, errors, gate) = (seen.clone(), errors.clone(), gate.clone());
            async move {
                let index = match headers["authorization"].to_str().unwrap() {
                    "Bearer fixture-balance-deepseek" => 0,
                    "Bearer fixture-balance-kimi" => 1,
                    _ => panic!("unexpected fixture key"),
                };
                seen.lock().unwrap().push(index);
                let hold = {
                    let mut hold = gate.lock().unwrap();
                    if hold.as_ref().is_some_and(|(target, _, _)| *target == index) { hold.take() } else { None }
                };
                if let Some((_, entered, release)) = hold {
                    entered.notify_one();
                    tokio::time::timeout(Duration::from_secs(4), release.notified()).await.unwrap();
                }
                if let Some((status, retry, value)) = errors.lock().unwrap()[index].clone() {
                    let mut response = (status, Json(json!({"error":value}))).into_response();
                    if let Some(retry) = retry {
                        response.headers_mut().insert("retry-after", HeaderValue::from_static(retry));
                    }
                    return response;
                }
                let text = if body["max_tokens"] == 128
                    && body["messages"].to_string().contains("ses_balanceHealthyIntent") {
                    "1"
                } else {
                    "OK"
                };
                Json(json!({"id":"balance-fixture","object":"chat.completion","model":body["model"],
                    "choices":[{"index":0,"message":{"role":"assistant","content":text},"finish_reason":"stop"}],
                    "usage":{"prompt_tokens":10,"completion_tokens":1}})).into_response()
            }
        }
    }));
    fixture.tasks.push(tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap()
    }));
    let credential = |key: &[u8]| {
        let reference = uuid::Uuid::new_v4().to_string();
        vault.set(&reference, key).unwrap();
        reference
    };
    let keys = [
        credential(b"fixture-balance-deepseek"),
        credential(b"fixture-balance-kimi"),
    ];
    let local = credential(b"fixture-balance-local");
    let provider_template = fixture.provider();
    let (routes, fixed, limited, intent, concurrent_intent, protected, providers) = {
        let store = fixture.state.store.lock().unwrap();
        let providers = [
            ("deepseek", "https://api.deepseek.com/v1", "deepseek-flash"),
            ("kimi", "https://api.moonshot.cn/v1", "kimi-k2.6"),
        ]
        .into_iter()
        .zip(keys)
        .map(|((vendor, endpoint, model), credential_ref)| {
            let mut source = provider_template.clone();
            source.id = format!("custom-{}", uuid::Uuid::new_v4());
            source.vendor = vendor.into();
            source.name = vendor.into();
            source.endpoint = endpoint.into();
            source.protocol = "chat-completions".into();
            source.model = model.into();
            source.models = vec![model.into()];
            source.credential_ref = Some(credential_ref.clone());
            store.save_provider(source.clone()).unwrap();
            let source = store.provider_by_id(&source.id).unwrap().unwrap();
            let reference = store
                .save_upstream_account(
                    &source,
                    &AccountEdit {
                        id: uuid::Uuid::new_v4().to_string(),
                        expected_revision: 0,
                        provider_revision: source.revision,
                        label: vendor.into(),
                        credential_ref,
                    },
                )
                .unwrap()
                .reference();
            (source, reference)
        })
        .collect::<Vec<_>>();
        let entry = |provider: &Provider| {
            json!({"id":uuid::Uuid::new_v4().to_string(),"name":provider.name,
                "providerId":provider.id,"model":provider.model,"capabilities":{"contextWindow":32000,"maxOutputTokens":4096,
                    "tools":true,"thinking":false,"inputModalities":["text"]}})
        };
        let connect = |agent: &str, models: Vec<serde_json::Value>, policy| {
            let service = store
                .save_model_service(
                    serde_json::from_value(json!({
                        "id":uuid::Uuid::new_v4().to_string(),"revision":0,"name":"Balance fixture",
                        "defaultModelId":models[0]["id"],"models":models,"routingPolicy":policy,
                    }))
                    .unwrap(),
                )
                .unwrap();
            let binding = store
                .create_model_service_binding(agent, "balance fixture", &local, &service)
                .unwrap()
                .0;
            format!(
                "{}/v1/messages",
                crate::gateway::dynamic::endpoint(&fixture.state, &binding)
            )
        };
        let limit = |concurrent, per_minute| {
            let bounded = entry(&providers[0].0);
            connect(
                "opencode",
                vec![bounded.clone()],
                json!({
                    "enabled":true,
                    "accountPools":[{"modelId":bounded["id"],"accounts":[providers[0].1]}],
                    "limits":[{"modelId":bounded["id"],"maxConcurrent":concurrent,"requestsPerMinute":per_minute}],
                }),
            )
        };
        let intent = |max_concurrent| {
            let classifier = entry(&providers[0].0);
            let answer = entry(&providers[1].0);
            connect(
                "opencode",
                vec![classifier.clone(), answer.clone()],
                json!({
                    "enabled":true,"maxAttempts":2,"cooldownSeconds":2,
                    "fallbackOnFailure":true,"fallbackOnRateLimit":true,"fallbackOnAccessDenied":true,"fallbackAcrossModels":true,
                    "intentClassifier":{"modelId":classifier["id"],"account":providers[0].1},
                    "accountPools":[{"modelId":classifier["id"],"accounts":[providers[0].1]}],
                    "limits":[{"modelId":classifier["id"],"maxConcurrent":max_concurrent,"requestsPerMinute":0}],
                    "rules":[{"id":uuid::Uuid::new_v4().to_string(),"name":"Intent fixture",
                        "modelId":answer["id"],"intent":"Fixture label"}],
                }),
            )
        };
        (
            std::array::from_fn::<_, 2, _>(|index| {
                let (primary, reference) = &providers[index];
                let primary = entry(primary);
                connect(
                    "opencode",
                    vec![primary.clone(), entry(&providers[1 - index].0)],
                    json!({
                        "enabled":true,"maxAttempts":2,"fallbackOnRateLimit":true,"fallbackOnFailure":true,"fallbackOnAccessDenied":true,"fallbackAcrossModels":true,
                        "accountPools":[{"modelId":primary["id"],"accounts":[reference]}],
                    }),
                )
            }),
            std::array::from_fn::<_, 2, _>(|index| {
                connect(
                    "claude",
                    vec![entry(&providers[index].0)],
                    json!({"enabled":false}),
                )
            }),
            [limit(1, 0), limit(0, 1)],
            intent(0),
            intent(2),
            connect(
                "opencode",
                vec![entry(&providers[0].0), entry(&providers[1].0)],
                json!({"enabled":true,"maxAttempts":2,"fallbackOnFailure":true,"fallbackAcrossModels":true}),
            ),
            providers,
        )
    };
    let scope = |index: usize| scopes(&providers[index].0).0;
    let cooldown = |scope: &str| {
        fixture
            .state
            .store
            .lock()
            .unwrap()
            .service_cooldowns()
            .unwrap()
            .get(scope)
            .cloned()
    };
    let after_cooldown = async |until: i64| {
        let remaining = (until - chrono::Utc::now().timestamp_millis()).max(0) as u64;
        tokio::time::sleep(Duration::from_millis(remaining + 25)).await;
    };
    let state = |index| {
        fixture
            .state
            .store
            .lock()
            .unwrap()
            .upstream_balance_state(&scope(index))
            .unwrap()
    };
    let refresh = |index| {
        fixture
            .state
            .store
            .lock()
            .unwrap()
            .record_upstream_balance(&scope(index), chrono::Utc::now(), true)
            .unwrap()
    };
    let display = async |index: usize, expected: serde_json::Value| {
        let before = seen.lock().unwrap().len();
        let provider = &providers[index].0;
        let Json(billing) = crate::billing::list(State(fixture.state.clone()))
            .await
            .unwrap();
        let rows: Vec<_> = billing["accounts"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|account| account["providerId"] == provider.id)
            .collect();
        assert_eq!(rows.len(), 2, "default and account pool entries");
        for account in rows {
            assert_eq!(account["routingQuota"], expected);
        }
        let Json(pool) = crate::providers::upstream_accounts::action(
            State(fixture.state.clone()),
            Path(provider.id.clone()),
            Json(local_agent_core::providers::upstream_accounts::AccountAction::List),
        )
        .await
        .unwrap();
        assert_eq!(pool["accounts"][0]["state"], "enabled");
        assert_eq!(pool["accounts"][0]["routingQuota"], expected);
        assert_eq!(
            seen.lock().unwrap().len(),
            before,
            "status reads never send"
        );
    };
    let request = |endpoint: &str, session: &str| {
        fixture
            .client
            .post(endpoint)
            .bearer_auth("fixture-balance-local")
            .header("x-opencode-session", session)
            .json(
                &json!({"model":local_agent_core::routing::ALIAS,"max_tokens":30,
                "messages":[{"role":"user","content":format!("Balance test {session}")}]}),
            )
            .timeout(Duration::from_secs(8))
    };
    let play = async |endpoint: &str, session: &str, expected, sent: &[usize]| {
        let before = seen.lock().unwrap().len();
        let response = request(endpoint, session).send().await.unwrap();
        let status = response.status();
        let body = response.text().await.unwrap();
        assert_eq!(status, expected, "{session}: {body}");
        assert_eq!(&seen.lock().unwrap()[before..], sent, "{session}");
        body
    };
    let (entered, release) = (Arc::new(Notify::new()), Arc::new(Notify::new()));
    *gate.lock().unwrap() = Some((0, entered.clone(), release.clone()));
    let fixed_send = tokio::spawn(request(&fixed[0], "ses_balanceFixedCapacity").send());
    tokio::time::timeout(Duration::from_secs(3), entered.notified())
        .await
        .unwrap();
    play(
        &limited[0],
        "ses_balanceConcurrentLimit",
        StatusCode::TOO_MANY_REQUESTS,
        &[],
    )
    .await;
    release.notify_one();
    let fixed_response = fixed_send.await.unwrap().unwrap();
    assert_eq!(fixed_response.status(), StatusCode::OK);
    assert!(fixed_response.text().await.unwrap().contains("OK"));
    play(
        &limited[1],
        "ses_balanceMinuteLimit",
        StatusCode::TOO_MANY_REQUESTS,
        &[],
    )
    .await;
    play(
        &limited[0],
        "ses_balanceReleasedCapacity",
        StatusCode::OK,
        &[0],
    )
    .await;

    errors.lock().unwrap()[0] = Some((
        StatusCode::PAYMENT_REQUIRED,
        None,
        json!({"type":"authentication_error","message":"Contradictory fixture rejection"}),
    ));
    play(
        &routes[0],
        "ses_balanceAmbiguous",
        StatusCode::PAYMENT_REQUIRED,
        &[0],
    )
    .await;
    assert_eq!(state(0), BalanceState::Open);
    errors.lock().unwrap()[0] = Some((
        StatusCode::PAYMENT_REQUIRED,
        None,
        json!({"type":"unknown_error","code":"invalid_request_error","message":"Fixture balance rejection"}),
    ));
    let body = play(&routes[0], "ses_balanceDeepSeek", StatusCode::OK, &[0, 1]).await;
    assert!(body.contains("首选来源额度不可用"));
    assert_eq!(state(0), BalanceState::Exhausted);
    assert_eq!(state(1), BalanceState::Open);
    *fixture.state.store.lock().unwrap() = Store::open(&fixture.root.path().join("data")).unwrap();
    display(0, json!("exhausted")).await;
    display(1, serde_json::Value::Null).await;
    play(
        &fixed[0],
        "ses_balanceFixedCold",
        StatusCode::TOO_MANY_REQUESTS,
        &[],
    )
    .await;
    play(&routes[0], "ses_balanceNext", StatusCode::OK, &[1]).await;

    refresh(0);
    display(0, json!("recovery")).await;
    errors.lock().unwrap()[0] = None;
    let (entered, release) = (Arc::new(Notify::new()), Arc::new(Notify::new()));
    *gate.lock().unwrap() = Some((0, entered.clone(), release.clone()));
    let probe = tokio::spawn(request(&fixed[0], "ses_balanceFixedProbe").send());
    tokio::time::timeout(Duration::from_secs(3), entered.notified())
        .await
        .unwrap();
    refresh(0);
    display(0, json!("recovering")).await;
    play(&routes[0], "ses_balanceParallel", StatusCode::OK, &[1]).await;
    release.notify_one();
    assert_eq!(probe.await.unwrap().unwrap().status(), StatusCode::OK);
    play(&routes[0], "ses_balanceRecovered", StatusCode::OK, &[0]).await;
    display(0, json!("recovered")).await;

    errors.lock().unwrap()[0] = Some((
        StatusCode::PAYMENT_REQUIRED,
        None,
        json!({"type":"insufficient_balance","message":"Fixture classifier balance rejection"}),
    ));
    play(&intent, "ses_balanceIntentQuota", StatusCode::OK, &[0, 1]).await;
    assert_eq!(state(0), BalanceState::Exhausted);
    play(&intent, "ses_balanceIntentHeld", StatusCode::OK, &[1]).await;
    refresh(0);
    errors.lock().unwrap()[0] = None;
    play(&fixed[0], "ses_balanceAfterIntent", StatusCode::OK, &[0]).await;

    let before = seen.lock().unwrap().len();
    let mut concurrent = Vec::new();
    for session in [
        "ses_balanceHealthyIntentFirst",
        "ses_balanceHealthyIntentSecond",
    ] {
        let (entered, release) = (Arc::new(Notify::new()), Arc::new(Notify::new()));
        *gate.lock().unwrap() = Some((0, entered.clone(), release.clone()));
        let running = tokio::spawn(request(&concurrent_intent, session).send());
        tokio::time::timeout(Duration::from_secs(1), entered.notified())
            .await
            .unwrap();
        concurrent.push((running, release));
    }
    assert_eq!(&seen.lock().unwrap()[before..], &[0, 0]);
    play(
        &concurrent_intent,
        "ses_balanceHealthyIntentAtCapacity",
        StatusCode::OK,
        &[1],
    )
    .await;
    for (running, release) in concurrent.into_iter().rev() {
        let before = seen.lock().unwrap().len();
        release.notify_one();
        let response = running.await.unwrap().unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(response.text().await.unwrap().contains("OK"));
        assert_eq!(
            &seen.lock().unwrap()[before..],
            &[1],
            "each concurrent classification must match the Kimi rule"
        );
    }

    let model_scope = scopes(&providers[0].0).1;
    let (waiting, entered) = vault.pause(providers[0].0.credential_ref.clone());
    let first = seen.lock().unwrap().len();
    let preparing = tokio::spawn(request(&intent, "ses_balanceLateIntentRecovery").send());
    tokio::time::timeout(Duration::from_secs(1), entered)
        .await
        .unwrap()
        .unwrap();
    let until = chrono::Utc::now().timestamp_millis() + 150;
    fixture
        .state
        .store
        .lock()
        .unwrap()
        .service_set_cooldown(&model_scope, until, "upstream_failure")
        .unwrap();
    after_cooldown(until).await;
    drop(waiting);
    let response = preparing.await.unwrap().unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response.text().await.unwrap().contains("OK"));
    assert_eq!(
        &seen.lock().unwrap()[first..],
        &[0],
        "only the answer may probe the newly expired restriction"
    );
    assert!(cooldown(&model_scope).is_none());

    errors.lock().unwrap()[0] = Some((
        StatusCode::TOO_MANY_REQUESTS,
        Some("1"),
        json!({"type":"rate_limit_reached_error","message":"Fixture classifier rate limit"}),
    ));
    play(&intent, "ses_balanceIntentRate", StatusCode::OK, &[0, 1]).await;
    let rate = cooldown(&scope(0)).unwrap();
    assert_eq!(rate.1, "upstream_rate_limit");
    assert!(cooldown(&model_scope).is_none());
    errors.lock().unwrap()[0] = None;
    after_cooldown(rate.0).await;
    play(
        &intent,
        "ses_balanceIntentRateRecovered",
        StatusCode::OK,
        &[0, 0],
    )
    .await;
    assert!(cooldown(&scope(0)).is_none());

    errors.lock().unwrap()[0] = Some((
        StatusCode::SERVICE_UNAVAILABLE,
        Some("2"),
        json!({"type":"api_error","message":"Fixture classifier unavailable"}),
    ));
    play(
        &intent,
        "ses_balanceIntentUnavailable",
        StatusCode::OK,
        &[0, 1],
    )
    .await;
    let failure = cooldown(&model_scope).unwrap();
    assert_eq!(failure.1, "upstream_failure");
    play(
        &intent,
        "ses_balanceIntentFailureCooling",
        StatusCode::OK,
        &[1],
    )
    .await;
    play(
        &fixed[0],
        "ses_balanceFixedClassifierCooling",
        StatusCode::TOO_MANY_REQUESTS,
        &[],
    )
    .await;
    assert_eq!(cooldown(&model_scope).unwrap(), failure);
    errors.lock().unwrap()[0] = None;
    after_cooldown(failure.0).await;
    let before = seen.lock().unwrap().len();
    let (entered, release) = (Arc::new(Notify::new()), Arc::new(Notify::new()));
    *gate.lock().unwrap() = Some((0, entered.clone(), release.clone()));
    let recovering = tokio::spawn(request(&intent, "ses_balanceIntentHttpRecovered").send());
    tokio::time::timeout(Duration::from_secs(1), entered.notified())
        .await
        .unwrap();
    play(
        &intent,
        "ses_balanceParallelIntentRecovery",
        StatusCode::OK,
        &[1],
    )
    .await;
    assert_eq!(cooldown(&model_scope).unwrap(), failure);
    release.notify_one();
    let response = recovering.await.unwrap().unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response.text().await.unwrap().contains("OK"));
    assert_eq!(&seen.lock().unwrap()[before..], &[0, 1, 0]);
    assert!(cooldown(&model_scope).is_none());

    let rejected_scope = access_scopes(&providers[0].0).0;
    errors.lock().unwrap()[0] = Some((
        StatusCode::UNAUTHORIZED,
        None,
        json!({"type":"authentication_error","message":"Fixture ambiguous rejection"}),
    ));
    play(
        &intent,
        "ses_balanceIntentUnconfirmedAccess",
        StatusCode::UNAUTHORIZED,
        &[0, 0],
    )
    .await;
    assert!(cooldown(&rejected_scope).is_none());
    errors.lock().unwrap()[0] = Some((
        StatusCode::UNAUTHORIZED,
        None,
        json!({"type":"authentication_error","code":"invalid_api_key","message":"Fixture invalid key"}),
    ));
    let mut waiting = Vec::new();
    for (endpoint, session, expected) in [
        (&routes[0], "ses_balanceLateAccess", StatusCode::OK),
        (
            &protected,
            "ses_balanceLateAccessNoFallback",
            StatusCode::UNAUTHORIZED,
        ),
        (&protected, "", StatusCode::UNAUTHORIZED),
    ] {
        let (gate, entered) = vault.pause(providers[0].0.credential_ref.clone());
        let mut upstream = request(endpoint, session).build().unwrap();
        if session.is_empty() {
            upstream.headers_mut().remove("x-opencode-session");
        }
        let client = fixture.client.clone();
        let in_flight = tokio::spawn(async move { client.execute(upstream).await.unwrap() });
        tokio::time::timeout(Duration::from_secs(3), entered)
            .await
            .unwrap()
            .unwrap();
        waiting.push((gate, in_flight, expected));
    }
    play(
        &intent,
        "ses_balanceIntentCredentialDenied",
        StatusCode::OK,
        &[0, 1],
    )
    .await;
    let denied = cooldown(&rejected_scope).unwrap();
    assert_eq!(denied.1, "upstream_credential_rejected");
    errors.lock().unwrap()[0] = None;
    for (gate, in_flight, expected) in waiting {
        let before = seen.lock().unwrap().len();
        drop(gate);
        let response = in_flight.await.unwrap();
        assert_eq!(response.status(), expected);
        let body = response.text().await.unwrap();
        if expected.is_success() {
            assert!(body.contains("OK"));
            assert_eq!(&seen.lock().unwrap()[before..], &[1]);
        } else {
            assert!(body.contains("上游拒绝了此账号的 API Key"));
            assert_eq!(seen.lock().unwrap().len(), before);
        }
        assert_eq!(cooldown(&rejected_scope).unwrap(), denied);
    }
    let detail = fixture
        .detail_when("ses_balanceLateAccess", |detail| {
            detail["routeTrace"]["httpStatus"] == 200
        })
        .await;
    let attempts = detail["request"]["attempts"].as_array().unwrap();
    assert_eq!(attempts.len(), 1);
    assert_eq!(
        attempts[0]["routing"]["service"]["fallbackReason"],
        "upstream_credential_rejected"
    );
    assert!(
        detail["routeTrace"]["decisions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|decision| decision["reason"] == "access-credential-rejected")
    );
    play(
        &intent,
        "ses_balanceIntentCredentialCooling",
        StatusCode::OK,
        &[1],
    )
    .await;
    assert_eq!(cooldown(&rejected_scope).unwrap(), denied);
    after_cooldown(denied.0).await;

    let (waiting, entered) = vault.pause(providers[0].0.credential_ref.clone());
    let before = seen.lock().unwrap().len();
    let preparing = tokio::spawn(request(&routes[0], "ses_balanceLateProbeScope").send());
    tokio::time::timeout(Duration::from_secs(3), entered)
        .await
        .unwrap()
        .unwrap();
    let until = chrono::Utc::now().timestamp_millis() + 150;
    fixture
        .state
        .store
        .lock()
        .unwrap()
        .service_set_cooldown(&model_scope, until, "upstream_failure")
        .unwrap();
    after_cooldown(until).await;
    drop(waiting);
    let response = preparing.await.unwrap().unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response.text().await.unwrap().contains("OK"));
    assert_eq!(&seen.lock().unwrap()[before..], &[1]);
    assert_eq!(cooldown(&rejected_scope).unwrap(), denied);
    assert_eq!(cooldown(&model_scope).unwrap().0, until);

    play(
        &intent,
        "ses_balanceIntentCredentialRecovered",
        StatusCode::OK,
        &[0, 0],
    )
    .await;
    assert!(cooldown(&rejected_scope).is_none());
    assert!(cooldown(&model_scope).is_none());

    let (waiting, entered) = vault.pause(providers[0].0.credential_ref.clone());
    let first = seen.lock().unwrap().len();
    let preparing = tokio::spawn(request(&routes[0], "ses_balanceLateCooling").send());
    tokio::time::timeout(Duration::from_secs(3), entered)
        .await
        .unwrap()
        .unwrap();
    errors.lock().unwrap()[0] = Some((
        StatusCode::SERVICE_UNAVAILABLE,
        Some("2"),
        json!({"type":"api_error","message":"Fixture temporary upstream failure"}),
    ));
    play(
        &fixed[0],
        "ses_balanceFixedFailure",
        StatusCode::SERVICE_UNAVAILABLE,
        &[0],
    )
    .await;
    let failure = cooldown(&model_scope).unwrap();
    assert_eq!(failure.1, "upstream_failure");
    drop(waiting);
    let response = preparing.await.unwrap().unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response.text().await.unwrap().contains("OK"));
    assert_eq!(&seen.lock().unwrap()[first..], &[0, 1]);
    assert_eq!(
        cooldown(&model_scope).unwrap(),
        failure,
        "a local rejection must not extend the deadline"
    );
    let detail = fixture
        .detail_when("ses_balanceLateCooling", |detail| {
            detail["routeTrace"]["httpStatus"] == 200
        })
        .await;
    assert_eq!(detail["request"]["attempts"].as_array().unwrap().len(), 1);
    assert!(
        detail["routeTrace"]["decisions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|decision| decision["reason"] == "failure-cooldown")
    );

    errors.lock().unwrap()[0] = None;
    play(
        &fixed[0],
        "ses_balanceFixedFailureCooling",
        StatusCode::TOO_MANY_REQUESTS,
        &[],
    )
    .await;
    after_cooldown(failure.0).await;
    let (entered, release) = (Arc::new(Notify::new()), Arc::new(Notify::new()));
    *gate.lock().unwrap() = Some((0, entered.clone(), release.clone()));
    let probe = tokio::spawn(request(&fixed[0], "ses_balanceFixedHttpProbe").send());
    tokio::time::timeout(Duration::from_secs(3), entered.notified())
        .await
        .unwrap();
    play(
        &fixed[0],
        "ses_balanceFixedHttpWaiting",
        StatusCode::TOO_MANY_REQUESTS,
        &[],
    )
    .await;
    play(
        &routes[0],
        "ses_balanceManagedHttpWaiting",
        StatusCode::OK,
        &[1],
    )
    .await;
    release.notify_one();
    let response = probe.await.unwrap().unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response.text().await.unwrap().contains("OK"));
    assert!(cooldown(&model_scope).is_none());
    play(&routes[0], "ses_balanceHttpRestored", StatusCode::OK, &[0]).await;

    errors.lock().unwrap()[1] = Some((
        StatusCode::TOO_MANY_REQUESTS,
        Some("2"),
        json!({"type":"rate_limit_reached_error","message":"Fixture temporary rate limit"}),
    ));
    play(&routes[1], "ses_balanceKimiRate", StatusCode::OK, &[1, 0]).await;
    assert_eq!(state(1), BalanceState::Open);
    let limited = cooldown(&scope(1)).unwrap();
    assert_eq!(limited.1, "upstream_rate_limit");
    play(
        &fixed[1],
        "ses_balanceFixedRateCooling",
        StatusCode::TOO_MANY_REQUESTS,
        &[],
    )
    .await;
    assert_eq!(cooldown(&scope(1)).unwrap(), limited);
    after_cooldown(limited.0).await;
    play(
        &fixed[1],
        "ses_balanceFixedRateProbe",
        StatusCode::TOO_MANY_REQUESTS,
        &[1],
    )
    .await;
    let renewed = cooldown(&scope(1)).unwrap();
    assert_eq!(renewed.1, "upstream_rate_limit");
    assert!(renewed.0 > limited.0);
    play(
        &routes[1],
        "ses_balanceManagedRateCooling",
        StatusCode::OK,
        &[0],
    )
    .await;
    after_cooldown(renewed.0).await;

    errors.lock().unwrap()[1] = Some((
        StatusCode::TOO_MANY_REQUESTS,
        None,
        json!({"type":"exceeded_current_quota_error","message":"Fixture quota rejection"}),
    ));
    play(&routes[1], "ses_balanceKimiQuota", StatusCode::OK, &[1, 0]).await;
    assert_eq!(state(1), BalanceState::Exhausted);
    play(
        &fixed[1],
        "ses_balanceKimiFixedHold",
        StatusCode::TOO_MANY_REQUESTS,
        &[],
    )
    .await;
    refresh(1);
    play(
        &routes[1],
        "ses_balanceKimiFailedProbe",
        StatusCode::OK,
        &[1, 0],
    )
    .await;
    assert_eq!(state(1), BalanceState::Exhausted);
    play(&routes[1], "ses_balanceKimiStillHeld", StatusCode::OK, &[0]).await;
}
