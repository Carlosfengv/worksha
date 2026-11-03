use super::*;
use std::sync::{Arc, atomic::Ordering};

#[tokio::test(flavor = "current_thread")]
async fn burst_keeps_one_running_batch_and_one_merged_followup() {
    let work = Arc::new(SessionWork::default());
    assert!(work.enqueue(None, false).unwrap());
    let started = Arc::new(Notify::new());
    let (release, gate) = std::sync::mpsc::channel();
    let worker = tokio::task::spawn_blocking({
        let work = work.clone();
        let started = started.clone();
        move || {
            let mut calls = 0;
            work.run(|batch| {
                calls += 1;
                if calls == 1 {
                    started.notify_one();
                    gate.recv_timeout(Duration::from_secs(3)).unwrap();
                } else {
                    assert_eq!(
                        batch.revoked,
                        BTreeSet::from(["claude".into(), "codex".into()])
                    );
                }
            });
            calls
        }
    });
    tokio::time::timeout(Duration::from_secs(1), started.notified())
        .await
        .unwrap();
    for i in 0..5000 {
        assert!(
            !work
                .enqueue(
                    Some(if i % 2 == 0 { "claude" } else { "codex" }.into()),
                    true
                )
                .unwrap()
        );
    }
    {
        let queue = work.queue.lock().unwrap();
        assert!(queue.running && queue.pending.refresh);
        assert_eq!(queue.pending.revoked.len(), 2);
    }
    release.send(()).unwrap();
    assert_eq!(worker.await.unwrap(), 2);
    assert!(work.drain(Duration::from_secs(1)).await);
    assert_eq!(work.completed.load(Ordering::Acquire), 2);
    // Finishing and re-enqueuing cannot leave a dirty batch without an owner.
    assert!(work.enqueue(None, false).unwrap());
    work.run(|_| {});
    assert!(work.drain(Duration::from_secs(1)).await);
}

#[tokio::test]
async fn panic_preserves_unfinished_cleanup_for_an_explicit_later_trigger() {
    let work = SessionWork::default();
    assert!(work.enqueue(Some("claude".into()), true).unwrap());
    work.run(|_| panic!("offline coalesced cleanup fixture"));
    assert!(!work.drain(Duration::from_millis(20)).await);
    {
        let queue = work.queue.lock().unwrap();
        assert!(!queue.running);
        assert!(queue.pending.revoked.contains("claude"));
    }
    assert!(work.enqueue(None, false).unwrap());
    work.run(|batch| assert!(batch.revoked.contains("claude")));
    assert!(work.drain(Duration::from_secs(1)).await);
}

#[tokio::test]
async fn stop_rejects_new_refresh_but_keeps_already_committed_revocation_cleanup() {
    let work = SessionWork::default();
    work.stop();
    assert!(work.enqueue(None, false).is_err());
    assert!(work.enqueue(Some("codex".into()), true).unwrap());
    work.run(|batch| {
        assert!(!batch.refresh);
        assert!(batch.revoked.contains("codex"));
    });
    assert!(work.drain(Duration::from_secs(1)).await);
}

#[tokio::test(flavor = "current_thread")]
async fn drain_timeout_never_claims_that_running_synchronous_work_has_stopped() {
    let work = Arc::new(SessionWork::default());
    assert!(work.enqueue(None, false).unwrap());
    let started = Arc::new(Notify::new());
    let (release, gate) = std::sync::mpsc::channel();
    let worker = tokio::task::spawn_blocking({
        let work = work.clone();
        let started = started.clone();
        move || {
            work.run(|_| {
                started.notify_one();
                gate.recv_timeout(Duration::from_secs(3)).unwrap();
            })
        }
    });
    tokio::time::timeout(Duration::from_secs(1), started.notified())
        .await
        .unwrap();
    work.stop();
    assert!(!work.drain(Duration::from_millis(10)).await);
    assert!(work.queue.lock().unwrap().running);
    release.send(()).unwrap();
    worker.await.unwrap();
    assert!(work.drain(Duration::from_secs(1)).await);
}

