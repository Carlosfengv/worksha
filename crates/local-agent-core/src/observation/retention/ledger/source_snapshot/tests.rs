use super::*;
use crate::observation::retention::test_support::{STORE, directory, retry};

#[test]
fn a_fixed_source_reuses_one_read_only_work_database_and_can_release_it() {
    let temp = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(temp.path()).unwrap();
    let (request, rows) = retry();
    store.save_batch(&rows).unwrap();
    let ledger = store.retention_ledger(STORE).unwrap();
    ledger
        .stage_attributed_batch(&[&request], directory(), &[])
        .unwrap();
    let source = ledger.source.as_ref().unwrap();
    let created = source.cache.borrow().as_ref().unwrap().created;
    ledger
        .stage_attributed_batch(&[&request], directory(), &[])
        .unwrap();
    assert_eq!(source.cache.borrow().as_ref().unwrap().created, created);
    ledger.release_source_snapshot();
    assert!(source.cache.borrow().is_none());
}

#[test]
fn changes_to_sources_or_failed_staging_cannot_keep_an_old_work_index() {
    let temp = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(temp.path()).unwrap();
    let (request, rows) = retry();
    store.save_batch(&rows).unwrap();
    let ledger = store.retention_ledger(STORE).unwrap();
    ledger
        .stage_attributed_batch(&[&request], directory(), &[])
        .unwrap();
    let mut late = rows[0].clone();
    late.id = "extra".into();
    store.save(&late).unwrap();
    assert_eq!(
        ledger.stage_attributed_batch(&[&request], directory(), &[]),
        Err(LedgerError::SourceChanged)
    );
    assert!(ledger.source.as_ref().unwrap().cache.borrow().is_none());
}

#[test]
fn external_commits_own_updates_and_rolled_back_writes_each_rebuild_the_index() {
    for mode in ["external_raw", "own_raw", "rollback", "external_other"] {
        let temp = tempfile::tempdir().unwrap();
        let store = ObservationStore::open(temp.path()).unwrap();
        let (request, rows) = retry();
        store.save_batch(&rows).unwrap();
        store
            .0
            .execute_batch("CREATE TABLE unrelated(value INTEGER);INSERT INTO unrelated VALUES(1);")
            .unwrap();
        let ledger = store.retention_ledger(STORE).unwrap();
        ledger
            .stage_attributed_batch(&[&request], directory(), &[])
            .unwrap();
        let source = ledger.source.as_ref().unwrap();
        let created = source.cache.borrow().as_ref().unwrap().created;
        let sql = "UPDATE model_observations SET value=value || ' ' WHERE id='private-attempt-0'";
        match mode {
            "external_raw" => {
                Connection::open(temp.path().join("observations.sqlite3"))
                    .unwrap()
                    .execute(sql, [])
                    .unwrap();
            }
            "own_raw" => {
                store.0.execute(sql, []).unwrap();
            }
            "rollback" => {
                let tx = store.0.unchecked_transaction().unwrap();
                tx.execute(sql, []).unwrap();
                tx.rollback().unwrap();
            }
            _ => {
                Connection::open(temp.path().join("observations.sqlite3"))
                    .unwrap()
                    .execute("UPDATE unrelated SET value=2", [])
                    .unwrap();
            }
        }
        ledger
            .stage_attributed_batch(&[&request], directory(), &[])
            .unwrap();
        assert_ne!(
            source.cache.borrow().as_ref().unwrap().created,
            created,
            "{mode}"
        );
        assert_eq!(ledger.staging_summary().unwrap().attempts, 4);
    }
}

#[test]
fn unknown_rows_after_a_cached_scan_cannot_use_the_old_membership_proof() {
    let temp = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(temp.path()).unwrap();
    let (request, rows) = retry();
    store.save_batch(&rows).unwrap();
    let ledger = store.retention_ledger(STORE).unwrap();
    ledger
        .stage_attributed_batch(&[&request], directory(), &[])
        .unwrap();
    let external = Connection::open(temp.path().join("observations.sqlite3")).unwrap();
    external
        .execute(
            "INSERT INTO model_observations VALUES('unknown','2026-01-01T00:00:00Z',1,'{}')",
            [],
        )
        .unwrap();
    assert_eq!(
        ledger.stage_attributed_batch(&[&request], directory(), &[]),
        Err(LedgerError::InvalidRecord)
    );
    assert!(ledger.source.as_ref().unwrap().cache.borrow().is_none());
    assert_eq!(ledger.staging_summary().unwrap().attempts, 4);
}

