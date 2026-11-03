use super::*;
use crate::backend::test_server::serve;
use std::sync::Arc;
use tokio::sync::Notify;

fn identity() -> IdentityV3 {
    IdentityV3 {
        store_id: uuid::Uuid::new_v4().to_string(),
        namespace_id: uuid::Uuid::new_v4().to_string(),
        operation_id: uuid::Uuid::new_v4().to_string(),
    }
}
fn capability(id: &IdentityV3) -> Value {
    json!({"version":2,"schemaVersion":15,"storeId":id.store_id,"namespaceId":id.namespace_id,"namespaceState":"active","operationLimit":4096,"pendingLimit":4,"cancellationLimit":4096,"pending":[]})
}
async fn submit(backend: &Backend, id: IdentityV3, kind: u8) -> Result<Value, String> {
    if kind < 2 {
        prepare_draft(
            backend,
            BindingDraft {
                agent_id: "claude".into(),
                expected_revision: "absent".into(),
                provider_id: Some("anthropic".into()),
                expected_provider_revision: Some(1),
                model: Some("fixture".into()),
                operation: Some(id),
            },
            kind == 1,
        )
        .await
    } else if kind == 2 {
        prepare_service(backend, json!({"agentId":"claude","expectedRevision":"absent","serviceId":uuid::Uuid::new_v4().to_string(),"expectedServiceRevision":1,"operation":id})).await
    } else {
        action(
            backend,
            id,
            ["query", "continue", "cancel"][(kind - 3) as usize],
        )
        .await
    }
}
#[tokio::test]
async fn numbered_commands_require_the_original_current_active_capability() {
    for kind in 0..6 {
        for invalid in 0..10 {
            let id = identity();
            let mut response = capability(&id);
            match invalid {
                0 => response["schemaVersion"] = json!(14),
                1 => response["schemaVersion"] = json!(16),
                2 => response["version"] = json!(1),
                3 => response["namespaceState"] = json!("unverified"),
                4 => response["storeId"] = json!(uuid::Uuid::new_v4().to_string()),
                5 => response["namespaceId"] = json!(uuid::Uuid::new_v4().to_string()),
                6 => response["pendingLimit"] = json!(5),
                7 => response["cancellationLimit"] = json!(0),
                8 => response["version"] = json!(3),
                _ => {
                    response
                        .as_object_mut()
                        .unwrap()
                        .remove("cancellationLimit");
                }
            }
            let server = serve(move |_| {
                let response = response.clone();
                async move { response.to_string() }
            })
            .await;
            let backend = Backend::test_connection(server.endpoint.clone());
            assert!(
                submit(&backend, id, kind)
                    .await
                    .unwrap_err()
                    .contains("local_token_protocol_unavailable")
            );
            let requests = server.requests.lock().unwrap();
            assert_eq!(requests.len(), 1);
            assert_eq!(requests[0].path, "/api/local-tokens/protocol");
            assert!(requests[0].body.is_null());
        }
    }
}
#[tokio::test]
async fn reconnect_while_admitting_cannot_forward_a_numbered_request() {
    for kind in 0..6 {
        let id = identity();
        let response = capability(&id);
        let entered = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let (hit, done) = (entered.clone(), release.clone());
        let original = serve(move |_| {
            let (hit, done, response) = (hit.clone(), done.clone(), response.clone());
            async move {
                hit.notify_one();
                done.notified().await;
                response.to_string()
            }
        })
        .await;
        let other = serve(|_| async { "{}".into() }).await;
        let backend = Arc::new(Backend::test_connection(original.endpoint.clone()));
        let worker = backend.clone();
        let job = tokio::spawn(async move { submit(&worker, id, kind).await });
        tokio::time::timeout(std::time::Duration::from_secs(4), entered.notified())
            .await
            .unwrap();
        backend.test_install_connection(other.endpoint.clone());
        release.notify_one();
        assert!(
            job.await
                .unwrap()
                .unwrap_err()
                .contains("backend_connection_changed")
        );
        assert_eq!(original.requests.lock().unwrap().len(), 1);
        assert!(other.requests.lock().unwrap().is_empty());
    }
}
#[tokio::test]
async fn valid_capability_preserves_the_three_existing_payloads_and_recovery_identity() {
    for kind in 0..6 {
        let id = identity();
        let response = capability(&id);
        let server = serve(move |r| {
            let response = response.clone();
            async move {
                if r.path == "/api/local-tokens/protocol" {
                    response.to_string()
                } else {
                    "{}".into()
                }
            }
        })
        .await;
        let backend = Backend::test_connection(server.endpoint.clone());
        submit(&backend, id.clone(), kind).await.unwrap();
        let requests = server.requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert_eq!(
            requests[1].path,
            [
                "/api/provider-switches",
                "/api/dynamic-connections",
                "/api/model-service-connections",
                "/api/local-tokens/query",
                "/api/local-tokens/continue",
                "/api/local-tokens/cancel"
            ][kind as usize]
        );
        assert_eq!(
            if kind < 3 {
                &requests[1].body["operation"]
            } else {
                &requests[1].body
            },
            &serde_json::to_value(&id).unwrap()
        );
    }
}
