use super::*;
use crate::observation::retention::test_support::{STORE, directory, retry};
use crate::observation::{ModelObservation, ObservationStore};
use chrono::{DateTime, Utc};
mod capacity;

fn fixture(
    id: &str,
    family: &str,
    product: Option<&str>,
    session: &str,
    project: &str,
    times: &[&str],
) -> (Request, Vec<ModelObservation>) {
    let (_, mut rows) = retry();
    rows.truncate(times.len());
    for (index, (row, time)) in rows.iter_mut().zip(times).enumerate() {
        row.id = format!("{id}-{index}");
        row.logical_id = Some(format!("model-service-{id}"));
        row.agent_id = family.into();
        row.agent_product_id = product.map(str::to_owned);
        row.session_id = Some(session.into());
        row.started_at = (*time).into();
        row.ended_at = DateTime::parse_from_rfc3339(time)
            .ok()
            .map(|at| (at + chrono::Duration::milliseconds(1)).to_rfc3339());
    }
    let mut grouped =
        crate::requests::logical_requests(rows.clone(), &std::collections::HashMap::new());
    assert_eq!(
        grouped.len(),
        1,
        "fixture must be one cross-Provider service request"
    );
    let mut request = grouped.remove(0);
    request.project_id = Some(project.into());
    (request, rows)
}

fn at(value: &str) -> DateTime<Utc> {
    value.parse().unwrap()
}
fn in_window(time: &str, filter: &GatewayFilter) -> bool {
    let parsed = DateTime::parse_from_rfc3339(time).ok().map(|v| v.to_utc());
    filter
        .start
        .is_none_or(|lo| parsed.is_some_and(|v| v >= lo))
        && filter.end.is_none_or(|hi| parsed.is_some_and(|v| v < hi))
}
fn provider_total(rows: &[ModelObservation], provider: Option<&str>) -> Value {
    let mut total = crate::provider_usage::Totals::default();
    for r in crate::provider_usage::gateway_records(rows.to_vec(), &[]) {
        if provider.is_none_or(|id| {
            if id == "_unknown" {
                r.provider_id.is_none()
            } else {
                r.provider_id.as_deref() == Some(id)
            }
        }) {
            total.add(&r);
        }
    }
    total.finish();
    serde_json::to_value(total).unwrap()
}
fn model_total(rows: &[ModelObservation], provider: Option<&str>) -> Value {
    let mut total = crate::model_usage::UsageTotals::default();
    for row in rows {
        let identity = crate::model_usage::provider_identity(
            Some(&row.provider),
            Some(&row.endpoint),
            Some(row.binding_revision),
        );
        if provider.is_none_or(|s| s == identity) {
            let cost = crate::provider_cost::calculate(
                Some(&row.endpoint),
                row.model.as_deref(),
                Some(&row.started_at),
                &row.status,
                &row.usage,
                row.reported_cost.as_ref(),
                row.ended_at.as_deref(),
            );
            total.add(&row.usage, &cost);
        }
    }
    serde_json::to_value(total).unwrap()
}
fn same(a: Value, b: Value) {
    super::super::tests::same(a, b);
}
fn pack(ledger: &FactLedger<'_>) -> PackedPreview {
    let keys = ledger
        .db
        .prepare("SELECT key FROM retention_fact_staging ORDER BY rowid")
        .unwrap()
        .query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    for batch in keys.chunks(MAX_BATCH) {
        ledger
            .pack_staging_batch(
                PackedKind::Gateway,
                &batch.iter().map(String::as_str).collect::<Vec<_>>(),
            )
            .unwrap();
    }
    ledger.packed_preview(PackedKind::Gateway).unwrap()
}

