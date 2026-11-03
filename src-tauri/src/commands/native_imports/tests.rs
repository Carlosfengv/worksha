use super::*;
use crate::backend::test_server::serve;
use std::sync::Arc;
use tokio::sync::Notify;

const STORE: &str = "71a3e175-52cf-41da-8631-c1fb652bb2a7";
const NAMESPACE: &str = "6e183d0d-6268-4e1b-9abc-25d3e66f8249";
fn supported() -> Value {
    json!({"version":1,"storeId":STORE,"namespaceId":NAMESPACE,"namespaceState":"active","sequence":0,"operationLimit":4096,"pendingLimit":4,"pendingRequests":[]})
}
fn input() -> ImportRequest {
    serde_json::from_value(json!({"candidateId":"claude-fixture","expectedRevision":"a".repeat(64),"model":"model","operation":{"storeId":STORE,"namespaceId":NAMESPACE,"operationId":uuid::Uuid::new_v4().to_string(),"expectedSequence":0}})).unwrap()
}
async fn action(backend: &Backend, action: u8, input: ImportRequest) -> Result<Value, String> {
    if action == 0 {
        submit(backend, input).await
    } else {
        result(backend, input, action == 2).await
    }
}
#[tokio::test]
async fn weak_future_foreign_or_unverified_protocols_never_submit_import_actions() {
    let mut bad = vec![json!({})];
    for (field, value) in [
        ("version", json!(2)),
        ("storeId", json!(uuid::Uuid::new_v4().to_string())),
        ("namespaceId", json!(uuid::Uuid::new_v4().to_string())),
        ("namespaceState", json!("unverified")),
        ("pendingLimit", json!(5)),
        ("sequence", json!(4097)),
    ] {
        let mut protocol = supported();
        protocol[field] = value;
        bad.push(protocol);
    }
    for action_id in 0..3 {
        for protocol in &bad {
            let protocol = protocol.to_string();
            let server = serve(move |_| {
                let protocol = protocol.clone();
                async move { protocol }
            })
            .await;
            let backend = Backend::test_connection(server.endpoint.clone());
            assert!(action(&backend, action_id, input()).await.is_err());
            let requests = server.requests.lock().unwrap();
            assert_eq!(requests.len(), 1);
            assert_eq!(requests[0].path, "/api/provider-imports/protocol");
            assert!(requests[0].body.is_null());
        }
    }
}
#[tokio::test]
async fn current_protocol_keeps_original_request_and_query_does_not_resubmit() {
    for action_id in 0..3 {
        let server = serve(|r| async move {
            if r.path.ends_with("/protocol") {
                supported().to_string()
            } else {
                "{}".into()
            }
        })
        .await;
        let backend = Backend::test_connection(server.endpoint.clone());
        let input = input();
        let expected = serde_json::to_value(&input).unwrap();
        action(&backend, action_id, input).await.unwrap();
        let requests = server.requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert_eq!(
            requests[1].path,
            match action_id {
                0 => "/api/provider-imports",
                1 => "/api/provider-imports/operations/query",
                _ => "/api/provider-imports/operations/cancel",
            }
        );
        assert_eq!(requests[1].body, expected);
        assert_eq!(requests[1].method, "POST");
    }
}
#[tokio::test]
async fn reconnect_during_import_preflight_does_not_redirect_any_action() {
    for action_id in 0..3 {
        let entered = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let (hit, done) = (entered.clone(), release.clone());
        let a = serve(move |_| {
            let (hit, done) = (hit.clone(), done.clone());
            async move {
                hit.notify_one();
                done.notified().await;
                supported().to_string()
            }
        })
        .await;
        let b = serve(|_| async { "{}".into() }).await;
        let backend = Arc::new(Backend::test_connection(a.endpoint.clone()));
        let worker = backend.clone();
        let task = tokio::spawn(async move { action(&worker, action_id, input()).await });
        tokio::time::timeout(std::time::Duration::from_secs(4), entered.notified())
            .await
            .unwrap();
        backend.test_install_connection(b.endpoint.clone());
        release.notify_one();
        assert!(
            task.await
                .unwrap()
                .unwrap_err()
                .contains("backend_connection_changed")
        );
        assert_eq!(a.requests.lock().unwrap().len(), 1);
        assert!(b.requests.lock().unwrap().is_empty());
    }
}
#[tokio::test]
async fn preview_reads_protocol_and_candidates_on_one_context_and_invalid_request_sends_nothing() {
    let server = serve(|r| async move {
        if r.path.ends_with("/protocol") {
            supported().to_string()
        } else {
            "[]".into()
        }
    })
    .await;
    let backend = Backend::test_connection(server.endpoint.clone());
    let value = preview(&backend).await.unwrap();
    assert_eq!(value["protocol"]["storeId"], STORE);
    assert_eq!(value["candidates"], json!([]));
    assert_eq!(server.requests.lock().unwrap().len(), 2);
    let mut invalid = input();
    invalid.operation = None;
    assert!(submit(&backend, invalid).await.is_err());
    assert_eq!(server.requests.lock().unwrap().len(), 2);
}
