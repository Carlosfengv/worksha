use super::*;
use local_agent_core::observation::content_epoch;
use rusqlite::{Connection, TransactionBehavior};

fn wait_for(writer: &Writer, predicate: impl Fn(&Health) -> bool) {
    let until = Instant::now() + Duration::from_secs(3);
    loop {
        let health = writer.health();
        if predicate(&health) {
            return;
        }
        assert!(
            Instant::now() < until,
            "writer state: {}",
            serde_json::to_string(&health).unwrap()
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn retry_reopen_preserves_the_original_epoch_and_parks_without_replaying_newer_revisions() {
    for known_at_start in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let store = ObservationStore::open(dir.path()).unwrap();
        let epoch = store.write_epoch().clone();
        let writer = Writer::new(
            dir.path().canonicalize().unwrap(),
            known_at_start.then_some(epoch.clone()),
        )
        .unwrap();
        writer.enqueue(row(2));
        writer.flush(Duration::from_secs(3)).unwrap();
        let mut db = Connection::open(dir.path().join("observations.sqlite3")).unwrap();
        db.execute_batch("CREATE TRIGGER fail BEFORE INSERT ON model_observations BEGIN SELECT RAISE(ABORT,'injected');END;").unwrap();
        writer.enqueue(row(3));
        wait_for(&writer, |health| {
            health.error.is_some() && health.pending > 0
        });
        let tx = db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        tx.execute_batch("DROP TRIGGER fail;DELETE FROM model_observations;")
            .unwrap();
        content_epoch::advance(&tx, &epoch).unwrap();
        tx.commit().unwrap();
        wait_for(&writer, |health| {
            health.error.as_deref() == Some(&ObservationWriteError::ScopeChanged.to_string())
        });
        writer.enqueue(row(5));
        writer.enqueue(row(4));
        let started = Instant::now();
        assert!(writer.flush(Duration::from_secs(30)).is_err());
        assert!(started.elapsed() < Duration::from_secs(1));
        assert!(store.load_all().unwrap().is_empty());
        assert_eq!(writer.health().pending, 1);
        assert!(writer.pending_for_epoch(&epoch).is_none());
        assert_eq!(writer.shared.0.lock().unwrap().rows["r"].revision, 5);
        let stopped = Instant::now();
        drop(writer);
        assert!(stopped.elapsed() < Duration::from_secs(1));
        assert!(store.load_all().unwrap().is_empty());
    }
}

#[test]
fn an_input_only_batch_in_an_old_epoch_is_retained_without_writing_the_json_cache() {
    let dir = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(dir.path()).unwrap();
    let epoch = store.write_epoch().clone();
    let writer = Writer::new(dir.path().canonicalize().unwrap(), Some(epoch.clone())).unwrap();
    let mut db = Connection::open(dir.path().join("observations.sqlite3")).unwrap();
    let tx = db
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .unwrap();
    content_epoch::advance(&tx, &epoch).unwrap();
    tx.commit().unwrap();
    let input = serde_json::from_value(serde_json::json!({"key":"input","agent":"claude","sessionId":null,"startedAt":null,"input":{"at":"2026-01-01T00:00:00Z","text":"synthetic-private-excerpt"}})).unwrap();
    writer.enqueue_input(input);
    wait_for(&writer, |health| {
        health.error.as_deref() == Some(&ObservationWriteError::ScopeChanged.to_string())
    });
    assert_eq!(writer.health().pending, 1);
    assert!(writer.flush(Duration::from_secs(1)).is_err());
    assert!(!dir.path().join("gateway-activity-inputs.json").exists());
    assert!(store.load_all().unwrap().is_empty());
}

#[test]
fn a_new_projection_epoch_cannot_use_old_live_rows_even_before_the_idle_writer_detects_it() {
    let dir = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(dir.path()).unwrap();
    let old = store.write_epoch().clone();
    let writer = Writer::new(dir.path().canonicalize().unwrap(), Some(old.clone())).unwrap();
    let mut db = Connection::open(dir.path().join("observations.sqlite3")).unwrap();
    let tx = db
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .unwrap();
    let current = content_epoch::advance(&tx, &old).unwrap();
    tx.commit().unwrap();
    assert_eq!(writer.health().pending, 0);
    assert!(writer.health().error.is_none());
    assert!(writer.pending_for_epoch(&current).is_none());
    assert!(writer.pending_for_epoch(&old).is_some());
}
