use super::*;
use crate::observation::{ObservationStore, sql::History};
use std::path::Path;

const RAW: &str = r#"{"synthetic":"private-metadata","integer":18446744073709551615}"#;

fn legacy(directory: &Path, revision_type: &str) -> Connection {
    std::fs::create_dir_all(directory).unwrap();
    let db = Connection::open(directory.join("observations.sqlite3")).unwrap();
    db.execute_batch(&format!("CREATE TABLE model_observations(id TEXT PRIMARY KEY,started_at TEXT NOT NULL,revision {revision_type} NOT NULL,value TEXT NOT NULL);")).unwrap();
    db.execute(
        "INSERT INTO model_observations VALUES('fixture','2026-01-01T00:00:00Z',1,?1)",
        [RAW],
    )
    .unwrap();
    db
}

fn raw(db: &Connection) -> String {
    db.query_row(
        "SELECT value FROM model_observations WHERE id='fixture'",
        [],
        |r| r.get(0),
    )
    .unwrap()
}

#[test]
fn new_and_legacy_stores_keep_one_epoch_across_reopen_without_consent_or_fact_scope() {
    for populated in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        if populated {
            drop(legacy(dir.path(), "INTEGER"));
        }
        let store = ObservationStore::open(dir.path()).unwrap();
        let token = read(&store.0).unwrap();
        assert_eq!(version(&store.0).unwrap(), 1);
        if populated {
            assert_eq!(raw(&store.0), RAW);
        }
        let scopes: i64 = store.0.query_row("SELECT count(*) FROM sqlite_master WHERE name LIKE 'retention_%' OR name LIKE '%consent%'", [], |r| r.get(0)).unwrap();
        assert_eq!(scopes, 0);
        drop(store);
        let reopened = ObservationStore::open(dir.path()).unwrap();
        assert_eq!(read(&reopened.0).unwrap(), token);
        if populated {
            assert_eq!(raw(&reopened.0), RAW);
        }
    }
}

#[test]
fn failed_additive_migration_rolls_back_header_and_metadata_without_changing_raw_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let db = legacy(dir.path(), "TEXT");
    assert!(ObservationStore::open(dir.path()).is_err());
    assert_eq!(version(&db).unwrap(), 0);
    assert_eq!(raw(&db), RAW);
    let created: bool = db
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='observation_content_epoch')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(!created);
    db.execute_batch("ALTER TABLE model_observations RENAME TO bad_fixture;CREATE TABLE model_observations(id TEXT PRIMARY KEY,started_at TEXT NOT NULL,revision INTEGER NOT NULL,value TEXT NOT NULL);INSERT INTO model_observations SELECT * FROM bad_fixture;DROP TABLE bad_fixture;").unwrap();
    assert!(ObservationStore::open(dir.path()).is_ok());
    assert_eq!(raw(&db), RAW);
}

#[test]
fn future_versions_are_rejected_before_changing_journal_mode_or_source() {
    let dir = tempfile::tempdir().unwrap();
    let db = legacy(dir.path(), "INTEGER");
    db.execute_batch("PRAGMA user_version=2;").unwrap();
    assert!(ObservationStore::open(dir.path()).is_err());
    assert_eq!(version(&db).unwrap(), 2);
    let journal: String = db
        .query_row("PRAGMA journal_mode", [], |r| r.get(0))
        .unwrap();
    assert_eq!(journal, "delete");
    assert_eq!(raw(&db), RAW);
}

