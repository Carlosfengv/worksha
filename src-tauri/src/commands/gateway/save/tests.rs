use super::*;
use crate::backend::{Backend, test_server::serve};
use std::{os::unix::fs::PermissionsExt, sync::Arc};
use tokio::sync::Notify;
const ID: &str = "e01ea7bb-1b3f-417c-8f74-43b8b79d4c87";
fn key() -> String {
    format!("la_{}", "1".repeat(64))
}
fn input() -> Value {
    json!({"name":"fixture","agent":"claude","providerId":"fixture","models":["fixture-model"],"concurrency":1})
}
fn directory() -> tempfile::TempDir {
    tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap()
}
async fn held<T>(pending: T) -> T::Output
where
    T: std::future::Future,
{
    tokio::time::timeout(std::time::Duration::from_secs(4), pending)
        .await
        .unwrap()
}

#[tokio::test]
async fn cancelled_selection_never_issues_or_queries_even_after_reconnect() {
    let a = serve(|_| async { "{}".into() }).await;
    let b = serve(|_| async { "{}".into() }).await;
    let backend = Backend::test_connection(a.endpoint.clone());
    let context = backend.context().unwrap();
    backend.test_install_connection(b.endpoint.clone());
    assert_eq!(
        create_at(&context, input(), None).await.unwrap()["status"],
        "cancelled"
    );
    assert!(a.requests.lock().unwrap().is_empty());
    assert!(b.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_context_captured_before_selection_cannot_issue_after_reconnect() {
    let a = serve(|_| async { "{}".into() }).await;
    let b = serve(|_| async { "{}".into() }).await;
    let backend = Backend::test_connection(a.endpoint.clone());
    let context = backend.context().unwrap();
    let dir = directory();
    let target = dir.path().join("client.json");
    backend.test_install_connection(b.endpoint.clone());
    let error = create_at(&context, input(), Some(target.clone()))
        .await
        .unwrap_err();
    assert!(error.contains("backend_connection_changed"));
    assert!(!target.exists());
    assert!(a.requests.lock().unwrap().is_empty());
    assert!(b.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn an_in_flight_issue_keeps_the_original_listen_key_and_confirmed_result_after_reconnect() {
    let entered = Arc::new(Notify::new());
    let released = Arc::new(Notify::new());
    let (hit, done) = (entered.clone(), released.clone());
    let a = serve(move |r| {
        let (hit, done) = (hit.clone(), done.clone());
        async move {
            if r.path.ends_with("/clients") {
                hit.notify_one();
                held(done.notified()).await;
                json!({"id":ID,"key":key()}).to_string()
            } else {
                json!({"listen":"127.0.0.1:18901"}).to_string()
            }
        }
    })
    .await;
    let b = serve(|_| async { json!({"listen":"127.0.0.1:18902"}).to_string() }).await;
    let backend = Backend::test_connection(a.endpoint.clone());
    let context = backend.context().unwrap();
    let origin = context.instance_id().to_owned();
    let dir = directory();
    let target = dir.path().join("client.json");
    let copy = target.clone();
    let task = tokio::spawn(async move { create_at(&context, input(), Some(copy)).await });
    held(entered.notified()).await;
    backend.test_install_connection(b.endpoint.clone());
    released.notify_one();
    let receipt = task.await.unwrap().unwrap();
    assert_eq!(receipt["status"], "saved");
    assert_eq!(receipt["sourceInstanceId"], origin);
    assert_eq!(receipt["sourceCurrent"], false);
    assert!(!receipt.to_string().contains(&key()));
    assert!(!receipt.to_string().contains("export-fixture-token"));
    let content: Value = serde_json::from_slice(&std::fs::read(&target).unwrap()).unwrap();
    assert_eq!(content["baseUrl"], "http://127.0.0.1:18901/v1");
    assert_eq!(content["apiKey"], key());
    assert_eq!(content["clientId"], ID);
    assert_eq!(
        std::fs::metadata(&target).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let requests = a.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[1].method, "POST");
    assert_eq!(requests[1].body, input());
    assert!(
        requests
            .iter()
            .all(|r| r.authorization == "Bearer export-fixture-token")
    );
    assert!(b.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn save_failure_compensates_the_original_backend_and_requires_an_explicit_revoke_acknowledgement()
 {
    for acknowledged in [true, false] {
        let entered = Arc::new(Notify::new());
        let released = Arc::new(Notify::new());
        let (hit, done) = (entered.clone(), released.clone());
        let a = serve(move |r| {
            let (hit, done) = (hit.clone(), done.clone());
            async move {
                if r.path.ends_with("/clients") {
                    hit.notify_one();
                    held(done.notified()).await;
                    json!({"id":ID,"key":key()}).to_string()
                } else if r.path.ends_with("/revoke") {
                    json!({"revoked":acknowledged}).to_string()
                } else {
                    json!({"listen":"127.0.0.1:18901"}).to_string()
                }
            }
        })
        .await;
        let b = serve(|_| async { "{\"revoked\":true}".into() }).await;
        let backend = Backend::test_connection(a.endpoint.clone());
        let context = backend.context().unwrap();
        let dir = directory();
        let target = dir.path().join("existing-directory");
        std::fs::create_dir(&target).unwrap();
        let copy = target.clone();
        let task = tokio::spawn(async move { create_at(&context, input(), Some(copy)).await });
        held(entered.notified()).await;
        backend.test_install_connection(b.endpoint.clone());
        released.notify_one();
        let error: Value = serde_json::from_str(&task.await.unwrap().unwrap_err()).unwrap();
        assert_eq!(error["outcomeUnknown"], !acknowledged);
        assert_eq!(
            error["code"],
            if acknowledged {
                "gateway_client_not_saved"
            } else {
                "gateway_client_revoke_unknown"
            }
        );
        assert!(!error.to_string().contains(&key()));
        assert!(target.is_dir());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
        let requests = a.requests.lock().unwrap();
        assert_eq!(requests.len(), 3);
        assert_eq!(
            requests[2].path,
            format!("/api/gateway-lan/clients/{ID}/revoke")
        );
        assert!(
            requests
                .iter()
                .all(|r| r.authorization == "Bearer export-fixture-token")
        );
        assert!(b.requests.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn invalid_listen_never_issues_and_malformed_issue_reply_is_unknown_without_reissuing_or_revoking_other_clients()
 {
    for (listen, reply, expected) in [
        ("not-an-address", json!({}), 1),
        ("127.0.0.1:18901", json!({"id":ID,"key":null}), 2),
    ] {
        let a = serve(move |r| {
            let reply = reply.clone();
            async move {
                if r.path.ends_with("/clients") {
                    reply.to_string()
                } else {
                    json!({"listen":listen}).to_string()
                }
            }
        })
        .await;
        let backend = Backend::test_connection(a.endpoint.clone());
        let dir = directory();
        let target = dir.path().join("client.json");
        let error: Value = serde_json::from_str(
            &create_at(&backend.context().unwrap(), input(), Some(target.clone()))
                .await
                .unwrap_err(),
        )
        .unwrap();
        assert_eq!(error["outcomeUnknown"], expected == 2);
        assert!(!target.exists());
        assert_eq!(a.requests.lock().unwrap().len(), expected);
    }
}
