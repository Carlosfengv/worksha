use super::*;
use axum::{Router, http::HeaderMap, response::IntoResponse, routing::get};
use serde_json::json;
use std::sync::{Arc, Mutex};

fn input(endpoint: &str, protocol: &str) -> ModelsRequest {
    ModelsRequest {
        endpoint: endpoint.into(),
        protocol: protocol.into(),
        vendor: "custom".into(),
        credential_ref: None,
    }
}

#[test]
fn discovery_preserves_custom_prefixes_and_bounds_deepseek_rewrite() {
    for (base, protocol, expected) in [
        (
            "https://models.invalid/proxy/v1",
            "responses",
            "https://models.invalid/proxy/v1/models",
        ),
        (
            "https://models.invalid/proxy",
            "messages",
            "https://models.invalid/proxy/v1/models",
        ),
        (
            "http://localhost:11434/v1",
            "chat-completions",
            "http://localhost:11434/v1/models",
        ),
        (
            "https://models.invalid/anthropic",
            "messages",
            "https://models.invalid/anthropic/v1/models",
        ),
    ] {
        assert_eq!(
            model_url(&input(base, protocol)).unwrap().as_str(),
            expected
        );
    }
    let mut deepseek = input("https://api.deepseek.com/anthropic", "messages");
    deepseek.vendor = "deepseek".into();
    assert_eq!(
        model_url(&deepseek).unwrap().as_str(),
        "https://api.deepseek.com/models"
    );
    for endpoint in [
        "http://remote.invalid/v1",
        "https://key@models.invalid",
        "https://models.invalid/?key=secret",
        "file:///tmp/models",
        "https://models.invalid/#fragment",
    ] {
        assert!(model_url(&input(endpoint, "responses")).is_err());
    }
}

#[test]
fn catalog_validates_deduplicates_bounds_and_never_echoes_key() {
    let mut models = BTreeMap::new();
    read_models(
        &json!({"data":[{"id":"b","name":"Model B"},{"id":"a"},{"id":"b","display_name":"B"}]}),
        &mut models,
        None,
    )
    .unwrap();
    assert_eq!(models.len(), 2);
    assert_eq!(models["b"].name, "B");
    for body in [
        json!({"error":"secret-fixture"}),
        json!({"data":[{}]}),
        json!({"data":[{"id":"a\nb"}]}),
        json!({"data":[{"id":"secret-fixture"}]}),
    ] {
        let error = read_models(&body, &mut models, Some("secret-fixture")).unwrap_err();
        assert!(!error.message.contains("secret-fixture"));
    }
    let body =
        json!({"data":(0..2001).map(|i|json!({"id":format!("model-{i}")})).collect::<Vec<_>>()});
    models.clear();
    assert!(read_models(&body, &mut models, None).unwrap());
    assert_eq!(models.len(), MAX_MODELS);
    models.clear();
    read_models(
        &json!({"data":[{"id":"ok","name":"secret-fixture"}]}),
        &mut models,
        Some("secret-fixture"),
    )
    .unwrap();
    assert_eq!(models["ok"].name, "ok");
}

#[tokio::test]
async fn discovery_get_only_auth_pagination_errors_and_no_redirect_follow() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let state = seen.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new()
                .route(
                    "/v1/models",
                    get(move |headers: HeaderMap, uri: axum::http::Uri| {
                        let state = state.clone();
                        async move {
                            state.lock().unwrap().push((headers.clone(), uri.clone()));
                            if uri.query().is_none() {
                                Json(json!({"data":[{"id":"one"}],"has_more":true,"last_id":"one"}))
                            } else {
                                Json(json!({"data":[{"id":"two"}],"has_more":false}))
                            }
                        }
                    }),
                )
                .route(
                    "/denied/models",
                    get(|| async { (axum::http::StatusCode::UNAUTHORIZED, "secret-fixture") }),
                )
                .route(
                    "/redirect/models",
                    get(|| async {
                        axum::response::Redirect::temporary("http://must-not-follow.invalid")
                            .into_response()
                    }),
                )
                .route("/bad/models", get(|| async { "secret-fixture" }))
                .route("/empty/models", get(|| async { Json(json!({"data":[]})) })),
        )
        .await
        .unwrap();
    });
    let client = crate::state::provider_client().unwrap();
    let catalog = fetch(
        &client,
        &input(&format!("{base}/v1"), "messages"),
        Some(Zeroizing::new(b"secret-fixture".to_vec())),
    )
    .await
    .unwrap();
    assert_eq!(catalog.models.len(), 2);
    assert!(!catalog.truncated);
    assert!(
        !serde_json::to_string(&catalog)
            .unwrap()
            .contains("secret-fixture")
    );
    {
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 2);
        assert_eq!(seen[0].0["authorization"], "Bearer secret-fixture");
        assert_eq!(seen[0].0["x-api-key"], "secret-fixture");
        assert_eq!(seen[0].0["anthropic-version"], "2023-06-01");
        assert_eq!(seen[1].1.query(), Some("after_id=one"));
    }
    for path in ["denied", "redirect", "bad"] {
        let error = fetch(
            &client,
            &input(&format!("{base}/{path}"), "responses"),
            None,
        )
        .await
        .err()
        .unwrap();
        assert!(!error.message.contains("secret-fixture"));
    }
    assert!(
        fetch(&client, &input(&format!("{base}/empty"), "responses"), None)
            .await
            .unwrap()
            .models
            .is_empty()
    );
    server.abort();
}

