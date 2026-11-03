use super::*;
use crate::{projects::ProjectCatalog, sessions::CallCatalog};
use std::sync::Arc;

#[test]
fn native_dashboard_groups_and_frozen_pages_match_canonical_after_rewrites() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = History::open(dir.path()).unwrap();
    h.refresh(&[]).unwrap();
    let mut c = crate::associated_cost_tests::catalog();
    let prototype = c.calls[0].clone();
    c.calls = (0..93)
        .map(|n| {
            let mut call = prototype.clone();
            call.id = format!("chart-{n:03}");
            call.model = (n % 5 != 0).then(|| format!("model-{}", n % 17));
            call.provider = Some(format!("provider-{}", n % 3));
            call.timestamp = match n % 7 {
                0 => None,
                1 => Some((now() + chrono::Duration::seconds(1)).to_rfc3339()),
                _ => Some((now() - chrono::Duration::hours(n % 12 + 1)).to_rfc3339()),
            };
            if n % 11 == 0 {
                call.usage = None;
            }
            call
        })
        .collect();
    let mut native = requests::sources::native_records(c.calls.clone(), &HashMap::new());
    for phase in 0..2 {
        if phase == 1 {
            native[4].usage.total_tokens = Some(u64::MAX);
            native[4].provider = "new provider label".into();
            native.remove(8);
        }
        h.sync_native(phase.to_string(), &HashMap::new(), native.clone())
            .unwrap();
        for metric in ["calls", "tokens", "cost", "ttft", "duration", "throughput"] {
            for dimension in ["agent", "model", "provider"] {
                let q = requests::Query {
                    source: Some("native".into()),
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
                assert_close(
                    &got,
                    &expected,
                    &format!("native {phase}/{metric}/{dimension}"),
                );
            }
        }
    }
}

#[test]
fn native_provider_allocates_only_requested_pages_and_keeps_scope_after_other_queries() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = History::open(dir.path()).unwrap();
    h.refresh(&[]).unwrap();
    let mut c = crate::associated_cost_tests::catalog();
    let prototype = c.calls[0].clone();
    c.calls = (0..800)
        .map(|n| {
            let mut call = prototype.clone();
            call.id = format!("lazy-page-{n:04}");
            call.model = Some(format!("model-{}", n % 3));
            call.timestamp = Some((now() - chrono::Duration::seconds(n % 7 + 1)).to_rfc3339());
            call
        })
        .collect();
    h.sync_native_provider(&c, &[]).unwrap();
    let q = provider_usage::Query {
        source: "native".into(),
        provider_id: Some("_unknown".into()),
        ..Default::default()
    };
    let first = h
        .provider_usage(q.clone(), &[], "asof".into(), "ready".into(), now())
        .unwrap();
    assert_eq!(first["totalCalls"], 800);
    assert_eq!(
        h.db.query_row("SELECT count(*) FROM provider_positions", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        40
    );
    let filtered = provider_usage::Query {
        model: Some("model-1".into()),
        ..q.clone()
    };
    h.provider_usage(filtered, &[], "asof".into(), "ready".into(), now())
        .unwrap();
    let q2 = provider_usage::Query {
        cursor: first["nextCursor"].as_str().map(str::to_owned),
        ..q.clone()
    };
    let next = h
        .provider_usage(q2, &[], "asof".into(), "ready".into(), now())
        .unwrap();
    let canonical = value(
        &provider_usage::aggregate(
            q,
            &[],
            provider_usage::native_records(c.calls.clone(), &[]),
            "asof".into(),
            "ready".into(),
            now(),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(first["calls"], canonical["calls"]);
    let mut records = provider_usage::native_records(c.calls.clone(), &[]);
    records.sort_by(|a, b| b.time.cmp(&a.time).then(a.id.cmp(&b.id)));
    assert_eq!(next["calls"], value(&&records[40..80]).unwrap());
    assert_eq!(next["summary"], first["summary"]);
    assert_eq!(
        h.db.query_row("SELECT count(*) FROM provider_positions", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        120
    );
}

#[test]
fn usage_source_progress_updates_metadata_without_reaggregating_or_losing_cursors() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = History::open(dir.path()).unwrap();
    h.refresh(&[]).unwrap();
    let mut c = crate::associated_cost_tests::catalog();
    let prototype = c.calls[0].clone();
    c.calls = (0..45)
        .map(|n| {
            let mut call = prototype.clone();
            call.id = format!("metadata-{n}");
            call.model = Some(format!("model-{n:02}"));
            call.timestamp = Some((now() - chrono::Duration::seconds(1)).to_rfc3339());
            call
        })
        .collect();
    h.sync_native_provider(&c, &[]).unwrap();
    h.sync_native_models(&c, "claude").unwrap();
    let mut p = provider_usage::Query {
        source: "native".into(),
        agent: Some("claude".into()),
        provider_id: Some("_unknown".into()),
        ..Default::default()
    };
    let mut m = crate::model_usage::UsageQuery {
        source: "native".into(),
        agent: "claude".into(),
        days: None,
        provider: None,
        model: None,
        cursor: None,
    };
    let first_p = h
        .provider_usage(p.clone(), &[], "asof".into(), "ready".into(), now())
        .unwrap();
    let first_m = h
        .model_usage(m.clone(), "asof".into(), "ready".into(), now())
        .unwrap();
    p.cursor = Some(first_p["nextCursor"].as_str().unwrap().into());
    m.cursor = Some(first_m["nextCursor"].as_str().unwrap().into());
    let next_p = h
        .provider_usage(p.clone(), &[], "asof".into(), "ready".into(), now())
        .unwrap();
    let next_m = h
        .model_usage(m.clone(), "asof".into(), "ready".into(), now())
        .unwrap();
    h.db.create_scalar_function(
        "agent_matches",
        3,
        rusqlite::functions::FunctionFlags::SQLITE_UTF8,
        |_| -> rusqlite::Result<bool> { Err(rusqlite::Error::InvalidQuery) },
    )
    .unwrap();
    for status in ["scanning", "partial", "ready"] {
        let mut expected_p = next_p.clone();
        expected_p["sourceState"] = json!(status);
        expected_p["sourceAsOf"] = json!("latest");
        let mut expected_m = next_m.clone();
        expected_m["sourceState"] = json!(status);
        expected_m["sourceAsOf"] = json!("latest");
        assert_eq!(
            h.provider_usage(p.clone(), &[], "latest".into(), status.into(), now())
                .unwrap(),
            expected_p
        );
        assert_eq!(
            h.model_usage(m.clone(), "latest".into(), status.into(), now())
                .unwrap(),
            expected_m
        );
    }
    aggregates::register_directory(&h.db, h.directory.clone()).unwrap();
    c.calls[0].usage.as_mut().unwrap().total_tokens = Some(999);
    c.revision += 1;
    h.sync_native_provider(&c, &[]).unwrap();
    h.sync_native_models(&c, "claude").unwrap();
    assert!(
        h.provider_usage(p, &[], "latest".into(), "ready".into(), now())
            .is_err()
    );
    assert!(
        h.model_usage(m, "latest".into(), "ready".into(), now())
            .is_err()
    );
}

#[test]
fn native_provider_options_decode_only_the_latest_current_label() {
    let db = Connection::open_in_memory().unwrap();
    db.execute_batch(
        "CREATE TABLE options(v INTEGER PRIMARY KEY,id TEXT UNIQUE,provider TEXT,value TEXT);
         INSERT INTO options(id,provider,value) VALUES('a','one','not JSON'),
         ('z','one','{\"provider\":\"new label\"}'),
         ('b','two','{\"provider\":\"other\"}'),
         ('c',NULL,'{\"provider\":null}');",
    )
    .unwrap();
    assert_eq!(
        dashboard::native_provider_options(&db, "options").unwrap(),
        vec![
            json!({"id":null,"label":null}),
            json!({"id":"one","label":"new label"}),
            json!({"id":"two","label":"other"})
        ]
    );
    assert!(
        dashboard::native_provider_options(&db, "(SELECT * FROM options WHERE 0)")
            .unwrap()
            .is_empty()
    );
    db.execute("DELETE FROM options WHERE id='a'", []).unwrap();
    db.execute(
        "INSERT INTO options(id,provider,value) VALUES('zz','one','{\"provider\":null}')",
        [],
    )
    .unwrap();
    let from = "(SELECT * FROM options WHERE provider IS NOT NULL)";
    let canonical = aggregates::list(&db, &format!("SELECT json_object('id',provider,'label',label) FROM (SELECT provider,json_extract(value,'$.provider') label,row_number() OVER(PARTITION BY provider ORDER BY id DESC) rank FROM {from}) WHERE rank=1 ORDER BY provider")).unwrap();
    assert_eq!(
        dashboard::native_provider_options(&db, from).unwrap(),
        canonical
    );
}

#[test]
fn native_provider_options_do_not_multiply_full_history_by_provider_count() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let db = Connection::open_in_memory().unwrap();
    db.execute_batch("CREATE TABLE records(v INTEGER PRIMARY KEY,id TEXT,kind TEXT,provider TEXT,value TEXT);
        CREATE TABLE current(kind TEXT,id TEXT,v INTEGER UNIQUE,PRIMARY KEY(kind,id));
        CREATE INDEX record_kind ON records(kind);
        WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i+1 FROM n WHERE i<10000)
        INSERT INTO records SELECT i,printf('%08d',i),'native','provider-'||(i%1000),json_object('provider','label-'||i) FROM n;
        INSERT INTO current SELECT kind,id,v FROM records;")
        .unwrap();
    let steps = Arc::new(AtomicUsize::new(0));
    let counted = steps.clone();
    db.progress_handler(
        1000,
        Some(move || counted.fetch_add(1, Ordering::Relaxed) >= 2000),
    )
    .unwrap();
    let rows = dashboard::native_provider_options(
        &db,
        "(SELECT r.* FROM records r JOIN current c ON c.v=r.v WHERE r.kind='native')",
    )
    .unwrap();
    assert_eq!(rows.len(), 1000);
    for row in rows {
        let group: usize = row["id"]
            .as_str()
            .unwrap()
            .trim_start_matches("provider-")
            .parse()
            .unwrap();
        assert_eq!(
            row["label"],
            format!("label-{}", if group == 0 { 10000 } else { 9000 + group })
        );
    }
    assert!(steps.load(Ordering::Relaxed) < 2000);
}

#[test]
fn usage_expiry_seeks_index_without_scanning_other_agents() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let db = Connection::open_in_memory().unwrap();
    db.execute_batch(
        "CREATE TABLE expiry_rows(time INTEGER,agent TEXT);
         CREATE INDEX expiry_time ON expiry_rows(time);
         WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i+1 FROM n WHERE i<50000)
         INSERT INTO expiry_rows SELECT i,'claude' FROM n;",
    )
    .unwrap();
    let visits = Arc::new(AtomicUsize::new(0));
    let counted = visits.clone();
    db.create_scalar_function(
        "scope_matches",
        1,
        rusqlite::functions::FunctionFlags::SQLITE_UTF8,
        move |ctx| {
            counted.fetch_add(1, Ordering::Relaxed);
            Ok(ctx.get::<String>(0)? == "claude")
        },
    )
    .unwrap();
    let from = "(SELECT * FROM expiry_rows WHERE scope_matches(agent))";
    let at = chrono::DateTime::from_timestamp_millis(60_000).unwrap();
    assert_eq!(usage::range_expiry(&db, from, None, at).unwrap(), None);
    assert_eq!(
        usage::range_expiry(&db, from, Some(1), at).unwrap(),
        Some(86_400_001)
    );
    assert!(visits.load(Ordering::Relaxed) <= 2);
    db.execute("INSERT INTO expiry_rows VALUES(60001,'claude')", [])
        .unwrap();
    assert_eq!(
        usage::range_expiry(&db, from, None, at).unwrap(),
        Some(60_001)
    );
    assert_eq!(
        usage::range_expiry(&db, from, Some(1), at).unwrap(),
        Some(60_001)
    );
    assert!(visits.load(Ordering::Relaxed) <= 5);
}

