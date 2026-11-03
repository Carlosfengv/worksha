use super::{adapters::*, *};
use axum::{
    Router,
    http::{HeaderMap, Uri},
    routing::{get, post},
};
use local_agent_core::{domain::provider::ProviderAdapter, plugin::*};
use serde_json::json;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

struct Alternative {
    path: Option<&'static str>,
    invalid_metadata: bool,
}
impl BuiltinPlugin for Alternative {
    fn descriptor(&self) -> PluginDescriptor {
        PluginDescriptor {
            id: "test-provider",
            version: "1.0.0",
            provides: vec![CapabilityKey {
                name: "domain.provider-adapter",
                version: 1,
                instance: "responses",
            }],
            requires: vec![],
        }
    }
    fn prepare(&self) -> Result<PluginResources, PluginError> {
        Ok(PluginResources::default())
    }
}
impl Contribution for Alternative {
    fn protocol(&self) -> &'static str {
        "responses"
    }
}
impl ProviderAdapter for Alternative {
    fn catalog_request(
        &self,
        context: &d::CatalogContext,
    ) -> Result<d::CatalogRequest, ConfigError> {
        Ok(d::CatalogRequest {
            path: self
                .path
                .map(String::from)
                .unwrap_or_else(|| format!("{}/catalog", context.prefix())),
        })
    }
    fn parse_catalog(
        &self,
        body: &Value,
        emit: &mut dyn FnMut(d::CatalogModel) -> Result<(), ConfigError>,
    ) -> Result<d::CatalogContinuation, ConfigError> {
        d::parse_standard_catalog(
            &json!({"data":body["models"],"has_more":body["more"],"last_id":body["next"]}),
            &mut |mut model| {
                if self.invalid_metadata {
                    model.metadata.context_window = Some(1_000_000_001);
                }
                emit(model)
            },
        )
    }
    fn verification_protocol(&self) -> &'static str {
        "responses"
    }
}
static ALTERNATIVE: Alternative = Alternative {
    path: None,
    invalid_metadata: false,
};
static INVALID_METADATA: Alternative = Alternative {
    path: None,
    invalid_metadata: true,
};
static FOREIGN: Alternative = Alternative {
    path: Some("https://foreign.invalid/models"),
    invalid_metadata: false,
};
static ESCAPE: Alternative = Alternative {
    path: Some("/models"),
    invalid_metadata: false,
};
static TRAVERSAL: Alternative = Alternative {
    path: Some("/proxy/../models"),
    invalid_metadata: false,
};
static QUERY: Alternative = Alternative {
    path: Some("/proxy/models?credential=x"),
    invalid_metadata: false,
};

fn request(endpoint: &str) -> ModelsRequest {
    ModelsRequest {
        endpoint: endpoint.into(),
        protocol: "responses".into(),
        vendor: "custom".into(),
        credential_ref: None,
    }
}
fn provider(endpoint: &str) -> local_agent_core::Provider {
    serde_json::from_value(json!({"id":"custom-adapter","name":"Owned adapter test","vendor":"custom","endpoint":endpoint,"protocol":"responses","model":"model-one","configured":true,"category":"local"})).unwrap()
}

