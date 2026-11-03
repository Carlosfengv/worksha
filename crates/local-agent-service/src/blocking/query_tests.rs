use super::*;
use axum::{Json, http::StatusCode};
use serde_json::json;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

fn response() -> crate::ApiResult {
    Ok(Json(json!({"ready":true})))
}

#[tokio::test]
async fn management_adapter_preserves_a_committed_or_domain_result() {
    let (status, Json(value)) =
        management(|| Err((StatusCode::CONFLICT, Json(json!({"code":"stale_snapshot"})))))
            .await
            .unwrap_err();
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(value["code"], "stale_snapshot");
    assert_eq!(management(response).await.unwrap().0, json!({"ready":true}));
    let (status, Json(value)) = management(|| panic!("offline management fixture"))
        .await
        .unwrap_err();
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(value["outcomeUnknown"], true);
}

#[tokio::test(flavor = "current_thread")]
async fn excess_queries_reject_before_starting_work_and_use_named_errors() {
    let capacity = query_capacity();
    assert_eq!(capacity.available_permits(), 8);
    let held = capacity.clone().acquire_many_owned(8).await.unwrap();
    let started = Arc::new(AtomicBool::new(false));
    let marker = started.clone();
    let (status, Json(value)) = query(capacity.clone(), move || {
        marker.store(true, Ordering::Release);
        response()
    })
    .await
    .unwrap_err();
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(value["code"], "busy");
    assert!(!started.load(Ordering::Acquire));
    drop(held);
    assert!(query(capacity, response).await.is_ok());
}

#[tokio::test(flavor = "current_thread")]
async fn cancelled_query_holds_its_real_worker_slot_without_blocking_management() {
    let capacity = Arc::new(Semaphore::new(1));
    let (release, gate) = std::sync::mpsc::channel();
    let started = Arc::new(tokio::sync::Notify::new());
    let caller = tokio::spawn({
        let capacity = capacity.clone();
        let started = started.clone();
        async move {
            query(capacity, move || {
                started.notify_one();
                gate.recv_timeout(Duration::from_secs(3)).unwrap();
                response()
            })
            .await
        }
    });
    tokio::time::timeout(Duration::from_secs(1), started.notified())
        .await
        .unwrap();
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    let (status, Json(value)) = query(capacity.clone(), response).await.unwrap_err();
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(value["code"], "busy");
    assert_eq!(
        tokio::time::timeout(Duration::from_millis(500), run(|| 42))
            .await
            .unwrap()
            .unwrap(),
        42
    );
    release.send(()).unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
    while capacity.available_permits() == 0 {
        assert!(tokio::time::Instant::now() < deadline);
        tokio::task::yield_now().await;
    }
    assert!(query(capacity, response).await.is_ok());
}

#[tokio::test]
async fn query_failure_preserves_domain_errors_and_panic_releases_capacity() {
    let capacity = Arc::new(Semaphore::new(1));
    let (status, Json(value)) = query(capacity.clone(), || {
        Err((StatusCode::CONFLICT, Json(json!({"code":"stale_snapshot"}))))
    })
    .await
    .unwrap_err();
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(value["code"], "stale_snapshot");
    let (status, Json(value)) = query(capacity.clone(), || panic!("offline query fixture"))
        .await
        .unwrap_err();
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(value["code"], "unavailable");
    assert_eq!(capacity.available_permits(), 1);
    assert!(query(capacity, response).await.is_ok());
}

#[tokio::test]
async fn independent_service_query_capacities_do_not_consume_each_other() {
    let first = query_capacity();
    let second = query_capacity();
    let _held = first.clone().acquire_many_owned(8).await.unwrap();
    assert!(query(first, response).await.is_err());
    assert!(query(second.clone(), response).await.is_ok());
    assert_eq!(second.available_permits(), 8);
}
