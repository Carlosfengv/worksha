use super::*;
use crate::observation::retention::{ExpirationCandidate, GatewayFact};
use crate::observation::{ModelObservation, ObservationStore};

fn now() -> DateTime<Utc> {
    "2026-10-27T00:00:00Z".parse().unwrap()
}
fn row(id: &str, at: &str) -> ModelObservation {
    let mut r = crate::observation::tests::request(at.parse().unwrap());
    r.id = id.into();
    r.logical_id = Some(id.into());
    r.status = "success".into();
    r.ended_at = Some(at.into());
    r
}
fn raw(store: &ObservationStore) -> Vec<(String, String)> {
    store
        .0
        .prepare("SELECT id,value FROM model_observations ORDER BY id")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}

#[test]
fn preview_uses_full_logical_groups_and_matches_fact_boundaries_without_writes() {
    let dir = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(dir.path()).unwrap();
    let cutoff = "2026-09-27T00:00:00Z";
    let mut rows = vec![
        row("old", "2026-09-26T23:59:59.999999999Z"),
        row("equal", cutoff),
        row("new", "2026-10-01T00:00:00Z"),
    ];
    for (id, status) in [
        ("active", "in_progress"),
        ("unknown", "future"),
        ("interrupted", "interrupted"),
        ("failed", "failed"),
        ("cancelled", "cancelled"),
        ("incomplete", "incomplete"),
    ] {
        let mut r = row(id, "2026-08-01T00:00:00Z");
        r.status = status.into();
        rows.push(r);
    }
    let mut bad = row("time", "2026-08-01T00:00:00Z");
    bad.ended_at = None;
    rows.push(bad);
    let mut reverse = row("reverse", "2026-08-01T00:00:00Z");
    reverse.ended_at = Some("2026-07-31T00:00:00Z".into());
    rows.push(reverse);
    let mut retry = row("old-retry", "2026-10-26T00:00:00Z");
    retry.logical_id = Some("old".into());
    rows.push(retry);
    store.save_batch(&rows).unwrap();
    let before = raw(&store);
    let result = gateway(dir.path(), 30, now()).unwrap();
    let requests = crate::requests::logical_requests(rows, &Default::default());
    assert_eq!(result.attempts, 12);
    assert_eq!(result.logical_requests, requests.len() as u64);
    let mut expired = 0;
    for r in requests {
        let f = GatewayFact::from_request(
            &r,
            super::super::test_support::STORE,
            crate::agents::builtin_directory(),
        )
        .unwrap();
        expired +=
            u64::from(f.expiration_candidate(30, now()).unwrap() == ExpirationCandidate::Expired);
    }
    assert_eq!(result.expired, expired);
    assert_eq!(result.expired, 3);
    assert_eq!(result.within_window, 3);
    assert_eq!(result.protected.active, 1);
    assert_eq!(result.protected.unknown_outcome, 2);
    assert_eq!(result.protected.unknown_time, 2);
    assert!(result.classification_complete);
    assert_eq!(before, raw(&store));
    let namespaces: i64 = store
        .0
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE name LIKE 'retention_%'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(namespaces, 0);
}

#[test]
fn routed_provider_retries_are_one_request_and_any_active_attempt_protects_group() {
    let dir = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(dir.path()).unwrap();
    let (_, mut rows) = super::super::test_support::retry();
    for r in &mut rows {
        r.started_at = "2026-08-01T00:00:00Z".into();
        r.ended_at = Some(r.started_at.clone());
    }
    rows[0].status = "in_progress".into();
    rows[0].ended_at = None;
    store.save_batch(&rows).unwrap();
    let result = gateway(dir.path(), 7, now()).unwrap();
    assert_eq!(result.logical_requests, 1);
    assert_eq!(result.attempts, 4);
    assert_eq!(result.protected.active, 1);
    assert_eq!(result.expired, 0);
}

#[test]
fn unknown_nested_or_null_fields_are_never_accepted_for_retention() {
    let (_, rows) = super::super::test_support::retry();
    for original in rows {
        let good = serde_json::to_string(&original).unwrap();
        assert!(source::observation(&good).is_ok());
        let value = serde_json::to_value(&original).unwrap();
        for path in ["", "/usage", "/routing", "/routing/service"] {
            let mut future = value.clone();
            future
                .pointer_mut(path)
                .unwrap()
                .as_object_mut()
                .unwrap()
                .insert("futureSource".into(), serde_json::Value::Null);
            let raw = serde_json::to_string(&future).unwrap();
            assert!(serde_json::from_str::<ModelObservation>(&raw).is_ok());
            assert!(source::observation(&raw).is_err(), "{path}");
        }
        assert!(source::observation(&(good.clone() + " {} ")).is_err());
        let duplicate = format!("{{\"revision\":{},{}", original.revision, &good[1..]);
        assert!(source::observation(&duplicate).is_err());
    }
}

