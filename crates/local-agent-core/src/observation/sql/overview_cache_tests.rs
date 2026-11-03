use super::*;
use crate::observation::{AnalysisCost, AnalysisRow, OverviewQuery};
use chrono::Duration;

fn query() -> OverviewQuery {
    OverviewQuery {
        agent: Some("claude".into()),
        range: 24,
        metric: "tokens".into(),
        dimension: "agent".into(),
        source: "native".into(),
    }
}
fn at() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-10-24T08:00:00Z")
        .unwrap()
        .with_timezone(&Utc)
}
fn native(time: Option<DateTime<Utc>>, i: usize) -> AnalysisRow {
    AnalysisRow {
        cost: AnalysisCost {
            actual_usd: i.is_multiple_of(3).then_some(0.0),
            estimated_usd: (i % 3 == 1).then_some(0.000001),
            unknown_attempts: usize::from(i % 3 == 2),
        },
        agent: if i.is_multiple_of(2) {
            "claude"
        } else {
            "codex"
        }
        .into(),
        model: (i % 3 != 2).then(|| format!("model-{}", i % 2)),
        time: time.map(|t| t.to_rfc3339()),
        tokens: (i % 3 != 2).then_some(12),
        provisional: i % 2 == 1,
        partial: i % 3 == 2,
    }
}

#[test]
fn compact_native_overview_matches_raw_queries_for_unknowns_large_integers_and_money_order() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = History::open(dir.path()).unwrap();
    h.refresh(&[]).unwrap();
    let original: Vec<_> = (0..250)
        .map(|i| {
            let mut row = native(Some(at() - Duration::minutes((i * 23 + 1) as i64)), i);
            row.tokens = (!i.is_multiple_of(5)).then_some((i * 31) as u64);
            row.cost.actual_usd = Some(if i == 0 { 1e16 } else { 1.0 });
            row
        })
        .collect();
    for phase in ["safe", "large", "unknown", "empty"] {
        let mut rows = original.clone();
        match phase {
            "large" => {
                rows[1].tokens = Some(9_007_199_254_740_993);
            }
            "unknown" => {
                for row in &mut rows {
                    row.tokens = None;
                }
            }
            "empty" => rows.clear(),
            _ => {}
        }
        h.set_native_analysis(1, &rows).unwrap();
        h.db.execute_batch("DELETE FROM analyses WHERE kind='gateway';INSERT INTO analyses SELECT id||'-reference','gateway',agent,model,time,tokens,partial,provisional,cost,attempts FROM analyses WHERE kind='native';").unwrap();
        for range in [6, 12, 24, 168] {
            for metric in ["calls", "tokens", "cost"] {
                for dimension in ["agent", "model"] {
                    for agent in [None, Some("claude".into())] {
                        let q = OverviewQuery {
                            range,
                            metric: metric.into(),
                            dimension: dimension.into(),
                            agent,
                            source: "native".into(),
                        };
                        let actual = h.analysis(&q, at()).unwrap();
                        let raw = OverviewQuery {
                            source: "gateway".into(),
                            ..q.clone()
                        };
                        let mut expected = h.analysis(&raw, at()).unwrap();
                        expected["query"] = json!(q);
                        assert_eq!(actual, expected, "{phase}/{range}/{metric}/{dimension}");
                    }
                }
            }
        }
    }
}

