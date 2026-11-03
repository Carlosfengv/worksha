use super::*;
use crate::observation::tests::request as observation;
use std::collections::HashMap;

const STORE: &str = "00000000-0000-4000-8000-000000000001";
fn request(model: &str) -> Request {
    let mut row = observation("2026-10-01T00:00:00Z".parse().unwrap());
    row.id = format!("private-request-{model}");
    row.model = Some(model.into());
    row.session_id = Some("private-session".into());
    row.endpoint = "https://private-endpoint.test".into();
    crate::requests::logical_requests(vec![row], &HashMap::new()).remove(0)
}
fn raw(store: &ObservationStore) -> Vec<(String, String, i64, String)> {
    store
        .0
        .prepare("SELECT id,started_at,revision,value FROM model_observations ORDER BY id")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
}
fn count(store: &ObservationStore) -> i64 {
    store
        .0
        .query_row("SELECT count(*) FROM retention_fact_staging", [], |r| {
            r.get(0)
        })
        .unwrap()
}
fn directory() -> &'static AgentDirectory {
    crate::agents::builtin_directory()
}

#[test]
fn staging_is_durable_idempotent_and_bound_to_the_original_database_identity() {
    let temp = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(temp.path()).unwrap();
    let mut request = request("model");
    store.save(&request.attempts[0].observation).unwrap();
    let source = serde_json::to_string(&raw(&store)).unwrap();
    let ledger = store.retention_ledger(STORE).unwrap();
    assert_eq!(
        ledger
            .stage_batch(&[(&request, [1; 32])], directory())
            .unwrap(),
        StageResult {
            inserted: 1,
            ..Default::default()
        }
    );
    let namespace = ledger.namespace.clone();
    assert_eq!(
        ledger
            .stage_batch(&[(&request, [1; 32])], directory())
            .unwrap(),
        StageResult {
            unchanged: 1,
            ..Default::default()
        }
    );
    request.attempts[0].observation.usage.total_tokens = Some(123);
    request = crate::requests::logical_requests(
        request
            .attempts
            .iter()
            .map(|a| a.observation.clone())
            .collect(),
        &HashMap::new(),
    )
    .remove(0);
    assert_eq!(
        ledger
            .stage_batch(&[(&request, [2; 32])], directory())
            .unwrap(),
        StageResult {
            updated: 1,
            ..Default::default()
        }
    );
    assert_eq!(
        ledger.staging_summary().unwrap().usage.total_tokens,
        Some(123)
    );
    assert_eq!(serde_json::to_string(&raw(&store)).unwrap(), source);
    drop(ledger);
    drop(store);
    let reopened = ObservationStore::open(temp.path()).unwrap();
    let ledger = reopened.retention_ledger(STORE).unwrap();
    assert_eq!(ledger.namespace, namespace);
    assert_eq!(count(&reopened), 1);
    assert_eq!(
        ledger.staging_summary().unwrap().usage.total_tokens,
        Some(123)
    );
    assert_eq!(
        reopened
            .retention_ledger("00000000-0000-4000-8000-000000000002")
            .err()
            .unwrap(),
        LedgerError::WrongStore
    );
    let saved: String = reopened
        .0
        .query_row("SELECT value FROM retention_fact_staging", [], |r| r.get(0))
        .unwrap();
    assert!(!saved.contains("private-"));
    assert_eq!(serde_json::to_string(&raw(&reopened)).unwrap(), source);
}

#[test]
fn batch_write_error_or_ignored_insert_rolls_back_all_staging_and_keeps_raw_rows() {
    for failure in ["RAISE(ABORT,'private-failure-message')", "RAISE(IGNORE)"] {
        let temp = tempfile::tempdir().unwrap();
        let store = ObservationStore::open(temp.path()).unwrap();
        let a = request("ok");
        let b = request("fail");
        store
            .save_batch(&[
                a.attempts[0].observation.clone(),
                b.attempts[0].observation.clone(),
            ])
            .unwrap();
        let before = serde_json::to_string(&raw(&store)).unwrap();
        let ledger = store.retention_ledger(STORE).unwrap();
        store.0.execute_batch(&format!("CREATE TRIGGER fixture_write_failure BEFORE INSERT ON retention_fact_staging WHEN json_extract(new.value,'$.model')='fail' BEGIN SELECT {failure};END;")).unwrap();
        assert_eq!(
            ledger.stage_batch(&[(&a, [1; 32]), (&b, [2; 32])], directory()),
            Err(LedgerError::Unavailable)
        );
        assert_eq!(count(&store), 0);
        assert_eq!(serde_json::to_string(&raw(&store)).unwrap(), before);
        store
            .0
            .execute_batch("DROP TRIGGER fixture_write_failure")
            .unwrap();
        assert_eq!(
            ledger
                .stage_batch(&[(&a, [1; 32]), (&b, [2; 32])], directory())
                .unwrap()
                .inserted,
            2
        );
    }
}