#[test]
fn logical_dates_and_attempt_dates_keep_distinct_nanosecond_and_timezone_boundaries() {
    let temp = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(temp.path()).unwrap();
    let (request, rows) = fixture(
        "boundary",
        "claude",
        None,
        "s",
        "p",
        &[
            "1969-12-31T23:59:59.999999999Z",
            "1970-01-01T08:00:00.000000000+08:00",
            "1970-01-01T00:00:00.000000001Z",
            "1970-01-01T00:00:00.000000002Z",
        ],
    );
    store.save_batch(&rows).unwrap();
    let ledger = store.retention_ledger(STORE).unwrap();
    ledger
        .stage_queryable_batch(&[&request], directory(), &[])
        .unwrap();
    let filter = GatewayFilter {
        start: Some(at("1970-01-01T00:00:00Z")),
        end: Some(at("1970-01-01T00:00:00.000000002Z")),
        agent: Some(AgentFilter::new(directory(), "claude").unwrap()),
        session: Some("s".into()),
        project: Some("p".into()),
    };
    assert_eq!(
        ledger.staging_summary_filtered(&filter).unwrap().requests,
        0,
        "root time={}, predicate={}",
        request.started_at,
        filter.sql(&ledger.namespace, "f.value")
    );
    let selected = rows
        .iter()
        .filter(|r| in_window(&r.started_at, &filter))
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(selected.len(), 2);
    same(
        ledger
            .staging_provider_contribution_filtered(None, &filter)
            .unwrap(),
        provider_total(&selected, None),
    );
    same(
        ledger
            .staging_model_contribution_filtered(None, &filter)
            .unwrap(),
        model_total(&selected, None),
    );
    let preview = pack(&ledger);
    assert_eq!(
        preview.gateway_summary_filtered(&filter).unwrap().requests,
        0
    );
    same(
        preview
            .provider_contribution_filtered(None, &filter)
            .unwrap(),
        provider_total(&selected, None),
    );
    same(
        preview.model_contribution_filtered(None, &filter).unwrap(),
        model_total(&selected, None),
    );
}

