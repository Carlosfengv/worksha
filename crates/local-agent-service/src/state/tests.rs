use axum::{
    Json, Router,
    extract::{ConnectInfo, Request, State},
    http::{StatusCode, header::LOCATION},
    response::IntoResponse,
};
use local_agent_core::Provider;
use serde_json::json;
use std::{
    collections::HashSet,
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::Instant,
};
use zeroize::Zeroizing;

struct Seen {
    peer: SocketAddr,
    path: String,
    authorization: Option<String>,
    api_key: Option<String>,
}
type Requests = Arc<Mutex<Vec<Seen>>>;

async fn upstream(
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    State(seen): State<Requests>,
    request: Request,
) -> axum::response::Response {
    let path = request.uri().path().to_owned();
    seen.lock().unwrap().push(Seen {
        peer,
        path: path.clone(),
        authorization: request
            .headers()
            .get("authorization")
            .map(|v| v.to_str().unwrap().into()),
        api_key: request
            .headers()
            .get("x-api-key")
            .map(|v| v.to_str().unwrap().into()),
    });
    axum::body::to_bytes(request.into_body(), 1024 * 1024)
        .await
        .unwrap();
    if path.starts_with("/redirect") {
        return (StatusCode::TEMPORARY_REDIRECT, [(LOCATION, "/trap")]).into_response();
    }
    if path.ends_with("/models") {
        Json(json!({"data":[{"id":"fixture-model"}]})).into_response()
    } else if path.ends_with("/chat/completions") {
        Json(json!({"id":"fixture","model":"fixture-model","object":"chat.completion","choices":[{"message":{"role":"assistant","content":"OK"}}]})).into_response()
    } else {
        Json(json!({"data":{"usage_monthly":0,"usage":3,"limit_remaining":0}})).into_response()
    }
}

#[tokio::test]
async fn pooled_checks_reuse_connections_without_sharing_credentials_or_billing_policy() {
    let seen: Requests = Arc::new(Mutex::new(vec![]));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let app = Router::new().fallback(upstream).with_state(seen.clone());
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });
    let client = super::provider_client().unwrap();
    let mut models = crate::provider_models::ModelsRequest {
        endpoint: format!("{base}/v1"),
        protocol: "messages".into(),
        vendor: "custom".into(),
        credential_ref: None,
    };
    let provider = Provider {
        id: "custom-fixture".into(),
        name: "Fixture".into(),
        vendor: "custom".into(),
        endpoint: format!("{base}/v1"),
        protocol: "chat-completions".into(),
        model: "fixture-model".into(),
        configured: true,
        credential_ref: None,
        credential_label: None,
        models: vec![],
        disabled_models: vec![],
        model_catalog: None,
        revision: 1,
        category: "custom".into(),
    };
    let pooled_started = Instant::now();
    crate::provider_models::fetch_detailed(
        &client,
        &models,
        Some(Zeroizing::new(b"catalog-fixture-key".to_vec())),
    )
    .await
    .unwrap();
    crate::providers::probe_detailed(
        &client.clone(),
        &provider,
        Some(Zeroizing::new(b"probe-fixture-key".to_vec())),
    )
    .await
    .unwrap();
    models.protocol = "chat-completions".into();
    crate::provider_models::fetch_detailed(&client, &models, None)
        .await
        .unwrap();
    let pooled_micros = pooled_started.elapsed().as_micros();

    let accounts = crate::billing::sources::AccountSources::builtin().unwrap();
    for key in ["billing-first-fixture", "billing-second-fixture"] {
        accounts
            .api("openrouter", &format!("{base}/billing"), key, None)
            .await
            .unwrap();
    }
    let fresh_started = Instant::now();
    models.protocol = "messages".into();
    crate::provider_models::fetch_detailed(
        &super::provider_client().unwrap(),
        &models,
        Some(Zeroizing::new(b"catalog-fixture-key".to_vec())),
    )
    .await
    .unwrap();
    crate::providers::probe_detailed(
        &super::provider_client().unwrap(),
        &provider,
        Some(Zeroizing::new(b"probe-fixture-key".to_vec())),
    )
    .await
    .unwrap();
    models.protocol = "chat-completions".into();
    crate::provider_models::fetch_detailed(&super::provider_client().unwrap(), &models, None)
        .await
        .unwrap();
    let fresh_micros = fresh_started.elapsed().as_micros();
    {
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 8);
        assert_eq!(
            seen[..3]
                .iter()
                .map(|r| r.peer)
                .collect::<HashSet<_>>()
                .len(),
            1
        );
        assert_eq!(
            seen[5..]
                .iter()
                .map(|r| r.peer)
                .collect::<HashSet<_>>()
                .len(),
            3
        );
        assert_eq!(
            seen[0].authorization.as_deref(),
            Some("Bearer catalog-fixture-key")
        );
        assert_eq!(seen[0].api_key.as_deref(), Some("catalog-fixture-key"));
        assert_eq!(
            seen[1].authorization.as_deref(),
            Some("Bearer probe-fixture-key")
        );
        assert!(seen[1].api_key.is_none());
        assert!(seen[2].authorization.is_none() && seen[2].api_key.is_none());
        assert_eq!(seen[3].peer, seen[4].peer);
        assert_ne!(seen[0].peer, seen[3].peer);
        assert_eq!(
            seen[3].authorization.as_deref(),
            Some("Bearer billing-first-fixture")
        );
        assert_eq!(
            seen[4].authorization.as_deref(),
            Some("Bearer billing-second-fixture")
        );
        assert!(seen[3..5].iter().all(|r| r.api_key.is_none()));
        for (pooled, fresh) in seen[..3].iter().zip(&seen[5..]) {
            assert_eq!(pooled.path, fresh.path);
            assert_eq!(pooled.authorization, fresh.authorization);
            assert_eq!(pooled.api_key, fresh.api_key);
        }
    }
    models.endpoint = format!("{base}/redirect/v1");
    assert_eq!(
        crate::provider_models::fetch_detailed(&client, &models, None)
            .await
            .err()
            .unwrap()
            .code,
        "catalog_redirect"
    );
    assert!(
        accounts
            .api(
                "openrouter",
                &format!("{base}/redirect"),
                "billing-fixture",
                None
            )
            .await
            .is_err()
    );
    assert!(seen.lock().unwrap().iter().all(|r| r.path != "/trap"));
    println!(
        "{}",
        json!({"fixture":"local-http1-only", "providerRequests":3,"pooledConnections":1,"freshConnections":3,"billingRequests":2,"billingConnections":1,"separatePools":true,"credentialIsolation":true,"pooledSequenceMicros":pooled_micros,"freshSequenceMicros":fresh_micros,"noLatencyImprovementClaim":true,"realProviderCalls":0})
    );
    server.abort();
}