fn dashboard_cache_fixture(
    future: bool,
) -> (
    tempfile::TempDir,
    History,
    ProjectCatalog,
    requests::Query,
    chrono::DateTime<chrono::Utc>,
) {
    let at = chrono::DateTime::parse_from_rfc3339("2026-10-20T01:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    let dir = tempfile::tempdir().unwrap();
    let mut h = History::open(dir.path()).unwrap();
    h.refresh(&[]).unwrap();
    let mut catalog = crate::associated_cost_tests::catalog();
    let base = catalog.calls[0].clone();
    catalog.calls = CallCatalog::shared(
        [Arc::new(
            (0..2)
                .map(|i| {
                    let mut call = base.clone();
                    call.id = format!("cache-native-{i}");
                    call.agent_id = "claude".into();
                    call.agent_product_id = None;
                    call.response_id = None;
                    call.timestamp = Some(
                        (at + chrono::Duration::seconds(if future && i == 1 { 1 } else { -60 }))
                            .to_rfc3339(),
                    );
                    call
                })
                .collect(),
        )]
        .into_iter(),
    );
    h.sync_native_catalog("initial".into(), &HashMap::new(), &catalog.calls)
        .unwrap();
    let query = requests::Query {
        source: Some("native".into()),
        metric: Some("calls".into()),
        start: Some((at - chrono::Duration::minutes(30)).to_rfc3339()),
        end: Some((at + chrono::Duration::hours(1)).to_rfc3339()),
        ..Default::default()
    };
    (dir, h, catalog, query, at)
}

#[test]
fn unchanged_dashboard_refresh_shares_positions_but_keeps_public_freezes_and_expiry() {
    use rusqlite::functions::FunctionFlags;
    let (_dir, mut h, _, q, at) = dashboard_cache_fixture(false);
    let report = h.source_report(&q, at, vec![], None).unwrap();
    let first = h.request_page(&q, at, report, None).unwrap();
    h.db.execute_batch("CREATE TEMP TRIGGER no_position_copy BEFORE INSERT ON positions BEGIN SELECT RAISE(ABORT,'unchanged positions copied');END;").unwrap();
    h.db.create_scalar_function(
        "summary_sum",
        3,
        FunctionFlags::SQLITE_UTF8,
        |_| -> rusqlite::Result<String> { Err(rusqlite::Error::InvalidQuery) },
    )
    .unwrap();
    let later = at + chrono::Duration::seconds(2);
    let report = h.source_report(&q, later, vec![], None).unwrap();
    let second = h.request_page(&q, later, report, None).unwrap();
    assert_ne!(first["snapshot"], second["snapshot"]);
    assert_eq!(first["items"], second["items"]);
    assert_eq!(first["summary"], second["summary"]);
    assert_eq!(
        second["buckets"].as_array().unwrap().last().unwrap()["end"],
        later.to_rfc3339()
    );
    let old = requests::Query {
        snapshot: Some(first["snapshot"].as_str().unwrap().into()),
        ..q.clone()
    };
    assert_eq!(
        h.request_page(&old, later, Value::Null, None).unwrap(),
        first
    );
    assert_eq!(
        h.frozen_csv(first["snapshot"].as_str().unwrap(), later)
            .unwrap()["asOf"],
        first["asOf"]
    );
    let expired = at + chrono::Duration::minutes(16);
    assert!(h.request_page(&old, expired, Value::Null, None).is_err());
    let report = h.source_report(&q, expired, vec![], None).unwrap();
    assert_eq!(
        h.request_page(&q, expired, report, None).unwrap()["items"],
        first["items"]
    );
}

#[test]
fn request_caches_expire_at_future_record_and_rebuild_after_catalog_changes() {
    let (_dir, mut h, mut catalog, q, at) = dashboard_cache_fixture(true);
    let report = h.source_report(&q, at, vec![], None).unwrap();
    let first = h.request_page(&q, at, report, None).unwrap();
    assert_eq!(first["total"], 1);
    let later = at + chrono::Duration::seconds(2);
    let report = h.source_report(&q, later, vec![], None).unwrap();
    let second = h.request_page(&q, later, report, None).unwrap();
    assert_eq!(second["total"], 2);
    catalog.calls.clear();
    h.sync_native_catalog("revoked".into(), &HashMap::new(), &catalog.calls)
        .unwrap();
    let report = h.source_report(&q, later, vec![], None).unwrap();
    assert_eq!(h.request_page(&q, later, report, None).unwrap()["total"], 0);
    let frozen = requests::Query {
        snapshot: Some(first["snapshot"].as_str().unwrap().into()),
        ..q
    };
    assert_eq!(
        h.request_page(&frozen, later, Value::Null, None).unwrap(),
        first
    );
}

#[test]
fn cached_request_time_ranges_match_rebuilt_results_across_bucket_and_calendar_edges() {
    let (_dir, mut h, _, mut q, at) = dashboard_cache_fixture(false);
    let millis = chrono::Duration::milliseconds(500);
    for future_start in [false, true] {
        q.start = Some(
            (at + if future_start {
                chrono::Duration::seconds(10)
            } else {
                -chrono::Duration::days(1)
            })
            .to_rfc3339(),
        );
        q.end = Some((at + chrono::Duration::days(70)).to_rfc3339());
        let times = if future_start {
            vec![at, at + chrono::Duration::seconds(11)]
        } else {
            vec![
                at - millis,
                at + millis,
                at + chrono::Duration::seconds(2),
                at + chrono::Duration::hours(15) - millis,
                at + chrono::Duration::hours(15) + millis,
                at + chrono::Duration::days(6) - millis,
                at + chrono::Duration::days(6) + millis,
                at + chrono::Duration::days(6) + chrono::Duration::seconds(2),
                at + chrono::Duration::days(59) - millis,
                at + chrono::Duration::days(59) + millis,
                at + chrono::Duration::days(59) + chrono::Duration::seconds(2),
            ]
        };
        for metric in ["calls", "tokens", "cost"] {
            q.metric = Some(metric.into());
            for time in &times {
                let report = h
                    .source_report(&q, *time, vec![], Some(time.to_rfc3339()))
                    .unwrap();
                let mut cached = h
                    .request_page(&q, *time, report, Some("fixture status".into()))
                    .unwrap();
                h.request_templates.clear();
                h.request_reports.borrow_mut().clear();
                let report = h
                    .source_report(&q, *time, vec![], Some(time.to_rfc3339()))
                    .unwrap();
                let mut rebuilt = h
                    .request_page(&q, *time, report, Some("fixture status".into()))
                    .unwrap();
                cached.as_object_mut().unwrap().remove("snapshot");
                rebuilt.as_object_mut().unwrap().remove("snapshot");
                assert_eq!(
                    cached, rebuilt,
                    "{metric}, future_start={future_start}, {time}"
                );
            }
        }
    }
}

#[test]
fn native_fee_coverage_uses_session_counts_and_only_rebuilds_changed_sessions() {
    use rusqlite::functions::FunctionFlags;
    use std::sync::atomic::{AtomicUsize, Ordering};
    let dir = tempfile::tempdir().unwrap();
    let mut h = History::open(dir.path()).unwrap();
    h.refresh(&[]).unwrap();
    let mut c = crate::associated_cost_tests::catalog();
    let templates: Vec<_> = c.calls.iter().cloned().collect();
    c.calls = (0..16384)
        .map(|n| {
            let mut call = templates[n % 2].clone();
            call.id = format!("coverage-{n}");
            call.response_id = None;
            call.agent_product_id = match n % 3 {
                0 => Some("claude-code-cli".into()),
                1 => Some("claude-code-desktop".into()),
                _ => None,
            };
            call
        })
        .collect();
    let ids: Vec<_> = c.sessions.iter().map(|s| s.id.clone()).collect();
    h.prepare_fees(&c).unwrap();
    assert_eq!(
        h.fee_summary(&ids, None, false).unwrap()["nativeResponses"],
        16384
    );
    let reads = Arc::new(AtomicUsize::new(0));
    let measured = reads.clone();
    h.db.create_scalar_function("agent_matches", 3, FunctionFlags::SQLITE_UTF8, move |ctx| {
        if measured.fetch_add(1, Ordering::SeqCst) >= 64 {
            return Err(rusqlite::Error::InvalidQuery);
        }
        let agent: Option<String> = ctx.get(0)?;
        let family: String = ctx.get(1)?;
        let product: Option<String> = ctx.get(2)?;
        Ok(crate::agents::matches(
            agent.as_deref(),
            &family,
            product.as_deref(),
        ))
    })
    .unwrap();
    for agent in ["claude-code-cli", "claude-code-desktop", "claude-unknown"] {
        h.fee_summaries.borrow_mut().clear();
        let expected =
            crate::associated_cost::Index::new(&c, vec![], false).summary(&ids, Some(agent));
        assert_close(
            &h.fee_summary(&ids, Some(agent), false).unwrap(),
            &value(&expected).unwrap(),
            "session coverage counters",
        );
    }
    assert!(reads.load(Ordering::SeqCst) <= 64);
    let untouched = c.calls[1].session_id.clone();
    h.db.execute_batch(&format!("CREATE TEMP TRIGGER coverage_stable_session BEFORE DELETE ON fee_native_counts WHEN old.session={} BEGIN SELECT RAISE(ABORT,'unchanged Session coverage');END;",aggregates::quote(&untouched))).unwrap();
    c.calls[0].agent_product_id = Some("claude-code-desktop".into());
    c.revision += 1;
    h.prepare_fees(&c).unwrap();
    h.fee_summaries.borrow_mut().clear();
    reads.store(0, Ordering::SeqCst);
    let expected = crate::associated_cost::Index::new(&c, vec![], false)
        .summary(&ids, Some("claude-code-cli"));
    assert_close(
        &h.fee_summary(&ids, Some("claude-code-cli"), false).unwrap(),
        &value(&expected).unwrap(),
        "changed Session coverage",
    );
    let groups: usize =
        h.db.query_row("SELECT count(*) FROM fee_native_counts", [], |r| {
            sql_count(r, 0)
        })
        .unwrap();
    assert!(groups <= 6);
}

#[test]
fn native_fee_counter_failure_rolls_back_and_updates_deletes_and_revocation_remain_exact() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = History::open(dir.path()).unwrap();
    h.refresh(&[]).unwrap();
    let mut c = crate::associated_cost_tests::catalog();
    let ids: Vec<_> = c.sessions.iter().map(|s| s.id.clone()).collect();
    h.prepare_fees(&c).unwrap();
    h.fee_summary(&ids, None, false).unwrap();
    let before:String=h.db.query_row("SELECT group_concat(n,',') FROM (SELECT n FROM fee_native_counts ORDER BY session,family,product)",[],|r|r.get(0)).unwrap();
    let clock: i64 =
        h.db.query_row("SELECT seq FROM fee_native_counts_clock", [], |r| r.get(0))
            .unwrap();
    let mut extra = c.calls[0].clone();
    extra.id = "new-coverage".into();
    extra.response_id = None;
    Arc::make_mut(&mut c.calls.chunks[0]).push(extra);
    c.revision += 1;
    h.prepare_fees(&c).unwrap();
    h.db.execute_batch("CREATE TEMP TRIGGER deny_coverage BEFORE INSERT ON fee_native_counts BEGIN SELECT RAISE(ABORT,'coverage failure');END;").unwrap();
    assert!(h.fee_summary(&ids, None, false).is_err());
    assert_eq!(
        h.db.query_row("SELECT seq FROM fee_native_counts_clock", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        clock
    );
    assert_eq!(h.db.query_row("SELECT group_concat(n,',') FROM (SELECT n FROM fee_native_counts ORDER BY session,family,product)",[],|r|r.get::<_,String>(0)).unwrap(),before);
    h.db.execute_batch("DROP TRIGGER deny_coverage").unwrap();
    let expected = crate::associated_cost::Index::new(&c, vec![], false).summary(&ids, None);
    assert_close(
        &h.fee_summary(&ids, None, false).unwrap(),
        &value(&expected).unwrap(),
        "coverage retry after rollback",
    );
    h.db.execute_batch(
        "UPDATE fee_native SET association='identity_conflict',conflict=1 WHERE id='new-coverage';",
    )
    .unwrap();
    h.fee_summaries.borrow_mut().clear();
    assert_eq!(
        h.fee_summary(&ids, None, false).unwrap()["conflictingNativeResponses"],
        1
    );
    h.db.execute_batch("DELETE FROM fee_native WHERE id='new-coverage';")
        .unwrap();
    h.fee_summaries.borrow_mut().clear();
    let deleted = h.fee_summary(&ids, None, false).unwrap();
    assert_eq!(deleted["nativeResponses"], 2);
    assert_eq!(deleted["conflictingNativeResponses"], 0);
    h.release_native_snapshots();
    c.calls.clear();
    c.revision += 1;
    h.prepare_fees(&c).unwrap();
    assert_eq!(
        h.fee_summary(&ids, None, false).unwrap()["nativeResponses"],
        0
    );
}

#[test]
fn restored_fee_catalogs_reuse_integer_summaries_but_gateway_changes_invalidate_them() {
    let dir = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(dir.path()).unwrap();
    let mut h = History::open(dir.path()).unwrap();
    h.refresh(&[]).unwrap();
    let first = crate::associated_cost_tests::catalog();
    let ids: Vec<_> = first.sessions.iter().map(|s| s.id.clone()).collect();
    h.prepare_fees(&first).unwrap();
    let old = h.fee_summary(&ids, None, false).unwrap();
    let mut next = first.clone();
    let mut extra = next.calls[0].clone();
    extra.id = "fee-replay-extra".into();
    extra.response_id = Some("fee-replay-response".into());
    Arc::make_mut(&mut next.calls.chunks[0]).push(extra);
    next.revision += 1;
    h.prepare_fees(&next).unwrap();
    let current = h.fee_summary(&ids, None, false).unwrap();
    assert_eq!(current["nativeResponses"], 3);
    h.db.create_scalar_function(
        "stats_sum",
        1,
        rusqlite::functions::FunctionFlags::SQLITE_UTF8,
        |_| -> rusqlite::Result<String> { Err(rusqlite::Error::InvalidQuery) },
    )
    .unwrap();
    h.prepare_fees(&first).unwrap();
    assert_eq!(h.fee_summary(&ids, None, false).unwrap(), old);
    h.prepare_fees(&next).unwrap();
    assert_eq!(h.fee_summary(&ids, None, false).unwrap(), current);
    let mut gateway = row(0);
    gateway.session_id = Some(ids[0].clone());
    gateway.response_id = None;
    store.save(&gateway).unwrap();
    h.refresh(&[]).unwrap();
    h.prepare_fees(&next).unwrap();
    assert!(
        h.fee_summary(&ids, None, false).is_err(),
        "a new attributed gateway attempt must not reuse the old summary"
    );
    aggregates::register(&h.db).unwrap();
    let expected =
        crate::associated_cost::Index::new(&next, vec![gateway.clone()], false).summary(&ids, None);
    assert_close(
        &h.fee_summary(&ids, None, false).unwrap(),
        &value(&expected).unwrap(),
        "restored fee summary with a new gateway attempt",
    );
    gateway.endpoint = "https://openrouter.ai/api/v1".into();
    gateway.reported_cost = Some(
        serde_json::from_value(
            json!({"amount":0,"currency":"USD","source":"openrouter.usage.cost"}),
        )
        .unwrap(),
    );
    gateway.revision += 1;
    store.save(&gateway).unwrap();
    h.refresh(&[]).unwrap();
    for catalog in [&first, &next] {
        h.prepare_fees(catalog).unwrap();
        assert_eq!(
            h.fee_summary(&ids, None, false).unwrap()["cost"]["actual"][0]["amount"],
            0.0
        );
    }
    h.db.create_scalar_function(
        "stats_sum",
        1,
        rusqlite::functions::FunctionFlags::SQLITE_UTF8,
        |_| -> rusqlite::Result<String> { Err(rusqlite::Error::InvalidQuery) },
    )
    .unwrap();
    h.prepare_fees(&first).unwrap();
    assert!(
        h.fee_summary(&ids, None, false).is_err(),
        "confirmed zero money still retains the physical aggregation order after restoration"
    );
    assert!(h.fee_versions.len() <= 8);
    h.release_native_snapshots();
    assert!(h.fee_versions.is_empty());
}

#[test]
fn native_fee_summary_reuses_unchanged_aggregates_and_release_clears_them() {
    use rusqlite::functions::FunctionFlags;
    let dir = tempfile::tempdir().unwrap();
    let mut h = History::open(dir.path()).unwrap();
    h.refresh(&[]).unwrap();
    let c = crate::associated_cost_tests::catalog();
    h.prepare_fees(&c).unwrap();
    let ids = c.sessions.iter().map(|s| s.id.clone()).collect::<Vec<_>>();
    let first = h.fee_summary(&ids, None, false).unwrap();
    h.db.create_scalar_function(
        "stats_sum",
        1,
        FunctionFlags::SQLITE_UTF8,
        |_| -> rusqlite::Result<String> { Err(rusqlite::Error::InvalidQuery) },
    )
    .unwrap();
    assert_eq!(h.fee_summary(&ids, None, false).unwrap(), first);
    let mut partial = first.clone();
    partial["sourceState"] = json!("partial");
    assert_eq!(h.fee_summary(&ids, None, true).unwrap(), partial);
    assert_eq!(h.fee_summary(&ids, None, false).unwrap(), first);
    assert_eq!(h.fee_summaries.borrow().len(), 1);
    h.release_native_snapshots();
    assert!(h.fee_summary(&ids, None, false).is_err());
}

#[test]
fn native_model_agent_switches_reuse_rows_and_keep_independent_result_caches() {
    let dir = tempfile::tempdir().unwrap();
    ObservationStore::open(dir.path())
        .unwrap()
        .save_batch(&[row(0)])
        .unwrap();
    let mut h = History::open(dir.path()).unwrap();
    h.refresh(&[]).unwrap();
    let mut c = crate::associated_cost_tests::catalog();
    let mut a = c.calls[0].clone();
    a.id = "switch-claude".into();
    a.agent_id = "claude".into();
    a.agent_product_id = None;
    a.response_id = None;
    a.timestamp = Some((now() - chrono::Duration::seconds(1)).to_rfc3339());
    a.usage = Some(Usage {
        input_tokens: Some(10),
        output_tokens: Some(2),
        total_tokens: Some(12),
        ..Default::default()
    });
    let mut b = a.clone();
    b.id = "switch-codex".into();
    b.agent_id = "codex".into();
    c.calls = CallCatalog::shared([Arc::new(vec![a, b])].into_iter());
    c.revision += 1;
    let q = crate::model_usage::UsageQuery {
        source: "native".into(),
        agent: "claude".into(),
        days: None,
        provider: None,
        model: None,
        cursor: None,
    };
    h.sync_native_models(&c, "claude").unwrap();
    let claude = h
        .model_usage(q.clone(), "asof".into(), "ready".into(), now())
        .unwrap();
    assert_eq!(claude["summary"]["attempts"], 1);
    assert_eq!(claude["summary"]["usage"]["totalTokens"], 12);
    h.db.execute_batch("CREATE TEMP TRIGGER immutable_model_insert BEFORE INSERT ON native_model_usage BEGIN SELECT RAISE(ABORT,'unchanged catalog inserted');END;CREATE TEMP TRIGGER immutable_model_delete BEFORE DELETE ON native_model_usage BEGIN SELECT RAISE(ABORT,'unchanged catalog deleted');END;").unwrap();
    assert_eq!(h.sync_native_models(&c, "codex").unwrap(), 0);
    let codex_query = crate::model_usage::UsageQuery {
        agent: "codex".into(),
        ..q.clone()
    };
    let codex = h
        .model_usage(codex_query.clone(), "asof".into(), "ready".into(), now())
        .unwrap();
    assert_eq!(codex["summary"]["attempts"], 1);
    assert_eq!(codex["summary"]["usage"]["totalTokens"], 12);
    h.model_usage(
        crate::model_usage::UsageQuery {
            source: "gateway".into(),
            ..q.clone()
        },
        "asof".into(),
        "ready".into(),
        now(),
    )
    .unwrap();
    h.db.execute_batch("CREATE TEMP TRIGGER immutable_model_blocks BEFORE DELETE ON native_model_blocks BEGIN SELECT RAISE(ABORT,'unchanged groups rebuilt');END;").unwrap();
    assert_eq!(h.sync_native_models(&c, "claude").unwrap(), 0);
    assert_eq!(
        h.model_usage(q, "asof".into(), "ready".into(), now())
            .unwrap(),
        claude
    );
    assert_eq!(
        h.model_usage(codex_query, "asof".into(), "ready".into(), now())
            .unwrap(),
        codex
    );
}

#[test]
fn provider_source_switches_keep_positions_and_cached_summaries() {
    let dir = tempfile::tempdir().unwrap();
    ObservationStore::open(dir.path())
        .unwrap()
        .save_batch(&[row(0)])
        .unwrap();
    let mut h = History::open(dir.path()).unwrap();
    h.refresh(&[]).unwrap();
    let c = crate::associated_cost_tests::catalog();
    h.sync_native_provider(&c, &[]).unwrap();
    let q = provider_usage::Query {
        source: "native".into(),
        provider_id: Some("_unknown".into()),
        ..Default::default()
    };
    let native = h
        .provider_usage(q.clone(), &[], "asof".into(), "ready".into(), now())
        .unwrap();
    h.provider_usage(
        provider_usage::Query {
            source: "gateway".into(),
            ..q.clone()
        },
        &[],
        "asof".into(),
        "ready".into(),
        now(),
    )
    .unwrap();
    h.db.execute_batch("CREATE TEMP TRIGGER immutable_provider_positions BEFORE DELETE ON provider_positions BEGIN SELECT RAISE(ABORT,'other source positions evicted');END;").unwrap();
    assert_eq!(
        h.provider_usage(q, &[], "asof".into(), "ready".into(), now())
            .unwrap(),
        native
    );
}

#[test]
fn uncached_large_provider_options_do_not_accumulate_position_versions() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = History::open(dir.path()).unwrap();
    h.refresh(&[]).unwrap();
    let mut c = crate::associated_cost_tests::catalog();
    let base = c.calls[0].clone();
    c.calls = CallCatalog::shared(
        [Arc::new(
            (0..1001)
                .map(|i| {
                    let mut call = base.clone();
                    call.id = format!("bounded-{i:04}");
                    call.model = Some(format!("unique-{i:04}"));
                    call.timestamp = Some((now() - chrono::Duration::seconds(1)).to_rfc3339());
                    call
                })
                .collect(),
        )]
        .into_iter(),
    );
    h.sync_native_provider(&c, &[]).unwrap();
    for days in [None, Some(1), Some(7), Some(30)] {
        let q = provider_usage::Query {
            source: "native".into(),
            provider_id: Some("_unknown".into()),
            days,
            ..Default::default()
        };
        let result = h
            .provider_usage(q, &[], "asof".into(), "ready".into(), now())
            .unwrap();
        assert_eq!(result["totalCalls"], 1001);
        assert!(h.provider_cache.is_empty());
        let count: i64 =
            h.db.query_row(
                "SELECT count(DISTINCT version) FROM provider_positions",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }
}

#[test]
fn gateway_paging_keeps_legacy_record_digest_and_does_not_use_native_hashes() {
    use rusqlite::functions::FunctionFlags;
    use sha2::{Digest, Sha256};
    let dir = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(dir.path()).unwrap();
    store
        .save_batch(&(0..45).map(row).collect::<Vec<_>>())
        .unwrap();
    let mut h = History::open(dir.path()).unwrap();
    h.refresh(&[]).unwrap();
    h.db.create_scalar_function(
        "row_digest",
        1,
        FunctionFlags::SQLITE_UTF8,
        |_| -> rusqlite::Result<String> { Err(rusqlite::Error::InvalidQuery) },
    )
    .unwrap();
    let mut q = provider_usage::Query {
        source: "gateway".into(),
        provider_id: Some("_unknown".into()),
        ..Default::default()
    };
    let first = h
        .provider_usage(q.clone(), &[], "asof".into(), "ready".into(), now())
        .unwrap();
    let records:String=h.db.query_row("SELECT hash_rows(value) FROM (SELECT json_set(provider_record,'$.providerId',NULL,'$.attribution','removed_provider') value FROM attempts ORDER BY id)",[],|r|r.get(0)).unwrap();
    let key = encode(&(
        &q.source,
        &q.agent,
        q.days,
        &q.provider_id,
        &q.model,
        "ready",
    ))
    .unwrap();
    let expected = format!("{:x}:40", Sha256::digest(format!("{key}\0{records}")));
    assert_eq!(first["nextCursor"], expected);
    q.cursor = Some(expected);
    let next = h
        .provider_usage(q, &[], "asof".into(), "ready".into(), now())
        .unwrap();
    assert_eq!(next["calls"].as_array().unwrap().len(), 5);
    assert_eq!(first["summary"], next["summary"]);
}

fn check_usage(h: &mut History, c: &ProjectCatalog) {
    h.sync_native_provider(c, &[]).unwrap();
    h.sync_native_models(c, "claude").unwrap();
    let p = provider_usage::Query {
        source: "native".into(),
        provider_id: Some("_unknown".into()),
        ..Default::default()
    };
    let mut got = h
        .provider_usage(p.clone(), &[], "asof".into(), "ready".into(), now())
        .unwrap();
    let mut expected = value(
        &provider_usage::aggregate(
            p,
            &[],
            provider_usage::native_records(c.calls.clone(), &[]),
            "asof".into(),
            "ready".into(),
            now(),
        )
        .unwrap(),
    )
    .unwrap();
    without_cursor(&mut got);
    without_cursor(&mut expected);
    assert_close(&got, &expected, "blocked provider totals");
    let m = crate::model_usage::UsageQuery {
        source: "native".into(),
        agent: "claude".into(),
        days: None,
        provider: None,
        model: None,
        cursor: None,
    };
    let mut got = h
        .model_usage(m.clone(), "asof".into(), "ready".into(), now())
        .unwrap();
    let mut expected = value(
        &crate::model_usage::aggregate(
            m,
            c.calls.iter().map(crate::sessions::usage_record).collect(),
            "asof".into(),
            "ready".into(),
            now(),
        )
        .unwrap(),
    )
    .unwrap();
    without_cursor(&mut got);
    without_cursor(&mut expected);
    assert_close(
        &got,
        &expected,
        "blocked model totals and latest references",
    );
}

#[test]
fn native_blocks_reuse_untouched_ranges_and_recompute_saturated_totals_after_deletion() {
    let mut c = crate::associated_cost_tests::catalog();
    let sample = c.calls[0].clone();
    c.calls = CallCatalog::shared((0..4).map(|file| {
        Arc::new(
            (0..4096)
                .map(|i| {
                    let mut call = sample.clone();
                    call.agent_id = "claude".into();
                    call.agent_product_id = Some("claude-cli".into());
                    call.id = format!("file-{file}-call-{i:05}");
                    call.session_id = format!("session-{file}");
                    call.response_id = None;
                    call.model = Some(format!("model-{}", i % 23));
                    call.timestamp = if i % 11 == 0 {
                        None
                    } else {
                        Some("2026-10-20T01:00:00Z".into())
                    };
                    call.duration_ms = (i % 3 == 0).then_some(i as u64);
                    call.usage = Some(Usage {
                        total_tokens: Some(if file == 3 && i == 0 { u64::MAX } else { 12 }),
                        input_tokens: Some(10),
                        output_tokens: Some(2),
                        ..Default::default()
                    });
                    call
                })
                .collect(),
        )
    }));
    c.revision += 1;
    let dir = tempfile::tempdir().unwrap();
    let mut h = History::open(dir.path()).unwrap();
    h.refresh(&[]).unwrap();
    check_usage(&mut h, &c);
    h.db.execute_batch("CREATE TEMP TRIGGER provider_stable_block BEFORE DELETE ON native_provider_blocks WHEN old.bucket=0 BEGIN SELECT RAISE(ABORT,'untouched block');END;CREATE TEMP TRIGGER model_stable_block BEFORE DELETE ON native_model_blocks WHEN old.bucket=0 BEGIN SELECT RAISE(ABORT,'untouched block');END;").unwrap();
    Arc::make_mut(&mut c.calls.chunks[3])[1]
        .usage
        .as_mut()
        .unwrap()
        .total_tokens = Some(0);
    c.revision += 1;
    check_usage(&mut h, &c);
    // Removing the saturating block must reveal the remaining precise sum.
    c.calls.chunks.remove(3);
    c.revision += 1;
    check_usage(&mut h, &c);
    let p = h.provider_cache.last().unwrap();
    assert_eq!(p.value["summary"]["usage"]["totalTokens"], 3 * 4096 * 12);
    h.db.execute_batch("DROP TRIGGER provider_stable_block;DROP TRIGGER model_stable_block;")
        .unwrap();
    c.calls.chunks.clear();
    c.revision += 1;
    check_usage(&mut h, &c);
}

#[test]
fn alternating_native_agents_keep_independent_dirty_blocks_and_bounded_ranges() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = History::open(dir.path()).unwrap();
    h.refresh(&[]).unwrap();
    let mut c = crate::associated_cost_tests::catalog();
    let prototype = c.calls[0].clone();
    c.calls = CallCatalog::shared((0..4).map(|file| {
        Arc::new(
            (0..4096)
                .map(|n| {
                    let mut call = prototype.clone();
                    call.id = format!("scope-{file}-{n}");
                    call.agent_id = if file % 2 == 0 { "claude" } else { "codex" }.into();
                    call.agent_product_id = None;
                    call.timestamp = Some((now() - chrono::Duration::hours(1)).to_rfc3339());
                    call.model = Some(format!("model-{}", n % 3));
                    call
                })
                .collect(),
        )
    }));
    let query = |agent: &str, days| crate::model_usage::UsageQuery {
        source: "native".into(),
        agent: agent.into(),
        days,
        provider: None,
        model: None,
        cursor: None,
    };
    h.sync_native_models(&c, "claude").unwrap();
    for agent in ["claude", "codex"] {
        assert_eq!(
            h.model_usage(query(agent, None), "asof".into(), "ready".into(), now())
                .unwrap()["summary"]["attempts"],
            8192
        );
    }
    Arc::make_mut(&mut c.calls.chunks[3])[0]
        .usage
        .as_mut()
        .unwrap()
        .total_tokens = Some(913);
    c.revision += 1;
    h.sync_native_models(&c, "claude").unwrap();
    h.db.execute_batch("CREATE TEMP TRIGGER untouched_scopes BEFORE DELETE ON native_model_blocks WHEN old.bucket=0 BEGIN SELECT RAISE(ABORT,'unchanged range rebuilt');END;").unwrap();
    let costs = h.response_costs().unwrap();
    for agent in ["claude", "codex", "claude", "codex"] {
        let q = query(agent, None);
        let mut got = h
            .model_usage(q.clone(), "asof".into(), "ready".into(), now())
            .unwrap();
        let records = c
            .calls
            .iter()
            .filter(|c| c.agent_id == agent)
            .map(|call| {
                let mut record = crate::sessions::usage_record(call);
                if let Some(cost) = costs.get(&call.id) {
                    record.cost = cost.clone();
                }
                record
            })
            .collect();
        let mut expected = value(
            &crate::model_usage::aggregate(q, records, "asof".into(), "ready".into(), now())
                .unwrap(),
        )
        .unwrap();
        without_cursor(&mut got);
        without_cursor(&mut expected);
        assert_close(&got, &expected, "alternating agent dirty blocks");
    }
    h.db.execute_batch("DROP TRIGGER untouched_scopes").unwrap();
    for (agent, days) in ["claude", "codex", "opencode"]
        .into_iter()
        .flat_map(|agent| [None, Some(1), Some(7), Some(30)].map(|days| (agent, days)))
    {
        h.model_usage(query(agent, days), "asof".into(), "ready".into(), now())
            .unwrap();
        assert!(
            h.db.query_row("SELECT count(*) FROM native_model_blocks_clock", [], |r| {
                r.get::<_, i64>(0)
            })
            .unwrap()
                <= 8
        );
        assert_eq!(h.db.query_row("SELECT count(*) FROM native_model_blocks b WHERE NOT EXISTS(SELECT 1 FROM native_model_blocks_clock c WHERE c.scope=b.scope)",[],|r|r.get::<_,i64>(0)).unwrap(),0);
    }
}

