use super::*;
use axum::http::StatusCode;
use serde_json::json;
use std::sync::atomic::{AtomicBool, Ordering};

fn ready() -> ApiResult {
    Ok(Json(json!({"ready": true})))
}

async fn until(test: impl Fn() -> bool) {
    tokio::time::timeout(Duration::from_secs(2), async {
        while !test() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "current_thread")]
async fn object_burst_queues_behind_two_workers_without_blocking_the_runtime() {
    let budget = ReadBudget::new();
    let held = budget.running.clone().acquire_many_owned(2).await.unwrap();
    let next = tokio::spawn({
        let budget = budget.clone();
        async move { budget.run(ready).await }
    });
    until(|| budget.admitted.available_permits() == 31).await;
    assert!(!next.is_finished());
    assert_eq!(crate::blocking::run(|| 42).await.unwrap(), 42);
    drop(held);
    assert_eq!(next.await.unwrap().unwrap().0, json!({"ready": true}));
    assert_eq!(budget.running.available_permits(), 2);
    assert_eq!(budget.admitted.available_permits(), 32);
}

#[tokio::test(flavor = "current_thread")]
async fn object_queue_overflow_and_cancelled_waiter_never_start_work() {
    let budget = ReadBudget::new();
    let _held = budget.running.clone().acquire_many_owned(2).await.unwrap();
    let queue = budget
        .admitted
        .clone()
        .acquire_many_owned(31)
        .await
        .unwrap();
    let started = Arc::new(AtomicBool::new(false));
    let queued = tokio::spawn({
        let budget = budget.clone();
        let started = started.clone();
        async move {
            budget
                .run(move || {
                    started.store(true, Ordering::Release);
                    ready()
                })
                .await
        }
    });
    until(|| budget.admitted.available_permits() == 0).await;
    let (status, Json(value)) = budget.run(|| panic!("overflow work")).await.unwrap_err();
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(value["code"], "busy");
    queued.abort();
    assert!(queued.await.unwrap_err().is_cancelled());
    assert!(!started.load(Ordering::Acquire));
    assert_eq!(budget.admitted.available_permits(), 1);
    drop(queue);
    assert_eq!(budget.admitted.available_permits(), 32);
}

#[tokio::test(flavor = "current_thread")]
async fn object_queue_timeout_cannot_execute_later_and_preserves_independent_capacity() {
    let budget = ReadBudget::new();
    let held = budget.running.clone().acquire_many_owned(2).await.unwrap();
    let started = Arc::new(AtomicBool::new(false));
    let marker = started.clone();
    let (status, Json(value)) = budget
        .run(move || {
            marker.store(true, Ordering::Release);
            ready()
        })
        .await
        .unwrap_err();
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(value["code"], "busy");
    assert_eq!(budget.admitted.available_permits(), 32);
    assert!(!started.load(Ordering::Acquire));
    assert!(ReadBudget::new().run(ready).await.is_ok());
    assert!(
        crate::blocking::query(crate::blocking::query_capacity(), ready)
            .await
            .is_ok()
    );
    drop(held);
    assert!(budget.run(ready).await.is_ok());
    assert!(!started.load(Ordering::Acquire));
}

#[tokio::test(flavor = "current_thread")]
async fn object_cancelled_running_work_keeps_both_permits_until_completion() {
    let budget = ReadBudget::new();
    let (release, gate) = std::sync::mpsc::channel();
    let started = Arc::new(AtomicBool::new(false));
    let next = tokio::spawn({
        let budget = budget.clone();
        let started = started.clone();
        async move {
            budget
                .run(move || {
                    started.store(true, Ordering::Release);
                    gate.recv_timeout(Duration::from_secs(3)).unwrap();
                    ready()
                })
                .await
        }
    });
    until(|| started.load(Ordering::Acquire)).await;
    next.abort();
    assert!(next.await.unwrap_err().is_cancelled());
    assert_eq!(budget.running.available_permits(), 1);
    assert_eq!(budget.admitted.available_permits(), 31);
    release.send(()).unwrap();
    until(|| budget.running.available_permits() == 2).await;
    assert_eq!(budget.admitted.available_permits(), 32);
}

#[tokio::test]
async fn object_worker_panic_releases_budget_and_domain_errors_are_preserved() {
    let budget = ReadBudget::new();
    let (status, Json(value)) = budget
        .run(|| panic!("owned fixture panic"))
        .await
        .unwrap_err();
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(value["code"], "unavailable");
    assert_eq!(budget.running.available_permits(), 2);
    assert_eq!(budget.admitted.available_permits(), 32);
    let (status, Json(value)) = budget
        .run(|| {
            Err((
                StatusCode::CONFLICT,
                Json(json!({"code": "stale_snapshot"})),
            ))
        })
        .await
        .unwrap_err();
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(value["code"], "stale_snapshot");
    assert_eq!(budget.running.available_permits(), 2);
    assert_eq!(budget.admitted.available_permits(), 32);
}