#[test]
fn date_agent_session_and_project_conditions_intersect_over_the_whole_draft_set() {
    let temp = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(temp.path()).unwrap();
    let mut data = vec![];
    for (id, family, product, session, project, time) in [
        ("a", "claude", None, "s1", "p1", "2026-10-27T00:00:01Z"),
        (
            "b",
            "codex",
            Some("codex-desktop"),
            "s1",
            "p2",
            "2026-10-28T00:00:01Z",
        ),
        ("c", "codex", None, "s2", "p1", "2026-10-28T00:00:01Z"),
    ] {
        let item = fixture(
            id,
            family,
            product,
            session,
            project,
            &[time, time, time, time],
        );
        store.save_batch(&item.1).unwrap();
        data.push(item);
    }
    let ledger = store.retention_ledger(STORE).unwrap();
    ledger
        .stage_queryable_batch(
            &data.iter().map(|(r, _)| r).collect::<Vec<_>>(),
            directory(),
            &[],
        )
        .unwrap();
    ledger.release_source_snapshot();
    let preview = pack(&ledger);
    for begin in [
        None,
        Some(at("2026-10-28T00:00:00Z")),
        Some(at("2026-10-29T00:00:00Z")),
    ] {
        for agent in [None, Some("codex"), Some("unknown")] {
            for session in [None, Some("s1"), Some("not-found")] {
                for project in [None, Some("p1"), Some("p2")] {
                    let filter = GatewayFilter {
                        start: begin,
                        agent: agent.map(|a| AgentFilter::new(directory(), a).unwrap()),
                        session: session.map(str::to_owned),
                        project: project.map(str::to_owned),
                        ..Default::default()
                    };
                    let selected = data
                        .iter()
                        .filter(|(r, _)| {
                            in_window(&r.started_at, &filter)
                                && directory().matches(
                                    agent,
                                    &r.agent_id,
                                    r.agent_product_id.as_deref(),
                                )
                                && session.is_none_or(|s| r.session_id.as_deref() == Some(s))
                                && project.is_none_or(|p| r.project_id.as_deref() == Some(p))
                        })
                        .collect::<Vec<_>>();
                    let requests = selected.iter().map(|(r, _)| r).collect::<Vec<_>>();
                    let expected = serde_json::to_value(Summary::of(requests)).unwrap();
                    same(
                        serde_json::to_value(ledger.staging_summary_filtered(&filter).unwrap())
                            .unwrap(),
                        expected.clone(),
                    );
                    same(
                        serde_json::to_value(preview.gateway_summary_filtered(&filter).unwrap())
                            .unwrap(),
                        expected,
                    );
                    let rows = selected
                        .iter()
                        .flat_map(|(_, rows)| rows.iter().cloned())
                        .collect::<Vec<_>>();
                    for provider in [
                        None,
                        Some("provider-a"),
                        Some("provider-b"),
                        Some("_unknown"),
                    ] {
                        let expected = provider_total(&rows, provider);
                        same(
                            ledger
                                .staging_provider_contribution_filtered(provider, &filter)
                                .unwrap(),
                            expected.clone(),
                        );
                        same(
                            preview
                                .provider_contribution_filtered(provider, &filter)
                                .unwrap(),
                            expected,
                        );
                    }
                    let model_scope = crate::model_usage::provider_identity(
                        Some(&data[0].1[0].provider),
                        Some(&data[0].1[0].endpoint),
                        Some(data[0].1[0].binding_revision),
                    );
                    for scope in [None, Some(model_scope.as_str())] {
                        let expected = model_total(&rows, scope);
                        same(
                            ledger
                                .staging_model_contribution_filtered(scope, &filter)
                                .unwrap(),
                            expected.clone(),
                        );
                        same(
                            preview.model_contribution_filtered(scope, &filter).unwrap(),
                            expected,
                        );
                    }
                    for dimension in [PartDimension::Provider, PartDimension::Model] {
                        let group = match dimension {
                            PartDimension::Provider => data[0].0.attempts[0].provider_key.clone(),
                            PartDimension::Model => data[0].1[0].routed_model.clone().unwrap(),
                        };
                        let fragments = selected
                            .iter()
                            .filter_map(|(r, _)| {
                                let attempts = r
                                    .attempts
                                    .iter()
                                    .filter(|a| match dimension {
                                        PartDimension::Provider => a.provider_key == group,
                                        PartDimension::Model => {
                                            a.observation
                                                .routed_model
                                                .as_deref()
                                                .or(a.observation.model.as_deref())
                                                .unwrap_or("模型未知")
                                                == group
                                        }
                                    })
                                    .cloned()
                                    .collect::<Vec<_>>();
                                (!attempts.is_empty())
                                    .then(|| crate::requests::with_attempts(r, attempts))
                            })
                            .collect::<Vec<_>>();
                        let expected = serde_json::to_value(Summary::of(fragments.iter())).unwrap();
                        same(
                            serde_json::to_value(
                                ledger
                                    .staging_part_summary_filtered(dimension, &group, &filter)
                                    .unwrap(),
                            )
                            .unwrap(),
                            expected.clone(),
                        );
                        same(
                            serde_json::to_value(
                                preview
                                    .part_summary_filtered(dimension, &group, &filter)
                                    .unwrap(),
                            )
                            .unwrap(),
                            expected,
                        );
                    }
                }
            }
        }
    }
}

fn custom(shared: bool, product: &str) -> AgentDirectory {
    let mut sources = directory().sources().to_vec();
    let mut products = directory().products().to_vec();
    sources.push(crate::agents::SourceDescriptor {
        id: "custom".into(),
        name: "Custom".into(),
        shared,
    });
    products.push(crate::agents::ProductDescriptor {
        id: product.into(),
        name: "Custom product".into(),
        family: "custom".into(),
        config_id: product.into(),
        locations: vec![],
    });
    AgentDirectory::new(sources, products).unwrap()
}

