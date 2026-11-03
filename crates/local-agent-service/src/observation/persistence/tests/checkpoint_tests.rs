use super::*;
use local_agent_core::observation::content_epoch;
use rusqlite::{Connection, TransactionBehavior};

fn wait_until(check: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(2);
    while !check() {
        assert!(
            Instant::now() < deadline,
            "worker did not reach the observed state"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn input(key: &str) -> CachedInput {
    serde_json::from_value(serde_json::json!({"key":key,"agent":"claude","sessionId":null,"startedAt":null,"input":{"at":"2026-01-01T00:00:00Z","text":"synthetic-checkpoint-input"}})).unwrap()
}

#[test]
fn the_worker_acknowledges_an_empty_checkpoint_without_inventing_a_successful_write() {
    let dir = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(dir.path()).unwrap();
    let writer = Writer::new(
        dir.path().canonicalize().unwrap(),
        Some(store.write_epoch().clone()),
    )
    .unwrap();
    let checkpoint = writer.checkpoint(Duration::from_secs(3)).unwrap();
    assert_eq!(checkpoint.epoch(), store.write_epoch());
    assert!(checkpoint.is_empty().unwrap());
    assert!(writer.health().last_success_at.is_none());
    assert!(store.load_all().unwrap().is_empty());
    assert!(!dir.path().join("gateway-activity-inputs.json").exists());
}

#[test]
fn a_held_checkpoint_keeps_admission_nonblocking_and_resumes_latest_rows_and_inputs_on_drop() {
    let dir = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(dir.path()).unwrap();
    let writer = Writer::new(
        dir.path().canonicalize().unwrap(),
        Some(store.write_epoch().clone()),
    )
    .unwrap();
    let checkpoint = writer.checkpoint(Duration::from_secs(3)).unwrap();
    std::thread::scope(|scope| {
        let (done, arrived) = std::sync::mpsc::channel();
        let writer = &writer;
        scope.spawn(move || {
            for revision in 2..=501 {
                writer.enqueue(row(revision));
            }
            writer.enqueue_input(input("r"));
            done.send(()).unwrap();
        });
        arrived
            .recv_timeout(Duration::from_secs(2))
            .expect("admission must not wait for the checkpoint owner");
    });
    assert_eq!(writer.health().pending, 2);
    assert!(!checkpoint.is_empty().unwrap());
    assert!(
        writer
            .pending_for_epoch(store.write_epoch())
            .unwrap()
            .contains("r")
    );
    assert!(writer.health().last_success_at.is_none());
    assert!(store.load_all().unwrap().is_empty());
    assert!(!dir.path().join("gateway-activity-inputs.json").exists());
    drop(checkpoint);
    writer
        .flush(Duration::from_secs(3))
        .unwrap_or_else(|error| {
            panic!(
                "{error}; {}",
                serde_json::to_string(&writer.health()).unwrap()
            )
        });
    assert_eq!(store.load_all().unwrap()[0].revision, 501);
    assert_eq!(writer.health().pending, 0);
    assert!(dir.path().join("gateway-activity-inputs.json").exists());
}

#[test]
fn an_in_flight_frontier_finishes_before_ready_while_later_updates_remain_buffered() {
    let dir = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(dir.path()).unwrap();
    let writer = Writer::new(
        dir.path().canonicalize().unwrap(),
        Some(store.write_epoch().clone()),
    )
    .unwrap();
    let mut db = Connection::open(dir.path().join("observations.sqlite3")).unwrap();
    let tx = db
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .unwrap();
    writer.enqueue(row(2));
    std::thread::scope(|scope| {
        let owner = scope.spawn(|| writer.checkpoint(Duration::from_secs(3)));
        wait_until(|| {
            let state = writer.shared.0.lock().unwrap();
            state.checkpoint.is_some() && state.writing == 1
        });
        writer.enqueue(row(5));
        let mut later = row(2);
        later.id = "later".into();
        writer.enqueue(later);
        tx.commit().unwrap();
        let checkpoint = owner.join().unwrap().unwrap();
        let saved = store.load_all().unwrap();
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].revision, 2);
        assert_eq!(writer.health().pending, 2);
        assert!(!checkpoint.is_empty().unwrap());
        drop(checkpoint);
    });
    writer.flush(Duration::from_secs(3)).unwrap();
    let saved = store.load_all().unwrap();
    assert_eq!(saved.len(), 2);
    assert_eq!(saved.iter().find(|row| row.id == "r").unwrap().revision, 5);
}

#[test]
fn timeout_does_not_finish_a_live_validation_or_let_its_late_ack_grant_a_new_checkpoint() {
    let dir = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(dir.path()).unwrap();
    let writer = Writer::new(
        dir.path().canonicalize().unwrap(),
        Some(store.write_epoch().clone()),
    )
    .unwrap();
    let mut db = Connection::open(dir.path().join("observations.sqlite3")).unwrap();
    let tx = db
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .unwrap();
    std::thread::scope(|scope| {
        let first = scope.spawn(|| writer.checkpoint(Duration::from_secs(2)).is_err());
        wait_until(|| writer.shared.0.lock().unwrap().checking_checkpoint == Some(0));
        assert!(first.join().unwrap());
        assert_eq!(writer.shared.0.lock().unwrap().checking_checkpoint, Some(0));
        assert!(writer.shared.0.lock().unwrap().checkpoint.is_none());
        assert!(writer.checkpoint(Duration::from_millis(50)).is_err());
        assert_eq!(writer.shared.0.lock().unwrap().checking_checkpoint, Some(0));
        writer.enqueue(row(7));
        tx.commit().unwrap();
        let current = writer.checkpoint(Duration::from_secs(3)).unwrap();
        assert_eq!(
            store.load_all().unwrap()[0].revision,
            7,
            "the new frontier must commit before its own ready acknowledgement"
        );
        assert!(current.is_empty().unwrap());
    });
}

#[test]
fn failed_frontier_timeout_preserves_the_newest_deferred_revision_for_recovery() {
    let dir = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(dir.path()).unwrap();
    let writer = Writer::new(
        dir.path().canonicalize().unwrap(),
        Some(store.write_epoch().clone()),
    )
    .unwrap();
    let db = Connection::open(dir.path().join("observations.sqlite3")).unwrap();
    db.execute_batch("CREATE TRIGGER fail BEFORE INSERT ON model_observations BEGIN SELECT RAISE(ABORT,'injected');END;").unwrap();
    writer.enqueue(row(2));
    std::thread::scope(|scope| {
        let owner = scope.spawn(|| writer.checkpoint(Duration::from_millis(300)).is_err());
        wait_until(|| writer.shared.0.lock().unwrap().checkpoint.is_some());
        writer.enqueue(row(9));
        assert!(owner.join().unwrap());
    });
    assert!(store.load_all().unwrap().is_empty());
    db.execute_batch("DROP TRIGGER fail;").unwrap();
    writer.flush(Duration::from_secs(3)).unwrap();
    assert_eq!(store.load_all().unwrap()[0].revision, 9);
}

#[test]
fn deferred_and_frontier_data_share_the_capacity_and_coalescing_budget() {
    let dir = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(dir.path()).unwrap();
    let writer = Writer::new(
        dir.path().canonicalize().unwrap(),
        Some(store.write_epoch().clone()),
    )
    .unwrap();
    let checkpoint = writer.checkpoint(Duration::from_secs(3)).unwrap();
    for index in 0..CAPACITY {
        let mut value = row(2);
        value.id = format!("row-{index}");
        writer.enqueue(value);
    }
    assert_eq!(writer.health().pending, CAPACITY);
    let mut overflow = row(2);
    overflow.id = "overflow".into();
    writer.enqueue(overflow);
    let mut newer = row(8);
    newer.id = "row-0".into();
    writer.enqueue(newer);
    assert_eq!(writer.health().pending, CAPACITY);
    assert_eq!(writer.health().dropped_updates, 1);
    assert!(checkpoint.is_empty().is_err());
    drop(checkpoint);
    wait_until(|| writer.health().pending == 0);
    assert!(writer.flush(Duration::from_secs(1)).is_err());
    let saved = store.load_all().unwrap();
    assert_eq!(saved.len(), CAPACITY);
    assert_eq!(
        saved.iter().find(|row| row.id == "row-0").unwrap().revision,
        8
    );
}

#[test]
fn unknown_or_changed_epoch_is_never_a_verified_empty_checkpoint() {
    let missing = tempfile::tempdir().unwrap();
    let unknown = Writer::new(missing.path().canonicalize().unwrap(), None).unwrap();
    assert!(unknown.flush(Duration::from_millis(20)).is_err());
    assert!(!missing.path().join("observations.sqlite3").exists());
    let dir = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(dir.path()).unwrap();
    let writer = Writer::new(
        dir.path().canonicalize().unwrap(),
        Some(store.write_epoch().clone()),
    )
    .unwrap();
    let mut db = Connection::open(dir.path().join("observations.sqlite3")).unwrap();
    let tx = db
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .unwrap();
    content_epoch::advance(&tx, store.write_epoch()).unwrap();
    tx.commit().unwrap();
    assert!(writer.flush(Duration::from_secs(3)).is_err());
    assert_eq!(writer.health().pending, 0);
    assert!(writer.health().error.is_some());
}

#[test]
fn unwinding_a_checkpoint_owner_resumes_its_admitted_updates() {
    let dir = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(dir.path()).unwrap();
    let writer = Writer::new(
        dir.path().canonicalize().unwrap(),
        Some(store.write_epoch().clone()),
    )
    .unwrap();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _checkpoint = writer.checkpoint(Duration::from_secs(3)).unwrap();
        writer.enqueue(row(4));
        panic!("injected owner cancellation");
    }));
    assert!(result.is_err());
    writer.flush(Duration::from_secs(3)).unwrap();
    assert_eq!(store.load_all().unwrap()[0].revision, 4);
}

#[test]
fn shutdown_reclaims_a_forgotten_checkpoint_without_losing_accepted_rows() {
    let dir = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(dir.path()).unwrap();
    let writer = Writer::new(
        dir.path().canonicalize().unwrap(),
        Some(store.write_epoch().clone()),
    )
    .unwrap();
    let checkpoint = writer.checkpoint(Duration::from_secs(3)).unwrap();
    writer.enqueue(row(6));
    std::mem::forget(checkpoint);
    drop(writer);
    assert_eq!(store.load_all().unwrap()[0].revision, 6);
}
