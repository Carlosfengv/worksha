use super::*;
use crate::observation::content_epoch;

fn row(id: &str, revision: u64) -> ModelObservation {
    serde_json::from_value(serde_json::json!({"id":id,"agentId":"claude","provider":"fixture","endpoint":"https://offline.invalid","bindingRevision":1,"startedAt":"2026-01-01T00:00:00Z","status":"success","protocol":"messages","revision":revision,"usage":{}})).unwrap()
}

fn rotate(store: &ObservationStore) {
    let tx = Transaction::new_unchecked(&store.0, TransactionBehavior::Immediate).unwrap();
    tx.execute("DELETE FROM model_observations", []).unwrap();
    content_epoch::advance(&tx, store.write_epoch()).unwrap();
    tx.commit().unwrap();
}

#[test]
fn old_connections_and_reopened_retries_cannot_reinsert_removed_observations() {
    let dir = tempfile::tempdir().unwrap();
    let old = ObservationStore::open(dir.path()).unwrap();
    old.save(&row("old", 1)).unwrap();
    let expected = old.write_epoch().clone();
    rotate(&old);
    assert!(old.save(&row("old", 2)).is_err());
    assert!(old.save_batch(&[row("old", 3)]).is_err());
    let new = ObservationStore::open(dir.path()).unwrap();
    assert_eq!(
        new.save_batch_scoped(&[row("old", 4)], &expected)
            .unwrap_err(),
        ObservationWriteError::ScopeChanged
    );
    assert!(new.load_all().unwrap().is_empty());
    new.save(&row("new-owner", 1)).unwrap();
    assert_eq!(new.load_all().unwrap()[0].id, "new-owner");
}

#[test]
fn a_waiting_writer_checks_the_epoch_after_acquiring_the_sqlite_writer_slot() {
    let dir = tempfile::tempdir().unwrap();
    let source = ObservationStore::open(dir.path()).unwrap();
    source.save(&row("old", 1)).unwrap();
    let waiting = ObservationStore::open(dir.path()).unwrap();
    let tx = Transaction::new_unchecked(&source.0, TransactionBehavior::Immediate).unwrap();
    let barrier = std::sync::Barrier::new(2);
    std::thread::scope(|scope| {
        let ready = &barrier;
        let worker = scope.spawn(move || {
            ready.wait();
            waiting.save_batch_scoped(&[row("old", 9)], waiting.write_epoch())
        });
        barrier.wait();
        tx.execute("DELETE FROM model_observations", []).unwrap();
        content_epoch::advance(&tx, source.write_epoch()).unwrap();
        tx.commit().unwrap();
        assert_eq!(
            worker.join().unwrap().unwrap_err(),
            ObservationWriteError::ScopeChanged
        );
    });
    assert!(source.load_all().unwrap().is_empty());
}

#[test]
fn unknown_or_future_metadata_stops_writes_without_repairing_or_overwriting_raw_rows() {
    for mutation in [
        "DELETE FROM observation_content_epoch",
        "DROP TABLE observation_content_epoch",
        "UPDATE observation_content_epoch SET format=2",
        "PRAGMA user_version=2",
    ] {
        let dir = tempfile::tempdir().unwrap();
        let store = ObservationStore::open(dir.path()).unwrap();
        store.save(&row("original", 1)).unwrap();
        store.0.execute_batch(mutation).unwrap();
        assert_eq!(
            store
                .save_batch_scoped(&[row("original", 9), row("new", 1)], store.write_epoch())
                .unwrap_err(),
            ObservationWriteError::ScopeChanged
        );
        let saved = store.load_all().unwrap();
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].revision, 1);
    }
}

#[test]
fn failed_batches_roll_back_all_rows_and_retry_in_the_same_epoch() {
    let dir = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(dir.path()).unwrap();
    store.0.execute_batch("CREATE TRIGGER fail BEFORE INSERT ON model_observations WHEN new.id='second' BEGIN SELECT RAISE(ABORT,'injected');END;").unwrap();
    let batch = [row("first", 1), row("second", 1)];
    assert_eq!(
        store
            .save_batch_scoped(&batch, store.write_epoch())
            .unwrap_err(),
        ObservationWriteError::Unavailable
    );
    assert!(store.load_all().unwrap().is_empty());
    assert_eq!(content_epoch::read(&store.0).unwrap(), *store.write_epoch());
    store.0.execute_batch("DROP TRIGGER fail;").unwrap();
    store.save_batch(&batch).unwrap();
    assert_eq!(store.load_all().unwrap().len(), 2);
}

#[test]
fn a_trigger_that_changes_the_epoch_rolls_back_both_its_change_and_the_batch() {
    let dir = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(dir.path()).unwrap();
    store.0.execute_batch("CREATE TRIGGER change_epoch AFTER INSERT ON model_observations BEGIN DELETE FROM observation_content_epoch;END;").unwrap();
    assert_eq!(
        store
            .save_batch_scoped(&[row("first", 1)], store.write_epoch())
            .unwrap_err(),
        ObservationWriteError::ScopeChanged
    );
    assert!(store.load_all().unwrap().is_empty());
    assert_eq!(content_epoch::read(&store.0).unwrap(), *store.write_epoch());
}

