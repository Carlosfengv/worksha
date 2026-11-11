use super::*;
use local_agent_core::providers::upstream_accounts::AccountEdit;

async fn accounting_fixture(
    reply: impl Fn(&str, serde_json::Value) -> Response + Copy + Send + Sync + 'static,
    mut policy: serde_json::Value,
) -> (Fixture, String, Arc<Mutex<Vec<&'static str>>>) {
    let (listener, client) = super::minimax::tls_upstream(
        "openrouter.ai",
        include_bytes!("fixtures/quota-fixture-cert.der"),
    )
    .await;
    let vault = Arc::new(MemoryVault::default());
    let mut fixture = Fixture::with_upstream(vault.clone(), Some(client)).await;
    let seen = Arc::new(Mutex::new(vec![]));
    let router = axum::Router::new().route(
        "/api/v1/chat/completions",
        axum::routing::post({
            let seen = seen.clone();
            move |headers: axum::http::HeaderMap,
                  axum::Json(body): axum::Json<serde_json::Value>| {
                let seen = seen.clone();
                async move {
                    let key = match headers["authorization"].to_str().unwrap() {
                        "Bearer charged-fixture-key" => "charged",
                        "Bearer incomplete-fixture-key" => "incomplete",
                        "Bearer fallback-fixture-key" => "fallback",
                        _ => panic!("Unexpected accounting fixture credential"),
                    };
                    seen.lock().unwrap().push(key);
                    reply(key, body)
                }
            }
        }),
    );
    fixture.tasks.push(tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap()
    }));
    let local_key = uuid::Uuid::new_v4().to_string();
    vault.set(&local_key, b"accounting-local-fixture").unwrap();
    let binding = {
        let store = fixture.state.store.lock().unwrap();
        let mut provider = store.provider_by_id("openrouter").unwrap().unwrap();
        provider.model = "accounting-fixture".into();
        provider.models = vec![provider.model.clone()];
        provider.credential_ref = Some(uuid::Uuid::new_v4().to_string());
        store.save_provider(provider).unwrap();
        let provider = store.provider_by_id("openrouter").unwrap().unwrap();
        let accounts = ["charged", "incomplete", "fallback"].map(|label| {
            let key = uuid::Uuid::new_v4().to_string();
            vault
                .set(&key, format!("{label}-fixture-key").as_bytes())
                .unwrap();
            store
                .save_upstream_account(
                    &provider,
                    &AccountEdit {
                        id: uuid::Uuid::new_v4().to_string(),
                        expected_revision: 0,
                        provider_revision: provider.revision,
                        label: label.into(),
                        credential_ref: key,
                    },
                )
                .unwrap()
                .reference()
        });
        let model = uuid::Uuid::new_v4().to_string();
        policy["accountPools"] = json!([{"modelId":model,"accounts":accounts}]);
        let service = store.save_model_service(serde_json::from_value(json!({
            "id":uuid::Uuid::new_v4().to_string(),"revision":0,"name":"Accounting fixture","defaultModelId":model,
            "models":[{"id":model,"name":"Accounting model","providerId":provider.id,"model":provider.model,
                "capabilities":{"contextWindow":32000,"maxOutputTokens":4096,"tools":true,"thinking":false,"inputModalities":["text"]}}],
            "routingPolicy":policy
        })).unwrap()).unwrap();
        store
            .create_model_service_binding("opencode", "accounting-fixture", &local_key, &service)
            .unwrap()
            .0
    };
    let endpoint = crate::gateway::dynamic::configured_endpoint(&fixture.state, &binding);
    (fixture, format!("{endpoint}/messages"), seen)
}