#[tokio::test]
async fn replacement_adapter_controls_actual_discovery_but_host_owns_auth_and_verification() {
    let gets = Arc::new(AtomicUsize::new(0));
    let posts = Arc::new(AtomicUsize::new(0));
    let seen_get = gets.clone();
    let seen_post = posts.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/proxy", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(listener, Router::new().route("/proxy/catalog", get(move |headers: HeaderMap, uri: Uri| {
            let seen = seen_get.clone(); async move {
                seen.fetch_add(1, Ordering::SeqCst);
                assert_eq!(headers["authorization"], "Bearer secret-fixture");
                assert!(!headers.contains_key("x-api-key"));
                assert_eq!(uri.query(), if uri.query().is_some() { Some("after_id=model-one") } else { None });
                Json(if uri.query().is_none() { json!({"models":[{"id":"model-one","display_name":"secret-fixture","context_window":128000,"capabilities":{"tool_use":{"supported":false}}}],"more":true,"next":"model-one"}) } else { json!({"models":[{"id":"model-two"}],"more":false}) })
            }
        })).route("/proxy/responses", post(move |Json(body): Json<Value>| {
            let seen = seen_post.clone(); async move {
                seen.fetch_add(1, Ordering::SeqCst);
                assert_eq!(body["max_output_tokens"], 16);
                assert_eq!(body["model"], "model-one");
                Json(json!({"id":"owned-response","object":"response","model":"model-one","status":"completed","output":[],"error":null}))
            }
        }))).await.unwrap();
    });
    let registry = Adapters::from_contributions(vec![&ALTERNATIVE]).unwrap();
    let client = crate::state::provider_client().unwrap();
    let result = fetch_detailed_with(
        &registry,
        &client,
        &request(&endpoint),
        Some(Zeroizing::new(b"secret-fixture".to_vec())),
    )
    .await
    .unwrap();
    let value = serde_json::to_value(&result).unwrap();
    assert_eq!(result.source_url, format!("{endpoint}/catalog"));
    assert_eq!(result.models.len(), 2);
    assert_eq!(value["models"][0]["name"], "model-one");
    assert_eq!(value["models"][0]["capabilities"]["tools"], false);
    assert_eq!(value["models"][1]["contextWindow"], Value::Null);
    assert!(!value.to_string().contains("secret-fixture"));
    assert_eq!(gets.load(Ordering::SeqCst), 2);
    assert_eq!(
        posts.load(Ordering::SeqCst),
        0,
        "catalog read must not imply inference verification"
    );
    crate::providers::probe_detailed_with(&registry, &client, &provider(&endpoint), None)
        .await
        .unwrap();
    assert_eq!(posts.load(Ordering::SeqCst), 1);
    let bad = Adapters::from_contributions(vec![&INVALID_METADATA]).unwrap();
    assert!(
        fetch_detailed_with(
            &bad,
            &client,
            &request(&endpoint),
            Some(Zeroizing::new(b"secret-fixture".to_vec()))
        )
        .await
        .is_err()
    );
    assert_eq!(posts.load(Ordering::SeqCst), 1);
    server.abort();
}

#[tokio::test]
async fn invalid_catalog_advice_and_stopped_adapters_never_send_or_fallback() {
    let received = Arc::new(AtomicUsize::new(0));
    let seen = received.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/proxy", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new().fallback(move || {
                let seen = seen.clone();
                async move {
                    seen.fetch_add(1, Ordering::SeqCst);
                    Json(json!({"data":[]}))
                }
            }),
        )
        .await
        .unwrap();
    });
    let client = crate::state::provider_client().unwrap();
    for adapter in [&FOREIGN, &ESCAPE, &TRAVERSAL, &QUERY] {
        let registry = Adapters::from_contributions(vec![adapter]).unwrap();
        assert!(
            fetch_detailed_with(&registry, &client, &request(&endpoint), None)
                .await
                .is_err()
        );
    }
    let registry = Adapters::from_contributions(vec![&ALTERNATIVE]).unwrap();
    registry.stop("test-provider");
    assert!(
        fetch_detailed_with(&registry, &client, &request(&endpoint), None)
            .await
            .is_err()
    );
    assert!(
        crate::providers::probe_detailed_with(&registry, &client, &provider(&endpoint), None)
            .await
            .is_err()
    );
    assert_eq!(received.load(Ordering::SeqCst), 0);
    assert!(Adapters::from_contributions(vec![&ALTERNATIVE, &ALTERNATIVE]).is_err());
    server.abort();
}

#[tokio::test]
async fn stopping_adapter_during_upstream_read_discards_result_and_next_page() {
    let received = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let seen = received.clone();
    let wait = release.clone();
    let count = Arc::new(AtomicUsize::new(0));
    let sent = count.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/v1", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new().route(
                "/v1/models",
                get(move || {
                    let seen = seen.clone();
                    let wait = wait.clone();
                    let count = sent.clone();
                    async move {
                        count.fetch_add(1, Ordering::SeqCst);
                        seen.notify_one();
                        wait.notified().await;
                        Json(json!({"data":[{"id":"late"}],"has_more":true,"last_id":"late"}))
                    }
                }),
            ),
        )
        .await
        .unwrap();
    });
    let registry = Arc::new(Adapters::builtin().unwrap());
    let task_registry = registry.clone();
    let task = tokio::spawn(async move {
        fetch_detailed_with(
            &task_registry,
            &crate::state::provider_client().unwrap(),
            &request(&endpoint),
            None,
        )
        .await
    });
    tokio::time::timeout(Duration::from_secs(3), received.notified())
        .await
        .unwrap();
    registry.stop("responses");
    release.notify_one();
    let result = tokio::time::timeout(Duration::from_secs(3), task)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result.err().unwrap().code, "models_unavailable");
    assert_eq!(count.load(Ordering::SeqCst), 1);
    assert!(registry.resolve("messages").is_ok());
    server.abort();
}