#[test]
fn original_agent_hashes_match_shared_unknown_and_changed_catalogue_rules_without_guessing_groups()
{
    let old = custom(false, "old-product");
    let temp = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(temp.path()).unwrap();
    let mut data = vec![];
    for (i, (family, product)) in [
        ("claude", None),
        ("codex", None),
        ("codex", Some("codex-cli")),
        ("codex", Some("codex-desktop")),
        ("custom", None),
        ("custom", Some("old-product")),
        ("custom", Some("new-product")),
        ("unlisted", None),
        ("unlisted", Some("")),
        ("unknown", Some("private-product/secret")),
        ("codex", Some("codex-unknown")),
        ("unlisted", Some("codex-unknown")),
        ("private-family/secret", Some("private-product/secret")),
    ]
    .into_iter()
    .enumerate()
    {
        let item = fixture(
            &format!("agent-{i}"),
            family,
            product,
            "s",
            "p",
            &["2026-10-27T00:00:00Z"],
        );
        store.save_batch(&item.1).unwrap();
        data.push(item);
    }
    let ledger = store.retention_ledger(STORE).unwrap();
    ledger
        .stage_queryable_batch(&data.iter().map(|(r, _)| r).collect::<Vec<_>>(), &old, &[])
        .unwrap();
    let encoded = ledger
        .db
        .query_row(
            "SELECT group_concat(value,'|') FROM retention_fact_staging",
            [],
            |r| r.get::<_, String>(0),
        )
        .unwrap();
    assert!(
        !encoded.contains("private-family/secret") && !encoded.contains("private-product/secret")
    );
    let preview = pack(&ledger);
    for current in [
        old,
        custom(false, "new-product"),
        custom(true, "new-product"),
    ] {
        let selectors = current
            .sources()
            .iter()
            .map(|s| s.id.as_str())
            .chain(current.products().iter().map(|p| p.id.as_str()))
            .chain(["unknown", "codex-unknown"]);
        for selector in selectors {
            let filter = GatewayFilter {
                agent: Some(AgentFilter::new(&current, selector).unwrap()),
                ..Default::default()
            };
            let expected = Summary::of(
                data.iter()
                    .filter(|(r, _)| {
                        current.matches(Some(selector), &r.agent_id, r.agent_product_id.as_deref())
                    })
                    .map(|(r, _)| r),
            );
            let expected = serde_json::to_value(expected).unwrap();
            same(
                serde_json::to_value(ledger.staging_summary_filtered(&filter).unwrap()).unwrap(),
                expected.clone(),
            );
            same(
                serde_json::to_value(preview.gateway_summary_filtered(&filter).unwrap()).unwrap(),
                expected,
            );
            let rows = data
                .iter()
                .flat_map(|(_, rows)| rows)
                .filter(|r| {
                    current.matches(Some(selector), &r.agent_id, r.agent_product_id.as_deref())
                })
                .cloned()
                .collect::<Vec<_>>();
            same(
                ledger
                    .staging_provider_contribution_filtered(None, &filter)
                    .unwrap(),
                provider_total(&rows, None),
            );
            same(
                preview.model_contribution_filtered(None, &filter).unwrap(),
                model_total(&rows, None),
            );
        }
    }
}

