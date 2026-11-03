use super::tests::{external, input, publication};
use super::*;
use crate::provider_cost::{MoneyTotal, Totals};
use d::requests::RequestUnit;

fn observed(id: &str) -> crate::observation::ModelObservation {
    serde_json::from_value(json!({"id":id,"agentId":"claude","agentProductId":"claude-code-cli","sessionId":null,"model":"fixture","provider":"Fixture","providerId":"fixture","endpoint":"https://openrouter.ai/api/v1","bindingRevision":1,"startedAt":"2026-10-01T00:00:00Z","endedAt":"2026-10-01T00:00:01Z","status":"success","httpStatus":200,"ttftMs":0.0,"durationMs":1000.0,"usage":{"inputTokens":1,"outputTokens":0,"totalTokens":1},"reportedCost":{"amount":0,"currency":"USD","source":"openrouter.usage.cost"},"protocol":"messages","revision":1})).unwrap()
}
fn native(id: &str, tokens: Option<u64>, metric: Option<f64>) -> requests::Request {
    let mut row = requests::logical_requests(vec![observed(id)], &HashMap::new())
        .pop()
        .unwrap();
    row.source = "native".into();
    row.usage.total_tokens = tokens;
    row.ttft_ms = metric;
    row.duration_ms = metric;
    row.native = Some(requests::NativeRecord {
        failure: None,
        event_id: id.into(),
        turn_id: "private-turn".into(),
        response_id: Some(format!("response-{id}")),
        reported_status: "recorded".into(),
        association: "unlinked".into(),
        gateway_request_id: None,
        matched_attempt_id: None,
        usage_conflict: false,
    });
    row
}
#[test]
fn every_sort_merges_complete_scoped_positions_and_keeps_unknown_evidence_last() {
    let root = tempfile::tempdir().unwrap();
    let mut history = History::open(root.path()).unwrap();
    let mut captured = vec![];
    for n in 0..85 {
        let mut row = native(
            &format!("host-{n:03}"),
            Some(n as u64),
            Some((n % 17) as f64),
        );
        row.started_at = format!("2026-10-01T00:{:02}:{:02}Z", n / 60, n % 60);
        row.cost = Totals {
            actual: vec![MoneyTotal {
                currency: "USD".into(),
                amount: (n % 11) as f64,
                calls: 1,
            }],
            ..Default::default()
        };
        history.insert_request(&row).unwrap();
        captured.push((
            row.id.clone(),
            "claude".to_string(),
            Some(n as u64),
            row.ttft_ms,
            row.duration_ms,
            Some((n % 11) as f64),
            timestamp(&row.started_at),
        ));
    }
    let mut unknown = native("unknown", None, None);
    unknown.duration_ms = Some(f64::INFINITY);
    unknown.ttft_ms = Some(f64::NAN);
    history.insert_request(&unknown).unwrap();
    captured.push((
        unknown.id.clone(),
        "claude".into(),
        None,
        None,
        None,
        Some(0.0),
        timestamp(&unknown.started_at),
    ));
    let mut reported = (0..83).map(external).collect::<Vec<_>>();
    for (n, row) in reported.iter_mut().enumerate() {
        row.record.usage.total_tokens = Some((n as u64).into());
        row.reported_duration_ms = Some(u64::MAX.into());
        captured.push((
            row.record.id.clone(),
            row.record.source_id.clone(),
            Some(n as u64),
            None,
            None,
            None,
            row.record.occurred_at.as_deref().and_then(timestamp),
        ));
    }
    for sort in [
        q::RequestSort::Time,
        q::RequestSort::Tokens,
        q::RequestSort::Ttft,
        q::RequestSort::Duration,
        q::RequestSort::CostUsd,
    ] {
        for descending in [false, true] {
            let mut query = input();
            query.unit = RequestUnit::ModelResponse;
            query.sort = Some(sort);
            query.descending = descending;
            query.include_evidence = Some(true);
            let first = history
                .unified_request_page(
                    &query,
                    Utc::now(),
                    "fixed",
                    Some(publication(reported.clone())),
                )
                .unwrap();
            let mut actual = first
                .items
                .iter()
                .map(|r| r.record.id.clone())
                .collect::<Vec<_>>();
            query.snapshot_token = Some(first.snapshot_token.clone());
            let mut next = first.next_cursor;
            while let Some(cursor) = next {
                query.cursor = Some(cursor);
                let page = history
                    .unified_request_page(&query, Utc::now(), "fixed", None)
                    .unwrap();
                assert_eq!(page.source_summaries, first.source_summaries);
                actual.extend(page.items.iter().map(|r| r.record.id.clone()));
                next = page.next_cursor;
            }
            type SortRow = (
                String,
                String,
                Option<u64>,
                Option<f64>,
                Option<f64>,
                Option<f64>,
                Option<i64>,
            );
            let value = |row: &SortRow| match sort {
                q::RequestSort::Time => row.6.map(|v| v as f64),
                q::RequestSort::Tokens => row.2.map(|v| v as f64),
                q::RequestSort::Ttft => row.3,
                q::RequestSort::Duration => row.4,
                q::RequestSort::CostUsd => row.5,
            };
            let mut expected = captured.clone();
            expected.sort_by(|a, b| {
                let (x, y) = (value(a), value(b));
                x.is_none()
                    .cmp(&y.is_none())
                    .then_with(|| {
                        let order = x.partial_cmp(&y).unwrap();
                        if descending { order.reverse() } else { order }
                    })
                    .then(a.1.cmp(&b.1))
                    .then(a.0.cmp(&b.0))
            });
            assert_eq!(
                actual,
                expected.into_iter().map(|r| r.0).collect::<Vec<_>>(),
                "{sort:?}/{descending}"
            );
            query.cursor = None;
            assert_eq!(
                history
                    .unified_request_page(&query, Utc::now(), "fixed", None)
                    .unwrap()
                    .items,
                first.items
            );
        }
    }
}
#[test]
fn u64_sort_is_exact_across_i64_and_javascript_limits_and_overflow_is_explicit() {
    let root = tempfile::tempdir().unwrap();
    let mut history = History::open(root.path()).unwrap();
    for (id, value) in [
        ("a", 9_007_199_254_740_992),
        ("z", 9_007_199_254_740_993),
        ("max", u64::MAX),
    ] {
        history
            .insert_request(&native(id, Some(value), None))
            .unwrap();
    }
    let mut claim = external(0);
    claim.record.usage.total_tokens = Some((u64::MAX - 1).into());
    let mut query = input();
    query.unit = RequestUnit::ModelResponse;
    query.sort = Some(q::RequestSort::Tokens);
    query.include_evidence = Some(true);
    let page = history
        .unified_request_page(&query, Utc::now(), "fixed", Some(publication(vec![claim])))
        .unwrap();
    assert_eq!(
        page.items
            .iter()
            .map(|r| r.record.id.as_str())
            .collect::<Vec<_>>(),
        ["max", "external-000", "z", "a"]
    );
    assert_eq!(
        page.items[0]
            .record
            .usage
            .total_tokens
            .as_ref()
            .unwrap()
            .value(),
        u64::MAX
    );
    let summary = page
        .source_summaries
        .iter()
        .find(|s| s.source_id == "claude")
        .unwrap();
    assert!(summary.usage.total_tokens.is_none());
    assert_eq!(summary.coverage, d::Coverage::Partial);
    assert_eq!(
        summary.evidence_summary.as_ref().unwrap().overflow_fields,
        ["totalTokens"]
    );
    assert_eq!(
        page.source_states
            .iter()
            .find(|s| s.source_id == "claude")
            .unwrap()
            .problem
            .as_deref(),
        Some("counter_overflow")
    );
    assert_eq!(summary.records.as_ref().unwrap().value(), 3);
}
#[test]
fn native_match_and_currency_evidence_stays_bound_to_frozen_records_and_scope() {
    let root = tempfile::tempdir().unwrap();
    let mut history = History::open(root.path()).unwrap();
    let mut rows = vec![];
    for (id, association) in [
        ("exact", "response_id"),
        ("ambiguous", "ambiguous"),
        ("conflict", "identity_conflict"),
        ("unlinked", "unlinked"),
    ] {
        let mut row = native(id, Some(1), Some(0.0));
        let n = row.native.as_mut().unwrap();
        n.association = association.into();
        if association == "response_id" {
            n.gateway_request_id = Some("logical".into());
            n.matched_attempt_id = Some("attempt".into());
            n.usage_conflict = true;
        }
        row.cost = Totals {
            actual: vec![MoneyTotal {
                currency: "USD".into(),
                amount: 0.0,
                calls: 1,
            }],
            estimated: vec![MoneyTotal {
                currency: "CNY".into(),
                amount: 1.25,
                calls: 2,
            }],
            unknown_calls: 3,
        };
        for n in 0..4 {
            let mut part = row.attempts[0].clone();
            part.observation.id = format!("{id}-part-{n}");
            part.cost = if n < 2 {
                crate::provider_cost::Cost {
                    kind: "estimated",
                    amount: Some(0.625),
                    currency: Some("CNY"),
                    reason: None,
                    source: None,
                    price: None,
                    components: vec![],
                }
            } else {
                crate::provider_cost::Cost::unknown("missing_usage")
            };
            row.attempts.push(part);
        }
        history.insert_request(&row).unwrap();
        rows.push(row);
    }
    let mut query = input();
    query.unit = RequestUnit::ModelResponse;
    query.include_evidence = Some(true);
    let first = history
        .unified_request_page(&query, Utc::now(), "fixed", Some(publication(vec![])))
        .unwrap();
    let summary = &first.source_summaries[0].evidence_summary.as_ref().unwrap();
    assert!(
        matches!(&summary.matching,q::RequestMatchingSummary::NativeResponses {exact,ambiguous,identity_conflicts,unlinked,usage_conflicts,..} if [exact,ambiguous,identity_conflicts,unlinked,usage_conflicts].iter().all(|v|v.value()==1))
    );
    let billing = summary.billing.as_ref().unwrap();
    assert_eq!(billing.actual[0].amount, "0");
    assert_eq!(billing.estimated[0].amount, "5");
    assert_eq!(billing.unknown_parts.value(), 12);
    let exact = first.items.iter().find(|r| r.record.id == "exact").unwrap();
    let details = exact.evidence_detail.as_ref().unwrap();
    assert!(
        matches!(&details.matching,q::RequestMatching::NativeResponse{state:q::NativeMatchState::ExactResponseId,logical_request_id:Some(id),usage_conflict:true,..} if id=="logical")
    );
    assert_eq!(
        details.billing.as_ref().unwrap().estimated[0].amount,
        "1.25"
    );
    let mut changed = rows.remove(0);
    changed.cost.actual[0].amount = 99.0;
    changed.native.as_mut().unwrap().association = "unlinked".into();
    history.insert_request(&changed).unwrap();
    history.prune().unwrap();
    query.snapshot_token = Some(first.snapshot_token.clone());
    assert_eq!(
        history
            .unified_request_page(&query, Utc::now(), "fixed", None)
            .unwrap()
            .items,
        first.items
    );
    let mut modified = query.clone();
    modified.include_evidence = None;
    assert!(
        history
            .unified_request_page(&modified, Utc::now(), "fixed", None)
            .is_err()
    );
    modified = query.clone();
    modified.sort = Some(q::RequestSort::CostUsd);
    assert!(
        history
            .unified_request_page(&modified, Utc::now(), "fixed", None)
            .is_err()
    );
}
#[test]
fn gateway_links_have_bounded_details_but_complete_matching_and_billing_totals() {
    let root = tempfile::tempdir().unwrap();
    let mut history = History::open(root.path()).unwrap();
    let observations = (0..41)
        .map(|n| {
            let mut r = observed(&format!("attempt-{n}"));
            r.logical_id = Some("logical-fixture".into());
            r
        })
        .collect();
    let mut row = requests::logical_requests(observations, &HashMap::new())
        .pop()
        .unwrap();
    row.native_links = (0..41)
        .map(|n| requests::NativeLink {
            event_id: format!("event-{n}"),
            session_id: "claude:session".into(),
            response_id: format!("response-{n}"),
            attempt_id: format!("attempt-{n}"),
            usage_conflict: n == 0,
        })
        .collect();
    history.insert_request(&row).unwrap();
    let mut query = input();
    query.include_evidence = Some(true);
    let page = history
        .unified_request_page(&query, Utc::now(), "fixed", Some(publication(vec![])))
        .unwrap();
    let details = page.items[0].evidence_detail.as_ref().unwrap();
    assert!(
        matches!(&details.matching,q::RequestMatching::GatewayRequest {matched_responses,links,links_limited:true} if matched_responses.value()==41 && links.len()==40)
    );
    let summary = &page.source_summaries[0].evidence_summary.as_ref().unwrap();
    assert!(
        matches!(&summary.matching,q::RequestMatchingSummary::GatewayRequests{with_exact_links,without_confirmed_links,matched_responses,usage_conflict_links} if with_exact_links.value()==1 && without_confirmed_links.value()==0 && matched_responses.value()==41 && usage_conflict_links.value()==1)
    );
    assert_eq!(
        summary.billing.as_ref().unwrap().actual[0].parts.value(),
        41
    );
    assert_eq!(summary.billing.as_ref().unwrap().actual[0].amount, "0");
    assert_eq!(
        page.source_summaries[0].records.as_ref().unwrap().value(),
        1
    );
    assert_eq!(
        page.source_summaries[0].attempts.as_ref().unwrap().value(),
        41
    );
}
#[test]
fn basic_call_shape_is_unchanged_and_external_host_evidence_forgery_is_rejected() {
    let root = tempfile::tempdir().unwrap();
    let mut history = History::open(root.path()).unwrap();
    history
        .insert_request(&native("host", Some(1), Some(0.0)))
        .unwrap();
    let mut query = input();
    query.unit = RequestUnit::ModelResponse;
    let page = history
        .unified_request_page(
            &query,
            Utc::now(),
            "fixed",
            Some(publication(vec![external(0)])),
        )
        .unwrap();
    let json = serde_json::to_value(page).unwrap();
    assert!(json["appliedQuery"].get("sort").is_none());
    assert!(json["appliedQuery"].get("includeEvidence").is_none());
    assert!(
        json["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|v| v.get("evidenceDetail").is_none())
    );
    assert!(
        json["sourceSummaries"]
            .as_array()
            .unwrap()
            .iter()
            .all(|v| v.get("evidenceSummary").is_none())
    );
    let mut forged = external(0);
    forged.evidence_detail = Some(q::RequestEvidenceDetail {
        matching: q::RequestMatching::SourceReported,
        billing: Some(q::RequestBilling {
            actual: vec![],
            estimated: vec![],
            unknown_parts: 0u64.into(),
        }),
        timing_note: None,
        overflow_fields: vec![],
    });
    assert!(
        history
            .unified_request_page(&query, Utc::now(), "fixed", Some(publication(vec![forged])))
            .is_err()
    );
    let mut forged = external(0);
    forged.record.unit = RequestUnit::LogicalRequest;
    query.unit = RequestUnit::LogicalRequest;
    assert!(
        history
            .unified_request_page(&query, Utc::now(), "fixed", Some(publication(vec![forged])))
            .is_err()
    );
}

#[test]
fn logical_request_part_overflow_is_not_sorted_or_displayed_as_a_capped_known_total() {
    let root = tempfile::tempdir().unwrap();
    let mut history = History::open(root.path()).unwrap();
    let mut a = observed("part-a");
    a.logical_id = Some("overflow-group".into());
    a.usage.total_tokens = Some(u64::MAX);
    let mut b = a.clone();
    b.id = "part-b".into();
    let overflow = requests::logical_requests(vec![a, b], &HashMap::new())
        .pop()
        .unwrap();
    let mut safe = observed("safe");
    safe.usage.total_tokens = Some(300);
    let safe = requests::logical_requests(vec![safe], &HashMap::new())
        .pop()
        .unwrap();
    history.insert_request(&overflow).unwrap();
    history.insert_request(&safe).unwrap();
    let mut missing = observed("missing-a");
    missing.logical_id = Some("missing-group".into());
    missing.usage.total_tokens = None;
    let mut second = missing.clone();
    second.id = "missing-b".into();
    let missing = requests::logical_requests(vec![missing, second], &HashMap::new())
        .pop()
        .unwrap();
    history.insert_request(&missing).unwrap();
    let mut query = input();
    query.sort = Some(q::RequestSort::Tokens);
    query.include_evidence = Some(true);
    let page = history
        .unified_request_page(&query, Utc::now(), "fixed", Some(publication(vec![])))
        .unwrap();
    assert_eq!(page.items[0].record.id, safe.id);
    let summary = &page.source_summaries[0];
    assert_eq!(summary.usage_missing.as_ref().unwrap().value(), 1);
    assert_eq!(
        summary
            .evidence_summary
            .as_ref()
            .unwrap()
            .usage_missing_parts
            .as_ref()
            .unwrap()
            .value(),
        2
    );
    assert!(summary.usage.total_tokens.is_none());
    assert_eq!(
        page.items
            .iter()
            .find(|r| r.record.id == overflow.id)
            .unwrap()
            .evidence_detail
            .as_ref()
            .unwrap()
            .overflow_fields,
        ["totalTokens"]
    );
    assert!(
        page.items
            .iter()
            .find(|r| r.record.id == overflow.id)
            .unwrap()
            .record
            .usage
            .total_tokens
            .is_none()
    );
    assert_eq!(
        history.request_detail(&overflow.id).unwrap().unwrap()["usage"]["totalTokens"],
        json!(u64::MAX),
        "the original request model is not rewritten by the new projection"
    );
}

#[test]
fn whole_scope_sorts_and_evidence_do_not_decode_unselected_attempt_payloads() {
    let root = tempfile::tempdir().unwrap();
    let mut history = History::open(root.path()).unwrap();
    let mut cold_version = 0;
    for n in 0..41 {
        let id = format!("row-{n:03}");
        let row = requests::logical_requests(vec![observed(&id)], &HashMap::new())
            .pop()
            .unwrap();
        cold_version = history.insert_request(&row).unwrap();
    }
    // Poison only a disposable private payload beyond the first page. The
    // immutable source observations are not changed. All five sorts and full
    // summaries must use indexed metadata; page two still requires that payload.
    history
        .db
        .execute(
            "UPDATE record_payloads SET value='unreadable cold payload' WHERE v=?1",
            [cold_version],
        )
        .unwrap();
    for sort in [
        q::RequestSort::Time,
        q::RequestSort::Tokens,
        q::RequestSort::Duration,
        q::RequestSort::Ttft,
        q::RequestSort::CostUsd,
    ] {
        let mut query = input();
        query.sort = Some(sort);
        query.include_evidence = Some(true);
        let first = history
            .unified_request_page(&query, Utc::now(), "fixed", Some(publication(vec![])))
            .unwrap();
        assert_eq!(first.items.len(), 40);
        assert_eq!(
            first.source_summaries[0].records.as_ref().unwrap().value(),
            41
        );
        assert_eq!(
            first.source_summaries[0]
                .usage
                .total_tokens
                .as_ref()
                .unwrap()
                .value(),
            41
        );
        assert_eq!(
            first.source_summaries[0]
                .evidence_summary
                .as_ref()
                .unwrap()
                .billing
                .as_ref()
                .unwrap()
                .actual[0]
                .parts
                .value(),
            41
        );
        query.snapshot_token = Some(first.snapshot_token);
        query.cursor = first.next_cursor;
        assert_eq!(
            history
                .unified_request_page(&query, Utc::now(), "fixed", None)
                .unwrap_err()
                .code,
            d::ErrorCode::Unavailable
        );
    }
}
