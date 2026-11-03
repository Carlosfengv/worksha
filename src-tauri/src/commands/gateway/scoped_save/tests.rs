use super::*;
use crate::backend::{
    Backend,
    test_server::{Request as HttpRequest, serve_status},
};
use std::{
    os::unix::fs::PermissionsExt,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use tokio::sync::Notify;
const STORE: &str = "00000000-0000-4000-8000-000000000001";
const ID: &str = "11111111-1111-4111-8111-111111111111";
const COMP: &str = "22222222-2222-4222-8222-222222222222";
fn key() -> String {
    format!("la_{}", "1".repeat(64))
}
fn input() -> Value {
    json!({"request":{"storeId":STORE,"operationId":ID,"action":{"kind":"create","draft":{"name":"fixture","agent":"claude","providerId":"fixture","models":["fixture-model"],"concurrency":1}}},"compensationOperationId":COMP})
}
fn outcome(revoke: bool) -> Value {
    let request = if revoke {
        json!({"storeId":STORE,"operationId":COMP,"action":{"kind":"revoke","clientId":ID}})
    } else {
        input()["request"].clone()
    };
    json!({"request":request,"receipt":{"apiVersion":1,"storeId":STORE,"operationId":if revoke{COMP}else{ID},"action":if revoke{"revoke"}else{"create"},"clientId":ID,"listen":"127.0.0.1:18902","completedAt":"2026-10-27T00:00:00Z"}})
}
fn status() -> Value {
    json!({"listen":"127.0.0.1:18901","writeProtocol":{"version":1,"storeId":STORE,"secretReplay":false,"maxClients":128},"clients":[{"id":ID,"enabled":true}]})
}
fn missing() -> (u16, String) {
    (
        404,
        json!({"code":"operation_not_found","message":"fixture missing","outcomeUnknown":false})
            .to_string(),
    )
}
fn reply(request: &HttpRequest) -> (u16, String) {
    if request.path == "/api/gateway-lan" {
        (200, status().to_string())
    } else if request.method == "GET" {
        missing()
    } else {
        let revoke = request.body["action"]["kind"] == "revoke";
        (
            200,
            json!({"outcome":outcome(revoke),"key":if revoke{Value::Null}else{json!(key())}})
                .to_string(),
        )
    }
}
fn directory() -> tempfile::TempDir {
    tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap()
}
async fn held<T: std::future::Future>(future: T) -> T::Output {
    tokio::time::timeout(std::time::Duration::from_secs(5), future)
        .await
        .unwrap()
}

#[tokio::test]
async fn cancelled_scoped_selection_does_not_query_or_issue_even_after_switch() {
    let a = serve_status(|_| async { (200, "{}".into()) }).await;
    let b = serve_status(|_| async { (200, "{}".into()) }).await;
    let backend = Backend::test_connection(a.endpoint.clone());
    let context = backend.context().unwrap();
    backend.test_install_connection(b.endpoint.clone());
    assert_eq!(
        create_at(&context, Input::parse(input()).unwrap(), None)
            .await
            .unwrap(),
        json!({"status":"cancelled"})
    );
    assert!(a.requests.lock().unwrap().is_empty() && b.requests.lock().unwrap().is_empty());
}
#[tokio::test]
async fn unsupported_or_changed_store_never_sends_an_issuance() {
    for value in [
        json!({"listen":"127.0.0.1:18901"}),
        json!({"listen":"127.0.0.1:18901","writeProtocol":{"version":1,"storeId":COMP,"secretReplay":false}}),
    ] {
        let a = serve_status(move |_| {
            let v = value.clone();
            async move { (200, v.to_string()) }
        })
        .await;
        let backend = Backend::test_connection(a.endpoint.clone());
        let dir = directory();
        let target = dir.path().join("client.json");
        assert!(
            create_at(
                &backend.context().unwrap(),
                Input::parse(input()).unwrap(),
                Some(target.clone())
            )
            .await
            .unwrap()["status"]
                == "notIssued"
        );
        assert!(!target.exists());
        assert_eq!(a.requests.lock().unwrap().len(), 1);
    }
}
#[tokio::test]
async fn existing_issuance_never_replays_a_key_or_overwrites_a_selected_file() {
    let a = serve_status(|r| async move {
        if r.path == "/api/gateway-lan" {
            (200, status().to_string())
        } else {
            (200, outcome(false).to_string())
        }
    })
    .await;
    let backend = Backend::test_connection(a.endpoint.clone());
    let dir = directory();
    let target = dir.path().join("existing.json");
    std::fs::write(&target, b"original").unwrap();
    let result = create_at(
        &backend.context().unwrap(),
        Input::parse(input()).unwrap(),
        Some(target.clone()),
    )
    .await
    .unwrap();
    assert_eq!(result["status"], "issued");
    assert_eq!(std::fs::read(&target).unwrap(), b"original");
    assert!(!result.to_string().contains(&key()));
    assert_eq!(a.requests.lock().unwrap().len(), 2);
    assert!(a.requests.lock().unwrap().iter().all(|r| r.method == "GET"));
}
#[tokio::test]
async fn held_issuance_saves_receipt_origin_and_reports_success_after_switch() {
    let entered = Arc::new(Notify::new());
    let released = Arc::new(Notify::new());
    let (hit, done) = (entered.clone(), released.clone());
    let a = serve_status(move |r| {
        let (hit, done) = (hit.clone(), done.clone());
        async move {
            if r.method == "POST" {
                hit.notify_one();
                held(done.notified()).await;
            }
            reply(&r)
        }
    })
    .await;
    let b = serve_status(|_| async { (200, "{}".into()) }).await;
    let backend = Backend::test_connection(a.endpoint.clone());
    let context = backend.context().unwrap();
    let origin = context.instance_id().to_owned();
    let dir = directory();
    let target = dir.path().join("client.json");
    let copy = target.clone();
    let task = tokio::spawn(async move {
        create_at(&context, Input::parse(input()).unwrap(), Some(copy)).await
    });
    held(entered.notified()).await;
    backend.test_install_connection(b.endpoint.clone());
    released.notify_one();
    let result = task.await.unwrap().unwrap();
    assert_eq!(result["status"], "saved");
    assert_eq!(result["sourceInstanceId"], origin);
    assert_eq!(result["sourceCurrent"], false);
    assert!(
        !result.to_string().contains(&key())
            && !result.to_string().contains("export-fixture-token")
    );
    let file: Value = serde_json::from_slice(&std::fs::read(&target).unwrap()).unwrap();
    assert_eq!(file["apiKey"], key());
    assert_eq!(file["baseUrl"], "http://127.0.0.1:18902/v1");
    assert_eq!(
        std::fs::metadata(&target).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let requests = a.requests.lock().unwrap();
    assert_eq!(requests.len(), 3);
    assert_eq!(requests[2].body, input()["request"]);
    assert!(
        requests
            .iter()
            .all(|r| r.authorization == "Bearer export-fixture-token")
    );
    assert!(b.requests.lock().unwrap().is_empty());
}
#[tokio::test]
async fn save_failure_uses_original_stable_compensation_and_rejects_unrelated_receipts() {
    for valid in [true, false] {
        let entered = Arc::new(Notify::new());
        let released = Arc::new(Notify::new());
        let (hit, done) = (entered.clone(), released.clone());
        let a = serve_status(move |r| {
            let (hit, done) = (hit.clone(), done.clone());
            async move {
                if r.method == "POST" && r.body["action"]["kind"] == "create" {
                    hit.notify_one();
                    held(done.notified()).await;
                }
                if r.method == "POST" && r.body["action"]["kind"] == "revoke" && !valid {
                    let mut out = outcome(true);
                    out["receipt"]["clientId"] = json!(COMP);
                    return (200, json!({"outcome":out,"key":null}).to_string());
                }
                reply(&r)
            }
        })
        .await;
        let b = serve_status(|_| async { (200, "{}".into()) }).await;
        let backend = Backend::test_connection(a.endpoint.clone());
        let context = backend.context().unwrap();
        let dir = directory();
        let target = dir.path().join("existing-directory");
        std::fs::create_dir(&target).unwrap();
        let copy = target.clone();
        let task = tokio::spawn(async move {
            create_at(&context, Input::parse(input()).unwrap(), Some(copy)).await
        });
        held(entered.notified()).await;
        backend.test_install_connection(b.endpoint.clone());
        released.notify_one();
        let result = task.await.unwrap();
        if valid {
            let value = result.unwrap();
            assert_eq!(value["status"], "revoked");
            assert_eq!(
                value["compensation"]["outcome"]["request"]["operationId"],
                COMP
            );
            assert!(!value.to_string().contains(&key()));
        } else {
            let error: Value = serde_json::from_str(&result.unwrap_err()).unwrap();
            assert_eq!(error["outcomeUnknown"], true);
            assert_eq!(error["code"], "gateway_client_revoke_unknown");
        }
        assert!(target.is_dir());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
        let requests = a.requests.lock().unwrap();
        let writes: Vec<_> = requests.iter().filter(|r| r.method == "POST").collect();
        assert_eq!(writes.len(), 2);
        assert_eq!(writes[1].body["operationId"], COMP);
        assert_eq!(writes[1].body["action"]["clientId"], ID);
        assert!(
            requests
                .iter()
                .all(|r| r.authorization == "Bearer export-fixture-token")
        );
        assert!(b.requests.lock().unwrap().is_empty());
    }
}
#[tokio::test]
async fn lost_reply_is_queried_at_original_origin_without_another_issuance_or_file_write() {
    let committed = Arc::new(AtomicBool::new(false));
    let saved = committed.clone();
    let a = serve_status(move |r| {
        let saved = saved.clone();
        async move {
            if r.path == "/api/gateway-lan" {
                (200, status().to_string())
            } else if r.method == "POST" {
                saved.store(true, Ordering::Release);
                (200, "".into())
            } else if saved.load(Ordering::Acquire) {
                (200, outcome(false).to_string())
            } else {
                missing()
            }
        }
    })
    .await;
    let backend = Backend::test_connection(a.endpoint.clone());
    let dir = directory();
    let target = dir.path().join("client.json");
    let result = create_at(
        &backend.context().unwrap(),
        Input::parse(input()).unwrap(),
        Some(target.clone()),
    )
    .await
    .unwrap();
    assert_eq!(result["status"], "issued");
    assert!(!target.exists());
    assert_eq!(
        a.requests
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.method == "POST")
            .count(),
        1
    );
}
#[tokio::test]
async fn metadata_revoke_boundary_never_issues_or_returns_a_key() {
    let a = serve_status(|_| async { (200, "{}".into()) }).await;
    let backend = Backend::test_connection(a.endpoint.clone());
    let input = Input::parse(input()).unwrap();
    assert!(
        operations::revoke(&backend.context().unwrap(), &input.request, false)
            .await
            .err()
            .unwrap()
            .contains("invalid_input")
    );
    assert!(a.requests.lock().unwrap().is_empty());
    let mut extra = outcome(false);
    extra["key"] = json!(key());
    assert!(operations::outcome(extra, Some(&input.request)).is_err());
}

#[tokio::test]
async fn explicit_rejection_without_a_receipt_is_not_a_submitted_unknown_attempt() {
    let a=serve_status(|r|async move{if r.method=="POST"{(400,json!({"code":"invalid_input","message":"fixture disabled model","outcomeUnknown":false}).to_string())}else{reply(&r)}}).await;
    let backend = Backend::test_connection(a.endpoint.clone());
    let dir = directory();
    let target = dir.path().join("client.json");
    let value = create_at(
        &backend.context().unwrap(),
        Input::parse(input()).unwrap(),
        Some(target.clone()),
    )
    .await
    .unwrap();
    assert_eq!(value["status"], "notIssued");
    assert!(!target.exists());
    assert_eq!(
        a.requests
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.method == "POST")
            .count(),
        1
    );
}
#[tokio::test]
async fn already_revoked_state_is_not_fabricated_as_the_original_operation_receipt() {
    let a = serve_status(|r| async move {
        if r.path == "/api/gateway-lan" {
            let mut value = status();
            value["clients"] = json!([{"id":ID,"enabled":false}]);
            (200, value.to_string())
        } else {
            missing()
        }
    })
    .await;
    let backend = Backend::test_connection(a.endpoint.clone());
    let input = Input::parse(input()).unwrap();
    let proof = serde_json::to_value(
        operations::revoke(&backend.context().unwrap(), &input.compensation(), false)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(proof["status"], "alreadyRevoked");
    assert_eq!(proof["request"]["operationId"], COMP);
    assert!(proof.get("outcome").is_none());
    assert!(a.requests.lock().unwrap().iter().all(|r| r.method == "GET"));
}

#[tokio::test]
async fn wrong_source_duplicate_or_non_boolean_client_state_cannot_confirm_revocation() {
    let mut wrong = status();
    wrong["writeProtocol"]["storeId"] = json!(COMP);
    wrong["clients"] = json!([{"id":ID,"enabled":false}]);
    let mut duplicate = status();
    duplicate["clients"] = json!([{"id":ID,"enabled":false},{"id":ID,"enabled":false}]);
    let mut non_boolean = status();
    non_boolean["clients"] = json!([{"id":ID,"enabled":"false"}]);
    for value in [wrong, duplicate, non_boolean] {
        let a=serve_status(move|r|{let value=value.clone();async move{
            if r.path=="/api/gateway-lan"{(200,value.to_string())}
            else if r.method=="POST"{(503,json!({"code":"gateway_result_unknown","outcomeUnknown":true,"message":"fixture failed"}).to_string())}
            else {missing()}
        }}).await;
        let backend = Backend::test_connection(a.endpoint.clone());
        let input = Input::parse(input()).unwrap();
        assert!(
            operations::revoke(&backend.context().unwrap(), &input.compensation(), false)
                .await
                .is_err()
        );
        assert_eq!(
            a.requests
                .lock()
                .unwrap()
                .iter()
                .filter(|r| r.method == "POST")
                .count(),
            0
        );
    }
}
