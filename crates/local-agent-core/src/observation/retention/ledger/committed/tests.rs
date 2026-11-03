use super::*;
use crate::observation::retention::test_support::{STORE, canonical, directory, retry};

#[test]
fn canonical_scope_boundaries_keep_unrelated_requests_out_of_the_selected_group() {
    let temp = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(temp.path()).unwrap();
    let (_, mut rows) = retry();
    let mut first = rows.remove(0);
    first.id = "selected".into();
    first.logical_id = Some("same-key".into());
    first.routing = None;
    let request = canonical(vec![first.clone()]);
    let mut all = vec![first.clone()];
    for boundary in 0..4 {
        let mut other = first.clone();
        other.id = format!("other-{boundary}");
        match boundary {
            0 => other.session_id = Some("another-session".into()),
            1 => other.provider_id = Some("another-provider".into()),
            2 => other.binding_revision += 1,
            _ => other.agent_id = "codex".into(),
        }
        assert_ne!(
            crate::requests::logical_request_key(&first),
            crate::requests::logical_request_key(&other)
        );
        all.push(other);
    }
    store.save_batch(&all).unwrap();
    let ledger = store.retention_ledger(STORE).unwrap();
    ledger
        .stage_attributed_batch(&[&request], directory(), &[])
        .unwrap();
    assert_eq!(ledger.staging_summary().unwrap().attempts, 1);
    assert_eq!(store.load_all().unwrap().len(), 5);
}

#[test]
fn repeated_identical_requests_are_idempotent_but_conflicting_membership_is_rejected() {
    let temp = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(temp.path()).unwrap();
    let (request, rows) = retry();
    store.save_batch(&rows).unwrap();
    let ledger = store.retention_ledger(STORE).unwrap();
    assert_eq!(
        ledger
            .stage_attributed_batch(&[&request, &request], directory(), &[])
            .unwrap(),
        StageResult {
            inserted: 1,
            unchanged: 1,
            updated: 0,
        }
    );
    let a = canonical(rows[..2].to_vec());
    let b = canonical(rows[2..].to_vec());
    assert_eq!(
        ledger.stage_attributed_batch(&[&a, &b], directory(), &[]),
        Err(LedgerError::SourceChanged)
    );
    assert_eq!(ledger.staging_summary().unwrap().attempts, 4);
}

#[test]
fn unreadable_or_future_rows_cannot_be_assumed_to_belong_to_an_unrelated_group() {
    for raw in [
        "{",
        "null",
        "{}",
        r#"{"id":"unknown","futurePrivateField":"cannot-classify"}"#,
    ] {
        let temp = tempfile::tempdir().unwrap();
        let store = ObservationStore::open(temp.path()).unwrap();
        let (request, rows) = retry();
        store.save_batch(&rows).unwrap();
        store
            .0
            .execute(
                "INSERT INTO model_observations VALUES('unclassified','2026-01-01T00:00:00Z',1,?1)",
                [raw],
            )
            .unwrap();
        let ledger = store.retention_ledger(STORE).unwrap();
        assert_eq!(
            ledger.stage_attributed_batch(&[&request], directory(), &[]),
            Err(LedgerError::InvalidRecord)
        );
        assert_eq!(ledger.staging_summary().unwrap().requests, 0);
    }
    let temp = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(temp.path()).unwrap();
    let (request, rows) = retry();
    store.save_batch(&rows).unwrap();
    let mut future = serde_json::to_value(&rows[0]).unwrap();
    future["id"] = serde_json::json!("future");
    future["usage"]["futurePrivateField"] = serde_json::json!(42);
    store
        .0
        .execute(
            "INSERT INTO model_observations VALUES('future','2026-10-27T00:00:01Z',1,?1)",
            [future.to_string()],
        )
        .unwrap();
    let ledger = store.retention_ledger(STORE).unwrap();
    assert_eq!(
        ledger.stage_attributed_batch(&[&request], directory(), &[]),
        Err(LedgerError::InvalidRecord)
    );
    assert_eq!(ledger.staging_summary().unwrap().requests, 0);
}