struct Fixture {
    state: AppState,
    _root: tempfile::TempDir,
}
impl Fixture {
    async fn new() -> Self {
        use local_agent_core::{
            Store,
            providers::{ProviderManager, vault::MemoryVault},
            sessions::NativeSessions,
        };
        let root = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
        let home = root.path().join("home");
        let data = root.path().join("data");
        std::fs::create_dir_all(&home).unwrap();
        let store = Store::open(&data).unwrap();
        store.save_native_source("claude", true).unwrap();
        let helper = root.path().join("offline-helper");
        std::fs::write(&helper, b"offline fixture").unwrap();
        let vault = Arc::new(MemoryVault::default());
        let providers = ProviderManager::new(home.clone(), &data, helper, vault.clone()).unwrap();
        let (gateway, _listener) = crate::gateway::Gateway::bind(&data).await.unwrap();
        let state = AppState {
            provider_adapters: crate::provider_models::adapters::builtin(),
            provider_client: crate::state::provider_client().unwrap(),
            credentials: vault.clone(),
            plugins: Arc::new(crate::plugin_host::external::Manager::open(&data, vault).unwrap()),
            readonly: Arc::new(crate::plugin_host::ReadonlyRuntime::builtin().unwrap()),
            store: Arc::new(Mutex::new(store)),
            token: Arc::new("offline-source-work-fixture".into()),
            host: Arc::new("127.0.0.1:9".into()),
            sessions: Arc::new(NativeSessions::new(home.clone(), &["claude".into()])),
            session_work: Arc::new(SessionWork::default()),
            observation: Arc::new(
                crate::observation::Observation::new(&data, "127.0.0.1:9").unwrap(),
            ),
            listeners: Arc::new(Mutex::new(Default::default())),
            providers: Arc::new(Mutex::new(providers)),
            verification_capacity: Arc::new(tokio::sync::Semaphore::new(2)),
            query_capacity: crate::blocking::query_capacity(),
            billing: Arc::new(Mutex::new(crate::billing::Billing::new(home, &data))),
            gateway: Arc::new(gateway),
        };
        Self { state, _root: root }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn cancelled_source_waiter_cannot_skip_committed_revocation_cleanup() {
    let fixture = Fixture::new().await;
    let state = fixture.state.clone();
    let held = Arc::new(Notify::new());
    let (release, gate) = std::sync::mpsc::channel();
    let blocker = std::thread::spawn({
        let state = state.clone();
        let held = held.clone();
        move || {
            let _lock = state.store.lock().unwrap();
            held.notify_one();
            gate.recv_timeout(Duration::from_secs(3)).unwrap();
        }
    });
    tokio::time::timeout(Duration::from_secs(1), held.notified())
        .await
        .unwrap();
    let request = tokio::spawn(crate::sessions::save_source(
        axum::extract::State(state.clone()),
        Json(crate::sessions::SourceChange {
            agent_id: "claude".into(),
            enabled: false,
        }),
    ));
    tokio::time::timeout(
        Duration::from_secs(1),
        state.session_work.source_started.notified(),
    )
    .await
    .unwrap();
    request.abort();
    assert!(request.await.unwrap_err().is_cancelled());
    release.send(()).unwrap();
    blocker.join().unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while state.session_work.completed.load(Ordering::Acquire) == 0 {
        assert!(tokio::time::Instant::now() < deadline);
        tokio::task::yield_now().await;
    }
    assert!(
        !state
            .store
            .lock()
            .unwrap()
            .native_sources()
            .unwrap()
            .contains(&"claude".into())
    );
    assert!(
        !state
            .sessions
            .sources()
            .unwrap()
            .iter()
            .find(|s| s.agent_id == "claude")
            .unwrap()
            .enabled
    );
    state.session_work.stop();
    assert!(state.session_work.drain(Duration::from_secs(1)).await);
}

#[tokio::test]
async fn stopped_work_rejects_source_change_before_storage_mutation() {
    let fixture = Fixture::new().await;
    let state = fixture.state.clone();
    state.session_work.stop();
    let (status, Json(value)) = crate::sessions::save_source(
        axum::extract::State(state.clone()),
        Json(crate::sessions::SourceChange {
            agent_id: "claude".into(),
            enabled: false,
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(value["outcomeUnknown"], false);
    assert!(
        state
            .store
            .lock()
            .unwrap()
            .native_sources()
            .unwrap()
            .contains(&"claude".into())
    );
    assert!(
        crate::sessions::refresh_sessions(axum::extract::State(state))
            .await
            .is_err()
    );
}