#[test]
fn normal_same_epoch_coalescing_keeps_the_latest_revision() {
    let dir = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(dir.path()).unwrap();
    for revision in [1, 3, 2] {
        store.save(&row("same", revision)).unwrap();
    }
    let saved = store.load_all().unwrap();
    assert_eq!(saved.len(), 1);
    assert_eq!(saved[0].revision, 3);
}

#[test]
fn a_same_epoch_copy_at_the_original_path_does_not_receive_old_connection_writes() {
    let dir = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(dir.path()).unwrap();
    store.save(&row("original", 1)).unwrap();
    let path = dir.path().join("observations.sqlite3");
    let copy = dir.path().join("copy.sqlite3");
    store
        .0
        .execute("VACUUM INTO ?1", [copy.to_str().unwrap()])
        .unwrap();
    std::fs::rename(&path, dir.path().join("old.sqlite3")).unwrap();
    std::fs::rename(&copy, &path).unwrap();
    assert_eq!(
        store.save_batch_scoped(&[row("original", 9), row("late", 1)], store.write_epoch()),
        Err(ObservationWriteError::ScopeChanged)
    );
    let current = Connection::open(path).unwrap();
    assert_eq!(content_epoch::read(&current).unwrap(), *store.write_epoch());
    assert_eq!(
        current
            .query_row("SELECT count(*) FROM model_observations", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        current
            .query_row("SELECT revision FROM model_observations", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        1
    );
}

#[test]
fn a_failed_final_epoch_read_is_retryable_and_rolls_back_the_uncommitted_rows() {
    use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
    use std::sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    };
    let dir = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(dir.path()).unwrap();
    let inserted = Arc::new(AtomicBool::new(false));
    let denied = Arc::new(AtomicUsize::new(0));
    let wrote = inserted.clone();
    store
        .0
        .update_hook(Some(move |_, _: &str, table: &str, _| {
            if table == "model_observations" {
                wrote.store(true, Ordering::SeqCst);
            }
        }))
        .unwrap();
    let wrote = inserted.clone();
    let refusals = denied.clone();
    store
        .0
        .authorizer(Some(move |context: AuthContext<'_>| {
            if wrote.load(Ordering::SeqCst)
                && matches!(
                    context.action,
                    AuthAction::Pragma {
                        pragma_name: "user_version",
                        pragma_value: None
                    }
                )
            {
                refusals.fetch_add(1, Ordering::SeqCst);
                Authorization::Deny
            } else {
                Authorization::Allow
            }
        }))
        .unwrap();
    assert_eq!(
        store
            .save_batch_scoped(&[row("rejected", 1)], store.write_epoch())
            .unwrap_err(),
        ObservationWriteError::Unavailable
    );
    assert!(
        inserted.load(Ordering::SeqCst),
        "the row write ran before the injected final-read failure"
    );
    assert!(denied.load(Ordering::SeqCst) > 0);
    store
        .0
        .authorizer(None::<fn(AuthContext<'_>) -> Authorization>)
        .unwrap();
    assert!(store.load_all().unwrap().is_empty());
    assert_eq!(content_epoch::read(&store.0).unwrap(), *store.write_epoch());
    store.save(&row("retry", 1)).unwrap();
    let saved = store.load_all().unwrap();
    assert_eq!(saved.len(), 1);
    assert_eq!(saved[0].id, "retry");
}

#[test]
fn checkpoint_verification_neither_creates_missing_storage_nor_migrates_legacy_storage() {
    let dir = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(dir.path()).unwrap();
    ObservationStore::check_existing_epoch(
        &dir.path().canonicalize().unwrap(),
        store.write_epoch(),
    )
    .unwrap();
    assert!(store.load_all().unwrap().is_empty());
    let missing = tempfile::tempdir().unwrap();
    assert_eq!(
        ObservationStore::check_existing_epoch(
            &missing.path().canonicalize().unwrap(),
            store.write_epoch()
        )
        .unwrap_err(),
        ObservationWriteError::Unavailable
    );
    assert_eq!(std::fs::read_dir(missing.path()).unwrap().count(), 0);
    let legacy = tempfile::tempdir().unwrap();
    let db = rusqlite::Connection::open(legacy.path().join("observations.sqlite3")).unwrap();
    db.execute_batch("CREATE TABLE model_observations(id TEXT PRIMARY KEY,started_at TEXT NOT NULL,revision INTEGER NOT NULL,value TEXT NOT NULL);").unwrap();
    assert_eq!(
        ObservationStore::check_existing_epoch(
            &legacy.path().canonicalize().unwrap(),
            store.write_epoch()
        )
        .unwrap_err(),
        ObservationWriteError::ScopeChanged
    );
    assert_eq!(
        db.query_row("PRAGMA user_version", [], |r| r.get::<_, u32>(0))
            .unwrap(),
        0
    );
    let tables: i64 = db
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE type='table'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(tables, 1);
}