#[test]
fn restoring_a_nonce_cannot_hide_raw_or_extra_trigger_writes() {
    for change_raw in [true, false] {
        let temp = tempfile::tempdir().unwrap();
        let store = ObservationStore::open(temp.path()).unwrap();
        let (request, rows) = retry();
        store.save_batch(&rows).unwrap();
        let ledger = store.retention_ledger(STORE).unwrap();
        store.0.execute_batch("CREATE TABLE saved_nonce(value BLOB);INSERT INTO saved_nonce SELECT nonce FROM retention_source_clock;").unwrap();
        let change = if change_raw {
            "UPDATE model_observations SET value=value || ' ' WHERE id='private-attempt-0';UPDATE retention_source_clock SET nonce=(SELECT value FROM saved_nonce);"
        } else {
            "INSERT INTO saved_nonce SELECT nonce FROM retention_source_clock;"
        };
        store.0.execute_batch(&format!("CREATE TRIGGER side_effect AFTER INSERT ON retention_fact_staging BEGIN {change} END;")).unwrap();
        let before = serde_json::to_value(store.load_all().unwrap()).unwrap();
        let marker = source_clock::read(&store.0, STORE, &ledger.namespace).unwrap();
        assert_eq!(
            ledger.stage_attributed_batch(&[&request], directory(), &[]),
            Err(LedgerError::SourceChanged)
        );
        assert_eq!(ledger.staging_summary().unwrap().requests, 0);
        assert_eq!(
            source_clock::read(&store.0, STORE, &ledger.namespace).unwrap(),
            marker
        );
        assert_eq!(
            serde_json::to_value(store.load_all().unwrap()).unwrap(),
            before
        );
        assert_eq!(
            store
                .0
                .query_row("SELECT count(*) FROM saved_nonce", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert!(ledger.source.as_ref().unwrap().cache.borrow().is_none());
    }
}

#[test]
fn corrupted_private_work_databases_are_rejected_and_released() {
    for damage in [
        "PRAGMA query_only=OFF",
        "PRAGMA query_only=OFF;DELETE FROM members;PRAGMA query_only=ON",
        "PRAGMA query_only=OFF;CREATE TEMP TABLE members(value TEXT);PRAGMA query_only=ON",
    ] {
        let temp = tempfile::tempdir().unwrap();
        let store = ObservationStore::open(temp.path()).unwrap();
        let (request, rows) = retry();
        store.save_batch(&rows).unwrap();
        let ledger = store.retention_ledger(STORE).unwrap();
        ledger
            .stage_attributed_batch(&[&request], directory(), &[])
            .unwrap();
        let source = ledger.source.as_ref().unwrap();
        source
            .cache
            .borrow()
            .as_ref()
            .unwrap()
            .work
            .execute_batch(damage)
            .unwrap();
        assert_eq!(
            ledger.stage_attributed_batch(&[&request], directory(), &[]),
            Err(LedgerError::InvalidRecord)
        );
        assert!(source.cache.borrow().is_none());
        assert_eq!(ledger.staging_summary().unwrap().requests, 1);
    }
}

#[test]
fn expired_and_failed_batches_release_the_old_index_without_reusing_it() {
    let temp = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(temp.path()).unwrap();
    let (request, rows) = retry();
    store.save_batch(&rows).unwrap();
    let ledger = store.retention_ledger(STORE).unwrap();
    ledger
        .stage_attributed_batch(&[&request], directory(), &[])
        .unwrap();
    let source = ledger.source.as_ref().unwrap();
    let expired = Instant::now() - MAX_AGE;
    source.cache.borrow_mut().as_mut().unwrap().created = expired;
    ledger
        .stage_attributed_batch(&[&request], directory(), &[])
        .unwrap();
    assert!(source.cache.borrow().as_ref().unwrap().created > expired);
    assert_eq!(
        ledger.stage_attributed_batch(&vec![&request; MAX_BATCH + 1], directory(), &[]),
        Err(LedgerError::BatchLimit)
    );
    assert!(source.cache.borrow().is_none());
    ledger
        .stage_attributed_batch(&[&request], directory(), &[])
        .unwrap();
    store
        .0
        .execute_batch("CREATE TEMP TABLE model_observations(value TEXT)")
        .unwrap();
    assert_eq!(
        ledger.stage_attributed_batch(&[&request], directory(), &[]),
        Err(LedgerError::InvalidRecord)
    );
    assert!(source.cache.borrow().is_none());
}

#[test]
fn connection_epoch_and_lineage_are_not_transferable_snapshot_permissions() {
    let temp = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(temp.path()).unwrap();
    let ledger = store.retention_ledger(STORE).unwrap();
    let source = ledger.source.as_ref().unwrap();
    let other = Connection::open(temp.path().join("observations.sqlite3")).unwrap();
    let tx = other.unchecked_transaction().unwrap();
    assert_eq!(
        source.header(&tx, STORE, &ledger.namespace),
        Err(LedgerError::SourceChanged)
    );
    tx.rollback().unwrap();
    let tx = store.0.unchecked_transaction().unwrap();
    crate::observation::content_epoch::advance(&tx, store.write_epoch()).unwrap();
    assert_eq!(
        source.header(&tx, STORE, &ledger.namespace),
        Err(LedgerError::SourceChanged)
    );
    tx.rollback().unwrap();
    let different = Source::new(
        &store.0,
        &store.2,
        &store.1,
        uuid::Uuid::new_v4().to_string(),
    );
    let tx = store.0.unchecked_transaction().unwrap();
    assert_eq!(
        different.header(&tx, STORE, &ledger.namespace),
        Err(LedgerError::SourceChanged)
    );
}

#[test]
fn snapshot_capture_obeys_real_row_byte_time_and_disk_page_budgets() {
    let temp = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(temp.path()).unwrap();
    let (_, rows) = retry();
    store.save_batch(&rows).unwrap();
    let ledger = store.retention_ledger(STORE).unwrap();
    let tx = store.0.unchecked_transaction().unwrap();
    let header = ledger
        .source
        .as_ref()
        .unwrap()
        .header(&tx, STORE, &ledger.namespace)
        .unwrap();
    for (limits, pages) in [
        (
            committed::Limits {
                rows: 3,
                ..Default::default()
            },
            MAX_WORK_PAGES,
        ),
        (
            committed::Limits {
                row_bytes: 1,
                ..Default::default()
            },
            MAX_WORK_PAGES,
        ),
        (
            committed::Limits {
                budget: Duration::ZERO,
                ..Default::default()
            },
            MAX_WORK_PAGES,
        ),
        (committed::Limits::default(), 1),
    ] {
        assert_eq!(
            Snapshot::capture(
                &tx,
                STORE,
                &ledger.namespace,
                header.clone(),
                Instant::now(),
                limits,
                pages
            )
            .err(),
            Some(LedgerError::BatchLimit)
        );
    }
    assert_eq!(
        tx.query_row("SELECT count(*) FROM retention_fact_staging", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        tx.query_row("SELECT count(*) FROM model_observations", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        4
    );
}

#[test]
fn uppercase_temp_tables_cannot_redirect_main_source_or_fact_access() {
    for (name, table) in [
        ("RETENTION_FACT_STAGING", "retention_fact_staging"),
        ("RetenTion_Fact_Scope", "retention_fact_scope"),
        ("MODEL_OBSERVATIONS", "model_observations"),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let store = ObservationStore::open(temp.path()).unwrap();
        let (request, rows) = retry();
        store.save_batch(&rows).unwrap();
        let ledger = store.retention_ledger(STORE).unwrap();
        store
            .0
            .execute_batch(&format!(
                "CREATE TEMP TABLE {name} AS SELECT * FROM main.{table}"
            ))
            .unwrap();
        assert_eq!(
            ledger.stage_attributed_batch(&[&request], directory(), &[]),
            Err(LedgerError::InvalidRecord),
            "{name}"
        );
        assert_eq!(
            store
                .0
                .query_row(
                    "SELECT count(*) FROM main.retention_fact_staging",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
        assert!(ledger.source.as_ref().unwrap().cache.borrow().is_none());
    }
}
