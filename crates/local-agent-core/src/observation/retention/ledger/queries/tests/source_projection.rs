use super::*;
use crate::observation::{ModelObservation, retention::FactError};

const CHANGES: &[&str] = &[
    "usage-input",
    "usage-output",
    "usage-total",
    "usage-missing",
    "status",
    "ttft",
    "duration",
    "throughput",
    "provider",
    "model",
    "attempt-cost",
    "tiny-attempt-cost",
    "attempt-issue",
    "cost-source",
    "price-version",
    "cost-component",
    "backoff",
    "order",
];

fn corrupt(request: &mut Request, change: &str) {
    match change {
        "usage-input" => request.usage.input_tokens = Some(9_999_999),
        "usage-output" => request.usage.output_tokens = Some(9_999_999),
        "usage-total" => request.usage.total_tokens = Some(9_999_999),
        "usage-missing" => request.usage_missing = 0,
        "status" => request.status = "failed".into(),
        "ttft" => request.ttft_ms = Some(987_654.0),
        "duration" => request.duration_ms = Some(765_432.0),
        "throughput" => request.throughput = Some(1234.0),
        "provider" => request.provider_key = "forged-provider".into(),
        "model" => request.model = Some("forged-model".into()),
        "attempt-cost" => {
            let cost = &mut request.attempts[0].cost;
            cost.kind = "actual";
            cost.amount = Some(123.456);
            cost.currency = Some("EUR");
        }
        "tiny-attempt-cost" => {
            let cost = &mut request.attempts[0].cost;
            cost.amount = Some(cost.amount.unwrap() + 1e-13);
        }
        "attempt-issue" => {
            request.attempts[0].issue = Some("private-projection-issue-sentinel".into());
        }
        "cost-source" => {
            request.attempts[0].cost.source = Some("private-projection-cost-source-sentinel");
        }
        "price-version" => {
            request.attempts[0].cost.price.as_mut().unwrap().version =
                "private-projection-price-version-sentinel";
        }
        "cost-component" => {
            request.attempts[0].cost.components[0].amount += 42.0;
        }
        "backoff" => request.attempts[1].backoff_ms = Some(1234.0),
        "order" => request.attempts.swap(0, 1),
        _ => unreachable!(),
    }
}

