use super::*;
use crate::sessions::CallCatalog;
use std::sync::Arc;

fn catalog(files: usize, calls: usize) -> crate::projects::ProjectCatalog {
    let mut c = crate::associated_cost_tests::catalog();
    let prototype = c.calls[0].clone();
    c.calls = CallCatalog::shared((0..files).map(|file| {
        Arc::new(
            (0..calls)
                .map(|n| {
                    let mut call = prototype.clone();
                    call.id = format!("cells-{file}-{n:05}");
                    call.response_id = Some(format!("response-{file}-{n}"));
                    call.session_id = format!("session-{file}");
                    call.model = Some(format!("model-{}", n % 7));
                    call.timestamp = Some((now() - chrono::Duration::hours(1)).to_rfc3339());
                    call.usage = Some(Usage {
                        input_tokens: Some(10),
                        output_tokens: Some(2),
                        total_tokens: Some(12),
                        ..Default::default()
                    });
                    call
                })
                .collect(),
        )
    }));
    c.revision += 1;
    c
}
#[test]
fn native_request_cells_reuse_untouched_blocks_recover_saturation_and_keep_public_freezes() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = History::open(dir.path()).unwrap();
    h.refresh(&[]).unwrap();
    let mut c = catalog(3, 4096);
    Arc::make_mut(&mut c.calls.chunks[2])[0]
        .usage
        .as_mut()
        .unwrap()
        .total_tokens = Some(u64::MAX);
    let q = requests::Query {
        source: Some("native".into()),
        ..Default::default()
    };
    h.sync_native_catalog("first".into(), &HashMap::new(), &c.calls)
        .unwrap();
    let report = h.source_report(&q, now(), vec![], None).unwrap();
    let first = h.request_page(&q, now(), report, None).unwrap();
    assert_eq!(first["summary"]["usage"]["totalTokens"], u64::MAX);
    h.db.execute_batch("CREATE TEMP TRIGGER untouched_request_block BEFORE DELETE ON native_request_cells WHEN old.bucket=0 BEGIN SELECT RAISE(ABORT,'unchanged request block rebuilt');END;").unwrap();
    Arc::make_mut(&mut c.calls.chunks[2])[0]
        .usage
        .as_mut()
        .unwrap()
        .total_tokens = Some(0);
    for (phase, total, tokens) in [("rewrite", 12288, 12287 * 12), ("delete", 8192, 8192 * 12)] {
        if phase == "delete" {
            c.calls.chunks.remove(2);
        }
        h.sync_native_catalog(phase.into(), &HashMap::new(), &c.calls)
            .unwrap();
        let report = h.source_report(&q, now(), vec![], None).unwrap();
        let page = h.request_page(&q, now(), report, None).unwrap();
        assert_eq!(page["total"], total);
        assert_eq!(page["summary"]["usage"]["totalTokens"], tokens);
        let old = requests::Query {
            snapshot: first["snapshot"].as_str().map(str::to_owned),
            ..q.clone()
        };
        assert_eq!(
            h.request_page(&old, now(), Value::Null, None).unwrap(),
            first
        );
    }
}
#[test]
fn compact_request_bins_match_canonical_at_nanosecond_edges_before_unix_epoch() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = History::open(dir.path()).unwrap();
    h.refresh(&[]).unwrap();
    let c = catalog(1, 4);
    let mut native = requests::sources::native_records(c.calls.clone(), &HashMap::new());
    for (n, time) in [
        "1969-12-31T23:59:59.999999999Z",
        "1970-01-01T00:00:00.000000499Z",
        "1970-01-01T00:00:00.000000500Z",
        "",
    ]
    .into_iter()
    .enumerate()
    {
        native[n].started_at = time.into();
    }
    native[1].usage = Usage::default();
    native[1].usage_missing = 1;
    h.sync_native("edge".into(), &HashMap::new(), native.clone())
        .unwrap();
    for metric in ["calls", "tokens", "cost"] {
        for dimension in ["agent", "model", "provider"] {
            let q = requests::Query {
                source: Some("native".into()),
                start: Some("1969-12-31T23:00:00.000000500Z".into()),
                end: Some("1970-01-01T02:00:00.000000500Z".into()),
                metric: Some(metric.into()),
                dimension: Some(dimension.into()),
                ..Default::default()
            };
            let got = h.request_page(&q, now(), Value::Null, None).unwrap();
            let snapshot = requests::Snapshot::new(
                q.clone(),
                native.clone(),
                now(),
                got["snapshot"].as_str().unwrap().into(),
                None,
            )
            .unwrap();
            let mut expected = value(&snapshot.page(&q).unwrap()).unwrap();
            expected["sourceReport"] = Value::Null;
            expected["coverage"] = got["coverage"].clone();
            assert_close(&got, &expected, "fractional compact request bins");
        }
    }
}
#[test]
fn native_cells_fall_back_for_confirmed_zero_money_and_observed_timing() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = History::open(dir.path()).unwrap();
    h.refresh(&[]).unwrap();
    let c = catalog(1, 3);
    let mut native = requests::sources::native_records(c.calls.clone(), &HashMap::new());
    native[0].usage.cached_input_tokens = Some(1);
    let q = requests::Query {
        source: Some("native".into()),
        ..Default::default()
    };
    // A native price field is not confirmed cost evidence. Only an explicit
    // gateway attempt can make this summary money-bearing.
    native[0].cost = serde_json::from_value(
        json!({"actual":[{"currency":"USD","amount":0,"calls":1}],"estimated":[],"unknownCalls":0}),
    )
    .unwrap();
    h.sync_native("unconfirmed".into(), &HashMap::new(), native.clone())
        .unwrap();
    assert!(h.native_request_cells(&q, now()).unwrap().is_some());
    for timing in [false, true] {
        if timing {
            native[0].attempts.clear();
            native[0].status = "success".into();
            native[0].ttft_ms = Some(0.0);
            native[0].duration_ms = Some(1000.0);
        } else {
            let mut gateway = row(1);
            gateway.endpoint = "https://openrouter.ai/api/v1".into();
            gateway.reported_cost = Some(
                serde_json::from_value(
                    json!({"amount":0,"currency":"USD","source":"openrouter.usage.cost"}),
                )
                .unwrap(),
            );
            native[0].attempts = requests::logical_requests(vec![gateway], &HashMap::new())
                .remove(0)
                .attempts;
        }
        h.sync_native(timing.to_string(), &HashMap::new(), native.clone())
            .unwrap();
        assert!(h.native_request_cells(&q, now()).unwrap().is_none());
        let report = h.source_report(&q, now(), vec![], None).unwrap();
        assert_close(
            &report["native"],
            &value(&requests::Summary::of(native.iter())).unwrap(),
            "nonassociative request fallback",
        );
    }
}
#[test]
fn canonical_native_versions_survive_pruning_until_source_release() {
    let dir = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(dir.path()).unwrap();
    let mut h = History::open(dir.path()).unwrap();
    h.refresh(&[]).unwrap();
    let c = catalog(1, 1);
    let id = c.calls[0].id.clone();
    h.sync_native_catalog("first".into(), &HashMap::new(), &c.calls)
        .unwrap();
    let original = h.request_detail(&id).unwrap().unwrap();
    let version: i64 =
        h.db.query_row("SELECT v FROM native_base WHERE id=?1", [&id], |r| r.get(0))
            .unwrap();
    let mut gateway = row(1);
    gateway.response_id = c.calls[0].response_id.clone();
    gateway.agent_product_id = c.calls[0].agent_product_id.clone();
    gateway.session_id = None;
    store.save(&gateway).unwrap();
    h.refresh(&[]).unwrap();
    h.refresh_native().unwrap();
    h.prune().unwrap();
    assert_eq!(
        h.request_detail(&id).unwrap().unwrap()["native"]["association"],
        "response_id"
    );
    assert_eq!(
        h.db.query_row("SELECT count(*) FROM records WHERE v=?1", [version], |r| {
            r.get::<_, i64>(0)
        })
        .unwrap(),
        1
    );
    store
        .0
        .execute("DELETE FROM model_observations", [])
        .unwrap();
    h.refresh(&[]).unwrap();
    h.refresh_native().unwrap();
    h.prune().unwrap();
    assert_eq!(h.request_detail(&id).unwrap().unwrap(), original);
    h.sync_native_catalog("revoked".into(), &HashMap::new(), &CallCatalog::default())
        .unwrap();
    h.prune().unwrap();
    assert_eq!(
        h.db.query_row("SELECT count(*) FROM records WHERE v=?1", [version], |r| {
            r.get::<_, i64>(0)
        })
        .unwrap(),
        0
    );
}
#[test]
fn cold_native_usage_has_no_unneeded_change_log_and_rebuilds_then_tracks_deltas() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = History::open(dir.path()).unwrap();
    h.refresh(&[]).unwrap();
    let mut c = catalog(1, 150);
    h.sync_native_provider(&c, &[]).unwrap();
    h.sync_native_models(&c, "claude").unwrap();
    assert_eq!(
        h.db.query_row("SELECT count(*) FROM native_usage_changes", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    let q = provider_usage::Query {
        source: "native".into(),
        provider_id: Some("_unknown".into()),
        ..Default::default()
    };
    assert_eq!(
        h.provider_usage(q.clone(), &[], "asof".into(), "ready".into(), now())
            .unwrap()["totalCalls"],
        150
    );
    Arc::make_mut(&mut c.calls.chunks[0])[0]
        .usage
        .as_mut()
        .unwrap()
        .total_tokens = Some(0);
    c.revision += 1;
    h.sync_native_provider(&c, &[]).unwrap();
    assert!(
        h.db.query_row(
            "SELECT count(*) FROM native_usage_changes WHERE kind='provider'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap()
            > 0
    );
    assert_eq!(
        h.provider_usage(q, &[], "asof".into(), "ready".into(), now())
            .unwrap()["summary"]["usage"]["totalTokens"],
        149 * 12
    );
}

#[test]
fn request_cell_failure_is_atomic_and_same_bucket_upserts_remain_retryable() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = History::open(dir.path()).unwrap();
    h.refresh(&[]).unwrap();
    let mut c = catalog(1, 30);
    let q = requests::Query {
        source: Some("native".into()),
        ..Default::default()
    };
    h.sync_native_catalog("first".into(), &HashMap::new(), &c.calls)
        .unwrap();
    h.native_request_cells(&q, now()).unwrap();
    let previous: String =
        h.db.query_row(
            "SELECT group_concat(stats) FROM native_request_cells",
            [],
            |r| r.get(0),
        )
        .unwrap();
    for n in 0..5 {
        Arc::make_mut(&mut c.calls.chunks[0])[n]
            .usage
            .as_mut()
            .unwrap()
            .total_tokens = Some(0);
    }
    h.sync_native_catalog("changed".into(), &HashMap::new(), &c.calls)
        .unwrap();
    h.db.execute_batch("CREATE TEMP TRIGGER failed_cells BEFORE INSERT ON native_request_cells BEGIN SELECT RAISE(ABORT,'fixture');END;").unwrap();
    assert!(h.native_request_cells(&q, now()).is_err());
    assert_eq!(
        h.db.query_row(
            "SELECT group_concat(stats) FROM native_request_cells",
            [],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        previous
    );
    assert!(
        h.db.query_row("SELECT count(*) FROM native_request_dirty", [], |r| r
            .get::<_, i64>(0))
            .unwrap()
            > 0
    );
    h.db.execute_batch("DROP TRIGGER failed_cells").unwrap();
    let report = h.source_report(&q, now(), vec![], None).unwrap();
    assert_eq!(report["native"]["requests"], 30);
    assert_eq!(report["native"]["usage"]["totalTokens"], 25 * 12);
}
#[test]
fn high_cardinality_cells_fall_back_without_loss_and_recover_when_history_shrinks() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = History::open(dir.path()).unwrap();
    h.refresh(&[]).unwrap();
    let mut c = catalog(1, 70_000);
    for (n, call) in Arc::make_mut(&mut c.calls.chunks[0]).iter_mut().enumerate() {
        call.model = Some(format!("unique-{n}"));
    }
    let q = requests::Query {
        source: Some("native".into()),
        ..Default::default()
    };
    h.sync_native_catalog("large".into(), &HashMap::new(), &c.calls)
        .unwrap();
    assert!(h.native_request_cells(&q, now()).unwrap().is_none());
    assert_eq!(
        h.db.query_row("SELECT count(*) FROM native_request_cells", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    let report = h.source_report(&q, now(), vec![], None).unwrap();
    assert_eq!(report["native"]["requests"], 70_000);
    assert_eq!(report["native"]["usage"]["totalTokens"], 70_000 * 12);
    Arc::make_mut(&mut c.calls.chunks[0]).truncate(50);
    h.sync_native_catalog("small".into(), &HashMap::new(), &c.calls)
        .unwrap();
    assert!(h.native_request_cells(&q, now()).unwrap().is_some());
    for n in 0..12 {
        let filtered = requests::Query {
            search: Some(format!("filter-{n}")),
            ..q.clone()
        };
        h.native_request_cells(&filtered, now()).unwrap();
        assert!(
            h.db.query_row("SELECT count(*) FROM native_request_clock", [], |r| r
                .get::<_, i64>(0))
                .unwrap()
                <= 8
        );
    }
    assert_eq!(h.db.query_row("SELECT count(*) FROM native_request_cells c WHERE NOT EXISTS(SELECT 1 FROM native_request_clock k WHERE k.scope=c.scope)",[],|r|r.get::<_,i64>(0)).unwrap(),0);
}

#[test]
fn deferred_cold_indexes_roll_back_and_preserve_gateway_snapshots() {
    let dir = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(dir.path()).unwrap();
    store.save(&row(1)).unwrap();
    let mut h = History::open(dir.path()).unwrap();
    h.refresh(&[]).unwrap();
    let indexes = |h: &History| -> Vec<(String, String)> {
        let mut s = h.db.prepare("SELECT name,sql FROM sqlite_temp_master WHERE type='index' AND tbl_name IN ('record_meta','native_base') AND sql IS NOT NULL ORDER BY name").unwrap();
        s.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    };
    let before = indexes(&h);
    let q = requests::Query::default();
    let frozen = h.request_page(&q, now(), Value::Null, None).unwrap();
    let c = catalog(1, 5);
    let native = requests::sources::native_records(c.calls.clone(), &HashMap::new());
    let mut duplicate = native.clone();
    duplicate.push(native[0].clone());
    assert!(
        h.sync_native("failed".into(), &HashMap::new(), duplicate)
            .is_err()
    );
    assert_eq!(indexes(&h), before);
    assert!(h.native_revision.is_none());
    h.sync_native("retry".into(), &HashMap::new(), native)
        .unwrap();
    assert_eq!(indexes(&h), before);
    let q = requests::Query {
        snapshot: frozen["snapshot"].as_str().map(str::to_owned),
        ..q
    };
    assert_eq!(
        h.request_page(&q, now(), Value::Null, None).unwrap(),
        frozen
    );
    assert_eq!(
        h.source_report(&requests::Query::default(), now(), vec![], None)
            .unwrap()["native"]["requests"],
        5
    );
}

#[test]
fn narrow_statistics_skip_wide_payloads_but_pages_and_exports_keep_exact_records() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let dir = tempfile::tempdir().unwrap();
    let mut h = History::open(dir.path()).unwrap();
    h.refresh(&[]).unwrap();
    let c = catalog(1, 2000);
    let mut native = requests::sources::native_records(c.calls.clone(), &HashMap::new());
    for row in &mut native {
        row.project_name = Some("large project metadata 你好 ".repeat(400));
    }
    h.sync_native("wide".into(), &HashMap::new(), native.clone())
        .unwrap();
    let reads = Arc::new(AtomicUsize::new(0));
    let counted = reads.clone();
    h.db.create_scalar_function(
        "read_payload",
        1,
        rusqlite::functions::FunctionFlags::SQLITE_UTF8,
        move |c| {
            counted.fetch_add(1, Ordering::Relaxed);
            c.get::<String>(0)
        },
    )
    .unwrap();
    h.db.execute_batch("DROP VIEW records;ALTER TABLE record_payloads RENAME TO record_bytes;
      CREATE TEMP VIEW record_payloads AS SELECT v,read_payload(value) value FROM record_bytes;
      CREATE TEMP TRIGGER delete_test_payload INSTEAD OF DELETE ON record_payloads BEGIN DELETE FROM record_bytes WHERE v=old.v;END;
      CREATE TEMP VIEW records AS SELECT m.*,(SELECT p.value FROM record_payloads p WHERE p.v=m.v) value FROM record_meta m;").unwrap();
    let q = requests::Query {
        source: Some("native".into()),
        model: Some("model-0".into()),
        ..Default::default()
    };
    let report = h.source_report(&q, now(), vec![], None).unwrap();
    let expected: Vec<_> = native
        .iter()
        .filter(|r| r.model.as_deref() == Some("model-0"))
        .cloned()
        .collect();
    assert_eq!(report["native"]["requests"], expected.len());
    assert_eq!(
        reads.load(Ordering::Relaxed),
        0,
        "statistics and nonmatching filters must not fetch request JSON"
    );
    let page = h.request_page(&q, now(), report, None).unwrap();
    assert_eq!(page["total"], expected.len());
    assert_eq!(
        reads.load(Ordering::Relaxed),
        40,
        "only this page's payloads are read"
    );
    assert!(page["items"][0]["projectName"].as_str().unwrap().len() > 8000);
    reads.store(0, Ordering::Relaxed);
    let csv = h
        .frozen_csv(page["snapshot"].as_str().unwrap(), now())
        .unwrap();
    assert_eq!(reads.load(Ordering::Relaxed), expected.len());
    let canonical =
        requests::Snapshot::new(q.clone(), native, now(), "canonical".into(), None).unwrap();
    assert_eq!(csv["csv"], canonical.csv());
}

#[test]
fn native_file_append_only_mutates_changed_response_rows_and_preserves_progress_caches() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = History::open(dir.path()).unwrap();
    h.refresh(&[]).unwrap();
    let mut c = catalog(2, 8);
    let time = now() + chrono::Duration::seconds(19);
    h.sync_native_provider(&c, &[]).unwrap();
    h.sync_native_models(&c, "claude").unwrap();
    h.sync_native_analysis(&c).unwrap();
    h.sync_native_catalog("first".into(), &HashMap::new(), &c.calls)
        .unwrap();
    let q = requests::Query {
        source: Some("native".into()),
        end: Some(time.to_rfc3339()),
        ..Default::default()
    };
    let frozen = h.request_page(&q, time, Value::Null, None).unwrap();
    let token = frozen["snapshot"].as_str().unwrap();
    let csv = h.frozen_csv(token, time).unwrap();
    let changed = c.calls.chunks[0][1].id.clone();
    let mut appended = c.calls.chunks[0][2].clone();
    appended.id = "new-appended-row".into();
    appended.response_id = Some("new-appended-response".into());
    let allow = format!(
        "({},{})",
        aggregates::quote(&changed),
        aggregates::quote(&appended.id)
    );
    for table in ["native_provider_usage", "native_model_usage", "fee_native"] {
        for operation in ["DELETE", "UPDATE"] {
            h.db.execute_batch(&format!("CREATE TEMP TRIGGER guard_{table}_{operation} BEFORE {operation} ON {table} WHEN old.id NOT IN {allow} BEGIN SELECT RAISE(ABORT,'unchanged native row rewritten');END;")).unwrap();
        }
    }
    h.db.execute_batch(&format!("CREATE TEMP TRIGGER guard_current BEFORE DELETE ON current WHEN old.kind='native' AND old.id NOT IN {allow} BEGIN SELECT RAISE(ABORT,'unchanged request retired');END;")).unwrap();
    let batch = Arc::make_mut(&mut c.calls.chunks[0]);
    batch[1].usage = Some(Usage {
        input_tokens: Some(20),
        output_tokens: Some(4),
        total_tokens: Some(24),
        ..Default::default()
    });
    batch.push(appended);
    c.revision += 1;
    assert_eq!(h.sync_native_provider(&c, &[]).unwrap(), 2);
    assert_eq!(h.sync_native_models(&c, "claude").unwrap(), 2);
    assert_eq!(h.sync_native_analysis(&c).unwrap(), 2);
    h.sync_native_catalog("changed".into(), &HashMap::new(), &c.calls)
        .unwrap();
    let report = h.source_report(&q, time, vec![], None).unwrap();
    assert_eq!(report["native"]["requests"], 17);
    assert_eq!(report["native"]["usage"]["totalTokens"], 216);
    assert_eq!(h.frozen_csv(token, time).unwrap(), csv);
    let p = provider_usage::Query {
        source: "native".into(),
        provider_id: Some("_unknown".into()),
        ..Default::default()
    };
    let m = crate::model_usage::UsageQuery {
        source: "native".into(),
        agent: "claude".into(),
        days: None,
        provider: None,
        model: None,
        cursor: None,
    };
    let a = crate::observation::OverviewQuery {
        source: "native".into(),
        agent: Some("claude".into()),
        range: 24,
        metric: "tokens".into(),
        dimension: "agent".into(),
    };
    assert_eq!(
        h.provider_usage(p.clone(), &[], "first".into(), "ready".into(), time)
            .unwrap()["summary"]["calls"],
        17
    );
    assert_eq!(
        h.model_usage(m.clone(), "first".into(), "ready".into(), time)
            .unwrap()["summary"]["usage"]["totalTokens"],
        216
    );
    assert_eq!(h.analysis(&a, time).unwrap()["total"], 216.0);
    let ids: Vec<_> = c.calls.iter().map(|c| c.session_id.clone()).collect();
    let fees = h.fee_summary(&ids, None, false).unwrap();
    assert_eq!(fees["nativeResponses"], 17);
    let epoch = h.request_epoch().unwrap();
    c.calls.chunks[0] = Arc::new(c.calls.chunks[0].as_ref().clone());
    c.revision += 1;
    assert_eq!(h.sync_native_provider(&c, &[]).unwrap(), 0);
    assert_eq!(h.sync_native_models(&c, "claude").unwrap(), 0);
    assert_eq!(h.sync_native_analysis(&c).unwrap(), 0);
    h.sync_native_catalog("progress".into(), &HashMap::new(), &c.calls)
        .unwrap();
    assert_eq!(h.request_epoch().unwrap(), epoch);
    h.db.create_scalar_function(
        "stats_sum",
        1,
        rusqlite::functions::FunctionFlags::SQLITE_UTF8,
        |_| -> rusqlite::Result<String> { Err(rusqlite::Error::InvalidQuery) },
    )
    .unwrap();
    h.db.execute_batch("CREATE TEMP TRIGGER guard_analysis BEFORE INSERT ON analysis_cells BEGIN SELECT RAISE(ABORT,'unchanged overview rebuilt');END;").unwrap();
    assert_eq!(
        h.provider_usage(p, &[], "progress".into(), "partial".into(), time)
            .unwrap()["summary"]["calls"],
        17
    );
    assert_eq!(
        h.model_usage(m, "progress".into(), "scanning".into(), time)
            .unwrap()["summary"]["usage"]["totalTokens"],
        216
    );
    assert_eq!(h.analysis(&a, time).unwrap()["total"], 216.0);
    let mut reordered = ids.clone();
    reordered.reverse();
    reordered.push(ids[0].clone());
    assert_eq!(h.fee_summary(&reordered, None, false).unwrap(), fees);
}

#[test]
fn progress_only_native_catalog_does_not_scan_all_responses_with_gateway_history() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let dir = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(dir.path()).unwrap();
    store.save(&row(0)).unwrap();
    let mut h = History::open(dir.path()).unwrap();
    h.refresh(&[]).unwrap();
    let c = catalog(4, 4096);
    h.sync_native_catalog("initial".into(), &HashMap::new(), &c.calls)
        .unwrap();
    let instructions = Arc::new(AtomicUsize::new(0));
    let counted = instructions.clone();
    h.db.progress_handler(
        1000,
        Some(move || counted.fetch_add(1000, Ordering::Relaxed) > 10_000),
    )
    .unwrap();
    h.sync_native_catalog("progress".into(), &HashMap::new(), &c.calls)
        .unwrap();
    h.db.progress_handler(0, None::<fn() -> bool>).unwrap();
    assert!(instructions.load(Ordering::Relaxed) <= 11_000);
}

#[test]
fn native_file_delta_retains_other_gateway_links_and_retires_changed_response_links() {
    let dir = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(dir.path()).unwrap();
    let mut c = catalog(2, 1);
    let mut gateway = row(0);
    gateway.session_id = Some(c.calls.chunks[0][0].session_id.clone());
    gateway.response_id = c.calls.chunks[0][0].response_id.clone();
    gateway.agent_product_id = c.calls.chunks[0][0].agent_product_id.clone();
    gateway.model = c.calls.chunks[0][0].model.clone();
    gateway.usage = c.calls.chunks[0][0].usage.clone().unwrap();
    store.save(&gateway).unwrap();
    let mut h = History::open(dir.path()).unwrap();
    h.refresh(&[]).unwrap();
    h.sync_native_catalog("first".into(), &HashMap::new(), &c.calls)
        .unwrap();
    let before = h.request_detail(&gateway.id).unwrap().unwrap();
    assert_eq!(before["nativeLinks"].as_array().unwrap().len(), 1);
    let version: i64 =
        h.db.query_row(
            "SELECT v FROM current WHERE kind='gateway' AND id=?1",
            [&gateway.id],
            |r| r.get(0),
        )
        .unwrap();
    let q = requests::Query {
        source: Some("native".into()),
        end: Some(now().to_rfc3339()),
        ..Default::default()
    };
    let frozen = h.request_page(&q, now(), Value::Null, None).unwrap();
    let token = frozen["snapshot"].as_str().unwrap();
    let csv = h.frozen_csv(token, now()).unwrap();
    Arc::make_mut(&mut c.calls.chunks[1])[0].model = Some("unrelated rewrite".into());
    h.sync_native_catalog("unrelated".into(), &HashMap::new(), &c.calls)
        .unwrap();
    assert_eq!(h.request_detail(&gateway.id).unwrap().unwrap(), before);
    assert_eq!(
        h.db.query_row(
            "SELECT v FROM current WHERE kind='gateway' AND id=?1",
            [&gateway.id],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        version
    );
    Arc::make_mut(&mut c.calls.chunks[0])[0].response_id = Some("new-response".into());
    h.sync_native_catalog("changed-response".into(), &HashMap::new(), &c.calls)
        .unwrap();
    assert!(
        h.request_detail(&gateway.id).unwrap().unwrap()["nativeLinks"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    Arc::make_mut(&mut c.calls.chunks[0])[0].response_id = gateway.response_id.clone();
    h.sync_native_catalog("restored-response".into(), &HashMap::new(), &c.calls)
        .unwrap();
    assert_eq!(
        h.request_detail(&gateway.id).unwrap().unwrap()["nativeLinks"],
        before["nativeLinks"]
    );
    c.calls.chunks.remove(0);
    h.sync_native_catalog("removed".into(), &HashMap::new(), &c.calls)
        .unwrap();
    assert!(
        h.request_detail(&gateway.id).unwrap().unwrap()["nativeLinks"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(h.frozen_csv(token, now()).unwrap(), csv);
}

#[test]
fn request_positions_ignore_fee_only_changes_but_invalidate_actual_request_mutations() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = History::open(dir.path()).unwrap();
    h.refresh(&[]).unwrap();
    let first = catalog(1, 80);
    h.sync_native_catalog("first".into(), &HashMap::new(), &first.calls)
        .unwrap();
    let q = requests::Query {
        source: Some("native".into()),
        end: Some(now().to_rfc3339()),
        ..Default::default()
    };
    let report = h.source_report(&q, now(), vec![], None).unwrap();
    let page = h.request_page(&q, now(), report, None).unwrap();
    let token = page["snapshot"].as_str().unwrap();
    let csv = h.frozen_csv(token, now()).unwrap();
    let epoch = h.request_epoch().unwrap();
    let clock: i64 =
        h.db.query_row("SELECT seq FROM query_clock", [], |r| r.get(0))
            .unwrap();
    h.db.execute_batch("CREATE TEMP TRIGGER no_position_rebuild BEFORE INSERT ON positions BEGIN SELECT RAISE(ABORT,'unexpected position rebuild');END;").unwrap();
    let next = catalog(1, 82);
    h.prepare_fees(&next).unwrap();
    assert!(
        h.db.query_row("SELECT seq FROM query_clock", [], |r| r.get::<_, i64>(0))
            .unwrap()
            > clock
    );
    assert_eq!(h.request_epoch().unwrap(), epoch);
    // A collector-only revision also leaves the same indexed request set.
    h.sync_native_catalog("progress-only".into(), &HashMap::new(), &first.calls)
        .unwrap();
    assert_eq!(h.request_epoch().unwrap(), epoch);
    let report = h.source_report(&q, now(), vec![], None).unwrap();
    assert_eq!(report["native"]["requests"], 80);
    let reused = h.request_page(&q, now(), report, None).unwrap();
    assert_eq!(reused["total"], 80);
    h.sync_native_catalog("actual-append".into(), &HashMap::new(), &next.calls)
        .unwrap();
    assert_ne!(h.request_epoch().unwrap(), epoch);
    assert!(h.request_page(&q, now(), Value::Null, None).is_err());
    h.db.execute_batch("DROP TRIGGER no_position_rebuild")
        .unwrap();
    let report = h.source_report(&q, now(), vec![], None).unwrap();
    let changed = h.request_page(&q, now(), report, None).unwrap();
    assert_eq!(changed["total"], 82);
    assert_eq!(changed["summary"]["usage"]["totalTokens"], 82 * 12);
    assert_eq!(h.frozen_csv(token, now()).unwrap(), csv);
    h.sync_native_catalog(
        "delete-only".into(),
        &HashMap::new(),
        &CallCatalog::default(),
    )
    .unwrap();
    let empty = h.request_page(&q, now(), Value::Null, None).unwrap();
    assert_eq!(empty["total"], 0);
    assert_eq!(h.frozen_csv(token, now()).unwrap(), csv);
}

#[test]
fn payload_insert_failure_rolls_back_metadata_and_preserves_frozen_exports() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = History::open(dir.path()).unwrap();
    h.refresh(&[]).unwrap();
    let first = catalog(1, 3);
    h.sync_native_catalog("first".into(), &HashMap::new(), &first.calls)
        .unwrap();
    let q = requests::Query {
        source: Some("native".into()),
        ..Default::default()
    };
    let frozen = h.request_page(&q, now(), Value::Null, None).unwrap();
    let token = frozen["snapshot"].as_str().unwrap();
    let old_csv = h.frozen_csv(token, now()).unwrap();
    let epoch = h.request_epoch().unwrap();
    h.db.execute_batch("CREATE TEMP TRIGGER fail_payload BEFORE INSERT ON record_payloads BEGIN SELECT RAISE(ABORT,'payload failure');END;").unwrap();
    let next = catalog(1, 4);
    assert!(
        h.sync_native_catalog("failed".into(), &HashMap::new(), &next.calls)
            .is_err()
    );
    assert_eq!(
        h.source_report(&q, now(), vec![], None).unwrap()["native"]["requests"],
        3
    );
    assert_eq!(h.frozen_csv(token, now()).unwrap(), old_csv);
    assert_eq!(h.request_epoch().unwrap(), epoch);
    assert_eq!(h.db.query_row("SELECT count(*) FROM record_meta m WHERE NOT EXISTS(SELECT 1 FROM record_payloads p WHERE p.v=m.v)",[],|r|r.get::<_,i64>(0)).unwrap(),0);
    h.db.execute_batch("DROP TRIGGER fail_payload").unwrap();
    h.sync_native_catalog("retry".into(), &HashMap::new(), &next.calls)
        .unwrap();
    assert_eq!(
        h.source_report(&q, now(), vec![], None).unwrap()["native"]["requests"],
        4
    );
    assert_eq!(h.frozen_csv(token, now()).unwrap(), old_csv);
    h.sync_native_catalog("removed".into(), &HashMap::new(), &CallCatalog::default())
        .unwrap();
    h.request_page(&q, now() + chrono::Duration::minutes(16), Value::Null, None)
        .unwrap();
    h.prune().unwrap();
    assert_eq!(
        h.db.query_row("SELECT count(*) FROM record_payloads", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        h.db.query_row("SELECT count(*) FROM record_meta", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn request_blocks_seek_record_ranges_instead_of_revisiting_the_entire_current_catalog() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let dir = tempfile::tempdir().unwrap();
    let mut h = History::open(dir.path()).unwrap();
    h.refresh(&[]).unwrap();
    let c = catalog(32, 4096);
    h.sync_native_catalog("budget".into(), &HashMap::new(), &c.calls)
        .unwrap();
    let instructions = Arc::new(AtomicUsize::new(0));
    let counted = instructions.clone();
    h.db.progress_handler(
        1000,
        Some(move || counted.fetch_add(1000, Ordering::Relaxed) > 80_000_000),
    )
    .unwrap();
    let q = requests::Query {
        source: Some("native".into()),
        ..Default::default()
    };
    let cells = h.native_request_cells(&q, now()).unwrap().unwrap();
    h.db.progress_handler(0, None::<fn() -> bool>).unwrap();
    let summary = aggregates::summary_base(&h.db, &cells).unwrap();
    assert_eq!(summary.requests, 32 * 4096);
    assert_eq!(summary.usage.total_tokens, Some(32 * 4096 * 12));
    assert!(instructions.load(Ordering::Relaxed) < 80_000_000);
}