#[test]
fn native_block_failure_rolls_back_dirty_markers_and_retry_rebuilds_both_kinds() {
    let mut c = crate::associated_cost_tests::catalog();
    let dir = tempfile::tempdir().unwrap();
    let mut h = History::open(dir.path()).unwrap();
    h.refresh(&[]).unwrap();
    check_usage(&mut h, &c);
    c.calls[0].usage.as_mut().unwrap().total_tokens = Some(111);
    c.revision += 1;
    h.sync_native_provider(&c, &[]).unwrap();
    h.sync_native_models(&c, "claude").unwrap();
    for kind in ["provider", "model"] {
        h.usage_facts(kind, true).unwrap();
        let before: String =
            h.db.query_row(
                &format!("SELECT coalesce(group_concat(stats),'') FROM native_{kind}_blocks"),
                [],
                |r| r.get(0),
            )
            .unwrap();
        h.db.execute_batch(&format!("CREATE TEMP TRIGGER block_fail BEFORE INSERT ON native_{kind}_blocks BEGIN SELECT RAISE(ABORT,'fixture');END;")).unwrap();
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
        if kind == "provider" {
            assert!(
                h.provider_usage(p, &[], "asof".into(), "ready".into(), now())
                    .is_err()
            );
        } else {
            assert!(
                h.model_usage(m, "asof".into(), "ready".into(), now())
                    .is_err()
            );
        }
        let after: String =
            h.db.query_row(
                &format!("SELECT coalesce(group_concat(stats),'') FROM native_{kind}_blocks"),
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(before, after);
        assert!(
            h.db.query_row(
                &format!("SELECT count(*) FROM native_{kind}_blocks_dirty"),
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap()
                > 0
        );
        h.db.execute_batch("DROP TRIGGER block_fail").unwrap();
    }
    check_usage(&mut h, &c);
}

#[test]
fn native_block_ranges_expire_on_fractional_edges_and_clock_rollback() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = History::open(dir.path()).unwrap();
    h.refresh(&[]).unwrap();
    let mut records = provider_usage::gateway_records((0..55).map(row).collect(), &[]);
    for (i, r) in records.iter_mut().enumerate() {
        r.cost = crate::provider_cost::Cost::unknown("native_usage");
        r.time = match i {
            0 => Some("2026-10-20T03:00:00.000000300Z".into()),
            1 => Some("2026-10-21T03:00:00.000000500Z".into()),
            2 => None,
            _ => Some("2026-10-21T02:00:00Z".into()),
        };
        r.model = Some(format!("model-{}", i % 23));
    }
    let models = || {
        records
            .iter()
            .map(|r| crate::model_usage::UsageRecord {
                cost: r.cost.clone(),
                id: r.id.clone(),
                model: r.model.clone(),
                provider: r.provider.clone(),
                endpoint: None,
                route_revision: None,
                time: r.time.clone(),
                usage: r.usage.clone(),
                session_id: r.session_id.clone(),
                request_id: r.request_id.clone(),
            })
            .collect::<Vec<_>>()
    };
    h.set_native_usage(records.clone(), models()).unwrap();
    for nanos in [200, 600, 400, 200] {
        let at = DateTime::parse_from_rfc3339(&format!("2026-10-21T03:00:00.000000{nanos}Z"))
            .unwrap()
            .with_timezone(&Utc);
        for days in [Some(1), None] {
            let p = provider_usage::Query {
                source: "native".into(),
                provider_id: Some("_unknown".into()),
                days,
                ..Default::default()
            };
            let mut got = h
                .provider_usage(p.clone(), &[], "asof".into(), "ready".into(), at)
                .unwrap();
            let mut expected = value(
                &provider_usage::aggregate(
                    p,
                    &[],
                    records.clone(),
                    "asof".into(),
                    "ready".into(),
                    at,
                )
                .unwrap(),
            )
            .unwrap();
            without_cursor(&mut got);
            without_cursor(&mut expected);
            assert_close(&got, &expected, "native range boundaries");
            let m = crate::model_usage::UsageQuery {
                source: "native".into(),
                agent: "claude".into(),
                days,
                provider: None,
                model: None,
                cursor: None,
            };
            let mut got = h
                .model_usage(m.clone(), "asof".into(), "ready".into(), at)
                .unwrap();
            let mut expected = value(
                &crate::model_usage::aggregate(m, models(), "asof".into(), "ready".into(), at)
                    .unwrap(),
            )
            .unwrap();
            without_cursor(&mut got);
            without_cursor(&mut expected);
            assert_close(&got, &expected, "native model range boundaries");
        }
    }
}

#[test]
fn native_provider_cursor_hashing_is_block_bounded_and_still_covers_every_visible_record() {
    use rusqlite::functions::{Aggregate, Context, FunctionFlags};
    use sha2::{Digest, Sha256};
    struct BoundedHash;
    impl Aggregate<(usize, Sha256), String> for BoundedHash {
        fn init(&self, _: &mut Context<'_>) -> rusqlite::Result<(usize, Sha256)> {
            Ok((0, Sha256::new()))
        }
        fn step(&self, c: &mut Context<'_>, s: &mut (usize, Sha256)) -> rusqlite::Result<()> {
            s.0 += 1;
            if s.0 > 4096 {
                return Err(rusqlite::Error::InvalidQuery);
            }
            let raw = c.get_raw(0).as_str()?;
            s.1.update((raw.len() as u64).to_le_bytes());
            s.1.update(raw.as_bytes());
            Ok(())
        }
        fn finalize(
            &self,
            _: &mut Context<'_>,
            s: Option<(usize, Sha256)>,
        ) -> rusqlite::Result<String> {
            Ok(format!(
                "{:x}",
                s.unwrap_or((0, Sha256::new())).1.finalize()
            ))
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let mut h = History::open(dir.path()).unwrap();
    h.refresh(&[]).unwrap();
    let mut records = provider_usage::gateway_records((0..16385).map(row).collect(), &[]);
    for (i, r) in records.iter_mut().enumerate() {
        r.model = Some(if i < 16380 { "selected" } else { "other" }.into());
        r.cost = crate::provider_cost::Cost::unknown("native_usage");
    }
    h.set_native_usage(records.clone(), vec![]).unwrap();
    h.db.create_aggregate_function("hash_rows", 1, FunctionFlags::SQLITE_UTF8, BoundedHash)
        .unwrap();
    let mut q = provider_usage::Query {
        source: "native".into(),
        provider_id: Some("_unknown".into()),
        model: Some("selected".into()),
        ..Default::default()
    };
    let mut first = h
        .provider_usage(q.clone(), &[], "asof".into(), "ready".into(), now())
        .unwrap();
    let mut expected = value(
        &provider_usage::aggregate(
            q.clone(),
            &[],
            records.clone(),
            "asof".into(),
            "ready".into(),
            now(),
        )
        .unwrap(),
    )
    .unwrap();
    q.cursor = first["nextCursor"].as_str().map(str::to_owned);
    without_cursor(&mut first);
    without_cursor(&mut expected);
    assert_close(&first, &expected, "block provider digest");
    // A nonvisible mutation must keep this cursor, including on the block edge.
    records[16384].provider = Some("unselected mutation".into());
    h.set_native_usage(records.clone(), vec![]).unwrap();
    assert!(
        h.provider_usage(q.clone(), &[], "asof".into(), "ready".into(), now())
            .is_ok()
    );
    records[8193].provider = Some("visible mutation on an offscreen page".into());
    h.set_native_usage(records, vec![]).unwrap();
    assert!(
        h.provider_usage(q, &[], "asof".into(), "ready".into(), now())
            .is_err()
    );
}

#[test]
fn projected_provider_digest_changes_for_visible_fields_but_keeps_unselected_cursors() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = History::open(dir.path()).unwrap();
    h.refresh(&[]).unwrap();
    let mut records = provider_usage::gateway_records((0..55).map(row).collect(), &[]);
    for (i, r) in records.iter_mut().enumerate() {
        r.model = Some(if i < 45 { "selected" } else { "other" }.into());
        r.cost = crate::provider_cost::Cost::unknown("native_usage");
    }
    h.set_native_usage(records.clone(), vec![]).unwrap();
    let mut q = provider_usage::Query {
        source: "native".into(),
        provider_id: Some("_unknown".into()),
        model: Some("selected".into()),
        ..Default::default()
    };
    let first = h
        .provider_usage(q.clone(), &[], "asof".into(), "ready".into(), now())
        .unwrap();
    q.cursor = first["nextCursor"].as_str().map(str::to_owned);
    assert!(q.cursor.is_some());
    records[50].provider = Some("outside filter".into());
    h.set_native_usage(records.clone(), vec![]).unwrap();
    let second = h
        .provider_usage(q.clone(), &[], "asof".into(), "ready".into(), now())
        .unwrap();
    assert_eq!(second["calls"].as_array().unwrap().len(), 5);
    assert_eq!(first["selected"], second["selected"]);
    records[0].provider = Some("visible field changed".into());
    h.set_native_usage(records, vec![]).unwrap();
    assert!(
        h.provider_usage(q, &[], "asof".into(), "ready".into(), now())
            .is_err()
    );
}

#[test]
fn native_charges_keep_sequential_money_order_across_block_boundaries() {
    let rows = (0..8200).map(row).collect::<Vec<_>>();
    let mut providers = provider_usage::gateway_records(rows.clone(), &[]);
    let mut models = model_rows(&rows);
    for i in 0..rows.len() {
        let mut cost = crate::provider_cost::Cost::unknown("native_usage");
        if [0, 4096, 8192].contains(&i) {
            cost.kind = "actual";
            cost.currency = Some("USD");
            cost.amount = Some(if i == 0 { 1e16 } else { 1.0 });
            cost.reason = None;
        }
        providers[i].cost = cost.clone();
        models[i].cost = cost;
    }
    let dir = tempfile::tempdir().unwrap();
    let mut h = History::open(dir.path()).unwrap();
    h.refresh(&[]).unwrap();
    h.set_native_usage(providers, models).unwrap();
    let p = h
        .provider_usage(
            provider_usage::Query {
                source: "native".into(),
                provider_id: Some("_unknown".into()),
                ..Default::default()
            },
            &[],
            "asof".into(),
            "ready".into(),
            now(),
        )
        .unwrap();
    let m = h
        .model_usage(
            crate::model_usage::UsageQuery {
                source: "native".into(),
                agent: "claude".into(),
                days: None,
                provider: None,
                model: None,
                cursor: None,
            },
            "asof".into(),
            "ready".into(),
            now(),
        )
        .unwrap();
    assert_eq!(p["summary"]["cost"]["actual"][0]["amount"], 1e16);
    assert_eq!(m["summary"]["cost"]["actual"][0]["amount"], 1e16);
    assert_eq!(m["items"][0]["totals"]["cost"]["actual"][0]["amount"], 1e16);
}

#[test]
fn releasing_native_snapshots_drops_shared_records_and_rebuilds_without_losing_gateway_history() {
    let c = crate::associated_cost_tests::catalog();
    let weak = Arc::downgrade(&c.calls.chunks[0]);
    let dir = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(dir.path()).unwrap();
    let gateway = row(17);
    store.save(&gateway).unwrap();
    let mut h = History::open(dir.path()).unwrap();
    h.refresh(&[]).unwrap();
    check(&mut h, &c, "claude", &[]);
    assert!(weak.strong_count() >= 6);
    h.release_native_snapshots();
    assert_eq!(weak.strong_count(), 1);
    assert_eq!(
        h.db.query_row("SELECT raw FROM attempts WHERE id=?1", [&gateway.id], |r| r
            .get::<_, String>(0))
            .unwrap(),
        serde_json::to_string(&gateway).unwrap()
    );
    check(&mut h, &c, "claude", &[]);
    h.release_native_snapshots();
    drop(c);
    assert!(weak.upgrade().is_none());
}

#[test]
fn native_consumers_keep_the_other_pages_cache_and_projection_valid() {
    let c = crate::associated_cost_tests::catalog();
    let dir = tempfile::tempdir().unwrap();
    let mut h = History::open(dir.path()).unwrap();
    h.refresh(&[]).unwrap();
    h.sync_native_provider(&c, &[]).unwrap();
    let p = provider_usage::Query {
        source: "native".into(),
        provider_id: Some("_unknown".into()),
        ..Default::default()
    };
    let first = h
        .provider_usage(p.clone(), &[], "asof".into(), "ready".into(), now())
        .unwrap();
    let clock = h.usage_kind_clock("provider", true).unwrap();
    h.db.execute_batch("DROP VIEW provider_work;CREATE TEMP VIEW provider_work AS SELECT * FROM native_provider_facts WHERE 0;").unwrap();
    h.sync_native_models(&c, "claude").unwrap();
    assert_eq!(h.usage_kind_clock("provider", true).unwrap(), clock);
    assert_eq!(
        h.provider_usage(p, &[], "asof".into(), "ready".into(), now())
            .unwrap(),
        first
    );
    let m = crate::model_usage::UsageQuery {
        source: "native".into(),
        agent: "claude".into(),
        days: None,
        provider: None,
        model: None,
        cursor: None,
    };
    let first = h
        .model_usage(m.clone(), "asof".into(), "ready".into(), now())
        .unwrap();
    let clock = h.usage_kind_clock("model", true).unwrap();
    h.db.execute("DELETE FROM model_groups", []).unwrap();
    let evidence = provider_usage::Evidence {
        agent: "claude".into(),
        provider_id: "retained".into(),
        provider_name: "retained".into(),
        endpoint: "https://fixture.invalid".into(),
        protocol: "messages".into(),
        revision: 1,
        native_provider: None,
        model_aliases: vec![("deepseek-flash".into(), "alias".into())],
    };
    h.sync_native_provider(&c, &[evidence]).unwrap();
    assert_eq!(h.usage_kind_clock("model", true).unwrap(), clock);
    assert_eq!(
        h.model_usage(m, "asof".into(), "ready".into(), now())
            .unwrap(),
        first
    );
    assert_eq!(
        h.db.query_row("SELECT count(*) FROM model_groups", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
}

fn check(h: &mut History, c: &ProjectCatalog, agent: &str, evidence: &[provider_usage::Evidence]) {
    h.sync_native_provider(c, evidence).unwrap();
    h.sync_native_models(c, agent).unwrap();
    h.sync_native_analysis(c).unwrap();
    h.sync_native_catalog(c.revision.to_string(), &HashMap::new(), &c.calls)
        .unwrap();
    let native = requests::sources::native_records(c.calls.clone(), &HashMap::new());
    for n in &native {
        assert_close(
            &h.request_detail(&n.id).unwrap().unwrap(),
            &value(n).unwrap(),
            "native request delta detail",
        );
    }
    let report = h
        .source_report(&requests::Query::default(), now(), vec![], None)
        .unwrap();
    assert_close(
        &report["native"],
        &value(&requests::Summary::of(native.iter())).unwrap(),
        "native request delta totals",
    );
    let p = provider_usage::Query {
        source: "native".into(),
        provider_id: Some("_unknown".into()),
        ..Default::default()
    };
    let mut got = h
        .provider_usage(p.clone(), &[], "asof".into(), "ready".into(), now())
        .unwrap();
    let mut expected = value(
        &provider_usage::aggregate(
            p,
            &[],
            provider_usage::native_records(c.calls.clone(), evidence),
            "asof".into(),
            "ready".into(),
            now(),
        )
        .unwrap(),
    )
    .unwrap();
    without_cursor(&mut got);
    without_cursor(&mut expected);
    assert_close(&got, &expected, "native provider batches");
    let m = crate::model_usage::UsageQuery {
        agent: agent.into(),
        source: "native".into(),
        days: None,
        provider: None,
        model: None,
        cursor: None,
    };
    let costs = h.response_costs().unwrap();
    let rows = c
        .calls
        .iter()
        .filter(|c| {
            c.kind == "model"
                && crate::agents::matches(Some(agent), &c.agent_id, c.agent_product_id.as_deref())
        })
        .map(|c| {
            let mut row = crate::sessions::usage_record(c);
            if let Some(cost) = costs.get(&row.id) {
                row.cost = cost.clone();
            }
            row
        })
        .collect();
    let mut got = h
        .model_usage(m.clone(), "asof".into(), "ready".into(), now())
        .unwrap();
    let mut expected = value(
        &crate::model_usage::aggregate(m, rows, "asof".into(), "ready".into(), now()).unwrap(),
    )
    .unwrap();
    without_cursor(&mut got);
    without_cursor(&mut expected);
    assert_close(&got, &expected, "native model batches and tied references");
    let ids: Vec<_> = c.sessions.iter().map(|s| s.id.clone()).collect();
    let oracle = crate::associated_cost::Index::new(c, vec![], false);
    assert_close(
        &h.fee_summary(&ids, None, false).unwrap(),
        &value(&oracle.summary(&ids, None)).unwrap(),
        "native fee delta",
    );
    let counts:(i64,i64)=h.db.query_row("SELECT count(*),coalesce(sum(CAST(tokens AS INTEGER)),0) FROM analyses WHERE kind='native'",[],|r|Ok((r.get(0)?,r.get(1)?))).unwrap();
    assert_eq!(
        counts.0,
        c.calls.iter().filter(|c| c.kind == "model").count() as i64
    );
    assert_eq!(
        counts.1,
        c.calls
            .iter()
            .filter(|c| c.kind == "model")
            .filter_map(|c| c.usage.as_ref().and_then(|u| u.total_tokens))
            .sum::<u64>() as i64
    );
}

#[test]
fn native_batch_deltas_match_oracles_across_append_rewrite_delete_revoke_and_scope_changes() {
    let mut c = crate::associated_cost_tests::catalog();
    let original = c.clone();
    let dir = tempfile::tempdir().unwrap();
    let mut h = History::open(dir.path()).unwrap();
    h.refresh(&[]).unwrap();
    check(&mut h, &c, "claude", &[]);
    assert_eq!(h.sync_native_provider(&c, &[]).unwrap(), 0);
    assert_eq!(h.sync_native_models(&c, "claude").unwrap(), 0);
    let stable_id = c.calls.chunks[1][0].id.clone();
    let escaped = stable_id.replace('\'', "''");
    h.db.execute_batch(&format!("CREATE TEMP TRIGGER untouched_usage BEFORE INSERT ON native_provider_usage WHEN new.id='{escaped}' BEGIN SELECT RAISE(ABORT,'unchanged file rewritten');END;CREATE TEMP TRIGGER untouched_model BEFORE INSERT ON native_model_usage WHEN new.id='{escaped}' BEGIN SELECT RAISE(ABORT,'unchanged file rewritten');END;CREATE TEMP TRIGGER untouched_fee BEFORE INSERT ON fee_native WHEN new.id='{escaped}' BEGIN SELECT RAISE(ABORT,'unchanged fee rewritten');END;CREATE TEMP TRIGGER untouched_analysis BEFORE INSERT ON analyses WHEN new.id='native-{escaped}' BEGIN SELECT RAISE(ABORT,'unchanged analysis rewritten');END;")).unwrap();
    let mut extra = c.calls[0].clone();
    extra.id = "extra".into();
    extra.response_id = Some("extra".into());
    extra.timestamp = None;
    Arc::make_mut(&mut c.calls.chunks[0]).push(extra);
    c.revision += 1;
    assert_eq!(h.sync_native_provider(&c, &[]).unwrap(), 1);
    assert_eq!(h.sync_native_models(&c, "claude").unwrap(), 1);
    check(&mut h, &c, "claude", &[]);
    c.calls[0].usage = None;
    c.calls[1].usage.as_mut().unwrap().total_tokens = Some(0);
    c.revision += 1;
    check(&mut h, &c, "claude", &[]);
    h.db.execute_batch(
        "DROP TRIGGER untouched_usage;DROP TRIGGER untouched_model;DROP TRIGGER untouched_fee;DROP TRIGGER untouched_analysis;",
    )
    .unwrap();
    c.calls.chunks.remove(0);
    c.sessions.remove(0);
    c.revision += 1;
    check(&mut h, &c, "claude", &[]);
    check(&mut h, &c, "claude-code-desktop", &[]);
    check(&mut h, &c, "claude-code-cli", &[]);
    c.calls.clear();
    c.sessions.clear();
    c.revision += 1;
    check(&mut h, &c, "claude", &[]);
    c = original;
    c.revision += 100;
    check(&mut h, &c, "claude", &[]);
    let evidence = provider_usage::Evidence {
        agent: "claude".into(),
        provider_id: "retained".into(),
        provider_name: "retained".into(),
        endpoint: "https://fixture.invalid".into(),
        protocol: "messages".into(),
        revision: 1,
        native_provider: None,
        model_aliases: vec![("deepseek-flash".into(), "aliased".into())],
    };
    check(&mut h, &c, "claude", &[evidence]);
    check(&mut h, &c, "claude", &[]);
}

#[test]
fn failed_native_delta_rolls_back_rows_clocks_and_shared_snapshot_before_retry() {
    let mut c = crate::associated_cost_tests::catalog();
    let dir = tempfile::tempdir().unwrap();
    let mut h = History::open(dir.path()).unwrap();
    h.refresh(&[]).unwrap();
    check(&mut h, &c, "claude", &[]);
    c.calls[0].model = Some("changed".into());
    c.calls[0].usage.as_mut().unwrap().input_tokens = Some(20);
    c.calls[0].usage.as_mut().unwrap().total_tokens = Some(25);
    c.revision += 1;
    let before = aggregates::list(
        &h.db,
        "SELECT json_array(p.id,p.provider_record,m.model_record) FROM native_provider_usage p LEFT JOIN native_model_usage m USING(id) ORDER BY p.id",
    )
    .unwrap();
    let clock = h.usage_clock(true).unwrap();
    h.db.execute_batch("CREATE TEMP TRIGGER fail_delta BEFORE INSERT ON native_provider_usage BEGIN SELECT RAISE(ABORT,'injected');END;").unwrap();
    assert!(h.sync_native_provider(&c, &[]).is_err());
    assert_eq!(h.usage_clock(true).unwrap(), clock);
    assert_eq!(
        aggregates::list(
            &h.db,
            "SELECT json_array(p.id,p.provider_record,m.model_record) FROM native_provider_usage p LEFT JOIN native_model_usage m USING(id) ORDER BY p.id"
        )
        .unwrap(),
        before
    );
    h.db.execute_batch("DROP TRIGGER fail_delta;CREATE TEMP TRIGGER fail_fee BEFORE INSERT ON fee_native BEGIN SELECT RAISE(ABORT,'injected');END;").unwrap();
    let epoch = h.fee_epoch().unwrap();
    assert!(h.prepare_fees(&c).is_err());
    assert_eq!(h.fee_epoch().unwrap(), epoch);
    h.db.execute_batch("DROP TRIGGER fail_fee;CREATE TEMP TRIGGER fail_analysis BEFORE INSERT ON analyses WHEN new.kind='native' BEGIN SELECT RAISE(ABORT,'injected');END;").unwrap();
    assert!(h.sync_native_analysis(&c).is_err());
    assert_eq!(h.native_analysis_revision, Some(c.revision - 1));
    h.db.execute_batch("DROP TRIGGER fail_analysis").unwrap();
    h.db.execute_batch("CREATE TEMP TRIGGER fail_request_delta BEFORE INSERT ON native_base BEGIN SELECT RAISE(ABORT,'injected');END;").unwrap();
    let revision = h.native_revision.clone();
    assert!(
        h.sync_native_catalog(c.revision.to_string(), &HashMap::new(), &c.calls)
            .is_err()
    );
    assert_eq!(h.native_revision, revision);
    assert_ne!(
        h.request_detail(&c.calls[0].id).unwrap().unwrap()["model"],
        "changed"
    );
    h.db.execute_batch("DROP TRIGGER fail_request_delta")
        .unwrap();
    check(&mut h, &c, "claude", &[]);
    // The legacy replacement path also invalidates both native scope caches.
    h.set_native_usage(vec![], vec![]).unwrap();
    check(&mut h, &c, "claude", &[]);
    c.calls[0].model = Some("model-retry".into());
    c.revision += 1;
    let before = aggregates::list(
        &h.db,
        "SELECT json_array(id,model_record) FROM native_model_usage ORDER BY id",
    )
    .unwrap();
    let clock = h.usage_kind_clock("model", true).unwrap();
    h.db.execute_batch("CREATE TEMP TRIGGER fail_model_delta BEFORE INSERT ON native_model_usage BEGIN SELECT RAISE(ABORT,'injected');END;").unwrap();
    assert!(h.sync_native_models(&c, "claude").is_err());
    assert_eq!(h.usage_kind_clock("model", true).unwrap(), clock);
    assert_eq!(
        aggregates::list(
            &h.db,
            "SELECT json_array(id,model_record) FROM native_model_usage ORDER BY id"
        )
        .unwrap(),
        before
    );
    h.db.execute_batch("DROP TRIGGER fail_model_delta").unwrap();
    assert_eq!(h.sync_native_models(&c, "claude").unwrap(), 1);
    check(&mut h, &c, "claude", &[]);
}

#[test]
fn native_equal_time_and_missing_time_references_follow_catalog_order_after_file_addition() {
    let mut c = crate::associated_cost_tests::catalog();
    let mut a = c.calls[0].clone();
    a.session_id = "aaa".into();
    a.id = "aaa".into();
    c.calls = CallCatalog::shared(
        std::iter::once(Arc::new(vec![a])).chain(c.calls.chunks.iter().cloned()),
    );
    let dir = tempfile::tempdir().unwrap();
    let mut h = History::open(dir.path()).unwrap();
    h.refresh(&[]).unwrap();
    check(&mut h, &c, "claude", &[]);
    for call in &mut c.calls {
        call.timestamp = None;
    }
    c.revision += 1;
    check(&mut h, &c, "claude", &[]);
    c.calls.chunks.remove(0);
    c.revision += 1;
    check(&mut h, &c, "claude", &[]);
}

#[test]
fn native_model_cache_rechecks_late_gateway_costs_conflicts_and_deletion_without_a_scan() {
    let c = crate::associated_cost_tests::catalog();
    let dir = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(dir.path()).unwrap();
    let mut h = History::open(dir.path()).unwrap();
    let mut rows = vec![];
    for phase in 0..4 {
        if phase == 1 {
            let mut r = row(1);
            r.agent_product_id = c.calls[0].agent_product_id.clone();
            r.response_id = c.calls[0].response_id.clone();
            r.usage = c.calls[0].usage.clone().unwrap();
            r.endpoint = "https://openrouter.ai/api/v1".into();
            r.reported_cost = Some(
                serde_json::from_value(
                    json!({"amount":0,"currency":"USD","source":"openrouter.usage.cost"}),
                )
                .unwrap(),
            );
            rows.push(r);
        }
        if phase == 2 {
            rows[0].usage.total_tokens = Some(999);
            rows[0].revision += 1;
        }
        if phase == 3 {
            store
                .0
                .execute("DELETE FROM model_observations", [])
                .unwrap();
            rows.clear();
        }
        store.save_batch(&rows).unwrap();
        h.refresh(&[]).unwrap();
        h.sync_native_models(&c, "claude").unwrap();
        let oracle = crate::associated_cost::Index::new(&c, rows.clone(), false);
        let records = c
            .calls
            .iter()
            .map(|call| {
                let mut r = crate::sessions::usage_record(call);
                r.cost = oracle.response_cost(&call.id);
                r
            })
            .collect();
        let q = crate::model_usage::UsageQuery {
            source: "native".into(),
            agent: "claude".into(),
            days: None,
            provider: None,
            model: None,
            cursor: None,
        };
        let mut got = h
            .model_usage(q.clone(), "asof".into(), "ready".into(), now())
            .unwrap();
        let mut expected = value(
            &crate::model_usage::aggregate(q, records, "asof".into(), "ready".into(), now())
                .unwrap(),
        )
        .unwrap();
        without_cursor(&mut got);
        without_cursor(&mut expected);
        assert_close(
            &got,
            &expected,
            "native model late gateway cost invalidation",
        );
        assert_eq!(
            got["summary"]["cost"]["unknownCalls"],
            if phase == 1 { 1 } else { 2 }
        );
    }
}

#[test]
fn native_provider_empty_timing_evidence_skips_full_scans_and_real_samples_still_match() {
    use rusqlite::functions::FunctionFlags;
    use std::sync::atomic::{AtomicUsize, Ordering};
    let dir = tempfile::tempdir().unwrap();
    let mut h = History::open(dir.path()).unwrap();
    h.refresh(&[]).unwrap();
    let mut c = crate::associated_cost_tests::catalog();
    let prototype = c.calls[0].clone();
    c.calls = (0..16384)
        .map(|n| {
            let mut call = prototype.clone();
            call.id = format!("no-timing-{n:05}");
            call.model = Some(format!("model-{}", n % 7));
            call.timestamp = Some((now() - chrono::Duration::seconds(1)).to_rfc3339());
            call
        })
        .collect();
    h.sync_native_provider(&c, &[]).unwrap();
    let mut q = provider_usage::Query {
        source: "native".into(),
        agent: Some("claude".into()),
        provider_id: Some("_unknown".into()),
        ..Default::default()
    };
    h.provider_usage(q.clone(), &[], "asof".into(), "ready".into(), now())
        .unwrap();
    h.provider_cache.clear();
    let reads = Arc::new(AtomicUsize::new(0));
    let measured = reads.clone();
    h.db.create_scalar_function("agent_matches", 3, FunctionFlags::SQLITE_UTF8, move |ctx| {
        if measured.fetch_add(1, Ordering::SeqCst) >= 512 {
            return Err(rusqlite::Error::InvalidQuery);
        }
        let agent: Option<String> = ctx.get(0)?;
        let family: String = ctx.get(1)?;
        let product: Option<String> = ctx.get(2)?;
        Ok(crate::agents::matches(
            agent.as_deref(),
            &family,
            product.as_deref(),
        ))
    })
    .unwrap();
    let records = provider_usage::native_records(c.calls.clone(), &[]);
    let mut got = h
        .provider_usage(q.clone(), &[], "asof".into(), "ready".into(), now())
        .unwrap();
    let mut expected = value(
        &provider_usage::aggregate(
            q.clone(),
            &[],
            records.clone(),
            "asof".into(),
            "ready".into(),
            now(),
        )
        .unwrap(),
    )
    .unwrap();
    without_cursor(&mut got);
    without_cursor(&mut expected);
    assert_close(&got, &expected, "empty timing evidence");
    assert!(reads.load(Ordering::SeqCst) <= 512);
    aggregates::register(&h.db).unwrap();
    let mut timed = records;
    for (n, r) in timed.iter_mut().take(2).enumerate() {
        r.status = "success".into();
        r.duration_ms = Some((n + 1) as f64 * 10.0);
        r.ttft_ms = Some(n as f64);
    }
    h.set_native_usage(timed.clone(), vec![]).unwrap();
    for model in [None, Some("model-0".into()), Some("model-6".into())] {
        q.model = model;
        let mut got = h
            .provider_usage(q.clone(), &[], "asof".into(), "ready".into(), now())
            .unwrap();
        let mut expected = value(
            &provider_usage::aggregate(
                q.clone(),
                &[],
                timed.clone(),
                "asof".into(),
                "ready".into(),
                now(),
            )
            .unwrap(),
        )
        .unwrap();
        without_cursor(&mut got);
        without_cursor(&mut expected);
        assert_close(&got, &expected, "real native timing evidence");
    }
}

#[test]
fn fee_scope_failure_restores_previous_selection_and_retry_preserves_money_order() {
    let dir = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(dir.path()).unwrap();
    let c = crate::associated_cost_tests::catalog();
    let ids: Vec<_> = c.sessions.iter().map(|s| s.id.clone()).collect();
    for (n, amount) in [1e16, 1.0, -1e16, 0.0].into_iter().enumerate() {
        let mut r = row(n);
        r.session_id = Some(ids[n % ids.len()].clone());
        r.endpoint = "https://openrouter.ai/api/v1".into();
        r.reported_cost = Some(
            serde_json::from_value(
                json!({"amount":amount,"currency":"USD","source":"openrouter.usage.cost"}),
            )
            .unwrap(),
        );
        store.save(&r).unwrap();
    }
    let mut h = History::open(dir.path()).unwrap();
    h.refresh(&[]).unwrap();
    h.prepare_fees(&c).unwrap();
    h.fee_summary(&ids[..1], None, false).unwrap();
    let before: Vec<Value> = aggregates::list(
        &h.db,
        "SELECT json_quote(id) FROM selected_sessions ORDER BY id",
    )
    .unwrap();
    h.db.execute_batch(&format!("CREATE TEMP TRIGGER reject_selected BEFORE INSERT ON selected_sessions WHEN new.id={} BEGIN SELECT RAISE(ABORT,'scope failure');END;",aggregates::quote(&ids[1]))).unwrap();
    assert!(h.fee_summary(&ids, None, false).is_err());
    assert_eq!(
        aggregates::list(
            &h.db,
            "SELECT json_quote(id) FROM selected_sessions ORDER BY id"
        )
        .unwrap(),
        before
    );
    assert!(h.db.is_autocommit());
    h.db.execute_batch("DROP TRIGGER reject_selected").unwrap();
    let all = h.fee_summary(&ids, None, false).unwrap();
    let rows = store.load().unwrap();
    let expected = crate::associated_cost::Index::new(&c, rows, false).summary(&ids, None);
    assert_close(
        &all,
        &value(&expected).unwrap(),
        "scope retry keeps original monetary order",
    );
}
