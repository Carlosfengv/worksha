use super::*;
use crate::domain::{Coverage, requests::query::RequestSourceSummary};

fn fixture(
    count: usize,
) -> (
    tempfile::TempDir,
    History,
    NativeSessions,
    q::UnifiedRequestQuery,
    x::Begin,
    DateTime<Utc>,
) {
    let root = tempfile::tempdir().unwrap();
    let mut h = History::open(root.path()).unwrap();
    h.refresh_pending(&[], &HashSet::new()).unwrap();
    let sessions = NativeSessions::new(root.path().join("owned-home"), &[]);
    let mut query = super::super::tests::input();
    query.unit = d::requests::RequestUnit::ModelResponse;
    query.include_evidence = Some(true);
    let mut p =
        super::super::tests::publication((0..count).map(super::super::tests::external).collect());
    p.summaries.push(RequestSourceSummary {
        source_id: "external-fixture".into(),
        unit: query.unit,
        evidence: q::RequestEvidence::SourceReported,
        coverage: Coverage::Complete,
        records: Some((count as u64).into()),
        usage: (&crate::sessions::Usage::default()).into(),
        usage_missing: Some(0u64.into()),
        attempts: None,
        unknown_time: Some(0u64.into()),
        evidence_summary: None,
    });
    let now = Utc::now();
    let page = h
        .unified_request_page(&query, now, "fixed", Some(p))
        .unwrap();
    let input = x::Begin {
        operation_id: uuid::Uuid::new_v4().to_string(),
        snapshot_token: page.snapshot_token,
    };
    (root, h, sessions, query, input, now)
}

#[test]
fn unified_csv_export_reads_full_frozen_selection_not_only_first_page() {
    let (_root, mut h, sessions, query, input, now) = fixture(300);
    let info = h
        .begin_unified_csv_export(&input, "fixed", &sessions, now)
        .unwrap();
    assert_eq!(info.count.value(), 300);
    assert_eq!(h.unified_exports.len(), 1);
    assert_eq!(
        h.begin_unified_csv_export(&input, "fixed", &sessions, now)
            .unwrap(),
        info
    );
    h.unified_request_page(
        &query,
        now,
        "fixed",
        Some(super::super::tests::publication(vec![])),
    )
    .unwrap();
    let mut offset = 0;
    let mut text = String::new();
    let mut batches = 0;
    loop {
        let chunk = h
            .unified_csv_export_chunk(&input.operation_id, offset, "fixed", &sessions, now)
            .unwrap();
        assert_eq!(chunk.offset.value(), offset);
        assert!(chunk.rows.value() <= EXPORT_ROWS as u64);
        assert!(chunk.csv.len() <= EXPORT_RECORD_BYTES * 2 + EXPORT_TARGET_BYTES);
        text.push_str(&chunk.csv);
        offset += chunk.rows.value();
        batches += 1;
        if chunk.next_offset.is_none() {
            break;
        }
        assert_eq!(chunk.next_offset.unwrap().value(), offset);
    }
    assert!(batches >= 3);
    assert_eq!(text.matches("\"record\",\"\"").count(), 300);
    assert!(text.contains("external-299"));
    assert!(text.contains("9007199254740993"));
    assert_eq!(
        h.finish_unified_csv_export(
            &input.operation_id,
            &x::Finish {
                next_offset: offset.into(),
                rows: offset.into()
            },
            "fixed",
            &sessions,
            now
        )
        .unwrap(),
        info
    );
    assert!(h.unified_exports.is_empty());
}

#[test]
fn unified_csv_export_shares_legacy_budget_and_rejects_foreign_tokens_or_operations() {
    let (_root, mut h, sessions, _query, input, now) = fixture(2);
    h.begin_unified_csv_export(&input, "fixed", &sessions, now)
        .unwrap();
    let legacy = h
        .request_page(&requests::Query::default(), now, Value::Null, None)
        .unwrap();
    let snapshot = legacy["snapshot"].as_str().unwrap();
    assert_eq!(
        h.begin_csv_export(&input.operation_id, snapshot, now)
            .unwrap_err(),
        ExportError::Invalid
    );
    let old_id = uuid::Uuid::new_v4().to_string();
    h.begin_csv_export(&old_id, snapshot, now).unwrap();
    let other = x::Begin {
        operation_id: uuid::Uuid::new_v4().to_string(),
        ..input.clone()
    };
    assert_eq!(
        h.begin_unified_csv_export(&other, "fixed", &sessions, now)
            .unwrap_err(),
        ExportError::Busy
    );
    h.cancel_csv_export(&old_id).unwrap();
    let foreign = x::Begin {
        operation_id: input.operation_id.clone(),
        snapshot_token: snapshot.into(),
    };
    assert_eq!(
        h.begin_unified_csv_export(&foreign, "fixed", &sessions, now)
            .unwrap_err(),
        ExportError::Invalid
    );
    assert!(
        h.unified_csv_export_chunk(&input.operation_id, 0, "fixed", &sessions, now)
            .is_ok()
    );
    assert_eq!(
        h.begin_csv_export(
            &uuid::Uuid::new_v4().to_string(),
            &input.snapshot_token,
            now
        )
        .unwrap_err(),
        ExportError::Stale
    );
}