#[test]
fn legacy_identity_coverage_is_explicit_and_queryable_upgrade_is_idempotent_without_downgrade() {
    let temp = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(temp.path()).unwrap();
    let (request, rows) = retry();
    store.save_batch(&rows).unwrap();
    let before: String = store.0.query_row("SELECT group_concat(value,char(0)) FROM (SELECT value FROM main.model_observations ORDER BY id)", [], |r| r.get(0)).unwrap();
    let ledger = store.retention_ledger(STORE).unwrap();
    ledger
        .stage_attributed_batch(&[&request], directory(), &[])
        .unwrap();
    let filter = GatewayFilter {
        agent: Some(AgentFilter::new(directory(), "claude").unwrap()),
        ..Default::default()
    };
    assert_eq!(
        ledger.staging_summary_filtered(&filter).err(),
        Some(LedgerError::IncompleteFacts)
    );
    assert_eq!(
        pack(&ledger).gateway_summary_filtered(&filter).err(),
        Some(LedgerError::IncompleteFacts)
    );
    assert_eq!(
        ledger
            .staging_summary_filtered(&GatewayFilter::default())
            .unwrap()
            .requests,
        1
    );
    assert_eq!(
        ledger
            .stage_queryable_batch(&[&request], directory(), &[])
            .unwrap()
            .updated,
        1
    );
    assert_eq!(
        ledger
            .stage_queryable_batch(&[&request], directory(), &[])
            .unwrap()
            .unchanged,
        1
    );
    assert_eq!(
        ledger.stage_attributed_batch(&[&request], directory(), &[]),
        Err(LedgerError::ProjectionDowngrade)
    );
    assert_eq!(
        ledger.staging_summary_filtered(&filter).unwrap().requests,
        1
    );
    drop(ledger);
    drop(store);
    let store = ObservationStore::open(temp.path()).unwrap();
    let ledger = store.retention_ledger(STORE).unwrap();
    assert_eq!(
        ledger.staging_summary_filtered(&filter).unwrap().attempts,
        4
    );
    assert_eq!(
        pack(&ledger)
            .gateway_summary_filtered(&filter)
            .unwrap()
            .attempts,
        4
    );
    let after: String = store.0.query_row("SELECT group_concat(value,char(0)) FROM (SELECT value FROM main.model_observations ORDER BY id)", [], |r| r.get(0)).unwrap();
    assert_eq!(after, before);
}

#[test]
fn unknown_time_is_preserved_without_bounds_and_not_counted_inside_a_dated_window() {
    let temp = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(temp.path()).unwrap();
    let (request, rows) = fixture("unknown-time", "claude", None, "s", "p", &["unknown-time"]);
    store.save_batch(&rows).unwrap();
    let ledger = store.retention_ledger(STORE).unwrap();
    ledger
        .stage_queryable_batch(&[&request], directory(), &[])
        .unwrap();
    assert_eq!(
        ledger
            .staging_summary_filtered(&GatewayFilter::default())
            .unwrap()
            .requests,
        1
    );
    let filter = GatewayFilter {
        start: Some(at("2026-01-01T00:00:00Z")),
        ..Default::default()
    };
    assert_eq!(
        ledger.staging_summary_filtered(&filter).unwrap().requests,
        0
    );
    same(
        ledger
            .staging_provider_contribution_filtered(None, &filter)
            .unwrap(),
        provider_total(&[], None),
    );
    same(
        pack(&ledger)
            .model_contribution_filtered(None, &filter)
            .unwrap(),
        model_total(&[], None),
    );
}

#[test]
fn invalid_ranges_scope_inputs_and_oversized_view_keys_never_widen_a_query() {
    let temp = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(temp.path()).unwrap();
    let (request, rows) = retry();
    store.save_batch(&rows).unwrap();
    let ledger = store.retention_ledger(STORE).unwrap();
    ledger
        .stage_queryable_batch(&[&request], directory(), &[])
        .unwrap();
    let before = ledger.db.total_changes();
    for filter in [
        GatewayFilter {
            start: Some(at("2026-01-02T00:00:00Z")),
            end: Some(at("2026-01-01T00:00:00Z")),
            ..Default::default()
        },
        GatewayFilter {
            session: Some(String::new()),
            ..Default::default()
        },
        GatewayFilter {
            project: Some("bad\0scope".into()),
            ..Default::default()
        },
        GatewayFilter {
            project: Some("x".repeat(1025)),
            ..Default::default()
        },
        GatewayFilter {
            start: Some(DateTime::<Utc>::MAX_UTC),
            ..Default::default()
        },
    ] {
        assert_eq!(
            ledger.staging_summary_filtered(&filter).err(),
            Some(LedgerError::InvalidQuery)
        );
        assert_eq!(
            ledger
                .staging_provider_contribution_filtered(None, &filter)
                .err(),
            Some(LedgerError::InvalidQuery)
        );
    }
    assert_eq!(
        AgentFilter::new(directory(), "invalid-agent")
            .err()
            .unwrap(),
        LedgerError::InvalidQuery
    );
    let large = "x".repeat(1025);
    let filter = GatewayFilter::default();
    assert_eq!(
        ledger
            .staging_part_summary_filtered(PartDimension::Model, &large, &filter)
            .err(),
        Some(LedgerError::InvalidQuery)
    );
    assert_eq!(
        ledger
            .staging_model_contribution_filtered(Some(&large), &filter)
            .err(),
        Some(LedgerError::InvalidQuery)
    );
    assert_eq!(ledger.db.total_changes(), before);
}