#[test]
fn unchanged_gateway_timing_reuses_percentiles_and_gateway_updates_invalidate() {
    let dir = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(dir.path()).unwrap();
    let rows: Vec<_> = (0..5005)
        .map(|n| {
            let mut r = row(n);
            r.agent_id = if n.is_multiple_of(2) {
                "claude"
            } else {
                "codex"
            }
            .into();
            r.agent_product_id = None;
            r.started_at = (at() - Duration::seconds(60)).to_rfc3339();
            r.status = if n % 13 == 1 { "error" } else { "success" }.into();
            r.ttft_ms = (!n.is_multiple_of(5)).then_some(100.0 + (n % 3) as f64);
            r.duration_ms = Some(if n.is_multiple_of(7) { 0.0 } else { 1000.0 });
            r
        })
        .collect();
    store.save_batch(&rows).unwrap();
    let mut h = History::open(dir.path()).unwrap();
    h.refresh(&[]).unwrap();
    let q = query();
    let initial = h.timing_stats(&q, at()).unwrap();
    assert!(initial["success"].as_u64().unwrap() > 1000);
    h.db.create_scalar_function(
        "agent_matches",
        3,
        rusqlite::functions::FunctionFlags::SQLITE_UTF8,
        |_| -> rusqlite::Result<bool> { Err(rusqlite::Error::InvalidQuery) },
    )
    .unwrap();
    let mut same = q.clone();
    same.source = "gateway".into();
    same.metric = "cost".into();
    same.dimension = "model".into();
    assert_eq!(
        h.timing_stats(&same, at() + Duration::seconds(1)).unwrap(),
        initial
    );
    assert_eq!(
        h.timing_stats(&same, at() + Duration::hours(24)).unwrap()["success"],
        0
    );
    aggregates::register_directory(&h.db, h.directory.clone()).unwrap();
    let mut changed = rows[0].clone();
    changed.status = "error".into();
    changed.revision += 1;
    store.save(&changed).unwrap();
    h.refresh(&[]).unwrap();
    assert_eq!(
        h.timing_stats(&q, at() + Duration::seconds(1)).unwrap()["success"].as_u64(),
        Some(initial["success"].as_u64().unwrap() - 1)
    );
    for range in [6, 12, 24, 168] {
        for agent in [None, Some("claude"), Some("codex")] {
            let mut q = q.clone();
            q.range = range;
            q.agent = agent.map(str::to_owned);
            h.timing_stats(&q, at() + Duration::seconds(2)).unwrap();
            assert!(h.timing_cache.borrow().len() <= 8);
        }
    }
    h.release_native_snapshots();
    assert!(h.timing_cache.borrow().is_empty());
}

#[test]
fn cached_timing_matches_fresh_percentiles_at_future_departure_and_clock_edges() {
    let dir = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(dir.path()).unwrap();
    let mut rows = vec![];
    for (n, hours) in [6, 12, 24, 168].into_iter().enumerate() {
        for millis in [-1, 0, 1] {
            let mut r = row(rows.len());
            r.agent_id = if n.is_multiple_of(2) {
                "claude"
            } else {
                "codex"
            }
            .into();
            r.agent_product_id = None;
            r.started_at =
                (at() - Duration::hours(hours) + Duration::milliseconds(millis)).to_rfc3339();
            r.ttft_ms = Some((rows.len() + 1) as f64);
            r.duration_ms = Some(1000.0);
            rows.push(r);
        }
    }
    let mut future = row(rows.len());
    future.started_at = (at() + Duration::milliseconds(1)).to_rfc3339();
    rows.push(future);
    store.save_batch(&rows).unwrap();
    let mut h = History::open(dir.path()).unwrap();
    h.refresh(&[]).unwrap();
    for range in [6, 12, 24, 168] {
        for agent in [None, Some("claude"), Some("codex")] {
            h.timing_cache.borrow_mut().clear();
            let mut q = query();
            q.range = range;
            q.agent = agent.map(str::to_owned);
            for millis in [-2, -1, 0, 1, 2, 1000, -1] {
                let now = at() + Duration::milliseconds(millis);
                let actual = h.timing_stats(&q, now).unwrap();
                h.timing_cache.borrow_mut().clear();
                let expected = h.timing_stats(&q, now).unwrap();
                assert_eq!(
                    actual, expected,
                    "range={range} agent={agent:?} offset={millis}"
                );
            }
        }
    }
}

