use super::*;
use d::requests::RequestUnit;
use serde_json::json;
pub(super) fn input() -> q::UnifiedRequestQuery {
    q::UnifiedRequestQuery {
        scope: d::QueryScope {
            kind: d::ScopeKind::Combined,
            source_ids: vec!["claude".into(), "codex".into(), "external-fixture".into()],
        },
        unit: RequestUnit::LogicalRequest,
        filter: Default::default(),
        descending: true,
        sort: None,
        include_evidence: None,
        snapshot_token: None,
        cursor: None,
    }
}
fn status(id: &str) -> d::SourceState {
    d::SourceState {
        source_id: id.into(),
        authorization: d::Authorization::Authorized,
        freshness: d::Freshness::Current,
        coverage: d::Coverage::Complete,
        problem: None,
    }
}

#[test]
fn object_request_reads_full_frozen_positions_and_rejects_foreign_sources_and_units() {
    let root = tempfile::tempdir().unwrap();
    let mut history = History::open(root.path()).unwrap();
    history.refresh_pending(&[], &HashSet::new()).unwrap();
    let sessions = crate::sessions::NativeSessions::new(root.path().join("owned-home"), &[]);
    let mut query = input();
    query.unit = RequestUnit::ModelResponse;
    let now = Utc::now();
    let first = history
        .unified_request_page(
            &query,
            now,
            "fixed",
            Some(publication((0..130).map(external).collect())),
        )
        .unwrap();
    assert_eq!(first.items.len(), 40);
    let row = history
        .object_request_record(
            &first.snapshot_token,
            "fixed",
            "external-fixture",
            "external-129",
            &sessions,
        )
        .unwrap();
    assert_eq!(
        row.usage.total_tokens.unwrap().value(),
        9_007_199_254_740_993
    );
    assert_eq!(row.attempts, None);
    assert!(
        history
            .object_request_record(
                &first.snapshot_token,
                "fixed",
                "claude",
                "external-129",
                &sessions
            )
            .is_err()
    );
    assert!(
        history
            .object_request_record(
                &first.snapshot_token,
                "changed",
                "external-fixture",
                "external-129",
                &sessions
            )
            .is_err()
    );
    assert!(
        history
            .object_request_record(
                &first.snapshot_token,
                "fixed",
                "external-fixture",
                "external-999",
                &sessions
            )
            .is_err()
    );
    assert_eq!(
        history
            .object_request_query(&first.snapshot_token)
            .unwrap()
            .unit,
        RequestUnit::ModelResponse
    );
    let mut newer = external(129);
    newer.record.usage.total_tokens = Some(1_u64.into());
    history
        .unified_request_page(&query, now, "fixed", Some(publication(vec![newer])))
        .unwrap();
    assert_eq!(
        history
            .object_request_record(
                &first.snapshot_token,
                "fixed",
                "external-fixture",
                "external-129",
                &sessions
            )
            .unwrap()
            .usage
            .total_tokens
            .unwrap()
            .value(),
        9_007_199_254_740_993
    );
    assert!(history.object_request_query("unrelated-token").is_err());
}
pub(super) fn publication(external: Vec<q::UnifiedRequestItem>) -> Publication {
    Publication {
        signature: "fixed".into(),
        states: vec![
            status("claude"),
            status("codex"),
            status("external-fixture"),
        ],
        native: vec![status("claude"), status("codex")],
        authorizations: vec![],
        hidden: vec![],
        external,
        summaries: vec![],
        history_partial: false,
    }
}
pub(super) fn external(id: usize) -> q::UnifiedRequestItem {
    q::UnifiedRequestItem {
        evidence_detail: None,
        record: q::RequestQueryRecord {
            id: format!("external-{id:03}"),
            source_id: "external-fixture".into(),
            unit: RequestUnit::ModelResponse,
            product_id: None,
            session_ref: None,
            session_title: Some("external fixture".into()),
            project_ref: None,
            project_name: None,
            occurred_at: Some("2026-10-01T00:00:00Z".into()),
            ended_at: None,
            model: None,
            provider_key: "external-provider".into(),
            provider_name: "Source".into(),
            outcome: "unknown".into(),
            usage: (&crate::sessions::Usage {
                total_tokens: Some(9_007_199_254_740_993),
                ..Default::default()
            })
                .into(),
            usage_missing: 0u64.into(),
            attempts: None,
            duration_ms: None,
            ttft_ms: None,
            throughput: None,
        },
        evidence: q::RequestEvidence::SourceReported,
        host_request_id: None,
        reported_response_id: Some("same-claimed-response".into()),
        reported_duration_ms: Some(10u64.into()),
    }
}
#[test]
fn scoped_full_statistics_and_frozen_pages_do_not_filter_only_one_legacy_page() {
    let root = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(root.path()).unwrap();
    let mut rows = vec![];
    for n in 0..135 {
        let mut row:crate::observation::ModelObservation=serde_json::from_value(json!({"id":format!("request-{n:03}"),"agentId":"claude","sessionId":null,"model":"fixture","provider":"Fixture","providerId":"fixture-provider","endpoint":"https://example.invalid/v1","bindingRevision":1,"startedAt":"2026-10-01T00:00:00Z","endedAt":"2026-10-01T00:00:01Z","status":"success","httpStatus":200,"durationMs":1000.0,"usage":{"totalTokens":10},"protocol":"responses","revision":1})).unwrap();
        row.agent_id = match n % 3 {
            0 => "claude",
            1 => "codex",
            _ => "opencode",
        }
        .into();
        row.agent_product_id = Some(row.agent_id.clone());
        row.started_at = "2026-10-01T00:00:00Z".into();
        row.ended_at = Some("2026-10-01T00:00:01Z".into());
        row.usage.total_tokens = Some(10);
        rows.push(row);
    }
    store.save_batch(&rows).unwrap();
    let mut history = History::open(root.path()).unwrap();
    history.refresh_pending(&[], &HashSet::new()).unwrap();
    let now = "2026-11-01T00:00:00Z".parse::<DateTime<Utc>>().unwrap();
    let mut query = input();
    let first = history
        .unified_request_page(&query, now, "fixed", Some(publication(vec![])))
        .unwrap();
    assert_eq!(first.items.len(), 40);
    assert_eq!(first.source_summaries.len(), 2);
    for s in &first.source_summaries {
        assert_eq!(s.records.as_ref().unwrap().value(), 45);
        assert_eq!(s.attempts.as_ref().unwrap().value(), 45);
        assert_eq!(s.usage.total_tokens.as_ref().unwrap().value(), 450);
    }
    assert!(first.items.iter().all(|r| r.record.source_id != "opencode"));
    // Replace every source record after freezing. Positions must retain originals
    // even when pruning retired versions and when replaying the first page.
    for row in &mut rows {
        row.revision += 1;
        row.usage.total_tokens = Some(999);
    }
    store.save_batch(&rows).unwrap();
    history.refresh_pending(&[], &HashSet::new()).unwrap();
    history.prune().unwrap();
    query.snapshot_token = Some(first.snapshot_token.clone());
    query.cursor = None;
    assert_eq!(
        history
            .unified_request_page(&query, now, "fixed", None)
            .unwrap()
            .items,
        first.items
    );
    let mut ids = first
        .items
        .iter()
        .map(|r| r.record.id.clone())
        .collect::<Vec<_>>();
    let mut next = first.next_cursor;
    while let Some(cursor) = next {
        query.cursor = Some(cursor);
        let page = history
            .unified_request_page(&query, now, "fixed", None)
            .unwrap();
        assert!(
            page.items
                .iter()
                .all(|r| r.record.usage.total_tokens.as_ref().unwrap().value() == 10)
        );
        ids.extend(page.items.iter().map(|r| r.record.id.clone()));
        next = page.next_cursor;
    }
    assert_eq!(ids.len(), 90);
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), 90);
    let mut changed = query.clone();
    changed.scope.source_ids.pop();
    assert!(
        history
            .unified_request_page(&changed, now, "fixed", None)
            .is_err()
    );
    assert!(
        history
            .unified_request_page(&query, now, "changed-permission", None)
            .is_err()
    );
    query.cursor = Some(format!("{}:999", first.snapshot_token));
    assert!(
        history
            .unified_request_page(&query, now, "fixed", None)
            .is_err()
    );
}
#[test]
fn reported_responses_keep_unknown_attempts_large_usage_and_claims_without_host_metrics() {
    let root = tempfile::tempdir().unwrap();
    let mut history = History::open(root.path()).unwrap();
    let mut query = input();
    query.unit = RequestUnit::ModelResponse;
    let rows = (0..83).map(external).collect();
    let now = Utc::now();
    let first = history
        .unified_request_page(&query, now, "fixed", Some(publication(rows)))
        .unwrap();
    assert_eq!(first.items.len(), 40);
    let row = &first.items[0];
    assert!(row.host_request_id.is_none());
    assert!(row.record.attempts.is_none());
    assert!(row.record.ttft_ms.is_none());
    assert!(row.record.duration_ms.is_none());
    assert_eq!(
        row.record.usage.total_tokens.as_ref().unwrap().value(),
        9_007_199_254_740_993
    );
    let mut ids = first
        .items
        .iter()
        .map(|r| r.record.id.clone())
        .collect::<Vec<_>>();
    query.snapshot_token = Some(first.snapshot_token.clone());
    let mut next = first.next_cursor;
    while let Some(cursor) = next {
        query.cursor = Some(cursor);
        let page = history
            .unified_request_page(&query, now, "fixed", None)
            .unwrap();
        ids.extend(page.items.iter().map(|r| r.record.id.clone()));
        next = page.next_cursor;
    }
    assert_eq!(ids.len(), 83);
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), 83);
    // The original request store and detail path never receive a reported record.
    assert!(history.request_detail("external-000").unwrap().is_none());
}
#[test]
fn legacy_and_unified_snapshots_share_the_original_eight_lease_budget_and_release_positions() {
    let root = tempfile::tempdir().unwrap();
    let mut history = History::open(root.path()).unwrap();
    let now = Utc::now();
    let mut query = input();
    query.unit = RequestUnit::ModelResponse;
    let first = history
        .unified_request_page(&query, now, "fixed", Some(publication(vec![external(0)])))
        .unwrap();
    for i in 0..12 {
        let legacy = requests::Query {
            search: Some(format!("no-match-{i}")),
            ..Default::default()
        };
        history
            .request_page(
                &legacy,
                now + chrono::Duration::seconds(i + 1),
                Value::Null,
                None,
            )
            .unwrap();
        assert!(history.snapshots.len() + history.unified_requests.len() <= 8);
    }
    assert!(!history.unified_requests.contains_key(&first.snapshot_token));
    assert_eq!(
        history
            .db
            .query_row("SELECT count(*) FROM unified_request_positions", [], |r| {
                r.get::<_, i64>(0)
            })
            .unwrap(),
        0
    );
    assert_eq!(
        history
            .db
            .query_row("SELECT count(*) FROM unified_request_input", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        json!(history.request_detail("external-000").unwrap()),
        Value::Null
    );
}

#[test]
fn quarantined_gateway_history_is_partial_instead_of_confirmed_complete_empty() {
    let root = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(root.path()).unwrap();
    store
        .0
        .execute(
            "INSERT INTO model_observations VALUES('malformed','2026-10-01T00:00:00Z',1,'{bad')",
            [],
        )
        .unwrap();
    let mut history = History::open(root.path()).unwrap();
    history.refresh(&[]).unwrap();
    assert_eq!(history.invalid, 1);
    let page = history
        .unified_request_page(&input(), Utc::now(), "fixed", Some(publication(vec![])))
        .unwrap();
    assert!(page.items.is_empty());
    assert!(
        page.source_summaries
            .iter()
            .all(|s| s.coverage == d::Coverage::Partial)
    );
    assert!(
        page.source_states
            .iter()
            .filter(|s| s.source_id != "external-fixture")
            .all(|s| s.problem.as_deref() == Some("partial_history"))
    );
    assert_eq!(
        store
            .0
            .query_row(
                "SELECT value FROM model_observations WHERE id='malformed'",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
        "{bad"
    );
}