#[test]
fn corrupt_or_future_query_identity_is_rejected_before_partial_statistics() {
    for change in 0..5 {
        let temp = tempfile::tempdir().unwrap();
        let store = ObservationStore::open(temp.path()).unwrap();
        let (request, rows) = retry();
        store.save_batch(&rows).unwrap();
        let ledger = store.retention_ledger(STORE).unwrap();
        ledger
            .stage_queryable_batch(&[&request], directory(), &[])
            .unwrap();
        let text = ledger
            .db
            .query_row("SELECT value FROM retention_fact_staging", [], |r| {
                r.get::<_, String>(0)
            })
            .unwrap();
        let mut value: Value = serde_json::from_str(&text).unwrap();
        match change {
            0 => {
                value["agentIdentity"]["family"] = serde_json::json!("invalid");
            }
            1 => {
                value.as_object_mut().unwrap().remove("agentIdentity");
            }
            2 => {
                value["attribution"]["attempts"][0]
                    .as_object_mut()
                    .unwrap()
                    .remove("agentIdentity");
            }
            3 => {
                value["version"] = serde_json::json!(2);
            }
            _ => {
                value["version"] = serde_json::json!(4);
            }
        }
        let text = value.to_string();
        ledger
            .db
            .execute(
                "UPDATE retention_fact_staging SET value=?1,projection_digest=?2",
                params![text, digest(&text)],
            )
            .unwrap();
        assert_eq!(
            ledger
                .staging_summary_filtered(&GatewayFilter::default())
                .err(),
            Some(LedgerError::InvalidRecord)
        );
        let keys = ledger
            .db
            .query_row("SELECT key FROM retention_fact_staging", [], |r| {
                r.get::<_, String>(0)
            })
            .unwrap();
        assert_eq!(
            ledger
                .pack_staging_batch(PackedKind::Gateway, &[&keys])
                .err(),
            Some(LedgerError::InvalidRecord)
        );
    }
}

#[test]
fn queryable_source_headers_cannot_be_replaced_by_caller_agent_or_date_values() {
    for field in 0..3 {
        let temp = tempfile::tempdir().unwrap();
        let store = ObservationStore::open(temp.path()).unwrap();
        let (mut request, rows) = retry();
        store.save_batch(&rows).unwrap();
        match field {
            0 => request.started_at = "1900-01-01T00:00:00Z".into(),
            1 => request.agent_id = "codex".into(),
            _ => request.agent_product_id = Some("codex-cli".into()),
        }
        let ledger = store.retention_ledger(STORE).unwrap();
        assert_eq!(
            ledger.stage_queryable_batch(&[&request], directory(), &[]),
            Err(LedgerError::SourceChanged)
        );
        assert_eq!(ledger.staging_summary().unwrap().requests, 0);
    }
}