#[test]
fn durable_index_columns_must_match_the_strict_source_for_every_scanned_row() {
    for change in [
        "id='another-id'",
        "started_at='2026-01-01T00:00:00Z'",
        "revision=revision+1",
    ] {
        let temp = tempfile::tempdir().unwrap();
        let store = ObservationStore::open(temp.path()).unwrap();
        let (request, rows) = retry();
        store.save_batch(&rows).unwrap();
        let ledger = store.retention_ledger(STORE).unwrap();
        store
            .0
            .execute_batch(&format!(
                "UPDATE model_observations SET {change} WHERE id='private-attempt-2'"
            ))
            .unwrap();
        assert_eq!(
            ledger.stage_attributed_batch(&[&request], directory(), &[]),
            Err(LedgerError::SourceChanged)
        );
        assert_eq!(ledger.staging_summary().unwrap().requests, 0);
    }
}

#[test]
fn row_and_elapsed_scan_budgets_never_return_a_partial_membership_proof() {
    let temp = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(temp.path()).unwrap();
    let (request, rows) = retry();
    store.save_batch(&rows).unwrap();
    let tx = Transaction::new_unchecked(&store.0, TransactionBehavior::Immediate).unwrap();
    assert_eq!(
        verify_bounded(&tx, &[&request], 3, SCAN_BUDGET, Instant::now()),
        Err(LedgerError::BatchLimit)
    );
    assert_eq!(
        verify_bounded(&tx, &[&request], 4, Duration::ZERO, Instant::now()),
        Err(LedgerError::BatchLimit)
    );
    let started = Instant::now();
    let a = verify_bounded(&tx, &[&request], 4, SCAN_BUDGET, started).unwrap();
    let b = verify_bounded(&tx, &[&request], 4, SCAN_BUDGET, started).unwrap();
    assert_eq!(a, b);
    tx.rollback().unwrap();
    assert_eq!(store.load_all().unwrap().len(), 4);
}

#[test]
fn an_oversized_unclassified_row_stops_the_whole_batch_before_facts_are_written() {
    let temp = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(temp.path()).unwrap();
    let (request, rows) = retry();
    store.save_batch(&rows).unwrap();
    store
        .0
        .execute(
            "INSERT INTO model_observations VALUES('oversized','2026-01-01T00:00:00Z',1,?1)",
            [" ".repeat(MAX_ROW_BYTES as usize + 1)],
        )
        .unwrap();
    let ledger = store.retention_ledger(STORE).unwrap();
    assert_eq!(
        ledger.stage_attributed_batch(&[&request], directory(), &[]),
        Err(LedgerError::BatchLimit)
    );
    assert_eq!(ledger.staging_summary().unwrap().requests, 0);
}

#[test]
fn mutations_by_fact_triggers_roll_back_both_facts_and_their_source_changes() {
    for change in [
        "INSERT INTO model_observations SELECT 'late',started_at,revision,json_set(value,'$.id','late') FROM model_observations WHERE id='private-attempt-0'",
        "UPDATE model_observations SET value=json_set(value,'$.usage.totalTokens',999) WHERE id='private-attempt-0'",
        "DELETE FROM model_observations WHERE id='unrelated'",
    ] {
        let temp = tempfile::tempdir().unwrap();
        let store = ObservationStore::open(temp.path()).unwrap();
        let (request, rows) = retry();
        store.save_batch(&rows).unwrap();
        let mut unrelated = rows[0].clone();
        unrelated.id = "unrelated".into();
        unrelated.logical_id = None;
        store.save(&unrelated).unwrap();
        let before = serde_json::to_value(store.load_all().unwrap()).unwrap();
        let ledger = store.retention_ledger(STORE).unwrap();
        store
            .0
            .execute_batch(&format!(
                "CREATE TRIGGER mutate_source AFTER INSERT ON retention_fact_staging BEGIN {change}; END;"
            ))
            .unwrap();
        assert_eq!(
            ledger.stage_attributed_batch(&[&request], directory(), &[]),
            Err(LedgerError::SourceChanged)
        );
        assert_eq!(ledger.staging_summary().unwrap().requests, 0);
        assert_eq!(
            serde_json::to_value(store.load_all().unwrap()).unwrap(),
            before
        );
    }
}