#[test]
fn unified_csv_export_pins_positions_after_query_gc_and_releases_after_cancel() {
    let (_root, mut h, sessions, _query, input, now) = fixture(2);
    let later = now + Duration::seconds(295);
    h.begin_unified_csv_export(&input, "fixed", &sessions, later)
        .unwrap();
    h.expire(now + Duration::seconds(301), false).unwrap();
    assert!(!h.unified_requests.contains_key(&input.snapshot_token));
    assert!(
        h.unified_csv_export_chunk(
            &input.operation_id,
            0,
            "fixed",
            &sessions,
            now + Duration::seconds(302)
        )
        .unwrap()
        .csv
        .contains("external-001")
    );
    assert!(
        h.unified_csv_export_query(&input.operation_id, now + Duration::seconds(302))
            .is_ok()
    );
    h.cancel_unified_csv_export(&input.operation_id).unwrap();
    assert_eq!(
        h.db.query_row("SELECT count(*) FROM unified_request_positions", [], |r| {
            r.get::<_, i64>(0)
        })
        .unwrap(),
        0
    );
}

#[test]
fn unified_csv_export_rechecks_authority_deadlines_offsets_and_finish_receipt() {
    let (_root, mut h, sessions, _query, input, now) = fixture(2);
    h.begin_unified_csv_export(&input, "fixed", &sessions, now)
        .unwrap();
    assert_eq!(
        h.unified_csv_export_chunk(&input.operation_id, 0, "revoked", &sessions, now)
            .unwrap_err(),
        ExportError::Stale
    );
    assert_eq!(
        h.unified_csv_export_chunk(&input.operation_id, 2, "fixed", &sessions, now)
            .unwrap_err(),
        ExportError::Invalid
    );
    assert_eq!(
        h.finish_unified_csv_export(
            &input.operation_id,
            &x::Finish {
                next_offset: 1u64.into(),
                rows: 2u64.into()
            },
            "fixed",
            &sessions,
            now
        )
        .unwrap_err(),
        ExportError::Invalid
    );
    assert_eq!(
        h.unified_csv_export_chunk(
            &input.operation_id,
            0,
            "fixed",
            &sessions,
            now + Duration::seconds(60)
        )
        .unwrap_err(),
        ExportError::Stale
    );
    h.expire(now + Duration::seconds(60), false).unwrap();
    assert!(h.unified_exports.is_empty());
    h.begin_unified_csv_export(&input, "fixed", &sessions, now)
        .unwrap();
    for minute in 1..15 {
        h.unified_csv_export_chunk(
            &input.operation_id,
            0,
            "fixed",
            &sessions,
            now + Duration::seconds(minute * 59),
        )
        .unwrap();
    }
    assert_eq!(
        h.unified_csv_export_chunk(
            &input.operation_id,
            0,
            "fixed",
            &sessions,
            now + Duration::minutes(15)
        )
        .unwrap_err(),
        ExportError::Stale
    );
}

#[test]
fn unified_csv_export_rejects_oversize_payload_before_decoding_without_truncation() {
    let (_root, mut h, sessions, _query, input, now) = fixture(2);
    h.begin_unified_csv_export(&input, "fixed", &sessions, now)
        .unwrap();
    let positions = &h.unified_exports[&input.operation_id].positions;
    h.db.execute(
        "UPDATE unified_request_positions SET external_value=?1 WHERE snapshot=?2 AND pos=1",
        params!["x".repeat(EXPORT_RECORD_BYTES + 1), positions],
    )
    .unwrap();
    assert_eq!(
        h.unified_csv_export_chunk(&input.operation_id, 0, "fixed", &sessions, now)
            .unwrap_err(),
        ExportError::RecordTooLarge
    );
    assert_eq!(h.unified_exports.len(), 1);
    h.cancel_unified_csv_export(&input.operation_id).unwrap();
}