#[test]
fn compiled_agent_predicates_equal_the_original_directory_matcher_for_all_input_shapes() {
    let db = Connection::open_in_memory().unwrap();
    for directory in [
        custom(false, "old-product"),
        custom(false, "new-product"),
        custom(true, "new-product"),
    ] {
        let families = directory
            .sources()
            .iter()
            .map(|s| s.id.as_str())
            .chain(directory.products().iter().map(|p| p.id.as_str()))
            .chain([
                "unknown",
                "codex-unknown",
                "unlisted",
                "private-family/secret",
            ])
            .collect::<Vec<_>>();
        let products = std::iter::once(None)
            .chain(directory.products().iter().map(|p| Some(p.id.as_str())))
            .chain([
                Some(""),
                Some("unknown"),
                Some("codex-unknown"),
                Some("private-product/secret"),
            ])
            .collect::<Vec<_>>();
        let selectors = directory
            .sources()
            .iter()
            .map(|s| s.id.as_str())
            .chain(directory.products().iter().map(|p| p.id.as_str()))
            .chain(["unknown", "codex-unknown"]);
        for selector in selectors {
            let predicate = AgentFilter::new(&directory, selector)
                .unwrap()
                .sql(STORE, "f.value");
            let mut query = db
                .prepare(&format!(
                    "SELECT coalesce({predicate},0) FROM (SELECT ?1 value) f"
                ))
                .unwrap();
            for family in &families {
                for product in &products {
                    let identity =
                        crate::observation::retention::agent_identity::AgentIdentity::new(
                            STORE, family, *product,
                        );
                    let value = serde_json::json!({"agentIdentity":identity}).to_string();
                    let actual: bool = query.query_row([value], |r| r.get(0)).unwrap();
                    assert_eq!(
                        actual,
                        directory.matches(Some(selector), family, *product),
                        "selector={selector}, family={family}, product={product:?}"
                    );
                }
            }
        }
    }
}

#[test]
fn additive_agent_identity_does_not_change_v1_or_v2_serialization() {
    let (request, rows) = retry();
    let old = GatewayFact::from_request(&request, STORE, directory()).unwrap();
    let attributed =
        GatewayFact::from_request_with_evidence(&request, &rows, STORE, directory(), &[]).unwrap();
    let queryable =
        GatewayFact::from_request_queryable(&request, &rows, STORE, directory(), &[]).unwrap();
    for (fact, version) in [(&old, 1), (&attributed, 2)] {
        let value = serde_json::to_value(fact).unwrap();
        assert_eq!(value["version"], version);
        assert!(value.get("agentIdentity").is_none());
        if let Some(attempts) = value
            .pointer("/attribution/attempts")
            .and_then(Value::as_array)
        {
            assert!(attempts.iter().all(|a| a.get("agentIdentity").is_none()));
        }
    }
    let value = serde_json::to_value(&queryable).unwrap();
    assert_eq!(value["version"], 3);
    assert_eq!(value["agentIdentity"]["family"].as_str().unwrap().len(), 64);
    assert!(
        value["attribution"]["attempts"]
            .as_array()
            .unwrap()
            .iter()
            .all(|a| a["agentIdentity"].is_object())
    );
    assert_eq!(
        serde_json::to_value(queryable.statistics()).unwrap(),
        serde_json::to_value(attributed.statistics()).unwrap()
    );
}

#[test]
fn an_oversized_catalogue_is_rejected_instead_of_returning_a_partial_agent_selection() {
    let mut sources = vec![];
    let mut products = vec![];
    for i in 0..1025 {
        let family = format!("family-{i}");
        let product = format!("product-{i}");
        sources.push(crate::agents::SourceDescriptor {
            id: family.clone(),
            name: family.clone(),
            shared: false,
        });
        products.push(crate::agents::ProductDescriptor {
            id: product.clone(),
            name: product.clone(),
            family,
            config_id: product,
            locations: vec![],
        });
    }
    let directory = AgentDirectory::new(sources, products).unwrap();
    assert_eq!(
        AgentFilter::new(&directory, "unknown").err().unwrap(),
        LedgerError::BatchLimit
    );
}