#[test]
fn metadata_preserves_documented_limits_support_and_unknown_without_echoing_payloads() {
    let mut models = BTreeMap::new();
    read_models(&json!({"data": [
        {"id":"router", "context_length":131072,
         "top_provider":{"max_completion_tokens":8192},
         "architecture":{"input_modalities":["text","image"]},
         "supported_parameters":["tools","reasoning","structured_outputs"],
         "description":"secret-fixture"},
        {"id":"claude", "max_input_tokens":200000, "max_tokens":64000,
         "capabilities":{"image_input":{"supported":false}, "thinking":{"supported":true}}},
        {"id":"unknown", "context_length":-1, "max_tokens":"secret-fixture",
         "capabilities":{"image_input":{"supported":"secret-fixture"}},
         "supported_parameters":[false], "architecture":{"input_modalities":[false]}},
        {"id":"text-only", "supported_parameters":[], "architecture":{"input_modalities":["text"]}}
    ]}), &mut models, Some("secret-fixture")).unwrap();
    let router = serde_json::to_value(&models["router"]).unwrap();
    assert_eq!(router["contextWindow"], 131072);
    assert_eq!(router["maxOutputTokens"], 8192);
    for field in ["tools", "vision", "reasoning", "structuredOutput"] {
        assert_eq!(router["capabilities"][field], true);
    }
    let claude = serde_json::to_value(&models["claude"]).unwrap();
    assert!(claude["contextWindow"].is_null());
    assert_eq!(claude["maxInputTokens"], 200000);
    assert_eq!(claude["capabilities"]["vision"], false);
    assert!(claude["capabilities"]["tools"].is_null());
    let unknown = serde_json::to_value(&models["unknown"]).unwrap();
    assert!(unknown["contextWindow"].is_null());
    assert!(unknown["maxOutputTokens"].is_null());
    assert!(unknown["capabilities"]["vision"].is_null());
    assert!(unknown["capabilities"]["tools"].is_null());
    assert_eq!(
        serde_json::to_value(&models["text-only"]).unwrap()["capabilities"]["tools"],
        false
    );
    assert!(
        !serde_json::to_string(&models)
            .unwrap()
            .contains("secret-fixture")
    );
}

#[test]
fn deepseek_metadata_accepts_declared_limits_modalities_and_effort_without_guessing_tools() {
    let mut models = BTreeMap::new();
    read_models(
        &json!({"data": [
            {"id":"deepseek-flash", "name":"DeepSeek Flash", "context_window":1048576,
             "max_output_tokens":393216, "input_modalities":["text","image"],
             "effort":{"supported_levels":["low","high","max"]}},
            {"id":"deepseek-v4-pro", "context_window":1048576, "max_output_tokens":393216,
             "input_modalities":["text"], "effort":{"supported_levels":["high"]}},
            {"id":"missing"},
            {"id":"malformed", "context_window":-2, "max_output_tokens":"secret-fixture",
             "input_modalities":[false], "effort":{"supported_levels":[]}}
        ]}),
        &mut models,
        Some("secret-fixture"),
    )
    .unwrap();
    let flash = serde_json::to_value(&models["deepseek-flash"]).unwrap();
    assert_eq!(flash["contextWindow"], 1048576);
    assert_eq!(flash["maxOutputTokens"], 393216);
    assert_eq!(flash["capabilities"]["vision"], true);
    assert_eq!(flash["capabilities"]["reasoning"], true);
    assert!(flash["capabilities"]["tools"].is_null());
    assert!(flash["capabilities"]["structuredOutput"].is_null());
    assert_eq!(
        serde_json::to_value(&models["deepseek-v4-pro"]).unwrap()["capabilities"]["vision"],
        false
    );
    for id in ["missing", "malformed"] {
        let value = serde_json::to_value(&models[id]).unwrap();
        assert!(value["contextWindow"].is_null());
        assert!(value["maxOutputTokens"].is_null());
        assert!(value["capabilities"]["vision"].is_null());
        assert!(value["capabilities"]["reasoning"].is_null());
    }
    assert!(
        !serde_json::to_string(&models)
            .unwrap()
            .contains("secret-fixture")
    );
}