#[test]
fn missing_corrupt_future_and_downgraded_epoch_records_are_never_reinitialized() {
    for mutation in [
        "DELETE FROM observation_content_epoch",
        "DROP TABLE observation_content_epoch",
        "UPDATE observation_content_epoch SET format=2",
        "UPDATE observation_content_epoch SET value='invalid'",
        "UPDATE observation_content_epoch SET value='AAAAAAAA-AAAA-4AAA-8AAA-AAAAAAAAAAAA'",
        "UPDATE observation_content_epoch SET value=zeroblob(1000000)",
        "ALTER TABLE observation_content_epoch ADD COLUMN future TEXT",
        "PRAGMA user_version=0",
        "PRAGMA user_version=2",
    ] {
        let dir = tempfile::tempdir().unwrap();
        drop(legacy(dir.path(), "INTEGER"));
        let store = ObservationStore::open(dir.path()).unwrap();
        store.0.execute_batch(mutation).unwrap();
        let before_version = version(&store.0).unwrap();
        let before_schema: String = store
            .0
            .query_row(
                "SELECT group_concat(sql,';') FROM (SELECT sql FROM sqlite_master ORDER BY name)",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(ObservationStore::open(dir.path()).is_err(), "{mutation}");
        assert!(read(&store.0).is_err(), "{mutation}");
        assert_eq!(version(&store.0).unwrap(), before_version);
        let after_schema: String = store
            .0
            .query_row(
                "SELECT group_concat(sql,';') FROM (SELECT sql FROM sqlite_master ORDER BY name)",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(after_schema, before_schema);
        assert_eq!(raw(&store.0), RAW);
    }
}

#[test]
fn readonly_checks_do_not_migrate_a_legacy_file_or_create_missing_metadata() {
    let dir = tempfile::tempdir().unwrap();
    let db = legacy(dir.path(), "INTEGER");
    assert_eq!(read(&db).unwrap_err(), EpochError::UnsupportedVersion);
    assert_eq!(version(&db).unwrap(), 0);
    let tables: i64 = db
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE type='table'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(tables, 1);
    drop(db);
    let store = ObservationStore::open(dir.path()).unwrap();
    let readonly = Connection::open_with_flags(
        dir.path().join("observations.sqlite3"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    assert_eq!(read(&readonly).unwrap(), read(&store.0).unwrap());
}

#[test]
fn content_removal_and_epoch_advance_publish_together_to_another_wal_connection() {
    let dir = tempfile::tempdir().unwrap();
    drop(legacy(dir.path(), "INTEGER"));
    let store = ObservationStore::open(dir.path()).unwrap();
    let reader = ObservationStore::open(dir.path()).unwrap();
    let old = read(&reader.0).unwrap();
    let tx = Transaction::new_unchecked(&store.0, TransactionBehavior::Immediate).unwrap();
    tx.execute("DELETE FROM model_observations", []).unwrap();
    let next = advance(&tx, &old).unwrap();
    assert_ne!(next, old);
    assert_eq!(read(&reader.0).unwrap(), old);
    assert_eq!(raw(&reader.0), RAW);
    tx.commit().unwrap();
    assert_eq!(read(&reader.0).unwrap(), next);
    assert_eq!(
        reader
            .0
            .query_row("SELECT count(*) FROM model_observations", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
    drop(store);
    assert_eq!(
        read(&ObservationStore::open(dir.path()).unwrap().0).unwrap(),
        next
    );
}

#[test]
fn epoch_write_failures_and_ignored_or_rewritten_updates_roll_back_the_content_change() {
    for trigger in [
        "CREATE TRIGGER fail BEFORE UPDATE ON observation_content_epoch BEGIN SELECT RAISE(ABORT,'injected');END;",
        "CREATE TRIGGER fail BEFORE UPDATE ON observation_content_epoch BEGIN SELECT RAISE(IGNORE);END;",
        "CREATE TRIGGER fail AFTER UPDATE ON observation_content_epoch BEGIN UPDATE observation_content_epoch SET value=old.value;END;",
    ] {
        let dir = tempfile::tempdir().unwrap();
        drop(legacy(dir.path(), "INTEGER"));
        let store = ObservationStore::open(dir.path()).unwrap();
        let old = read(&store.0).unwrap();
        store.0.execute_batch(trigger).unwrap();
        let tx = Transaction::new_unchecked(&store.0, TransactionBehavior::Immediate).unwrap();
        tx.execute("DELETE FROM model_observations", []).unwrap();
        assert!(advance(&tx, &old).is_err());
        drop(tx);
        assert_eq!(raw(&store.0), RAW);
        assert_eq!(read(&store.0).unwrap(), old);
    }
}

#[test]
fn a_token_from_another_store_cannot_advance_or_publish_its_pending_content_change() {
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    drop(legacy(first.path(), "INTEGER"));
    let store = ObservationStore::open(first.path()).unwrap();
    let other = ObservationStore::open(second.path()).unwrap();
    let old = read(&store.0).unwrap();
    let tx = Transaction::new_unchecked(&store.0, TransactionBehavior::Immediate).unwrap();
    tx.execute("DELETE FROM model_observations", []).unwrap();
    assert_eq!(
        advance(&tx, &read(&other.0).unwrap()).unwrap_err(),
        EpochError::Changed
    );
    drop(tx);
    assert_eq!(raw(&store.0), RAW);
    assert_eq!(read(&store.0).unwrap(), old);
}

#[test]
fn concurrent_legacy_openers_observe_one_committed_epoch() {
    let dir = tempfile::tempdir().unwrap();
    let db = legacy(dir.path(), "INTEGER");
    db.execute_batch("PRAGMA journal_mode=WAL;").unwrap();
    let barrier = std::sync::Barrier::new(3);
    let tokens = std::thread::scope(|scope| {
        let mut workers = vec![];
        for _ in 0..3 {
            let barrier = &barrier;
            let path = dir.path();
            workers.push(scope.spawn(move || {
                barrier.wait();
                let store = ObservationStore::open(path).unwrap();
                read(&store.0).unwrap()
            }));
        }
        workers
            .into_iter()
            .map(|w| w.join().unwrap())
            .collect::<Vec<_>>()
    });
    assert!(tokens.iter().all(|token| token == &tokens[0]));
    assert_eq!(raw(&db), RAW);
}

#[test]
fn history_refuses_a_token_pinned_by_an_old_read_transaction() {
    let dir = tempfile::tempdir().unwrap();
    let history = History::open(dir.path()).unwrap();
    let writer = ObservationStore::open(dir.path()).unwrap();
    assert!(history.content_is_current().unwrap());
    history.db.execute_batch("BEGIN;").unwrap();
    let pinned = read(&history.db).unwrap();
    let tx = Transaction::new_unchecked(&writer.0, TransactionBehavior::Immediate).unwrap();
    advance(&tx, &pinned).unwrap();
    tx.commit().unwrap();
    assert_eq!(read(&history.db).unwrap(), pinned);
    assert!(history.content_is_current().is_err());
    history.db.execute_batch("COMMIT;").unwrap();
    assert!(!history.content_is_current().unwrap());
}

#[test]
fn opening_a_current_store_does_not_wait_for_an_unrelated_wal_writer() {
    let dir = tempfile::tempdir().unwrap();
    let writer = ObservationStore::open(dir.path()).unwrap();
    let tx = Transaction::new_unchecked(&writer.0, TransactionBehavior::Immediate).unwrap();
    let started = std::time::Instant::now();
    let reader = ObservationStore::open(dir.path()).unwrap();
    assert!(started.elapsed() < std::time::Duration::from_secs(1));
    assert_eq!(read(&reader.0).unwrap(), read(&tx).unwrap());
}

#[test]
fn temp_tables_cannot_replace_or_repair_the_persistent_epoch() {
    let dir = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(dir.path()).unwrap();
    let old = read(&store.0).unwrap();
    store.0.execute_batch("CREATE TEMP TABLE observation_content_epoch(id INTEGER PRIMARY KEY,format INTEGER NOT NULL,value TEXT NOT NULL);").unwrap();
    store
        .0
        .execute(
            "INSERT INTO temp.observation_content_epoch VALUES(1,1,?1)",
            [uuid::Uuid::new_v4().to_string()],
        )
        .unwrap();
    assert_eq!(read(&store.0).unwrap(), old);
    store
        .0
        .execute_batch("DROP TABLE main.observation_content_epoch;")
        .unwrap();
    assert!(read(&store.0).is_err());
    assert!(ObservationStore::open(dir.path()).is_err());
}
