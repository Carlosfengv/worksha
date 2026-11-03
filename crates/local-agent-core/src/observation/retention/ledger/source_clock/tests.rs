use super::*;
use crate::observation::retention::test_support::{STORE, directory, retry};

#[test]
fn ordinary_and_already_open_connections_rotate_the_same_durable_clock() {
    let temp = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(temp.path()).unwrap();
    let legacy = Connection::open(temp.path().join("observations.sqlite3")).unwrap();
    let (request, rows) = retry();
    store.save_batch(&rows).unwrap();
    let ledger = store.retention_ledger(STORE).unwrap();
    let a = read(&store.0, STORE, &ledger.namespace).unwrap();
    legacy
        .execute(
            "UPDATE model_observations SET value=value || ' ' WHERE id=?1",
            [&rows[0].id],
        )
        .unwrap();
    let b = read(&store.0, STORE, &ledger.namespace).unwrap();
    assert_ne!(a.nonce, b.nonce);
    assert_eq!(a.lineage, b.lineage);
    ledger
        .stage_attributed_batch(&[&request], directory(), &[])
        .unwrap();
    assert_eq!(ledger.staging_summary().unwrap().attempts, 4);
}

#[test]
fn a_known_required_clock_is_not_reinitialized_after_its_row_disappears() {
    let temp = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(temp.path()).unwrap();
    let ledger = store.retention_ledger(STORE).unwrap();
    let namespace = ledger.namespace.clone();
    drop(ledger);
    store
        .0
        .execute("DELETE FROM retention_source_clock", [])
        .unwrap();
    assert!(matches!(
        store.retention_ledger(STORE),
        Err(LedgerError::InvalidRecord)
    ));
    assert_eq!(
        store
            .0
            .query_row("SELECT count(*) FROM retention_source_clock", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(identity(&store.0, STORE).unwrap(), namespace);
}

#[test]
fn upgrading_a_legacy_scope_preserves_its_identity_raw_bytes_and_existing_facts() {
    let temp = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(temp.path()).unwrap();
    let (request, rows) = retry();
    store.save_batch(&rows).unwrap();
    let raw = store
        .0
        .query_row(
            "SELECT group_concat(value,'|') FROM model_observations",
            [],
            |r| r.get::<_, String>(0),
        )
        .unwrap();
    let legacy = FactLedger::prepare(&store.0, STORE, None).unwrap();
    legacy
        .stage_batch(&[(&request, [1; 32])], directory())
        .unwrap();
    let namespace = legacy.namespace.clone();
    let fact = store
        .0
        .query_row("SELECT value FROM retention_fact_staging", [], |r| {
            r.get::<_, String>(0)
        })
        .unwrap();
    drop(legacy);
    let ledger = store.retention_ledger(STORE).unwrap();
    assert_eq!(ledger.namespace, namespace);
    assert_eq!(
        store
            .0
            .query_row("SELECT version FROM retention_fact_scope", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        2
    );
    assert_eq!(
        store
            .0
            .query_row("SELECT value FROM retention_fact_staging", [], |r| r
                .get::<_, String>(0))
            .unwrap(),
        fact
    );
    assert_eq!(
        store
            .0
            .query_row(
                "SELECT group_concat(value,'|') FROM model_observations",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
        raw
    );
    let marker = read(&store.0, STORE, &namespace).unwrap();
    drop(ledger);
    drop(store);
    let reopened = ObservationStore::open(temp.path()).unwrap();
    let ledger = reopened.retention_ledger(STORE).unwrap();
    assert_eq!(ledger.namespace, namespace);
    assert_eq!(read(&reopened.0, STORE, &namespace).unwrap(), marker);
}

#[test]
fn damaged_or_future_required_protocols_fail_without_repair() {
    for (damage, expected) in [
        (
            "DROP TABLE retention_source_clock",
            LedgerError::InvalidRecord,
        ),
        (
            "DROP TRIGGER retention_source_delete_v1",
            LedgerError::InvalidRecord,
        ),
        (
            "UPDATE retention_source_clock SET format=2",
            LedgerError::UnsupportedVersion,
        ),
        (
            "UPDATE retention_source_clock SET nonce=x'01'",
            LedgerError::InvalidRecord,
        ),
        (
            "UPDATE retention_source_clock SET nonce='0123456789abcdef'",
            LedgerError::InvalidRecord,
        ),
        (
            "UPDATE retention_source_clock SET store_id='00000000-0000-4000-8000-000000000099'",
            LedgerError::InvalidRecord,
        ),
        (
            "UPDATE retention_source_clock SET identity_digest=zeroblob(64)",
            LedgerError::InvalidRecord,
        ),
        (
            "ALTER TABLE retention_source_clock ADD COLUMN future INTEGER",
            LedgerError::InvalidRecord,
        ),
        (
            "CREATE TRIGGER unexpected AFTER UPDATE ON model_observations BEGIN SELECT 1;END",
            LedgerError::InvalidRecord,
        ),
        (
            "CREATE TEMP TRIGGER local_only AFTER INSERT ON main.model_observations BEGIN SELECT 1;END",
            LedgerError::InvalidRecord,
        ),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let store = ObservationStore::open(temp.path()).unwrap();
        let ledger = store.retention_ledger(STORE).unwrap();
        let namespace = ledger.namespace.clone();
        drop(ledger);
        store.0.execute_batch(damage).unwrap();
        assert_eq!(
            store.retention_ledger(STORE).err().unwrap(),
            expected,
            "{damage}"
        );
        assert_eq!(identity(&store.0, STORE).unwrap(), namespace);
        assert_eq!(
            store
                .0
                .query_row("SELECT count(*) FROM model_observations", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        if damage.starts_with("DROP TABLE") {
            assert!(!exists(&store.0, TABLE).unwrap());
        }
    }
}

#[test]
fn orphan_clock_artifacts_and_downgraded_scope_markers_cannot_create_a_new_lineage() {
    for orphan in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let store = ObservationStore::open(temp.path()).unwrap();
        let ledger = store.retention_ledger(STORE).unwrap();
        let namespace = ledger.namespace.clone();
        let marker = read(&store.0, STORE, &namespace).unwrap();
        drop(ledger);
        if orphan {
            store
                .0
                .execute_batch("DROP TABLE retention_fact_staging;DROP TABLE retention_fact_scope;")
                .unwrap();
        } else {
            store
                .0
                .execute(
                    "UPDATE retention_fact_scope SET version=1,identity_digest=?1",
                    [scope_digest(STORE, &namespace)],
                )
                .unwrap();
        }
        assert_eq!(
            store.retention_ledger(STORE).err().unwrap(),
            LedgerError::InvalidRecord
        );
        assert_eq!(
            store
                .0
                .query_row("SELECT lineage FROM retention_source_clock", [], |r| r
                    .get::<_, String>(
                    0
                ))
                .unwrap(),
            marker.lineage
        );
        assert_eq!(exists(&store.0, "retention_fact_scope").unwrap(), !orphan);
    }
}

#[test]
fn no_match_rollback_multirow_replace_and_nested_writes_have_transactional_versions() {
    let temp = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(temp.path()).unwrap();
    let (_, rows) = retry();
    store.save_batch(&rows).unwrap();
    let ledger = store.retention_ledger(STORE).unwrap();
    let before = read(&store.0, STORE, &ledger.namespace).unwrap();
    store.0.execute_batch("UPDATE model_observations SET value=value WHERE 0;DELETE FROM model_observations WHERE 0;").unwrap();
    assert_eq!(read(&store.0, STORE, &ledger.namespace).unwrap(), before);
    let changes = store.0.total_changes();
    let tx = store.0.unchecked_transaction().unwrap();
    tx.execute("UPDATE model_observations SET value=value || ' '", [])
        .unwrap();
    assert_ne!(
        read(&tx, STORE, &ledger.namespace).unwrap().nonce,
        before.nonce
    );
    assert_eq!(tx.total_changes() - changes, 8);
    tx.rollback().unwrap();
    assert_eq!(read(&store.0, STORE, &ledger.namespace).unwrap(), before);
    assert_eq!(store.0.total_changes() - changes, 8);
    store.0.execute_batch("INSERT OR REPLACE INTO model_observations SELECT * FROM model_observations WHERE id='private-attempt-0';").unwrap();
    let replaced = read(&store.0, STORE, &ledger.namespace).unwrap();
    assert_ne!(replaced.nonce, before.nonce);
    store.0.execute_batch("CREATE TABLE clock_probe(value INTEGER);CREATE TRIGGER nested AFTER INSERT ON clock_probe BEGIN UPDATE model_observations SET value=value || ' ' WHERE id='private-attempt-0';END;INSERT INTO clock_probe VALUES(1);").unwrap();
    assert_ne!(
        read(&store.0, STORE, &ledger.namespace).unwrap().nonce,
        replaced.nonce
    );
    assert_eq!(store.load_all().unwrap().len(), 4);
}

#[test]
fn writers_refuse_missing_required_clock_rows_without_committing_or_reinitializing_them() {
    let temp = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(temp.path()).unwrap();
    let (_, mut rows) = retry();
    store.save_batch(&rows).unwrap();
    let ledger = store.retention_ledger(STORE).unwrap();
    let before = serde_json::to_value(store.load_all().unwrap()).unwrap();
    store
        .0
        .execute("DELETE FROM retention_source_clock", [])
        .unwrap();
    for row in &mut rows {
        row.revision += 1;
    }
    assert!(store.save_batch(&rows).is_err());
    assert_eq!(
        serde_json::to_value(store.load_all().unwrap()).unwrap(),
        before
    );
    assert_eq!(
        store
            .0
            .query_row("SELECT count(*) FROM retention_source_clock", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert!(ledger.source.is_some());
}

#[test]
fn normal_history_hooks_coexist_before_and_after_explicit_source_initialization() {
    for history_first in [true, false] {
        let temp = tempfile::tempdir().unwrap();
        let history =
            history_first.then(|| crate::observation::sql::History::open(temp.path()).unwrap());
        let store = ObservationStore::open(temp.path()).unwrap();
        let (request, rows) = retry();
        store.save_batch(&rows).unwrap();
        let ledger = store.retention_ledger(STORE).unwrap();
        let late_history =
            (!history_first).then(|| crate::observation::sql::History::open(temp.path()).unwrap());
        let before = read(&store.0, STORE, &ledger.namespace).unwrap();
        store.save_batch(&rows).unwrap();
        assert_ne!(
            read(&store.0, STORE, &ledger.namespace).unwrap().nonce,
            before.nonce
        );
        assert_eq!(
            store
                .0
                .query_row("SELECT count(*) FROM observation_query_changes", [], |r| {
                    r.get::<_, i64>(0)
                })
                .unwrap(),
            4
        );
        ledger
            .stage_attributed_batch(&[&request], directory(), &[])
            .unwrap();
        assert_eq!(ledger.staging_summary().unwrap().attempts, 4);
        drop((history, late_history));
        store
            .0
            .execute_batch("DROP TRIGGER observation_query_insert;")
            .unwrap();
        assert_eq!(
            ledger.stage_attributed_batch(&[&request], directory(), &[]),
            Err(LedgerError::InvalidRecord)
        );
        assert!(ledger.source.as_ref().unwrap().detach().unwrap().is_none());
    }
}

#[test]
fn an_open_ledger_cannot_accept_a_scope_downgrade_even_with_a_matching_legacy_digest() {
    let temp = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(temp.path()).unwrap();
    let (request, rows) = retry();
    store.save_batch(&rows).unwrap();
    let ledger = store.retention_ledger(STORE).unwrap();
    ledger
        .stage_attributed_batch(&[&request], directory(), &[])
        .unwrap();
    store
        .0
        .execute(
            "UPDATE retention_fact_scope SET version=1,identity_digest=?1",
            [scope_digest(STORE, &ledger.namespace)],
        )
        .unwrap();
    assert_eq!(
        ledger.stage_attributed_batch(&[&request], directory(), &[]),
        Err(LedgerError::InvalidRecord)
    );
}

#[test]
fn uppercase_orphan_protocol_artifacts_never_get_a_replacement_namespace() {
    let temp = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(temp.path()).unwrap();
    store
        .0
        .execute_batch("CREATE TABLE RETENTION_SOURCE_FUTURE(value TEXT)")
        .unwrap();
    assert_eq!(
        store.retention_ledger(STORE).err(),
        Some(LedgerError::InvalidRecord)
    );
    assert!(!exists(&store.0, "retention_fact_scope").unwrap());
}