#[test]
fn unchanged_overview_reuses_aggregates_and_mutations_invalidate_bounded_cache() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = History::open(dir.path()).unwrap();
    h.refresh(&[]).unwrap();
    let mut r = native(Some(at() - Duration::seconds(65)), 0);
    h.set_native_analysis(1, &[r.clone()]).unwrap();
    let q = query();
    assert_eq!(h.analysis(&q, at()).unwrap()["total"], 12.0);
    h.db.create_scalar_function(
        "agent_group_matches",
        2,
        rusqlite::functions::FunctionFlags::SQLITE_UTF8,
        |_| -> rusqlite::Result<bool> { Err(rusqlite::Error::InvalidQuery) },
    )
    .unwrap();
    let next = at() + Duration::seconds(1);
    let hit = h.analysis(&q, next).unwrap();
    assert_eq!(hit["total"], 12.0);
    assert_eq!(hit["asOf"], next.to_rfc3339());
    assert!(
        h.analysis(&q, at() + Duration::hours(1)).is_err(),
        "bucket changes must rebuild even when source data is unchanged"
    );
    aggregates::register_directory(&h.db, h.directory.clone()).unwrap();
    r.tokens = Some(24);
    h.set_native_analysis(1, &[r]).unwrap();
    assert_eq!(h.analysis(&q, next).unwrap()["total"], 24.0);
    let mut gateway = q.clone();
    gateway.source = "gateway".into();
    assert_eq!(h.analysis(&gateway, at()).unwrap()["samples"], 0);
    let store = ObservationStore::open(dir.path()).unwrap();
    let mut raw = row(0);
    raw.started_at = (at() - Duration::seconds(60)).to_rfc3339();
    raw.ended_at = Some(raw.started_at.clone());
    raw.agent_id = "claude".into();
    raw.agent_product_id = None;
    raw.usage.total_tokens = Some(19);
    store.save(&raw).unwrap();
    h.refresh(&[]).unwrap();
    assert_eq!(h.analysis(&gateway, at()).unwrap()["total"], 19.0);
    raw.usage.total_tokens = Some(20);
    store.save(&raw).unwrap();
    h.refresh(&[]).unwrap();
    assert_eq!(h.analysis(&gateway, at()).unwrap()["total"], 20.0);
    for range in [6, 12, 24, 168] {
        for metric in ["calls", "tokens", "cost"] {
            for dimension in ["agent", "model"] {
                let mut scope = q.clone();
                scope.range = range;
                scope.metric = metric.into();
                scope.dimension = dimension.into();
                h.analysis(&scope, at()).unwrap();
                assert!(h.analysis_cache.borrow().len() <= 8);
            }
        }
    }
    h.release_native_snapshots();
    assert!(h.analysis_cache.borrow().is_empty());
}

#[test]
fn overview_cached_windows_match_rebuilt_results_at_bucket_future_and_departure_edges() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = History::open(dir.path()).unwrap();
    h.refresh(&[]).unwrap();
    for range in [6, 12, 24, 168] {
        let step = if range == 6 || range == 12 {
            1800
        } else if range == 24 {
            3600
        } else {
            21600
        };
        for edges in [false, true] {
            let times = if edges {
                vec![
                    None,
                    Some(at() - Duration::hours(range.into()) - Duration::milliseconds(1)),
                    Some(at() - Duration::hours(range.into())),
                    Some(at() - Duration::hours(range.into()) + Duration::milliseconds(1)),
                    Some(at() - Duration::seconds(step) - Duration::milliseconds(1)),
                    Some(at() - Duration::seconds(step)),
                    Some(at() - Duration::seconds(step) + Duration::milliseconds(1)),
                    Some(at() - Duration::milliseconds(1)),
                    Some(at()),
                    Some(at() + Duration::milliseconds(1)),
                ]
            } else {
                vec![
                    None,
                    Some(at() - Duration::seconds(65)),
                    Some(at() - Duration::hours(i64::from(range) - 1) - Duration::seconds(123)),
                ]
            };
            let rows: Vec<_> = times
                .into_iter()
                .enumerate()
                .map(|(i, t)| native(t, i))
                .collect();
            h.set_native_analysis(1, &rows).unwrap();
            for metric in ["calls", "tokens", "cost"] {
                for dimension in ["agent", "model"] {
                    for agent in [None, Some("claude".into())] {
                        let q = OverviewQuery {
                            range,
                            metric: metric.into(),
                            dimension: dimension.into(),
                            agent,
                            source: "native".into(),
                        };
                        for delta in [
                            Duration::nanoseconds(-1),
                            Duration::nanoseconds(1),
                            Duration::milliseconds(1),
                            Duration::seconds(2),
                            Duration::seconds(step - 65),
                            Duration::seconds(step - 65) + Duration::milliseconds(1),
                            Duration::hours(range.into()) + Duration::milliseconds(1),
                        ] {
                            h.analysis_cache.borrow_mut().clear();
                            h.analysis(&q, at()).unwrap();
                            let actual = h.analysis(&q, at() + delta).unwrap();
                            h.analysis_cache.borrow_mut().clear();
                            let expected = h.analysis(&q, at() + delta).unwrap();
                            assert_eq!(
                                actual, expected,
                                "range={range}, edges={edges}, metric={metric}, dimension={dimension}, delta={delta}"
                            );
                        }
                    }
                }
            }
        }
    }
}