#[test]
fn unified_csv_export_empty_still_contains_scope_and_preserves_unknown_sources() {
    let (_root, mut h, sessions, _query, input, now) = fixture(0);
    let info = h
        .begin_unified_csv_export(&input, "fixed", &sessions, now)
        .unwrap();
    let chunk = h
        .unified_csv_export_chunk(&input.operation_id, 0, "fixed", &sessions, now)
        .unwrap();
    assert_eq!(info.count.value(), 0);
    assert_eq!(chunk.rows.value(), 0);
    assert_eq!(chunk.next_offset, None);
    assert!(chunk.csv.starts_with('\u{feff}'));
    assert!(chunk.csv.contains("\"scope\""));
}

#[test]
fn unified_csv_export_cleanup_failure_keeps_pin_until_successful_retry() {
    let (_root, mut h, sessions, _query, input, now) = fixture(2);
    h.begin_unified_csv_export(&input, "fixed", &sessions, now)
        .unwrap();
    h.unified_requests.remove(&input.snapshot_token);
    h.db.execute_batch("CREATE TEMP TRIGGER reject_unified_release BEFORE DELETE ON unified_request_positions BEGIN SELECT RAISE(FAIL,'owned cleanup failure'); END;").unwrap();
    assert_eq!(
        h.cancel_unified_csv_export(&input.operation_id),
        Err(ExportError::Unavailable)
    );
    assert!(h.unified_exports.contains_key(&input.operation_id));
    assert_eq!(
        h.unified_csv_export_chunk(&input.operation_id, 0, "fixed", &sessions, now)
            .unwrap()
            .rows
            .value(),
        2
    );
    h.db.execute_batch("DROP TRIGGER reject_unified_release;")
        .unwrap();
    h.cancel_unified_csv_export(&input.operation_id).unwrap();
    assert!(h.unified_exports.is_empty());
    assert_eq!(
        h.db.query_row("SELECT count(*) FROM unified_request_positions", [], |r| {
            r.get::<_, i64>(0)
        })
        .unwrap(),
        0
    );
}

#[test]
fn unified_csv_export_retains_original_host_payload_through_update_query_gc_and_prune() {
    let (root, mut h, sessions, _query, _input, now) = fixture(0);
    let store = ObservationStore::open(root.path()).unwrap();
    let mut row: crate::observation::ModelObservation = serde_json::from_value(serde_json::json!({"id":"owned-host","agentId":"claude","model":"fixture","provider":"old immutable provider","providerId":"fixture","endpoint":"https://example.invalid/v1","bindingRevision":1,"startedAt":"2026-10-01T00:00:00Z","endedAt":"2026-10-01T00:00:01Z","status":"success","httpStatus":200,"durationMs":125.5,"usage":{"totalTokens":13},"protocol":"responses","revision":1})).unwrap();
    store.save(&row).unwrap();
    h.refresh_pending(&[], &HashSet::new()).unwrap();
    let mut query = super::super::tests::input();
    query.scope.kind = d::ScopeKind::Builtin;
    query.scope.source_ids = vec!["claude".into()];
    query.include_evidence = Some(true);
    let mut publication = super::super::tests::publication(vec![]);
    publication.states.retain(|s| s.source_id == "claude");
    publication.native.retain(|s| s.source_id == "claude");
    let page = h
        .unified_request_page(&query, now, "fixed", Some(publication))
        .unwrap();
    assert_eq!(page.items.len(), 1);
    let input = x::Begin {
        operation_id: uuid::Uuid::new_v4().to_string(),
        snapshot_token: page.snapshot_token,
    };
    h.begin_unified_csv_export(&input, "fixed", &sessions, now)
        .unwrap();
    row.provider = "new provider must not replace frozen payload".into();
    row.revision += 1;
    store.save(&row).unwrap();
    h.refresh_pending(&[], &HashSet::new()).unwrap();
    let frozen = h.unified_requests.remove(&input.snapshot_token).unwrap();
    h.release_request_positions([frozen.positions].into_iter().collect())
        .unwrap();
    h.prune().unwrap();
    let csv = h
        .unified_csv_export_chunk(&input.operation_id, 0, "fixed", &sessions, now)
        .unwrap()
        .csv;
    assert!(csv.contains("old immutable provider"));
    assert!(csv.contains("\"125.5\""));
    assert!(!csv.contains("new provider must not replace"));
    h.cancel_unified_csv_export(&input.operation_id).unwrap();
    assert_eq!(
        h.db.query_row(
            "SELECT count(*) FROM record_payloads WHERE value LIKE '%old immutable provider%'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
}
