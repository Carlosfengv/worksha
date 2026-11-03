use super::*;
use crate::observation::{
    ObservationStore,
    retention::test_support::{STORE, canonical, directory, retry},
};
use serde_json::{Value, json};
mod source_projection;

pub(super) fn same(actual: Value, mut expected: Value) {
    fn amounts(a: &Value, b: &mut Value) {
        match (a, b) {
            (Value::Object(a), Value::Object(b)) => {
                for (key, value) in a {
                    if key == "amount" {
                        let other = b.get(key).unwrap();
                        assert!(
                            (value.as_f64().unwrap() - other.as_f64().unwrap()).abs() <= 1e-12,
                            "amount differs: actual={value}, expected={other}"
                        );
                        b.insert(key.clone(), value.clone());
                    } else if let Some(other) = b.get_mut(key) {
                        amounts(value, other);
                    }
                }
            }
            (Value::Array(a), Value::Array(b)) => {
                for (a, b) in a.iter().zip(b) {
                    amounts(a, b);
                }
            }
            _ => {}
        }
    }
    amounts(&actual, &mut expected);
    assert_eq!(actual, expected);
}

#[test]
fn missing_or_changed_sources_roll_back_the_whole_attribution_batch() {
    let temp = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(temp.path()).unwrap();
    let ledger = store.retention_ledger(STORE).unwrap();
    let (a, rows) = retry();
    store.save_batch(&rows).unwrap();
    let mut other = rows[0].clone();
    other.id = "other-attempt".into();
    other.logical_id = None;
    let b = canonical(vec![other.clone()]);
    let empty = || {
        assert_eq!(ledger.staging_summary().unwrap().requests, 0);
    };
    assert_eq!(
        ledger.stage_attributed_batch(&[&a, &b], directory(), &[]),
        Err(LedgerError::SourceChanged)
    );
    empty();
    other.revision += 1;
    store.save(&other).unwrap();
    assert_eq!(
        ledger.stage_attributed_batch(&[&a, &b], directory(), &[]),
        Err(LedgerError::SourceChanged)
    );
    empty();
    let current = canonical(vec![other.clone()]);
    other.usage.input_tokens = Some(9999);
    store.save(&other).unwrap();
    assert_eq!(
        ledger.stage_attributed_batch(&[&a, &current], directory(), &[]),
        Err(LedgerError::SourceChanged)
    );
    empty();
    assert_eq!(store.load_all().unwrap().len(), 5);
}

#[test]
fn an_extra_committed_retry_rejects_the_stale_request_without_changing_any_fact() {
    let temp = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(temp.path()).unwrap();
    let ledger = store.retention_ledger(STORE).unwrap();
    let (request, mut rows) = retry();
    store.save_batch(&rows).unwrap();
    ledger
        .stage_attributed_batch(&[&request], directory(), &[])
        .unwrap();
    let before: String = store
        .0
        .query_row("SELECT value FROM retention_fact_staging", [], |r| r.get(0))
        .unwrap();
    let mut late = rows.last().unwrap().clone();
    late.id = "late-retry".into();
    late.revision += 1;
    late.status = "in_progress".into();
    rows.push(late.clone());
    store.save(&late).unwrap();
    assert_eq!(
        ledger.stage_attributed_batch(&[&request], directory(), &[]),
        Err(LedgerError::SourceChanged)
    );
    let after: String = store
        .0
        .query_row("SELECT value FROM retention_fact_staging", [], |r| r.get(0))
        .unwrap();
    assert_eq!(after, before);
    let current = canonical(rows);
    ledger
        .stage_attributed_batch(&[&current], directory(), &[])
        .unwrap();
    assert_eq!(ledger.staging_summary().unwrap().attempts, 5);
}

