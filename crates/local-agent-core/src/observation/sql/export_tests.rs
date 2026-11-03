use super::*;
use crate::requests::export::{EXPORT_RECORD_BYTES, EXPORT_ROWS, ExportError};

#[test]
fn bounded_export_pins_the_original_rows_through_updates_eviction_and_full_csv_parity() {
    let dir = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(dir.path()).unwrap();
    let mut rows: Vec<_> = (0..2049).map(row).collect();
    rows[0].provider = "=formula,\"quoted\"\r\nfield".into();
    rows[1].ttft_ms = None;
    rows[1].usage = Usage::default();
    store.save_batch(&rows).unwrap();
    let mut history = History::open(dir.path()).unwrap();
    history.refresh(&[]).unwrap();
    let page = history
        .request_page(&requests::Query::default(), now(), Value::Null, None)
        .unwrap();
    let snapshot = page["snapshot"].as_str().unwrap();
    let expected = history.frozen_csv(snapshot, now()).unwrap();
    let operation = uuid::Uuid::new_v4().to_string();
    let info = history
        .begin_csv_export(&operation, snapshot, now())
        .unwrap();
    let mut changed = rows[0].clone();
    changed.provider = "changed".into();
    store.save(&changed).unwrap();
    store
        .0
        .execute("DELETE FROM model_observations WHERE id='r-000004'", [])
        .unwrap();
    history.refresh(&[]).unwrap();
    for index in 1..20 {
        history
            .request_page(
                &requests::Query::default(),
                now() + chrono::Duration::seconds(index),
                Value::Null,
                None,
            )
            .unwrap();
    }
    assert!(!history.snapshots.contains_key(snapshot));
    let mut csv = String::new();
    let mut after = -1;
    let mut count = 0;
    loop {
        let part = history.csv_export_chunk(&operation, after, now()).unwrap();
        assert!(part.rows <= EXPORT_ROWS as u64);
        assert_eq!(part.after, after);
        count += part.rows;
        after += part.rows as i64;
        csv.push_str(&part.csv);
        if part.next_after.is_none() {
            break;
        }
        assert_eq!(part.next_after, Some(after));
    }
    assert_eq!(csv, expected["csv"]);
    assert_eq!(count, 2049);
    assert!(
        history
            .finish_csv_export(&operation, after, count - 1, now())
            .is_err()
    );
    assert_eq!(
        history
            .finish_csv_export(&operation, after, count, now())
            .unwrap(),
        info
    );
    assert!(history.exports.is_empty());
}

#[test]
fn export_admission_is_bounded_and_idle_expiry_releases_pinned_positions() {
    let dir = tempfile::tempdir().unwrap();
    let mut history = History::open(dir.path()).unwrap();
    history.refresh(&[]).unwrap();
    let page = history
        .request_page(&requests::Query::default(), now(), Value::Null, None)
        .unwrap();
    let snapshot = page["snapshot"].as_str().unwrap();
    let ids: Vec<_> = (0..3).map(|_| uuid::Uuid::new_v4().to_string()).collect();
    let first = history.begin_csv_export(&ids[0], snapshot, now()).unwrap();
    assert_eq!(
        history.begin_csv_export(&ids[0], snapshot, now()).unwrap(),
        first
    );
    history.begin_csv_export(&ids[1], snapshot, now()).unwrap();
    assert_eq!(
        history
            .begin_csv_export(&ids[2], snapshot, now())
            .unwrap_err(),
        ExportError::Busy
    );
    let part = history.csv_export_chunk(&ids[0], -1, now()).unwrap();
    assert_eq!(part.rows, 0);
    assert!(part.csv.starts_with('\u{feff}'));
    assert!(part.next_after.is_none());
    assert_eq!(
        history
            .csv_export_chunk(&ids[0], -1, now() + chrono::Duration::seconds(60))
            .unwrap_err(),
        ExportError::Stale
    );
    history
        .begin_csv_export(&ids[2], snapshot, now() + chrono::Duration::seconds(60))
        .unwrap();
    assert_eq!(history.exports.len(), 1);
    history.cancel_csv_export(&ids[2]).unwrap();
    history.cancel_csv_export(&ids[2]).unwrap();
    assert!(history.exports.is_empty());
}

#[test]
fn oversized_or_missing_export_records_fail_instead_of_silently_truncating() {
    let dir = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(dir.path()).unwrap();
    store.save(&row(1)).unwrap();
    let mut history = History::open(dir.path()).unwrap();
    history.refresh(&[]).unwrap();
    let page = history
        .request_page(&requests::Query::default(), now(), Value::Null, None)
        .unwrap();
    let operation = uuid::Uuid::new_v4().to_string();
    history
        .begin_csv_export(&operation, page["snapshot"].as_str().unwrap(), now())
        .unwrap();
    history
        .db
        .execute(
            "UPDATE record_payloads SET value=json_set(value,'$.provider',?1)",
            ["x".repeat(EXPORT_RECORD_BYTES)],
        )
        .unwrap();
    assert_eq!(
        history.csv_export_chunk(&operation, -1, now()).unwrap_err(),
        ExportError::RecordTooLarge
    );
    history.db.execute("DELETE FROM positions", []).unwrap();
    assert_eq!(
        history.csv_export_chunk(&operation, -1, now()).unwrap_err(),
        ExportError::Unavailable
    );
}

#[test]
fn failed_export_cleanup_rolls_back_and_remains_retryable_without_touching_original_data() {
    let dir = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(dir.path()).unwrap();
    store.save(&row(1)).unwrap();
    let mut history = History::open(dir.path()).unwrap();
    history.refresh(&[]).unwrap();
    let page = history
        .request_page(&requests::Query::default(), now(), Value::Null, None)
        .unwrap();
    let operation = uuid::Uuid::new_v4().to_string();
    history
        .begin_csv_export(&operation, page["snapshot"].as_str().unwrap(), now())
        .unwrap();
    history.snapshots.clear();
    history.request_templates.clear();
    history.db.execute_batch("CREATE TEMP TRIGGER fail_export_cleanup BEFORE DELETE ON positions BEGIN SELECT RAISE(ABORT,'injected cleanup failure');END;").unwrap();
    assert_eq!(
        history.cancel_csv_export(&operation).unwrap_err(),
        ExportError::Unavailable
    );
    assert!(history.exports.contains_key(&operation));
    assert_eq!(
        history
            .csv_export_chunk(&operation, -1, now())
            .unwrap()
            .rows,
        1
    );
    history
        .db
        .execute_batch("DROP TRIGGER fail_export_cleanup;")
        .unwrap();
    history.cancel_csv_export(&operation).unwrap();
    assert!(history.exports.is_empty());
    assert_eq!(store.load().unwrap().len(), 1);
}