#[tokio::test]
async fn fallback_attributes_reported_error_cost_without_counting_incomplete_evidence() {
    let (fixture, endpoint, seen) = accounting_fixture(
        |key, body| {
            let error = |cost| json!({
                "error":{"type":"server_error","message":"Fixture upstream failure"},
                "usage":{"prompt_tokens":10,"completion_tokens":2,"cost":cost}
            });
            match key {
                "charged" => (StatusCode::BAD_GATEWAY, axum::Json(error(0.002))).into_response(),
                "incomplete" => {
                    let prefix = Bytes::from(serde_json::to_vec(&error(9.99)).unwrap());
                    let body = Body::from_stream(async_stream::stream! {
                        yield Ok::<_, std::io::Error>(prefix);
                        tokio::time::sleep(Duration::from_millis(30)).await;
                        yield Ok(Bytes::from(vec![b' '; 80 * 1024]));
                    });
                    (StatusCode::BAD_GATEWAY, [("content-type", "application/json")], body).into_response()
                }
                _ => axum::Json(json!({
                    "id":"fixture-accounting", "object":"chat.completion", "model":body["model"],
                    "choices":[{"index":0,"message":{"role":"assistant","content":"OK"},"finish_reason":"stop"}],
                    "usage":{"prompt_tokens":5,"completion_tokens":2,"total_tokens":7}
                })).into_response(),
            }
        },
        json!({"enabled":true,"maxAttempts":3}),
    )
    .await;
    let response = fixture
        .client
        .post(endpoint)
        .bearer_auth("accounting-local-fixture")
        .header("x-opencode-session", "ses_accountingfixture")
        .json(
            &json!({"model":local_agent_core::routing::ALIAS,"max_tokens":30,
            "messages":[{"role":"user","content":"Check attempt accounting"}]}),
        )
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    assert!(response.text().await.unwrap().contains("OK"));
    assert_eq!(*seen.lock().unwrap(), ["charged", "incomplete", "fallback"]);
    let detail = fixture
        .detail_when("ses_accountingfixture", |detail| {
            detail["request"]["status"] == "recovered"
                && detail["request"]["attempts"]
                    .as_array()
                    .is_some_and(|a| a.len() == 3)
        })
        .await;
    let request = &detail["request"];
    let attempts = request["attempts"].as_array().unwrap();
    assert_eq!(
        attempts[0]["routing"]["service"]["upstreamAccountLabel"],
        "charged"
    );
    assert_eq!(attempts[0]["httpStatus"], 502);
    assert_eq!(attempts[0]["usage"]["totalTokens"], 12);
    assert_eq!(attempts[0]["cost"]["kind"], "actual");
    assert_eq!(attempts[0]["cost"]["amount"], 0.002);
    assert_eq!(
        attempts[1]["routing"]["service"]["upstreamAccountLabel"],
        "incomplete"
    );
    assert_eq!(attempts[1]["httpStatus"], 502);
    assert!(attempts[1]["usage"]["totalTokens"].is_null());
    assert!(attempts[1]["cost"]["amount"].is_null());
    assert_eq!(attempts[2]["usage"]["totalTokens"], 7);
    assert_eq!(attempts[2]["cost"]["kind"], "unknown");
    assert_eq!(
        request["cost"]["actual"],
        json!([{"currency":"USD","amount":0.002,"calls":1}])
    );
    assert_eq!(request["cost"]["unknownCalls"], 2);
    assert_eq!(request["usage"]["totalTokens"], 19);
    assert_eq!(request["usageMissing"], 1);
}

#[tokio::test]
async fn openrouter_embedded_rejections_respect_the_policy_and_keep_attempt_costs() {
    for (stream, allow_rate, expected) in
        [(true, false, 429), (true, true, 200), (false, true, 403)]
    {
        let (fixture, endpoint, seen) = accounting_fixture(
            |key, body| {
                let usage = json!({"prompt_tokens":10,"completion_tokens":2,"cost":0.002});
                if body["stream"] == true {
                    let result = if key == "charged" {
                        json!({"id":"gen-fixture-rejected","object":"chat.completion.chunk",
                            "error":{"code":429,"message":"Fixture throttle","metadata":{"error_type":"rate_limit_exceeded"}},
                            "choices":[{"index":0,"delta":{"content":""},"finish_reason":"error"}],"usage":usage})
                    } else {
                        json!({"id":"gen-fixture-backup","model":body["model"],
                            "choices":[{"index":0,"delta":{"content":"OK"},"finish_reason":"stop"}],
                            "usage":{"prompt_tokens":5,"completion_tokens":2,"cost":0.001}})
                    };
                    return (
                        [("content-type", "text/event-stream"), ("retry-after", "17")],
                        format!("data: {result}\n\ndata: [DONE]\n\n"),
                    ).into_response();
                }
                axum::Json(json!({"id":"gen-fixture-policy","object":"chat.completion",
                    "choices":[{"index":0,"finish_reason":"error","message":{"content":"UNDELIVERED"},
                        "error":{"code":403,"message":"Fixture filter","metadata":{"error_type":"content_policy_violation"}}}],
                    "usage":usage})).into_response()
            },
            json!({"enabled":true,"maxAttempts":3,"fallbackOnFailure":!allow_rate,"fallbackOnRateLimit":allow_rate,"fallbackOnAccessDenied":true}),
        ).await;
        let response = fixture
            .client
            .post(endpoint)
            .bearer_auth("accounting-local-fixture")
            .header("x-opencode-session", "ses_embeddedreply")
            .json(
                &json!({"model":local_agent_core::routing::ALIAS,"max_tokens":30,"stream":stream,
                "messages":[{"role":"user","content":"Check embedded provider reply"}]}),
            )
            .send()
            .await
            .unwrap();
        let status = response.status().as_u16();
        let retry_after = response
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        let body = response.text().await.unwrap();
        assert_eq!(status, expected, "{body}");
        if expected == 429 {
            assert_eq!(retry_after.as_deref(), Some("17"));
        }
        assert!(!body.contains("UNDELIVERED"));
        let wanted = if expected == 200 {
            vec!["charged", "incomplete"]
        } else {
            vec!["charged"]
        };
        assert_eq!(*seen.lock().unwrap(), wanted);
        let detail = fixture
            .detail_when("ses_embeddedreply", |detail| {
                detail["request"]["attempts"]
                    .as_array()
                    .is_some_and(|a| a.len() == wanted.len())
                    && detail["request"]["status"] != "in_progress"
                    && detail["routeTrace"].is_object()
            })
            .await;
        let request = &detail["request"];
        let first = &request["attempts"][0];
        assert_eq!(
            first["routing"]["service"]["upstreamAccountLabel"],
            "charged"
        );
        assert_eq!(first["status"], "error");
        assert_eq!(first["httpStatus"], 200);
        assert_eq!(detail["routeTrace"]["httpStatus"], expected);
        assert_eq!(first["usage"]["totalTokens"], 12);
        assert_eq!(first["cost"]["amount"], 0.002);
        let (tokens, cost) = if expected == 200 {
            let backup = &request["attempts"][1];
            assert_eq!(
                backup["routing"]["service"]["fallbackReason"],
                "upstream_rate_limit"
            );
            assert_eq!(backup["usage"]["totalTokens"], 7);
            assert_eq!(backup["cost"]["amount"], 0.001);
            (19, 0.003)
        } else {
            (12, 0.002)
        };
        assert_eq!(request["usage"]["totalTokens"], tokens);
        assert_eq!(request["cost"]["actual"][0]["amount"], cost);
    }
}