#[test]
fn attributed_staging_rejects_future_source_fields_without_overwriting_raw_or_facts() {
    use super::super::{LedgerError, test_support};
    let dir = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(dir.path()).unwrap();
    let (request, rows) = test_support::retry();
    store.save_batch(&rows).unwrap();
    let ledger = store.retention_ledger(test_support::STORE).unwrap();
    let mut future = serde_json::to_value(&rows[1]).unwrap();
    future["usage"]["futureUsage"] = serde_json::Value::Null;
    store
        .0
        .execute(
            "UPDATE model_observations SET value=?1 WHERE id=?2",
            params![future.to_string(), rows[1].id],
        )
        .unwrap();
    let before = raw(&store);
    assert_eq!(
        ledger
            .stage_attributed_batch(&[&request], test_support::directory(), &[])
            .unwrap_err(),
        LedgerError::InvalidRecord
    );
    assert_eq!(raw(&store), before);
    assert_eq!(
        store
            .0
            .query_row("SELECT COUNT(*) FROM retention_fact_staging", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn invalid_rows_are_counted_without_inventing_complete_request_coverage() {
    let dir = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(dir.path()).unwrap();
    let r = row("old", "2026-08-01T00:00:00Z");
    store.save(&r).unwrap();
    let mut value = serde_json::to_value(&r).unwrap();
    value["usage"]["future"] = serde_json::Value::Null;
    store
        .0
        .execute(
            "UPDATE model_observations SET value=?1",
            [value.to_string()],
        )
        .unwrap();
    store
        .0
        .execute(
            "INSERT INTO model_observations VALUES('broken','unknown',1,'{')",
            [],
        )
        .unwrap();
    store
        .0
        .execute(
            "INSERT INTO model_observations VALUES('oversize','unknown',1,?1)",
            ["x".repeat(MAX_ROW_BYTES as usize + 1)],
        )
        .unwrap();
    let before = raw(&store);
    let result = gateway(dir.path(), 30, now()).unwrap();
    assert_eq!(result.attempts, 3);
    assert_eq!(result.logical_requests, 1);
    assert_eq!(result.expired, 0);
    assert_eq!(result.protected.unsupported_source, 1);
    assert_eq!(result.unreadable_attempts, 2);
    assert!(!result.classification_complete);
    assert_eq!(raw(&store), before);
}

#[test]
fn mismatched_source_identity_revision_and_indexed_time_protect_whole_group() {
    for sql in [
        "UPDATE model_observations SET id='reused'",
        "UPDATE model_observations SET revision=99",
        "UPDATE model_observations SET started_at='changed'",
    ] {
        let dir = tempfile::tempdir().unwrap();
        let store = ObservationStore::open(dir.path()).unwrap();
        store.save(&row("old", "2026-08-01T00:00:00Z")).unwrap();
        store.0.execute_batch(sql).unwrap();
        let result = gateway(dir.path(), 90, now()).unwrap();
        assert_eq!(result.protected.unsupported_source, 1);
        assert_eq!(result.expired, 0);
        assert!(!result.classification_complete);
    }
}

#[test]
fn empty_is_confirmed_but_missing_and_future_schema_are_not_empty() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(
        gateway(dir.path(), 30, now()).unwrap_err(),
        Error::SourceAbsent
    );
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    let store = ObservationStore::open(dir.path()).unwrap();
    let empty = gateway(dir.path(), 30, now()).unwrap();
    assert_eq!(empty.attempts, 0);
    assert!(empty.classification_complete);
    store.0.execute_batch("PRAGMA user_version=2").unwrap();
    assert_eq!(
        gateway(dir.path(), 30, now()).unwrap_err(),
        Error::UnsupportedSchema
    );
    store
        .0
        .execute_batch(
            "PRAGMA user_version=0; ALTER TABLE model_observations ADD COLUMN future TEXT",
        )
        .unwrap();
    assert_eq!(
        gateway(dir.path(), 30, now()).unwrap_err(),
        Error::UnsupportedSchema
    );
}

#[test]
fn legacy_preview_does_not_migrate_and_current_preview_rejects_missing_epoch_metadata() {
    let dir = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(dir.path()).unwrap();
    store.save(&row("old", "2026-08-01T00:00:00Z")).unwrap();
    store
        .0
        .execute_batch("DROP TABLE observation_content_epoch")
        .unwrap();
    assert_eq!(
        gateway(dir.path(), 30, now()).unwrap_err(),
        Error::UnsupportedSchema
    );
    let created: bool = store
        .0
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='observation_content_epoch')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(!created);
    store.0.execute_batch("PRAGMA user_version=0").unwrap();
    assert_eq!(gateway(dir.path(), 30, now()).unwrap().attempts, 1);
    let version: u32 = store
        .0
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(version, 0);
    drop(store);
    let migrated = ObservationStore::open(dir.path()).unwrap();
    assert_eq!(gateway(dir.path(), 30, now()).unwrap().attempts, 1);
    migrated.0.execute_batch("PRAGMA user_version=0").unwrap();
    assert_eq!(
        gateway(dir.path(), 30, now()).unwrap_err(),
        Error::UnsupportedSchema
    );
}

#[test]
fn bounded_scan_fails_instead_of_reporting_a_partial_collection() {
    let dir = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(dir.path()).unwrap();
    store.save(&row("old", "2026-08-01T00:00:00Z")).unwrap();
    assert_eq!(
        scan(dir.path(), 30, now(), 0, std::time::Duration::from_secs(1)).unwrap_err(),
        Error::Limit
    );
    assert_eq!(
        scan(dir.path(), 30, now(), 10, std::time::Duration::ZERO).unwrap_err(),
        Error::Limit
    );
    assert_eq!(
        gateway(dir.path(), 1, now()).unwrap_err(),
        Error::InvalidInput
    );
}

#[test]
fn symlink_source_is_rejected_without_following_or_changing_it() {
    let dir = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(other.path()).unwrap();
    store.save(&row("old", "2026-08-01T00:00:00Z")).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(
        other.path().join("observations.sqlite3"),
        dir.path().join("observations.sqlite3"),
    )
    .unwrap();
    assert_eq!(
        gateway(dir.path(), 30, now()).unwrap_err(),
        Error::SourceChanged
    );
    assert_eq!(raw(&store).len(), 1);
}