#[test]
fn a_valid_partial_group_cannot_be_staged_as_the_complete_committed_request() {
    let temp = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(temp.path()).unwrap();
    let ledger = store.retention_ledger(STORE).unwrap();
    let (_, rows) = retry();
    store.save_batch(&rows).unwrap();
    let partial = canonical(rows[..2].to_vec());
    assert_eq!(
        ledger.stage_attributed_batch(&[&partial], directory(), &[]),
        Err(LedgerError::SourceChanged)
    );
    assert_eq!(ledger.staging_summary().unwrap().requests, 0);
    assert_eq!(store.load_all().unwrap().len(), 4);
}

#[test]
fn source_reuse_requires_a_durable_clock_initialized_only_by_explicit_staging() {
    let temp = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(temp.path()).unwrap();
    let (_, rows) = retry();
    store.save_batch(&rows).unwrap();
    let exists = || {
        store.0.query_row("SELECT EXISTS(SELECT 1 FROM main.sqlite_master WHERE name='retention_source_clock')", [], |r|r.get::<_,bool>(0)).unwrap()
    };
    assert!(!exists());
    let _ledger = store.retention_ledger(STORE).unwrap();
    assert!(exists());
    assert_eq!(
        store
            .0
            .query_row("SELECT version FROM retention_fact_scope", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        2
    );
    assert_eq!(store.load_all().unwrap().len(), 4);
}

#[test]
fn source_reuse_requires_the_original_raw_table_shape_even_if_json_is_still_valid() {
    let temp = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(temp.path()).unwrap();
    let (request, rows) = retry();
    store.save_batch(&rows).unwrap();
    let ledger = store.retention_ledger(STORE).unwrap();
    store
        .0
        .execute_batch("ALTER TABLE model_observations ADD COLUMN rowid INTEGER DEFAULT 0;")
        .unwrap();
    assert_eq!(
        ledger.stage_attributed_batch(&[&request], directory(), &[]),
        Err(LedgerError::InvalidRecord)
    );
    assert_eq!(ledger.staging_summary().unwrap().requests, 0);
}

#[test]
fn source_reuse_requires_the_same_file_even_when_a_replacement_copies_the_epoch() {
    let temp = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(temp.path()).unwrap();
    let (request, rows) = retry();
    store.save_batch(&rows).unwrap();
    let ledger = store.retention_ledger(STORE).unwrap();
    let path = temp.path().join("observations.sqlite3");
    let copy = temp.path().join("copy.sqlite3");
    store
        .0
        .execute("VACUUM INTO ?1", [copy.to_str().unwrap()])
        .unwrap();
    std::fs::rename(&path, temp.path().join("original.sqlite3")).unwrap();
    std::fs::rename(copy, &path).unwrap();
    assert_eq!(
        ledger.stage_attributed_batch(&[&request], directory(), &[]),
        Err(LedgerError::SourceChanged)
    );
    assert_eq!(
        Connection::open(path)
            .unwrap()
            .query_row("SELECT count(*) FROM model_observations", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        4
    );
}

#[test]
fn exact_durable_bytes_drive_idempotence_and_complete_facts_cannot_be_downgraded() {
    let temp = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(temp.path()).unwrap();
    let ledger = store.retention_ledger(STORE).unwrap();
    let (request, rows) = retry();
    store.save_batch(&rows).unwrap();
    assert_eq!(
        ledger
            .stage_attributed_batch(&[&request], directory(), &[])
            .unwrap()
            .inserted,
        1
    );
    assert_eq!(
        ledger
            .stage_attributed_batch(&[&request], directory(), &[])
            .unwrap()
            .unchanged,
        1
    );
    let source = || {
        store
            .0
            .query_row(
                "SELECT source_fingerprint FROM retention_fact_staging",
                [],
                |r| r.get::<_, String>(0),
            )
            .unwrap()
    };
    let before = source();
    store
        .0
        .execute(
            "UPDATE model_observations SET value=value || ' ' WHERE id=?1",
            [&rows[0].id],
        )
        .unwrap();
    assert_eq!(
        ledger
            .stage_attributed_batch(&[&request], directory(), &[])
            .unwrap()
            .updated,
        1
    );
    assert_ne!(source(), before);
    let before = source();
    assert_eq!(
        ledger.stage_batch(&[(&request, [0; 32])], directory()),
        Err(LedgerError::ProjectionDowngrade)
    );
    assert_eq!(source(), before);
    assert_eq!(ledger.attribution_coverage().unwrap().complete_facts, 1);
}

#[test]
fn attributed_batches_bound_requests_attempts_and_raw_bytes_without_partial_writes() {
    let temp = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(temp.path()).unwrap();
    let ledger = store.retention_ledger(STORE).unwrap();
    let (mut request, rows) = retry();
    store.save_batch(&rows).unwrap();
    assert_eq!(
        ledger.stage_attributed_batch(&vec![&request; 65], directory(), &[]),
        Err(LedgerError::BatchLimit)
    );
    let attempt = request.attempts[0].clone();
    request.attempts = vec![attempt; 1025];
    assert_eq!(
        ledger.stage_attributed_batch(&[&request], directory(), &[]),
        Err(LedgerError::BatchLimit)
    );
    let (request, _) = retry();
    store
        .0
        .execute(
            "UPDATE model_observations SET value=value || ?2 WHERE id=?1",
            params![rows[0].id, " ".repeat(8 * 1024 * 1024)],
        )
        .unwrap();
    assert_eq!(
        ledger.stage_attributed_batch(&[&request], directory(), &[]),
        Err(LedgerError::BatchLimit)
    );
    assert_eq!(ledger.staging_summary().unwrap().requests, 0);
    assert_eq!(store.load_all().unwrap().len(), 4);
}

#[test]
fn staged_request_parts_match_the_existing_consumption_groups_without_duplicating_logical_counts() {
    let temp = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(temp.path()).unwrap();
    let ledger = store.retention_ledger(STORE).unwrap();
    let (request, rows) = retry();
    store.save_batch(&rows).unwrap();
    ledger
        .stage_attributed_batch(&[&request], directory(), &[])
        .unwrap();
    assert_eq!(ledger.staging_summary().unwrap().requests, 1);
    for dimension in [PartDimension::Provider, PartDimension::Model] {
        for metric in ["tokens", "cost"] {
            let query = crate::requests::Query {
                metric: Some(metric.into()),
                dimension: Some(
                    match dimension {
                        PartDimension::Provider => "provider",
                        PartDimension::Model => "model",
                    }
                    .into(),
                ),
                ..Default::default()
            };
            let snapshot = crate::requests::Snapshot::new(
                query,
                vec![request.clone()],
                "2026-10-28T00:00:00Z".parse().unwrap(),
                "fixture".into(),
                None,
            )
            .unwrap();
            assert_eq!(snapshot.dashboard.groups.len(), 2);
            for group in &snapshot.dashboard.groups {
                let contribution = ledger.staging_part_summary(dimension, &group.id).unwrap();
                let actual = contribution.metric(metric, "USD");
                if metric == "cost" {
                    match (actual, group.value) {
                        (Some(a), Some(b)) => assert!((a - b).abs() <= 1e-12),
                        (None, None) => {}
                        _ => panic!("cost coverage differs"),
                    }
                } else {
                    assert_eq!(actual, group.value);
                }
                assert_eq!(contribution.requests, 1);
                assert_eq!(contribution.attempts, 2);
            }
        }
    }
}

#[test]
fn provider_and_model_contributions_match_their_canonical_attempt_statistics() {
    let temp = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(temp.path()).unwrap();
    let ledger = store.retention_ledger(STORE).unwrap();
    let (request, rows) = retry();
    store.save_batch(&rows).unwrap();
    ledger
        .stage_attributed_batch(&[&request], directory(), &[])
        .unwrap();
    let records = crate::provider_usage::gateway_records(rows.clone(), &[]);
    for selected in [
        None,
        Some("provider-a"),
        Some("provider-b"),
        Some("_unknown"),
    ] {
        let mut expected = crate::provider_usage::Totals::default();
        for record in &records {
            if selected.is_none_or(|id| {
                if id == "_unknown" {
                    record.provider_id.is_none()
                } else {
                    record.provider_id.as_deref() == Some(id)
                }
            }) {
                expected.add(record);
            }
        }
        expected.finish();
        same(
            ledger.staging_provider_contribution(selected).unwrap(),
            serde_json::to_value(expected).unwrap(),
        );
    }
    let mut scopes = vec![None];
    scopes.extend(rows.iter().map(|row| {
        Some(crate::model_usage::provider_identity(
            Some(&row.provider),
            Some(&row.endpoint),
            Some(row.binding_revision),
        ))
    }));
    for scope in scopes {
        let mut expected = crate::model_usage::UsageTotals::default();
        for row in &rows {
            let identity = crate::model_usage::provider_identity(
                Some(&row.provider),
                Some(&row.endpoint),
                Some(row.binding_revision),
            );
            if scope.as_ref().is_none_or(|id| id == &identity) {
                let cost = crate::provider_cost::calculate(
                    Some(&row.endpoint),
                    row.model.as_deref(),
                    Some(&row.started_at),
                    &row.status,
                    &row.usage,
                    row.reported_cost.as_ref(),
                    row.ended_at.as_deref(),
                );
                expected.add(&row.usage, &cost);
            }
        }
        same(
            ledger.staging_model_contribution(scope.as_deref()).unwrap(),
            serde_json::to_value(expected).unwrap(),
        );
    }
}

#[test]
fn anonymous_session_and_project_filters_preserve_scope_without_storing_paths() {
    let temp = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(temp.path()).unwrap();
    let ledger = store.retention_ledger(STORE).unwrap();
    let (a, mut rows) = retry();
    store.save_batch(&rows).unwrap();
    for row in &mut rows {
        row.id.push_str("-b");
        row.session_id = Some("another-private-session".into());
    }
    store.save_batch(&rows).unwrap();
    let mut b = canonical(rows);
    b.project_id = Some("another-private-project".into());
    ledger
        .stage_attributed_batch(&[&a, &b], directory(), &[])
        .unwrap();
    assert_eq!(
        ledger.staging_scope_summary(None, None).unwrap().requests,
        2
    );
    assert_eq!(
        ledger
            .staging_scope_summary(a.session_id.as_deref(), None)
            .unwrap()
            .requests,
        1
    );
    assert_eq!(
        ledger
            .staging_scope_summary(None, a.project_id.as_deref())
            .unwrap()
            .requests,
        1
    );
    assert_eq!(
        ledger
            .staging_scope_summary(a.session_id.as_deref(), b.project_id.as_deref())
            .unwrap()
            .requests,
        0
    );
    assert_eq!(
        ledger
            .staging_scope_summary(Some("' OR 1=1 --"), None)
            .unwrap()
            .requests,
        0
    );
    let mut statement = store
        .0
        .prepare("SELECT value FROM retention_fact_staging")
        .unwrap();
    for value in statement.query_map([], |r| r.get::<_, String>(0)).unwrap() {
        assert!(!value.unwrap().contains("private-"));
    }
}

#[test]
fn legacy_summary_only_facts_remain_readable_but_cannot_fabricate_attribution() {
    let temp = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(temp.path()).unwrap();
    let ledger = store.retention_ledger(STORE).unwrap();
    let (request, rows) = retry();
    store.save_batch(&rows).unwrap();
    let old = GatewayFact::from_request(&request, &ledger.namespace, directory()).unwrap();
    let value = serde_json::to_string(&old).unwrap();
    store
        .0
        .execute(
            "INSERT INTO retention_fact_staging VALUES(?1,?2,?3,'staged',?4)",
            params![old.key, "01".repeat(32), digest(&value), value],
        )
        .unwrap();
    assert_eq!(ledger.staging_summary().unwrap().requests, 1);
    assert_eq!(ledger.attribution_coverage().unwrap().summary_only_facts, 1);
    assert!(matches!(
        ledger.staging_part_summary(PartDimension::Provider, "provider-a"),
        Err(LedgerError::IncompleteFacts)
    ));
    assert!(matches!(
        ledger.staging_provider_contribution(None),
        Err(LedgerError::IncompleteFacts)
    ));
    assert!(matches!(
        ledger.staging_model_contribution(None),
        Err(LedgerError::IncompleteFacts)
    ));
    assert_eq!(
        ledger
            .stage_attributed_batch(&[&request], directory(), &[])
            .unwrap()
            .updated,
        1
    );
    assert_eq!(
        ledger.attribution_coverage().unwrap(),
        AttributionCoverage {
            complete_facts: 1,
            summary_only_facts: 0
        }
    );
    let namespace = ledger.namespace.clone();
    drop(ledger);
    drop(store);
    let reopened = ObservationStore::open(temp.path()).unwrap();
    let ledger = reopened.retention_ledger(STORE).unwrap();
    assert_eq!(ledger.namespace, namespace);
    assert_eq!(ledger.staging_summary().unwrap().requests, 1);
}

#[test]
fn sealed_legacy_and_unknown_nested_attribution_are_preserved() {
    let temp = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(temp.path()).unwrap();
    let ledger = store.retention_ledger(STORE).unwrap();
    let (request, rows) = retry();
    store.save_batch(&rows).unwrap();
    let old = GatewayFact::from_request(&request, &ledger.namespace, directory()).unwrap();
    let value = serde_json::to_string(&old).unwrap();
    store
        .0
        .execute(
            "INSERT INTO retention_fact_staging VALUES(?1,?2,?3,'sealed',?4)",
            params![old.key, "01".repeat(32), digest(&value), value],
        )
        .unwrap();
    assert_eq!(
        ledger.stage_attributed_batch(&[&request], directory(), &[]),
        Err(LedgerError::ProtectedFact)
    );
    assert_eq!(
        store
            .0
            .query_row("SELECT value FROM retention_fact_staging", [], |r| r
                .get::<_, String>(0))
            .unwrap(),
        value
    );
    store
        .0
        .execute("UPDATE retention_fact_staging SET state='staged'", [])
        .unwrap();
    ledger
        .stage_attributed_batch(&[&request], directory(), &[])
        .unwrap();
    let mut full: Value = serde_json::from_str(
        &store
            .0
            .query_row("SELECT value FROM retention_fact_staging", [], |r| {
                r.get::<_, String>(0)
            })
            .unwrap(),
    )
    .unwrap();
    full["attribution"]["attempts"][0]["providerStats"]["future_private"] = json!("must-keep");
    let changed = full.to_string();
    store
        .0
        .execute(
            "UPDATE retention_fact_staging SET value=?1,projection_digest=?2",
            params![changed, digest(&changed)],
        )
        .unwrap();
    assert!(matches!(
        ledger.staging_provider_contribution(None),
        Err(LedgerError::InvalidRecord)
    ));
    assert_eq!(
        ledger.stage_attributed_batch(&[&request], directory(), &[]),
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

#[test]
fn provider_attribution_uses_recorded_evidence_and_does_not_guess_from_endpoint() {
    let temp = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(temp.path()).unwrap();
    let ledger = store.retention_ledger(STORE).unwrap();
    let (_, mut rows) = retry();
    let mut row = rows.remove(0);
    row.logical_id = None;
    row.provider_id = None;
    store.save(&row).unwrap();
    let request = canonical(vec![row.clone()]);
    ledger
        .stage_attributed_batch(&[&request], directory(), &[])
        .unwrap();
    assert_eq!(
        ledger
            .staging_provider_contribution(Some("provider-a"))
            .unwrap()["calls"],
        0
    );
    assert_eq!(
        ledger
            .staging_provider_contribution(Some("_unknown"))
            .unwrap()["calls"],
        1
    );
    let evidence = |id: &str, revision| crate::provider_usage::Evidence {
        agent: row.agent_id.clone(),
        provider_id: id.into(),
        provider_name: row.provider.clone(),
        endpoint: row.endpoint.clone(),
        protocol: row.protocol.clone(),
        revision,
        native_provider: None,
        model_aliases: vec![],
    };
    ledger
        .stage_attributed_batch(
            &[&request],
            directory(),
            &[evidence("provider-a", row.binding_revision)],
        )
        .unwrap();
    assert_eq!(
        ledger
            .staging_provider_contribution(Some("provider-a"))
            .unwrap()["calls"],
        1
    );
    ledger
        .stage_attributed_batch(
            &[&request],
            directory(),
            &[
                evidence("provider-a", row.binding_revision),
                evidence("provider-b", row.binding_revision),
            ],
        )
        .unwrap();
    assert_eq!(
        ledger
            .staging_provider_contribution(Some("_unknown"))
            .unwrap()["calls"],
        1
    );
    ledger
        .stage_attributed_batch(
            &[&request],
            directory(),
            &[evidence("provider-a", row.binding_revision + 1)],
        )
        .unwrap();
    assert_eq!(
        ledger
            .staging_provider_contribution(Some("provider-a"))
            .unwrap()["calls"],
        0
    );
}

#[test]
fn latest_seen_compares_instants_across_valid_rfc3339_spellings() {
    let temp = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(temp.path()).unwrap();
    let ledger = store.retention_ledger(STORE).unwrap();
    let mut requests = vec![];
    let mut records = vec![];
    for (index, time) in [
        "2026-10-27T00:00:00Z",
        "2026-10-27T00:00:00.000000001+00:00",
        "2026-10-27T08:00:00.000000002+08:00",
    ]
    .into_iter()
    .enumerate()
    {
        let at = chrono::DateTime::parse_from_rfc3339(time).unwrap().to_utc();
        let mut row = crate::observation::tests::request(at);
        row.id = index.to_string();
        row.started_at = time.into();
        row.ended_at = Some((at + chrono::Duration::seconds(1)).to_rfc3339());
        records.extend(crate::provider_usage::gateway_records(
            vec![row.clone()],
            &[],
        ));
        store.save(&row).unwrap();
        requests.push(canonical(vec![row]));
    }
    ledger
        .stage_attributed_batch(&requests.iter().collect::<Vec<_>>(), directory(), &[])
        .unwrap();
    let mut total = crate::provider_usage::Totals::default();
    total.last_seen = Some("2026-10-27T00:00:00Z".into());
    for r in records {
        total.add(&r);
    }
    assert_eq!(
        total.last_seen.as_deref(),
        Some("2026-10-27T00:00:00.000000002+00:00")
    );
    let (id, value): (String, String) = store
        .0
        .query_row(
            "SELECT key,value FROM retention_fact_staging ORDER BY key LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    let mut value: Value = serde_json::from_str(&value).unwrap();
    value["attribution"]["attempts"][0]["providerStats"]["lastSeen"] =
        json!("2026-10-27T00:00:00Z");
    let changed = value.to_string();
    store
        .0
        .execute(
            "UPDATE retention_fact_staging SET value=?2,projection_digest=?3 WHERE key=?1",
            params![id, changed, digest(&changed)],
        )
        .unwrap();
    let seen = ledger.staging_provider_contribution(None).unwrap()["lastSeen"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(
        chrono::DateTime::parse_from_rfc3339(&seen).unwrap()
            >= chrono::DateTime::parse_from_rfc3339("2026-10-27T00:00:00.000000001Z").unwrap()
    );
}