#[tokio::test]
async fn untrusted_retry_after_uses_published_cooldown_across_wire_paths() {
    let mut wrong_cooldowns = Vec::new();
    for (wire, duplicate) in [
        ("http", true),
        ("json", true),
        ("sse", true),
        ("json", false),
        ("sse", false),
    ] {
        let (fixture, endpoint, seen) = accounting_fixture(
            move |key, body| {
                if key != "charged" {
                    let mut result = json!({"id":"fixture-backup","model":body["model"],
                        "choices":[{"index":0,"message":{"role":"assistant","content":"OK"},"finish_reason":"stop"}],
                        "usage":{"prompt_tokens":5,"completion_tokens":2,"total_tokens":7}});
                    if body["stream"] == true {
                        result["choices"][0]["delta"] = json!({"content":"OK"});
                        return (
                            [("content-type", "text/event-stream")],
                            format!("data: {result}\n\ndata: [DONE]\n\n"),
                        )
                            .into_response();
                    }
                    return axum::Json(result).into_response();
                }
                let error = json!({"error":{"code":429,"message":"Fixture rate limit"}});
                let mut response = match wire {
                    "http" => (StatusCode::TOO_MANY_REQUESTS, axum::Json(error)).into_response(),
                    "json" => axum::Json(error).into_response(),
                    "sse" => (
                        [("content-type", "text/event-stream")],
                        format!("data: {error}\n\n"),
                    )
                        .into_response(),
                    _ => unreachable!(),
                };
                if duplicate {
                    response.headers_mut().append("retry-after", HeaderValue::from_static("1"));
                    response.headers_mut().append("retry-after", HeaderValue::from_static("3600"));
                }
                response
            },
            json!({"enabled":true,"maxAttempts":2,"fallbackOnRateLimit":true,"cooldownSeconds":43}),
        )
        .await;
        let before = chrono::Utc::now().timestamp_millis();
        let response = fixture
            .client
            .post(endpoint)
            .bearer_auth("accounting-local-fixture")
            .header("x-opencode-session", "ses_retryafterfixture")
            .json(
                &json!({"model":local_agent_core::routing::ALIAS,"max_tokens":30,"stream":wire == "sse",
                "messages":[{"role":"user","content":"Retry delay fixture"}]}),
            )
            .send()
            .await
            .unwrap();
        let status = response.status();
        let body = response.text().await.unwrap();
        assert_eq!(status, StatusCode::OK, "{wire}: {body}");
        assert!(body.contains("OK"), "{wire}: {body}");
        assert_eq!(*seen.lock().unwrap(), ["charged", "incomplete"], "{wire}");
        let after = chrono::Utc::now().timestamp_millis();
        let cooldowns = fixture
            .state
            .store
            .lock()
            .unwrap()
            .service_cooldowns()
            .unwrap();
        assert_eq!(cooldowns.len(), 1);
        let (until, reason) = cooldowns.into_values().next().unwrap();
        assert_eq!(reason, "upstream_rate_limit");
        if !(before + 43_000..=after + 43_000).contains(&until) {
            let kind = if duplicate { "duplicate" } else { "absent" };
            wrong_cooldowns.push(format!("{wire} / {kind}: {} ms", until - before));
        }
    }
    assert!(
        wrong_cooldowns.is_empty(),
        "Expected 43 seconds: {wrong_cooldowns:?}"
    );
}