fn raw(db: &rusqlite::Connection) -> Vec<(String, i64, String, String)> {
    db.prepare("SELECT id,revision,started_at,value FROM model_observations ORDER BY id")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

fn stage(
    ledger: &FactLedger<'_>,
    version: u32,
    requests: &[&Request],
) -> Result<StageResult, LedgerError> {
    match version {
        2 => ledger.stage_attributed_batch(requests, directory(), &[]),
        3 => ledger.stage_queryable_batch(requests, directory(), &[]),
        _ => unreachable!(),
    }
}

#[test]
fn source_constructors_reject_caller_derived_statistics_and_attempt_evidence() {
    let mut accepted = vec![];
    for version in [2, 3] {
        for change in CHANGES {
            let (mut request, rows) = retry();
            corrupt(&mut request, change);
            let result = match version {
                2 => GatewayFact::from_request_with_evidence(
                    &request,
                    &rows,
                    STORE,
                    directory(),
                    &[],
                ),
                _ => GatewayFact::from_request_queryable(&request, &rows, STORE, directory(), &[]),
            };
            match result {
                Err(FactError::InvalidRequest) => {}
                Err(error) => accepted.push(format!("v{version} {change}: wrong error {error:?}")),
                Ok(fact) => accepted.push(format!(
                    "v{version} {change}: accepted; private_marker={}",
                    serde_json::to_string(&fact)
                        .unwrap()
                        .contains("private-projection-")
                )),
            }
        }
    }
    assert!(accepted.is_empty(), "{}", accepted.join("\n"));
}

#[test]
fn source_staging_rejects_forged_projections_without_committing_or_changing_raw_rows() {
    let mut accepted = vec![];
    for version in [2, 3] {
        for change in CHANGES {
            let temp = tempfile::tempdir().unwrap();
            let store = ObservationStore::open(temp.path()).unwrap();
            let (mut request, rows) = retry();
            store.save_batch(&rows).unwrap();
            let ledger = store.retention_ledger(STORE).unwrap();
            let before = raw(&store.0);
            corrupt(&mut request, change);
            let result = stage(&ledger, version, &[&request]);
            let values: Vec<String> = store
                .0
                .prepare("SELECT value FROM retention_fact_staging")
                .unwrap()
                .query_map([], |r| r.get(0))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap();
            assert_eq!(before, raw(&store.0));
            if result != Err(LedgerError::SourceChanged) {
                accepted.push(format!(
                    "v{version} {change}: {result:?}; facts={}; private_marker={}",
                    values.len(),
                    values.iter().any(|v| v.contains("private-projection-"))
                ));
            } else {
                assert!(values.is_empty(), "v{version} {change}");
            }
        }
    }
    assert!(accepted.is_empty(), "{}", accepted.join("\n"));
}

fn sibling() -> (Request, Vec<ModelObservation>) {
    let (_, mut rows) = retry();
    for row in &mut rows {
        row.id = format!("sibling-{}", row.id);
        row.logical_id = Some("model-service-sibling".into());
    }
    let requests = crate::requests::logical_requests(rows.clone(), &Default::default());
    assert_eq!(requests.len(), 1);
    (requests[0].clone(), rows)
}

#[test]
fn one_forged_projection_rolls_back_all_insertions_and_existing_fact_updates() {
    let mut failures = vec![];
    for version in [2, 3] {
        for updating in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let store = ObservationStore::open(temp.path()).unwrap();
            let (mut a, mut rows_a) = retry();
            let (mut b, rows_b) = sibling();
            store.save_batch(&rows_a).unwrap();
            store.save_batch(&rows_b).unwrap();
            let ledger = store.retention_ledger(STORE).unwrap();
            if updating {
                stage(&ledger, version, &[&a]).unwrap();
                rows_a[0].revision += 1;
                rows_a[0].usage.input_tokens = Some(rows_a[0].usage.input_tokens.unwrap_or(0) + 1);
                store.save_batch(&rows_a).unwrap();
                a = canonical(rows_a);
            }
            let before: Vec<String> = store
                .0
                .prepare("SELECT value FROM retention_fact_staging ORDER BY key")
                .unwrap()
                .query_map([], |r| r.get(0))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap();
            let source = raw(&store.0);
            corrupt(&mut b, "usage-input");
            let result = stage(&ledger, version, &[&a, &b]);
            let after: Vec<String> = store
                .0
                .prepare("SELECT value FROM retention_fact_staging ORDER BY key")
                .unwrap()
                .query_map([], |r| r.get(0))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap();
            assert_eq!(source, raw(&store.0));
            if result != Err(LedgerError::SourceChanged) || before != after {
                failures.push(format!(
                    "v{version} updating={updating}: {result:?}; unchanged={}",
                    before == after
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn canonical_statistics_keep_allowed_context_annotations_and_idempotent_payloads() {
    for version in [2, 3] {
        let temp = tempfile::tempdir().unwrap();
        let store = ObservationStore::open(temp.path()).unwrap();
        let (mut request, rows) = retry();
        let expected = serde_json::to_value(crate::requests::Summary::of([&request])).unwrap();
        request.session_id = Some("context-session".into());
        request.project_id = Some("context-project".into());
        request.session_title = Some("private-context-display-sentinel".into());
        request.project_name = Some("private-context-display-sentinel".into());
        request.provider = "private-context-display-sentinel".into();
        store.save_batch(&rows).unwrap();
        let ledger = store.retention_ledger(STORE).unwrap();
        let before = raw(&store.0);
        assert_eq!(stage(&ledger, version, &[&request]).unwrap().inserted, 1);
        let payload: String = store
            .0
            .query_row("SELECT value FROM retention_fact_staging", [], |r| r.get(0))
            .unwrap();
        let parsed: Value = serde_json::from_str(&payload).unwrap();
        assert_eq!(parsed["version"], version);
        same(parsed["stats"].clone(), expected);
        assert_eq!(
            parsed["attribution"]["session"],
            key(&ledger.namespace, "session", "context-session")
        );
        assert_eq!(
            parsed["attribution"]["project"],
            key(&ledger.namespace, "project", "context-project")
        );
        assert!(!payload.contains("private-context-display-sentinel"));
        assert_eq!(stage(&ledger, version, &[&request]).unwrap().unchanged, 1);
        assert_eq!(before, raw(&store.0));
        let next: String = store
            .0
            .query_row("SELECT value FROM retention_fact_staging", [], |r| r.get(0))
            .unwrap();
        assert_eq!(payload, next);
    }
}

#[test]
fn valid_zero_unknown_and_unmeasurable_source_statistics_remain_distinct() {
    for version in [2, 3] {
        for variant in ["zero", "missing", "time", "clock", "model"] {
            let temp = tempfile::tempdir().unwrap();
            let store = ObservationStore::open(temp.path()).unwrap();
            let (_, mut rows) = retry();
            for row in &mut rows {
                match variant {
                    "zero" => {
                        row.usage = crate::sessions::Usage {
                            input_tokens: Some(0),
                            output_tokens: Some(0),
                            total_tokens: Some(0),
                            ..Default::default()
                        };
                    }
                    "missing" => row.usage = Default::default(),
                    "time" => row.started_at = "time-unknown".into(),
                    "clock" => {
                        row.clock_id = None;
                        row.start_tick_ms = None;
                    }
                    "model" => {
                        row.model = None;
                        row.routed_model = None;
                    }
                    _ => unreachable!(),
                }
            }
            let request = canonical(rows.clone());
            let expected = serde_json::to_value(crate::requests::Summary::of([&request])).unwrap();
            store.save_batch(&rows).unwrap();
            let before = raw(&store.0);
            let ledger = store.retention_ledger(STORE).unwrap();
            stage(&ledger, version, &[&request]).unwrap();
            same(
                serde_json::to_value(ledger.staging_summary().unwrap()).unwrap(),
                expected,
            );
            assert_eq!(before, raw(&store.0));
        }
    }
}

#[test]
fn raw_input_order_does_not_change_the_canonical_projection_or_attempt_order() {
    let (request, mut rows) = retry();
    for queryable in [false, true] {
        let make = if queryable {
            GatewayFact::from_request_queryable
        } else {
            GatewayFact::from_request_with_evidence
        };
        let expected =
            serde_json::to_value(make(&request, &rows, STORE, directory(), &[]).unwrap()).unwrap();
        rows.reverse();
        let actual =
            serde_json::to_value(make(&request, &rows, STORE, directory(), &[]).unwrap()).unwrap();
        assert_eq!(actual, expected);
    }
}

#[test]
fn explicitly_restaging_a_legacy_bad_projection_repairs_it_from_current_raw_source() {
    for version in [2, 3] {
        let temp = tempfile::tempdir().unwrap();
        let store = ObservationStore::open(temp.path()).unwrap();
        let (request, rows) = retry();
        store.save_batch(&rows).unwrap();
        let ledger = store.retention_ledger(STORE).unwrap();
        stage(&ledger, version, &[&request]).unwrap();
        let good: String = store
            .0
            .query_row("SELECT value FROM retention_fact_staging", [], |r| r.get(0))
            .unwrap();
        let mut bad: Value = serde_json::from_str(&good).unwrap();
        bad["stats"]["usage"]["inputTokens"] = json!(777_777);
        let bad = bad.to_string();
        store
            .0
            .execute(
                "UPDATE retention_fact_staging SET value=?1,projection_digest=?2",
                rusqlite::params![bad, digest(&bad)],
            )
            .unwrap();
        let before = raw(&store.0);
        assert_eq!(stage(&ledger, version, &[&request]).unwrap().updated, 1);
        let repaired: String = store
            .0
            .query_row("SELECT value FROM retention_fact_staging", [], |r| r.get(0))
            .unwrap();
        assert_eq!(repaired, good);
        assert_eq!(before, raw(&store.0));
    }
}

#[test]
fn summary_only_facts_do_not_gain_source_proof_without_an_explicit_valid_upgrade() {
    for version in [2, 3] {
        let temp = tempfile::tempdir().unwrap();
        let store = ObservationStore::open(temp.path()).unwrap();
        let (request, rows) = retry();
        store.save_batch(&rows).unwrap();
        let ledger = store.retention_ledger(STORE).unwrap();
        ledger
            .stage_batch(&[(&request, [7; 32])], directory())
            .unwrap();
        assert_eq!(ledger.attribution_coverage().unwrap().summary_only_facts, 1);
        let mut forged = request.clone();
        corrupt(&mut forged, "usage-input");
        assert_eq!(
            stage(&ledger, version, &[&forged]),
            Err(LedgerError::SourceChanged)
        );
        assert_eq!(ledger.attribution_coverage().unwrap().summary_only_facts, 1);
        assert_eq!(stage(&ledger, version, &[&request]).unwrap().updated, 1);
        assert_eq!(ledger.attribution_coverage().unwrap().complete_facts, 1);
    }
}