#[test]
fn another_sqlite_writer_cannot_add_a_retry_between_the_two_source_checks() {
    let temp = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(temp.path()).unwrap();
    let (request, rows) = retry();
    store.save_batch(&rows).unwrap();
    let external = Connection::open(temp.path().join("observations.sqlite3")).unwrap();
    external.busy_timeout(Duration::ZERO).unwrap();
    let tx = Transaction::new_unchecked(&store.0, TransactionBehavior::Immediate).unwrap();
    let before = verify(&tx, &[&request], Instant::now()).unwrap();
    let sql = "INSERT INTO model_observations SELECT 'late',started_at,revision,json_set(value,'$.id','late') FROM model_observations WHERE id='private-attempt-0'";
    let error = external.execute(sql, []).unwrap_err();
    assert_eq!(
        error.sqlite_error_code(),
        Some(rusqlite::ErrorCode::DatabaseBusy)
    );
    assert_eq!(verify(&tx, &[&request], Instant::now()).unwrap(), before);
    tx.commit().unwrap();
    external.execute(sql, []).unwrap();
    let ledger = store.retention_ledger(STORE).unwrap();
    assert_eq!(
        ledger.stage_attributed_batch(&[&request], directory(), &[]),
        Err(LedgerError::SourceChanged)
    );
    assert_eq!(ledger.staging_summary().unwrap().requests, 0);
}

#[test]
fn fact_triggers_cannot_rotate_or_damage_the_content_epoch_and_commit_old_source_facts() {
    for change in [
        "UPDATE observation_content_epoch SET value='00000000-0000-4000-8000-000000000099'",
        "UPDATE observation_content_epoch SET format=2",
        "DELETE FROM observation_content_epoch",
    ] {
        let temp = tempfile::tempdir().unwrap();
        let store = ObservationStore::open(temp.path()).unwrap();
        let (request, rows) = retry();
        store.save_batch(&rows).unwrap();
        let ledger = store.retention_ledger(STORE).unwrap();
        store.0.execute_batch(&format!("CREATE TRIGGER change_epoch AFTER INSERT ON retention_fact_staging BEGIN {change}; END;")).unwrap();
        assert_eq!(
            ledger.stage_attributed_batch(&[&request], directory(), &[]),
            Err(LedgerError::SourceChanged)
        );
        assert_eq!(ledger.staging_summary().unwrap().requests, 0);
        assert_eq!(
            crate::observation::content_epoch::read(&store.0).unwrap(),
            *store.write_epoch()
        );
        assert_eq!(store.load_all().unwrap().len(), 4);
    }
}

#[test]
fn an_unverifiable_epoch_is_never_reinitialized_while_checking_committed_sources() {
    for damage in [
        "DELETE FROM observation_content_epoch",
        "UPDATE observation_content_epoch SET format=2",
        "PRAGMA user_version=2",
    ] {
        let temp = tempfile::tempdir().unwrap();
        let store = ObservationStore::open(temp.path()).unwrap();
        let (request, rows) = retry();
        store.save_batch(&rows).unwrap();
        let ledger = store.retention_ledger(STORE).unwrap();
        store.0.execute_batch(damage).unwrap();
        assert_eq!(
            ledger.stage_attributed_batch(&[&request], directory(), &[]),
            Err(LedgerError::SourceChanged)
        );
        assert_eq!(ledger.staging_summary().unwrap().requests, 0);
        assert!(crate::observation::content_epoch::read(&store.0).is_err());
        assert_eq!(store.load_all().unwrap().len(), 4);
    }
}