#[test]
fn protected_facts_cannot_be_replaced_by_a_new_staging_request() {
    let temp = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(temp.path()).unwrap();
    let request = request("model");
    let ledger = store.retention_ledger(STORE).unwrap();
    ledger
        .stage_batch(&[(&request, [1; 32])], directory())
        .unwrap();
    store
        .0
        .execute("UPDATE retention_fact_staging SET state='sealed'", [])
        .unwrap();
    let saved: String = store
        .0
        .query_row("SELECT value FROM retention_fact_staging", [], |r| r.get(0))
        .unwrap();
    assert_eq!(
        ledger.stage_batch(&[(&request, [2; 32])], directory()),
        Err(LedgerError::ProtectedFact)
    );
    assert_eq!(ledger.staging_summary().unwrap().requests, 0);
    let kept: String = store
        .0
        .query_row("SELECT value FROM retention_fact_staging", [], |r| r.get(0))
        .unwrap();
    assert_eq!(kept, saved);
}

#[test]
fn missing_invalid_or_future_identity_is_never_reinitialized() {
    for (sql, expected) in [
        (
            "DELETE FROM retention_fact_scope",
            LedgerError::InvalidRecord,
        ),
        (
            "UPDATE retention_fact_scope SET namespace='invalid'",
            LedgerError::InvalidRecord,
        ),
        (
            "UPDATE retention_fact_scope SET namespace='00000000-0000-4000-8000-000000000099'",
            LedgerError::InvalidRecord,
        ),
        (
            "UPDATE retention_fact_scope SET version=3",
            LedgerError::UnsupportedVersion,
        ),
        (
            "ALTER TABLE retention_fact_staging ADD COLUMN future_backup TEXT",
            LedgerError::InvalidRecord,
        ),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let store = ObservationStore::open(temp.path()).unwrap();
        let request = request("model");
        let ledger = store.retention_ledger(STORE).unwrap();
        ledger
            .stage_batch(&[(&request, [1; 32])], directory())
            .unwrap();
        drop(ledger);
        let original: String = store
            .0
            .query_row("SELECT value FROM retention_fact_staging", [], |r| r.get(0))
            .unwrap();
        store.0.execute_batch(sql).unwrap();
        assert_eq!(store.retention_ledger(STORE).err().unwrap(), expected);
        assert_eq!(count(&store), 1);
        assert_eq!(
            store
                .0
                .query_row("SELECT value FROM retention_fact_staging", [], |r| r
                    .get::<_, String>(0))
                .unwrap(),
            original
        );
    }
}

#[test]
fn malformed_or_unknown_projection_is_not_skipped_overwritten_or_reported_as_empty() {
    for change in ["json", "future-top", "future-summary", "source"] {
        let temp = tempfile::tempdir().unwrap();
        let store = ObservationStore::open(temp.path()).unwrap();
        let request = request("model");
        let ledger = store.retention_ledger(STORE).unwrap();
        ledger
            .stage_batch(&[(&request, [1; 32])], directory())
            .unwrap();
        let original: String = store
            .0
            .query_row("SELECT value FROM retention_fact_staging", [], |r| r.get(0))
            .unwrap();
        let mut value: serde_json::Value = serde_json::from_str(&original).unwrap();
        let changed = match change {
            "json" => "{private-raw-sentinel".into(),
            "future-top" => {
                value["future"] = serde_json::json!("keep");
                value.to_string()
            }
            "future-summary" => {
                value["stats"]["futureUsage"] = serde_json::json!(42);
                value.to_string()
            }
            "source" => original.clone(),
            _ => unreachable!(),
        };
        store.0.execute("UPDATE retention_fact_staging SET value=?1,projection_digest=?2,source_fingerprint=?3",params![changed,digest(&changed),if change=="source"{"invalid"}else{"0101010101010101010101010101010101010101010101010101010101010101"}]).unwrap();
        assert!(matches!(
            ledger.staging_summary(),
            Err(LedgerError::InvalidRecord)
        ));
        assert_eq!(
            ledger.stage_batch(&[(&request, [1; 32])], directory()),
            Err(LedgerError::InvalidRecord)
        );
        assert_eq!(
            store
                .0
                .query_row("SELECT value FROM retention_fact_staging", [], |r| r
                    .get::<_, String>(0))
                .unwrap(),
            changed
        );
    }
}

#[test]
fn bounded_staging_rejects_oversized_batch_before_mutating_facts() {
    let temp = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(temp.path()).unwrap();
    let ledger = store.retention_ledger(STORE).unwrap();
    let request = request("model");
    assert_eq!(
        ledger.stage_batch(&vec![(&request, [0; 32]); 65], directory()),
        Err(LedgerError::BatchLimit)
    );
    assert_eq!(count(&store), 0);
    assert_eq!(
        ledger.stage_batch(&[], directory()).unwrap(),
        StageResult::default()
    );
}
