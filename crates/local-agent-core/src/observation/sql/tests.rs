use super::*;
use chrono::{DateTime, Utc};
#[path = "export_tests.rs"]
mod export_tests;
#[path = "native_bulk_tests.rs"]
mod native_bulk_tests;
#[path = "native_usage_tests.rs"]
mod native_usage_tests;
#[path = "overview_cache_tests.rs"]
mod overview_cache_tests;
#[path = "provider_model_tests.rs"]
mod provider_model_tests;
#[path = "request_cells_tests.rs"]
mod request_cells_tests;

#[test]
fn custom_directory_updates_both_sql_matchers_and_preserves_unknown_semantics() {
    use crate::agents::{AgentDirectory, ProductDescriptor, SourceDescriptor};
    let data = tempfile::tempdir().unwrap();
    let mut history = History::open(data.path()).unwrap();
    for shared in [false, true] {
        let directory = AgentDirectory::new(
            vec![SourceDescriptor {
                id: "fixture".into(),
                name: "Fixture".into(),
                shared,
            }],
            vec![ProductDescriptor {
                id: "fixture-reader".into(),
                family: "fixture".into(),
                config_id: "fixture".into(),
                name: "Fixture Reader".into(),
                locations: vec![],
            }],
        )
        .unwrap();
        history.use_directory(&directory).unwrap();
        let (unknown, group): (bool, bool) = history.db.query_row("SELECT agent_matches('unknown','fixture',NULL),agent_group_matches('fixture','fixture-reader')", [], |r| Ok((r.get(0)?,r.get(1)?))).unwrap();
        assert_eq!(unknown, shared);
        assert!(group);
    }
}
#[test]
fn cold_projection_matches_delta_path_with_retries_reserved_ids_and_live_overlay() {
    let dir = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(dir.path()).unwrap();
    let mut cold = History::open(dir.path()).unwrap();
    let mut delta = History::open(dir.path()).unwrap();
    delta.watermark = Some(0);
    let mut rows: Vec<_> = (0..80).map(row).collect();
    rows[0].logical_id = Some("retry".into());
    rows[1].logical_id = Some("retry".into());
    rows[1].agent_product_id = rows[0].agent_product_id.clone();
    rows[1].status = "error".into();
    rows[2].id = requests::logical_request_key(&rows[0]);
    rows[3].status = "in_progress".into();
    rows[4].session_id = Some("native-session".into());
    rows[5].started_at = "invalid".into();
    rows[6].usage = Usage::default();
    rows[7].response_id = Some("native-response".into());
    store.save_batch(&rows).unwrap();
    store
        .0
        .execute(
            "INSERT INTO model_observations VALUES('broken','bad',1,'{bad')",
            [],
        )
        .unwrap();
    let mut live = rows[3].clone();
    live.revision += 1;
    live.usage.total_tokens = Some(777);
    let pending = HashSet::from([live.id.clone()]);
    for h in [&mut cold, &mut delta] {
        h.refresh_pending(std::slice::from_ref(&live), &pending)
            .unwrap();
    }
    let projection = |h: &History| {
        aggregates::list(
            &h.db,
            "SELECT value FROM records r JOIN current c ON c.v=r.v ORDER BY r.kind,r.id",
        )
        .unwrap()
    };
    assert_eq!(projection(&cold), projection(&delta));
    assert_eq!(cold.invalid, 1);
    for q in [
        requests::Query::default(),
        requests::Query {
            metric: Some("tokens".into()),
            dimension: Some("model".into()),
            ..Default::default()
        },
    ] {
        assert_close(
            &cold.source_report(&q, now(), vec![], None).unwrap(),
            &delta.source_report(&q, now(), vec![], None).unwrap(),
            "cold/delta source report",
        );
    }
    // A late retry forces the existing projection onto the normal group path.
    let mut late = rows[0].clone();
    late.id = "late".into();
    late.revision += 1;
    store.save(&late).unwrap();
    cold.refresh(&[]).unwrap();
    delta.refresh(&[]).unwrap();
    assert_eq!(projection(&cold), projection(&delta));
}

#[test]
fn failed_cold_load_rolls_back_data_and_deferred_indexes_before_retry() {
    let dir = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(dir.path()).unwrap();
    store
        .save_batch(&(0..20).map(row).collect::<Vec<_>>())
        .unwrap();
    let mut sql = History::open(dir.path()).unwrap();
    let schema: String = sql
        .db
        .query_row(
            "SELECT sql FROM sqlite_temp_master WHERE name='fragments'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    sql.db.execute("DROP TABLE fragments", []).unwrap();
    assert!(sql.refresh(&[]).is_err());
    assert!(!sql.initialized());
    for table in ["attempts", "records", "current", "analyses"] {
        assert_eq!(
            sql.db
                .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }
    sql.db
        .execute_batch(&schema.replacen("CREATE TABLE", "CREATE TEMP TABLE", 1))
        .unwrap();
    // Fail after three deferred indexes were created, not only while loading.
    sql.db
        .execute("CREATE INDEX temp.analyses_time ON invalid_rows(id)", [])
        .unwrap();
    assert!(sql.refresh(&[]).is_err());
    assert!(!sql.initialized());
    assert_eq!(
        sql.db
            .query_row("SELECT count(*) FROM current", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        sql.db
            .query_row(
                "SELECT count(*) FROM sqlite_temp_master WHERE name IN ('attempts_response','attempts_session','attempts_time')",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
    sql.db.execute("DROP INDEX temp.analyses_time", []).unwrap();
    sql.refresh(&[]).unwrap();
    assert_eq!(sql.observation_page(None, None, 0).unwrap()["total"], 20);
    assert_eq!(
        sql.db
            .query_row(
                "SELECT count(*) FROM sqlite_temp_master WHERE name='records_samples'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        1
    );
}

#[test]
fn aggregate_decode_cache_eviction_and_large_values_preserve_every_addition() {
    let dir = tempfile::tempdir().unwrap();
    let sql = History::open(dir.path()).unwrap();
    sql.db
        .execute("CREATE TEMP TABLE inputs(value TEXT)", [])
        .unwrap();
    for i in 0..160 {
        let row = json!({"calls":i+1,"usage":{"inputTokens":u64::MAX-i,"totalTokens":if i%2==0 {Some(0)} else {None}},"cost":{"actual":[{"currency":if i%2==0 {"USD"} else {"CNY"},"amount":0.25}]},"lastSeen":if i==0 {"x".repeat(9000)} else {format!("t-{i:03}")}});
        sql.db
            .execute("INSERT INTO inputs VALUES(?1)", [row.to_string()])
            .unwrap();
    }
    let result = aggregates::list(&sql.db, "SELECT stats_sum(value) FROM inputs")
        .unwrap()
        .remove(0);
    assert_eq!(result["calls"], 12880);
    assert_eq!(result["usage"]["inputTokens"], u64::MAX);
    assert_eq!(result["usage"]["totalTokens"], 0);
    assert_eq!(result["lastSeen"], "x".repeat(9000));
    assert_eq!(result["cost"]["actual"].as_array().unwrap().len(), 2);
    for v in result["cost"]["actual"].as_array().unwrap() {
        assert_eq!(v["amount"], 20.0);
    }
}

#[test]
fn usage_facts_are_lazy_incremental_and_atomic_without_changing_payloads() {
    let dir = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(dir.path()).unwrap();
    let mut rows: Vec<_> = (0..90).map(row).collect();
    store.save_batch(&rows).unwrap();
    let mut sql = History::open(dir.path()).unwrap();
    sql.refresh(&[]).unwrap();
    let count =
        |sql: &History, query: &str| sql.db.query_row(query, [], |r| r.get::<_, i64>(0)).unwrap();
    assert_eq!(
        count(
            &sql,
            "SELECT count(*) FROM sqlite_temp_master WHERE name LIKE '%_facts'"
        ),
        0
    );
    let p = provider_usage::Query {
        source: "gateway".into(),
        provider_id: Some("_unknown".into()),
        ..Default::default()
    };
    let m = crate::model_usage::UsageQuery {
        agent: "claude".into(),
        source: "gateway".into(),
        days: None,
        provider: None,
        model: None,
        cursor: None,
    };
    let check = |sql: &mut History, rows: &[ModelObservation]| {
        let got = sql
            .provider_usage(p.clone(), &[], "asof".into(), "ready".into(), now())
            .unwrap();
        let expected = value(
            &provider_usage::aggregate(
                p.clone(),
                &[],
                provider_usage::gateway_records(rows.to_vec(), &[]),
                "asof".into(),
                "ready".into(),
                now(),
            )
            .unwrap(),
        )
        .unwrap();
        let mut got = got;
        let mut expected = expected;
        without_cursor(&mut got);
        without_cursor(&mut expected);
        assert_close(&got, &expected, "incremental provider");
        let mut got = sql
            .model_usage(m.clone(), "asof".into(), "ready".into(), now())
            .unwrap();
        let mut expected = value(
            &crate::model_usage::aggregate(
                m.clone(),
                model_rows(rows),
                "asof".into(),
                "ready".into(),
                now(),
            )
            .unwrap(),
        )
        .unwrap();
        without_cursor(&mut got);
        without_cursor(&mut expected);
        assert_close(&got, &expected, "incremental model");
    };
    check(&mut sql, &rows);
    let plans: Vec<String> = sql
        .db
        .prepare(
            "EXPLAIN QUERY PLAN SELECT stats FROM provider_work WHERE original_provider='saved'",
        )
        .unwrap()
        .query_map([], |r| r.get(3))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert!(
        plans
            .iter()
            .any(|p| p.contains("provider_facts_scope (provider=?)")),
        "{plans:?}"
    );
    sql.db.execute_batch("CREATE TEMP TABLE fact_writes(kind TEXT,id TEXT);CREATE TEMP TRIGGER provider_writes AFTER INSERT ON provider_facts BEGIN INSERT INTO fact_writes VALUES('provider',new.id);END;CREATE TEMP TRIGGER model_writes AFTER INSERT ON model_facts BEGIN INSERT INTO fact_writes VALUES('model',new.id);END;").unwrap();
    // A changed scope recomputes totals without rebuilding either projection.
    let mut filtered = p.clone();
    filtered.agent = Some("claude-code-cli".into());
    sql.provider_usage(filtered, &[], "asof".into(), "ready".into(), now())
        .unwrap();
    let mut filtered = m.clone();
    filtered.model = Some("FLASH".into());
    sql.model_usage(filtered, "asof".into(), "ready".into(), now())
        .unwrap();
    assert_eq!(count(&sql, "SELECT count(*) FROM fact_writes"), 0);
    // Same revision, deletion, ID move, malformed replacement and late import.
    rows[0].usage.total_tokens = Some(u64::MAX);
    rows[0].model = Some("changed".into());
    store.save(&rows[0]).unwrap();
    let removed = rows.remove(1);
    store
        .0
        .execute("DELETE FROM model_observations WHERE id=?1", [&removed.id])
        .unwrap();
    let moved = rows[1].id.clone();
    rows[1].id = "moved".into();
    store
        .0
        .execute(
            "UPDATE model_observations SET id=?2,value=?3 WHERE id=?1",
            params![moved, rows[1].id, encode(&rows[1]).unwrap()],
        )
        .unwrap();
    let bad = rows.remove(2);
    store
        .0
        .execute(
            "UPDATE model_observations SET value='{bad' WHERE id=?1",
            [&bad.id],
        )
        .unwrap();
    rows.push(row(100));
    store.save(rows.last().unwrap()).unwrap();
    sql.refresh(&[]).unwrap();
    let before: String = sql
        .db
        .query_row(
            "SELECT group_concat(id||stats) FROM provider_facts",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let previous = count(
        &sql,
        "SELECT seq FROM usage_projection_clock WHERE kind='provider_facts'",
    );
    sql.db.execute_batch("CREATE TEMP TRIGGER fail_usage BEFORE INSERT ON provider_facts WHEN new.id='r-000100' BEGIN SELECT RAISE(ABORT,'injected');END;").unwrap();
    assert!(
        sql.provider_usage(p.clone(), &[], "asof".into(), "ready".into(), now())
            .is_err()
    );
    assert_eq!(
        previous,
        count(
            &sql,
            "SELECT seq FROM usage_projection_clock WHERE kind='provider_facts'"
        )
    );
    assert_eq!(
        before,
        sql.db
            .query_row::<String, _, _>(
                "SELECT group_concat(id||stats) FROM provider_facts",
                [],
                |r| r.get(0)
            )
            .unwrap()
    );
    assert_eq!(count(&sql, "SELECT count(*) FROM fact_writes"), 0);
    sql.db.execute_batch("DROP TRIGGER fail_usage").unwrap();
    check(&mut sql, &rows);
    assert_eq!(
        count(
            &sql,
            "SELECT count(*) FROM fact_writes WHERE kind='provider'"
        ),
        3
    );
    assert_eq!(
        count(&sql, "SELECT count(*) FROM fact_writes WHERE kind='model'"),
        3
    );
    assert_eq!(count(&sql, "SELECT count(*) FROM usage_changes"), 0);
    assert_eq!(
        count(&sql, "SELECT count(*) FROM provider_facts"),
        rows.len() as i64
    );
    assert_eq!(
        count(&sql, "SELECT count(*) FROM model_facts"),
        rows.len() as i64
    );
    assert_eq!(sql.decoded, 94);
    for table in ["provider_facts", "model_facts"] {
        assert_eq!(
            count(
                &sql,
                &format!(
                    "SELECT count(*) FROM {table} f JOIN attempts a ON a.id=f.id WHERE f.source_order<>a.rowid"
                )
            ),
            0
        );
    }
    // Restart reconstructs private tables; no source payload was rewritten.
    let raw = || {
        store
            .0
            .prepare("SELECT id,value FROM model_observations ORDER BY id")
            .unwrap()
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    };
    let original = raw();
    drop(sql);
    let mut sql = History::open(dir.path()).unwrap();
    sql.refresh(&[]).unwrap();
    check(&mut sql, &rows);
    assert_eq!(raw(), original);
}

#[test]
fn model_money_addition_order_survives_provider_filters_and_projection_rebuilds() {
    let dir = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(dir.path()).unwrap();
    let mut rows: Vec<_> = (0..3).map(row).collect();
    rows[0].model = Some("z".into());
    rows[1].model = Some("a".into());
    rows[2].model = Some("a".into());
    store.save_batch(&rows).unwrap();
    let mut sql = History::open(dir.path()).unwrap();
    sql.refresh(&[]).unwrap();
    for (i, r) in rows.iter().enumerate() {
        let amount = if i == 0 { 1e16 } else { 1.0 };
        sql.db.execute("UPDATE attempts SET model_record=json_set(model_record,'$._stats.cost.actual',json(?2)) WHERE id=?1",params![r.id,json!([{"currency":"USD","amount":amount}]).to_string()]).unwrap();
    }
    let mut q = crate::model_usage::UsageQuery {
        agent: "claude".into(),
        source: "gateway".into(),
        days: None,
        provider: None,
        model: None,
        cursor: None,
    };
    let first = sql
        .model_usage(q.clone(), "asof".into(), "ready".into(), now())
        .unwrap();
    q.provider = first["providers"][0]["id"].as_str().map(str::to_owned);
    for cycle in 0..2 {
        if cycle == 1 {
            // Reverse dirty-ID order relative to input, without changing the
            // source's order or amounts. Reinserted facts must keep that order.
            sql.db.execute_batch("UPDATE attempts SET model_record=model_record WHERE id='r-000002';UPDATE attempts SET model_record=model_record WHERE id='r-000000';").unwrap();
        }
        let result = sql
            .model_usage(q.clone(), "asof".into(), "ready".into(), now())
            .unwrap();
        assert_eq!(
            result["summary"]["cost"]["actual"][0]["amount"].as_f64(),
            Some(1e16)
        );
        assert_eq!(result["summary"], first["summary"]);
    }
}

#[test]
fn native_usage_replacement_and_revocation_never_reuse_gateway_facts() {
    let dir = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(dir.path()).unwrap();
    store
        .save_batch(&(0..45).map(row).collect::<Vec<_>>())
        .unwrap();
    let mut sql = History::open(dir.path()).unwrap();
    sql.refresh(&[]).unwrap();
    let mut rows: Vec<_> = (100..147).map(row).collect();
    let mut p = provider_usage::Query {
        source: "native".into(),
        provider_id: Some("_unknown".into()),
        ..Default::default()
    };
    let m = crate::model_usage::UsageQuery {
        agent: "claude".into(),
        source: "native".into(),
        days: None,
        provider: None,
        model: None,
        cursor: None,
    };
    for cycle in 0..3 {
        if cycle == 1 {
            rows.truncate(43);
            rows[0].usage.total_tokens = Some(0);
            rows[0].model = None;
        }
        if cycle == 2 {
            rows.clear();
        }
        sql.set_native_usage(
            provider_usage::gateway_records(rows.clone(), &[]),
            model_rows(&rows),
        )
        .unwrap();
        let mut got = sql
            .provider_usage(p.clone(), &[], "asof".into(), "ready".into(), now())
            .unwrap();
        let mut expected = value(
            &provider_usage::aggregate(
                p.clone(),
                &[],
                provider_usage::gateway_records(rows.clone(), &[]),
                "asof".into(),
                "ready".into(),
                now(),
            )
            .unwrap(),
        )
        .unwrap();
        without_cursor(&mut got);
        without_cursor(&mut expected);
        assert_close(&got, &expected, "native provider replacement");
        let mut got = sql
            .model_usage(m.clone(), "asof".into(), "ready".into(), now())
            .unwrap();
        let mut expected = value(
            &crate::model_usage::aggregate(
                m.clone(),
                model_rows(&rows),
                "asof".into(),
                "ready".into(),
                now(),
            )
            .unwrap(),
        )
        .unwrap();
        without_cursor(&mut got);
        without_cursor(&mut expected);
        assert_close(&got, &expected, "native model replacement");
        p.source = "gateway".into();
        assert_eq!(
            sql.provider_usage(p.clone(), &[], "asof".into(), "ready".into(), now())
                .unwrap()["summary"]["calls"],
            45
        );
        p.source = "native".into();
    }
}

#[test]
fn native_usage_replacement_is_atomic_when_an_insert_fails() {
    let dir = tempfile::tempdir().unwrap();
    let mut sql = History::open(dir.path()).unwrap();
    sql.refresh(&[]).unwrap();
    let original: Vec<_> = (0..45).map(row).collect();
    sql.set_native_usage(
        provider_usage::gateway_records(original.clone(), &[]),
        model_rows(&original),
    )
    .unwrap();
    let before: Vec<(String, Option<String>, Option<String>)> = {
        let mut statement = sql
            .db
            .prepare("SELECT id,provider_record,model_record FROM native_usage ORDER BY id")
            .unwrap();
        statement
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    };
    sql.db.execute_batch("CREATE TEMP TRIGGER fail_native_usage BEFORE INSERT ON native_usage WHEN (SELECT count(*) FROM native_usage)=3 BEGIN SELECT RAISE(ABORT,'injected');END;").unwrap();
    assert!(
        sql.set_native_usage(
            provider_usage::gateway_records((100..145).map(row).collect(), &[]),
            vec![]
        )
        .is_err()
    );
    let after: Vec<(String, Option<String>, Option<String>)> = {
        let mut statement = sql
            .db
            .prepare("SELECT id,provider_record,model_record FROM native_usage ORDER BY id")
            .unwrap();
        statement
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    };
    assert_eq!(before, after);
    sql.db
        .execute_batch("DROP TRIGGER fail_native_usage")
        .unwrap();
    sql.set_native_usage(vec![], vec![]).unwrap();
    assert_eq!(
        sql.db
            .query_row("SELECT count(*) FROM native_usage", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn cold_native_fees_and_late_gateway_evidence_match_canonical_association() {
    let mut catalog = crate::associated_cost_tests::catalog();
    let template = catalog.calls[0].clone();
    catalog.calls = (0..257)
        .map(|i| {
            let mut call = template.clone();
            call.id = format!("native-{i:04}");
            call.response_id = Some(format!("response-{i:04}"));
            call
        })
        .collect();
    // Duplicate native IDs and missing response IDs keep the established rules.
    catalog.calls[1].response_id = catalog.calls[0].response_id.clone();
    catalog.calls[2].response_id = None;
    let dir = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(dir.path()).unwrap();
    let mut sql = History::open(dir.path()).unwrap();
    let mut gateway = vec![];
    for cycle in 0..6 {
        if cycle == 1 {
            for (i, response) in [
                (0, "response-0000"),
                (1, "response-0003"),
                (2, "response-0004"),
            ] {
                let mut r = row(i);
                r.response_id = Some(response.into());
                r.session_id = None;
                r.agent_id = template.agent_id.clone();
                r.agent_product_id = template.agent_product_id.clone();
                r.usage = template.usage.clone().unwrap_or_default();
                gateway.push(r);
            }
        }
        if cycle == 2 {
            gateway[2].agent_product_id = Some("claude-code-desktop".into());
        }
        if cycle == 3 {
            gateway[1].usage.total_tokens = Some(999);
        }
        if cycle == 4 {
            store
                .0
                .execute("DELETE FROM model_observations", [])
                .unwrap();
            gateway.clear();
        }
        if cycle == 5 {
            catalog.calls.clear();
            catalog.sessions.clear();
            catalog.revision += 1;
        }
        store.save_batch(&gateway).unwrap();
        sql.refresh(&[]).unwrap();
        sql.prepare_fees(&catalog).unwrap();
        let oracle = crate::associated_cost::Index::new(&catalog, gateway.clone(), false);
        let ids: Vec<_> = catalog.sessions.iter().map(|s| s.id.clone()).collect();
        assert_close(
            &sql.fee_summary(&ids, None, false).unwrap(),
            &value(&oracle.summary(&ids, None)).unwrap(),
            "cold/late fee summary",
        );
        let costs = sql.response_costs().unwrap();
        for call in &catalog.calls {
            let cost = costs
                .get(&call.id)
                .cloned()
                .unwrap_or_else(|| crate::provider_cost::Cost::unknown("native_price_unconfirmed"));
            assert_close(
                &value(&cost).unwrap(),
                &value(&oracle.response_cost(&call.id)).unwrap(),
                "bulk native response cost",
            );
            assert_close(
                &value(&cost).unwrap(),
                &value(&sql.response_cost(&call.id).unwrap()).unwrap(),
                "single response cost parity",
            );
        }
    }
}

#[test]
fn usage_pages_reuse_totals_without_rescanning_work_tables() {
    let dir = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(dir.path()).unwrap();
    let rows: Vec<_> = (0..90)
        .map(|i| {
            let mut r = row(i);
            r.model = Some(format!("model-{i:03}"));
            r
        })
        .collect();
    store.save_batch(&rows).unwrap();
    let mut sql = History::open(dir.path()).unwrap();
    sql.refresh(&[]).unwrap();
    let mut q = provider_usage::Query {
        source: "gateway".into(),
        provider_id: Some("_unknown".into()),
        ..Default::default()
    };
    let first = sql
        .provider_usage(q.clone(), &[], "asof".into(), "ready".into(), now())
        .unwrap();
    sql.db.execute_batch("DROP VIEW provider_work;CREATE TEMP VIEW provider_work AS SELECT * FROM provider_facts WHERE 0;").unwrap();
    q.cursor = first["nextCursor"].as_str().map(str::to_owned);
    let next = sql
        .provider_usage(q, &[], "new-asof".into(), "ready".into(), now())
        .unwrap();
    assert_eq!(next["summary"], first["summary"]);
    assert_eq!(next["calls"].as_array().unwrap().len(), 40);
    assert_eq!(next["sourceAsOf"], "new-asof");
    assert_eq!(
        sql.db
            .query_row("SELECT count(*) FROM provider_work", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
    let mut q = crate::model_usage::UsageQuery {
        agent: "claude".into(),
        source: "gateway".into(),
        days: None,
        provider: None,
        model: None,
        cursor: None,
    };
    let first = sql
        .model_usage(q.clone(), "asof".into(), "ready".into(), now())
        .unwrap();
    sql.db.execute("DELETE FROM model_groups", []).unwrap();
    q.cursor = first["nextCursor"].as_str().map(str::to_owned);
    let next = sql
        .model_usage(q, "new-asof".into(), "ready".into(), now())
        .unwrap();
    assert_eq!(next["summary"], first["summary"]);
    assert_eq!(next["items"].as_array().unwrap().len(), 20);
    assert_eq!(
        sql.db
            .query_row("SELECT count(*) FROM model_groups", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn usage_cache_expires_at_time_boundaries_and_invalidates_on_changes() {
    let dir = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(dir.path()).unwrap();
    let at = now();
    let mut a = row(0);
    let mut b = row(1);
    a.started_at =
        (at - chrono::Duration::days(1) + chrono::Duration::milliseconds(1)).to_rfc3339();
    b.started_at = (at + chrono::Duration::milliseconds(1)).to_rfc3339();
    b.usage.total_tokens = Some(220);
    store.save_batch(&[a, b.clone()]).unwrap();
    let mut sql = History::open(dir.path()).unwrap();
    sql.refresh(&[]).unwrap();
    let q = provider_usage::Query {
        source: "gateway".into(),
        provider_id: Some("_unknown".into()),
        days: Some(1),
        ..Default::default()
    };
    let m = crate::model_usage::UsageQuery {
        agent: "claude".into(),
        source: "gateway".into(),
        days: Some(1),
        provider: None,
        model: None,
        cursor: None,
    };
    assert_eq!(
        sql.provider_usage(q.clone(), &[], "asof".into(), "ready".into(), at)
            .unwrap()["selected"]["usage"]["totalTokens"],
        110
    );
    assert_eq!(
        sql.model_usage(m.clone(), "asof".into(), "ready".into(), at)
            .unwrap()["summary"]["usage"]["totalTokens"],
        110
    );
    let later = at + chrono::Duration::milliseconds(2);
    assert_eq!(
        sql.provider_usage(q.clone(), &[], "asof".into(), "ready".into(), later)
            .unwrap()["selected"]["usage"]["totalTokens"],
        220
    );
    assert_eq!(
        sql.model_usage(m.clone(), "asof".into(), "ready".into(), later)
            .unwrap()["summary"]["usage"]["totalTokens"],
        220
    );
    b.usage.total_tokens = Some(333);
    store.save(&b).unwrap();
    sql.refresh(&[]).unwrap();
    assert_eq!(
        sql.provider_usage(q.clone(), &[], "asof".into(), "ready".into(), later)
            .unwrap()["selected"]["usage"]["totalTokens"],
        333
    );
    assert_eq!(
        sql.model_usage(m.clone(), "asof".into(), "ready".into(), later)
            .unwrap()["summary"]["usage"]["totalTokens"],
        333
    );
    // A clock correction must not keep a now-future record in cached totals.
    assert_eq!(
        sql.provider_usage(q, &[], "asof".into(), "ready".into(), at)
            .unwrap()["selected"]["usage"]["totalTokens"],
        110
    );
    assert_eq!(
        sql.model_usage(m, "asof".into(), "ready".into(), at)
            .unwrap()["summary"]["usage"]["totalTokens"],
        110
    );
}

#[test]
fn removing_an_unscoped_duplicate_response_recomputes_inferred_session_fees() {
    let catalog = crate::associated_cost_tests::catalog();
    let dir = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(dir.path()).unwrap();
    let mut sql = History::open(dir.path()).unwrap();
    let mut a = row(1);
    a.agent_product_id = Some("claude-code-cli".into());
    a.session_id = None;
    a.response_id = Some("response-one".into());
    let mut b = a.clone();
    b.id = "duplicate".into();
    store.save_batch(&[a.clone(), b.clone()]).unwrap();
    sql.refresh(&[]).unwrap();
    sql.prepare_fees(&catalog).unwrap();
    let ids = vec!["claude:one".into()];
    assert_eq!(
        sql.fee_summary(&ids, None, false).unwrap()["gatewayAttempts"],
        0
    );
    store
        .0
        .execute("DELETE FROM model_observations WHERE id=?1", [&b.id])
        .unwrap();
    sql.refresh(&[]).unwrap();
    sql.prepare_fees(&catalog).unwrap();
    let expected = crate::associated_cost::Index::new(&catalog, vec![a], false);
    let got = sql.fee_summary(&ids, None, false).unwrap();
    assert_close(
        &got,
        &value(&expected.summary(&ids, None)).unwrap(),
        "duplicate removal",
    );
    assert_eq!(got["gatewayAttempts"], 1);
}

#[test]
fn legacy_evidence_late_import_and_large_unsigned_totals_keep_paging_contracts() {
    let dir = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(dir.path()).unwrap();
    let mut rows: Vec<_> = (0..90).map(row).collect();
    for (i, r) in rows.iter_mut().enumerate() {
        r.provider_id = if i.is_multiple_of(3) {
            Some("p-a".into())
        } else {
            None
        };
        r.usage.total_tokens = Some(u64::MAX - (i as u64));
        r.model = Some(format!("model-{i:03}"));
    }
    let providers:Vec<crate::Provider>=["p-b","p-a"].into_iter().map(|id|serde_json::from_value(json!({"id":id,"name":"saved","endpoint":"https://api.deepseek.com","protocol":"messages","model":"model","configured":true})).unwrap()).collect();
    let evidence = vec![provider_usage::Evidence {
        agent: "claude".into(),
        provider_id: "p-b".into(),
        provider_name: "saved".into(),
        endpoint: "https://api.deepseek.com".into(),
        protocol: "messages".into(),
        revision: 1,
        native_provider: None,
        model_aliases: vec![],
    }];
    store.save_batch(&rows[..89]).unwrap();
    let mut sql = History::open(dir.path()).unwrap();
    sql.refresh(&[]).unwrap();
    sql.set_evidence(&evidence).unwrap();
    store.save(&rows[89]).unwrap();
    sql.refresh(&[]).unwrap();
    sql.set_evidence(&evidence).unwrap();
    let q = provider_usage::Query {
        source: "gateway".into(),
        provider_id: Some("p-b".into()),
        ..Default::default()
    };
    let first = sql
        .provider_usage(q.clone(), &providers, "asof".into(), "ready".into(), now())
        .unwrap();
    let mut expected = value(
        &provider_usage::aggregate(
            q.clone(),
            &providers,
            provider_usage::gateway_records(rows.clone(), &evidence),
            "asof".into(),
            "ready".into(),
            now(),
        )
        .unwrap(),
    )
    .unwrap();
    let mut got = first.clone();
    without_cursor(&mut got);
    without_cursor(&mut expected);
    assert_close(&got, &expected, "legacy provider");
    let mut page = q;
    page.cursor = first["nextCursor"].as_str().map(str::to_owned);
    let second = sql
        .provider_usage(
            page.clone(),
            &providers,
            "asof".into(),
            "ready".into(),
            now(),
        )
        .unwrap();
    assert_eq!(second["calls"].as_array().unwrap().len(), 20);
    let mut unrelated = rows[0].clone();
    unrelated.revision += 1;
    unrelated.usage.total_tokens = Some(7);
    store.save(&unrelated).unwrap();
    sql.refresh(&[]).unwrap();
    sql.set_evidence(&evidence).unwrap();
    assert!(
        sql.provider_usage(page, &providers, "asof".into(), "ready".into(), now())
            .is_ok()
    );
    let q = crate::model_usage::UsageQuery {
        agent: "claude".into(),
        source: "gateway".into(),
        days: None,
        provider: None,
        model: None,
        cursor: None,
    };
    let first = sql
        .model_usage(q.clone(), "asof".into(), "ready".into(), now())
        .unwrap();
    assert_eq!(first["items"].as_array().unwrap().len(), 20);
    assert_eq!(first["summary"]["usage"]["totalTokens"], u64::MAX);
    let mut q = q;
    q.cursor = first["nextCursor"].as_str().map(str::to_owned);
    let second = sql
        .model_usage(q, "asof".into(), "ready".into(), now())
        .unwrap();
    assert_eq!(second["items"].as_array().unwrap().len(), 20);
    assert!(second["items"].as_array().unwrap().iter().all(|b| {
        first["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|a| a["model"] != b["model"])
    }));
}

#[test]
fn fractional_time_boundaries_and_native_latest_references_preserve_precision() {
    let now = DateTime::parse_from_rfc3339("2026-10-21T03:00:00.000000500Z")
        .unwrap()
        .with_timezone(&Utc);
    let dir = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(dir.path()).unwrap();
    let mut rows: Vec<_> = (0..6).map(row).collect();
    for (i, r) in rows.iter_mut().enumerate() {
        r.started_at = format!("2026-10-21T03:00:00.000000{}Z", i * 100);
        r.session_id = Some(format!("s-{i}"));
    }
    store.save_batch(&rows).unwrap();
    let mut sql = History::open(dir.path()).unwrap();
    sql.refresh(&[]).unwrap();
    for start in [None, Some("2026-10-21T11:00:00.000000100+08:00".into())] {
        let q = requests::Query {
            start,
            end: Some("2026-10-21T03:00:00.000000400Z".into()),
            ..Default::default()
        };
        let got = sql.request_page(&q, now, Value::Null, None).unwrap();
        let old = requests::Snapshot::new(
            q.clone(),
            requests::logical_requests(rows.clone(), &HashMap::new()),
            now,
            got["snapshot"].as_str().unwrap().into(),
            None,
        )
        .unwrap();
        let mut expected = value(&old.page(&q).unwrap()).unwrap();
        expected["sourceReport"] = Value::Null;
        expected["coverage"] = got["coverage"].clone();
        assert_close(&got, &expected, "fractional dashboard");
    }
    let q = provider_usage::Query {
        source: "gateway".into(),
        provider_id: Some("_unknown".into()),
        ..Default::default()
    };
    let mut got = sql
        .provider_usage(q.clone(), &[], "asof".into(), "ready".into(), now)
        .unwrap();
    let mut expected = value(
        &provider_usage::aggregate(
            q,
            &[],
            provider_usage::gateway_records(rows.clone(), &[]),
            "asof".into(),
            "ready".into(),
            now,
        )
        .unwrap(),
    )
    .unwrap();
    without_cursor(&mut got);
    without_cursor(&mut expected);
    assert_close(&got, &expected, "fractional provider");
    let mut q = crate::model_usage::UsageQuery {
        agent: "claude".into(),
        source: "gateway".into(),
        days: None,
        provider: None,
        model: None,
        cursor: None,
    };
    let mut got = sql
        .model_usage(q.clone(), "asof".into(), "ready".into(), now)
        .unwrap();
    let mut expected = value(
        &crate::model_usage::aggregate(
            q.clone(),
            model_rows(&rows),
            "asof".into(),
            "ready".into(),
            now,
        )
        .unwrap(),
    )
    .unwrap();
    without_cursor(&mut got);
    without_cursor(&mut expected);
    assert_close(&got, &expected, "fractional model");
    // Native ties keep the original source iteration order, including all-unknown times.
    q.source = "native".into();
    for unknown in [false, true] {
        let mut models = model_rows(&rows);
        for m in &mut models {
            m.time = if unknown {
                None
            } else {
                Some("2026-10-21T02:00:00Z".into())
            };
        }
        let mut expected = value(
            &crate::model_usage::aggregate(
                q.clone(),
                model_rows_native(&rows, unknown),
                "asof".into(),
                "ready".into(),
                now,
            )
            .unwrap(),
        )
        .unwrap();
        sql.set_native_usage(vec![], models).unwrap();
        let mut got = sql
            .model_usage(q.clone(), "asof".into(), "ready".into(), now)
            .unwrap();
        without_cursor(&mut got);
        without_cursor(&mut expected);
        assert_close(&got, &expected, "native time ties");
    }
}
fn model_rows_native(
    rows: &[ModelObservation],
    unknown: bool,
) -> Vec<crate::model_usage::UsageRecord> {
    let mut rows = model_rows(rows);
    for r in &mut rows {
        r.time = if unknown {
            None
        } else {
            Some("2026-10-21T02:00:00Z".into())
        };
    }
    rows
}
fn now() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-10-21T03:00:00Z")
        .unwrap()
        .with_timezone(&Utc)
}
fn row(n: usize) -> ModelObservation {
    serde_json::from_value(json!({"id":format!("r-{n:06}"),"agentId":"claude","agentProductId":if n.is_multiple_of(3){Some("claude-code-cli")}else if n%3==1{Some("claude-code-desktop")}else{None},"provider":"saved","providerId":"saved","endpoint":"https://api.deepseek.com","bindingRevision":1,"startedAt":format!("2026-10-21T02:{:02}:00Z",n%60),"endedAt":format!("2026-10-21T02:{:02}:01Z",n%60),"status":"success","model":"deepseek-flash","protocol":"messages","revision":1,"usage":{"inputTokens":100,"outputTokens":10,"totalTokens":110,"cachedInputTokens":20},"durationMs":1000.0,"ttftMs":200.0})).unwrap()
}

#[test]
#[ignore = "explicit uncached usage benchmark; isolated generated metadata only"]
fn first_usage_statistics_benchmark() {
    let setting = |key: &str, default: usize| {
        std::env::var(key)
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(default)
    };
    let n = setting("LOCAL_AGENT_HISTORY_ROWS", 100_000);
    let repeats = setting("LOCAL_AGENT_USAGE_REPEATS", 3);
    let combinations = setting("LOCAL_AGENT_USAGE_COMBINATIONS", 1);
    let dir = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(dir.path()).unwrap();
    for start in (0..n).step_by(256) {
        let rows: Vec<_> = (start..(start + 256).min(n))
            .map(|i| {
                let mut r = row(i);
                if combinations > 1 {
                    r.provider = format!("provider-{:02}", i % 20);
                    r.provider_id = Some(r.provider.clone());
                    r.model = Some(format!("model-{:03}", (i / 20) % combinations));
                }
                r
            })
            .collect();
        store.save_batch(&rows).unwrap();
    }
    let providers: Vec<crate::Provider> = if combinations > 1 {
        (0..20).map(|i|serde_json::from_value(json!({"id":format!("provider-{i:02}"),"name":"saved","endpoint":"https://api.deepseek.com","protocol":"messages","model":"model","configured":true})).unwrap()).collect()
    } else {
        vec![]
    };
    let mut sql = History::open(dir.path()).unwrap();
    let t = std::time::Instant::now();
    sql.refresh(&[]).unwrap();
    eprintln!(
        "usage_benchmark rows={n} combinations={combinations} cold={:?}",
        t.elapsed()
    );
    for round in 0..repeats {
        if round > 0 {
            sql.provider_cache.clear();
            sql.model_cache.clear();
            // Rebuild projection, percentile/rank work and page positions on
            // every sample. Never time a cached response as a first report.
            for name in [
                "provider_work",
                "provider_facts",
                "model_facts",
                "provider_positions",
                "model_positions",
                "model_groups",
            ] {
                let kind: Option<String> = sql
                    .db
                    .query_row(
                        "SELECT type FROM sqlite_temp_master WHERE name=?1",
                        [name],
                        |r| r.get(0),
                    )
                    .optional()
                    .unwrap();
                if let Some(kind) = kind {
                    sql.db
                        .execute_batch(&format!("DROP {kind} {name}"))
                        .unwrap();
                }
            }
            if sql.db.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_temp_master WHERE name='usage_projection_clock')",[],|r|r.get::<_,bool>(0)).unwrap(){sql.db.execute("DELETE FROM usage_projection_clock",[]).unwrap();}
        }
        let q = provider_usage::Query {
            source: "gateway".into(),
            provider_id: Some(
                if combinations > 1 {
                    "provider-00"
                } else {
                    "_unknown"
                }
                .into(),
            ),
            ..Default::default()
        };
        let t = std::time::Instant::now();
        let provider = sql
            .provider_usage(q, &providers, "asof".into(), "ready".into(), now())
            .unwrap();
        eprintln!("usage_round={round} provider_first={:?}", t.elapsed());
        assert_eq!(provider["summary"]["calls"], n);
        assert_eq!(provider["summary"]["usage"]["totalTokens"], n * 110);
        assert_eq!(
            provider["totalCalls"],
            if combinations > 1 { n.div_ceil(20) } else { n }
        );
        let q = crate::model_usage::UsageQuery {
            agent: "claude".into(),
            source: "gateway".into(),
            days: None,
            provider: None,
            model: None,
            cursor: None,
        };
        let t = std::time::Instant::now();
        let mut model = sql
            .model_usage(q.clone(), "asof".into(), "ready".into(), now())
            .unwrap();
        eprintln!("usage_round={round} model_first={:?}", t.elapsed());
        assert_eq!(model["summary"]["attempts"], n);
        assert_eq!(model["summary"]["usage"]["totalTokens"], n * 110);
        let groups = model["total"].as_u64().unwrap() as usize;
        assert_eq!(
            groups,
            if combinations > 1 {
                20 * combinations
            } else {
                1
            }
        );
        let mut items = vec![];
        loop {
            items.extend(model["items"].as_array().unwrap().clone());
            let Some(cursor) = model["nextCursor"].as_str() else {
                break;
            };
            let mut next = q.clone();
            next.cursor = Some(cursor.into());
            model = sql
                .model_usage(next, "asof".into(), "ready".into(), now())
                .unwrap();
        }
        assert_eq!(items.len(), groups);
        assert_eq!(
            items
                .iter()
                .map(|v| v["totals"]["attempts"].as_u64().unwrap())
                .sum::<u64>(),
            n as u64
        );
        assert_eq!(sql.decoded, n);
    }
}
fn assert_close(a: &Value, b: &Value, path: &str) {
    match (a, b) {
        (Value::Number(a), Value::Number(b)) => {
            let (a, b) = (a.as_f64().unwrap(), b.as_f64().unwrap());
            assert!(
                (a - b).abs() <= 1e-9_f64.max(a.abs() * 1e-12),
                "{path}: {a} != {b}"
            );
        }
        (Value::Object(a), Value::Object(b)) => {
            assert_eq!(
                a.len(),
                b.len(),
                "{path}: keys {:?} != {:?}",
                a.keys(),
                b.keys()
            );
            for (k, v) in a {
                assert_close(
                    v,
                    b.get(k).unwrap_or_else(|| panic!("missing {path}.{k}")),
                    &format!("{path}.{k}"),
                );
            }
        }
        (Value::Array(a), Value::Array(b)) => {
            assert_eq!(a.len(), b.len(), "{path}: array sizes");
            for (i, (a, b)) in a.iter().zip(b).enumerate() {
                assert_close(a, b, &format!("{path}[{i}]"));
            }
        }
        _ => assert_eq!(a, b, "{path}"),
    }
}
#[test]
fn sql_dashboard_matches_canonical_filters_aggregates_ranks_heatmap_and_order() {
    let dir = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(dir.path()).unwrap();
    let mut rows: Vec<_> = (0..87).map(row).collect();
    rows[0].logical_id = Some("retry".into());
    rows[3].logical_id = Some("retry".into());
    rows[0].status = "error".into();
    rows[0].http_status = Some(429);
    rows[1].usage = Usage::default();
    rows[2].usage.input_tokens = Some(0);
    rows[2].usage.total_tokens = Some(0);
    rows[4].started_at = "unknown".into();
    rows[5].status = "interrupted".into();
    rows[5].timing_issue = Some("断流超时".into());
    store.save_batch(&rows).unwrap();
    let mut sql = History::open(dir.path()).unwrap();
    sql.refresh(&[]).unwrap();
    for metric in ["calls", "tokens", "cost", "ttft", "duration", "throughput"] {
        for dimension in ["agent", "model", "provider"] {
            let q = requests::Query {
                metric: Some(metric.into()),
                dimension: Some(dimension.into()),
                ..Default::default()
            };
            let got = sql.request_page(&q, now(), Value::Null, None).unwrap();
            let id = got["snapshot"].as_str().unwrap();
            let old = requests::Snapshot::new(
                q.clone(),
                requests::logical_requests(rows.clone(), &HashMap::new()),
                now(),
                id.into(),
                None,
            )
            .unwrap();
            let mut expected = value(&old.page(&q).unwrap()).unwrap();
            expected["sourceReport"] = Value::Null;
            expected["coverage"] = got["coverage"].clone();
            assert_close(&got, &expected, &format!("{metric}/{dimension}"));
        }
    }
}
#[test]
fn history_updates_deletions_and_same_revision_edits_only_decode_the_delta() {
    let dir = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(dir.path()).unwrap();
    store
        .save_batch(&(0..300).map(row).collect::<Vec<_>>())
        .unwrap();
    let mut sql = History::open(dir.path()).unwrap();
    sql.refresh(&[]).unwrap();
    assert_eq!(sql.decoded, 300);
    sql.refresh(&[]).unwrap();
    assert_eq!(sql.decoded, 300);
    let mut changed = row(1);
    changed.usage.total_tokens = Some(9);
    store.save(&changed).unwrap();
    sql.refresh(&[]).unwrap();
    assert_eq!(sql.decoded, 301);
    assert_eq!(
        sql.observation_page(None, Some(&changed.id), 0).unwrap()["items"][0]["usage"]["totalTokens"],
        9
    );
    store
        .0
        .execute("DELETE FROM model_observations WHERE id=?1", [&changed.id])
        .unwrap();
    sql.refresh(&[]).unwrap();
    assert_eq!(sql.observation_page(None, None, 0).unwrap()["total"], 299);
}

fn model_rows(rows: &[ModelObservation]) -> Vec<crate::model_usage::UsageRecord> {
    rows.iter()
        .map(|r| crate::model_usage::UsageRecord {
            cost: crate::provider_cost::calculate(
                Some(&r.endpoint),
                r.model.as_deref(),
                Some(&r.started_at),
                &r.status,
                &r.usage,
                r.reported_cost.as_ref(),
                r.ended_at.as_deref(),
            ),
            id: r.id.clone(),
            model: r.model.clone(),
            provider: Some(r.provider.clone()),
            endpoint: Some(r.endpoint.clone()),
            route_revision: Some(r.binding_revision),
            time: Some(r.started_at.clone()),
            usage: r.usage.clone(),
            session_id: r.session_id.clone(),
            request_id: Some(r.id.clone()),
        })
        .collect()
}
fn without_cursor(v: &mut Value) {
    if !v["nextCursor"].is_null() {
        v["nextCursor"] = json!(v["nextCursor"].as_str().unwrap().split_once(':').unwrap().1);
    }
}

#[test]
fn filters_sorting_labels_unknowns_and_snapshot_capacity_match_canonical() {
    let dir = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(dir.path()).unwrap();
    let mut rows: Vec<_> = (0..47).map(row).collect();
    for (i, r) in rows.iter_mut().enumerate() {
        r.provider = format!("连接 {}", i % 4);
        r.session_id = Some(format!("session-{}", i % 2));
        r.duration_ms = Some((i % 5) as f64);
        if i % 7 == 0 {
            r.usage = Usage::default();
        }
        if i % 11 == 0 {
            r.status = "error".into();
            r.http_status = Some(429);
        }
        if i % 9 == 0 {
            r.started_at = "unknown".into();
        }
        r.routed_model = Some(format!("routed-{}", i % 3));
    }
    rows[1].logical_id = Some("retry".into());
    rows[4].logical_id = Some("retry".into());
    store.save_batch(&rows).unwrap();
    let mut sql = History::open(dir.path()).unwrap();
    sql.refresh(&[]).unwrap();
    for sort in ["time", "tokens", "duration", "ttft", "cost"] {
        for descending in [false, true] {
            for dimension in ["agent", "provider", "model"] {
                let q = requests::Query {
                    sort: Some(sort.into()),
                    descending: Some(descending),
                    dimension: Some(dimension.into()),
                    metric: Some("tokens".into()),
                    ..Default::default()
                };
                let got = sql.request_page(&q, now(), Value::Null, None).unwrap();
                let id = got["snapshot"].as_str().unwrap();
                let old = requests::Snapshot::new(
                    q.clone(),
                    requests::logical_requests(rows.clone(), &HashMap::new()),
                    now(),
                    id.into(),
                    None,
                )
                .unwrap();
                let mut expected = value(&old.page(&q).unwrap()).unwrap();
                expected["sourceReport"] = Value::Null;
                expected["coverage"] = got["coverage"].clone();
                assert_close(&got, &expected, "sort and labels");
            }
        }
    }
    let filters = [
        requests::Query {
            agent: Some("claude-unknown".into()),
            ..Default::default()
        },
        requests::Query {
            session: Some("session-1".into()),
            ..Default::default()
        },
        requests::Query {
            provider: Some("saved".into()),
            model: Some("routed-1".into()),
            ..Default::default()
        },
        requests::Query {
            issue: Some("rate_limit".into()),
            ..Default::default()
        },
        requests::Query {
            issue: Some("unmeasurable".into()),
            ..Default::default()
        },
        requests::Query {
            search: Some("连接 2".into()),
            ..Default::default()
        },
        requests::Query {
            start: Some("2026-10-21T10:05:00+08:00".into()),
            end: Some("2026-10-21T10:15:00+08:00".into()),
            ..Default::default()
        },
    ];
    for q in filters {
        let got = sql.request_page(&q, now(), Value::Null, None).unwrap();
        let id = got["snapshot"].as_str().unwrap();
        let old = requests::Snapshot::new(
            q.clone(),
            requests::logical_requests(rows.clone(), &HashMap::new()),
            now(),
            id.into(),
            None,
        )
        .unwrap();
        let mut expected = value(&old.page(&q).unwrap()).unwrap();
        expected["sourceReport"] = Value::Null;
        expected["coverage"] = got["coverage"].clone();
        assert_close(&got, &expected, "filters");
    }
    assert_eq!(sql.snapshots.len(), 8);
}

#[test]
fn corrupt_source_rows_id_moves_and_live_checkpoints_preserve_authoritative_values() {
    let dir = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(dir.path()).unwrap();
    let mut r = row(1);
    store.save(&r).unwrap();
    let mut sql = History::open(dir.path()).unwrap();
    sql.refresh(&[]).unwrap();
    let original: String = store
        .0
        .query_row(
            "SELECT value FROM model_observations WHERE id=?1",
            [&r.id],
            |r| r.get(0),
        )
        .unwrap();
    store
        .0
        .execute(
            "INSERT INTO model_observations VALUES('broken','bad',1,'{invalid')",
            [],
        )
        .unwrap();
    sql.refresh(&[]).unwrap();
    assert_eq!(sql.invalid, 1);
    assert_eq!(sql.observation_page(None, None, 0).unwrap()["total"], 1);
    let stale = r.clone();
    r.usage.total_tokens = Some(777);
    store.save(&r).unwrap();
    sql.refresh(std::slice::from_ref(&stale)).unwrap();
    assert_eq!(
        sql.observation_page(None, Some(&r.id), 0).unwrap()["items"][0]["usage"]["totalTokens"],
        777
    );
    let mut live = r.clone();
    live.revision += 1;
    live.usage.total_tokens = Some(999);
    sql.refresh_pending(&[live.clone()], &HashSet::from([live.id.clone()]))
        .unwrap();
    assert_eq!(
        sql.observation_page(None, Some(&r.id), 0).unwrap()["items"][0]["usage"]["totalTokens"],
        999
    );
    store.save(&live).unwrap();
    sql.refresh(&[]).unwrap();
    let before: String = store
        .0
        .query_row(
            "SELECT value FROM model_observations WHERE id=?1",
            [&r.id],
            |r| r.get(0),
        )
        .unwrap();
    sql.refresh(&[]).unwrap();
    assert_eq!(
        before,
        store
            .0
            .query_row::<String, _, _>(
                "SELECT value FROM model_observations WHERE id=?1",
                [&r.id],
                |r| r.get(0)
            )
            .unwrap()
    );
    assert_ne!(before, original);
    let old = live.id.clone();
    live.id = "moved".into();
    store
        .0
        .execute(
            "UPDATE model_observations SET id=?2,value=?3 WHERE id=?1",
            params![old, live.id, encode(&live).unwrap()],
        )
        .unwrap();
    sql.refresh(&[]).unwrap();
    assert_eq!(sql.observation_page(None, None, 0).unwrap()["total"], 1);
    assert!(sql.request_detail(&old).unwrap().is_none());
    assert!(sql.request_detail("moved").unwrap().is_some());
}
#[test]
fn provider_and_model_queries_match_canonical_totals_filters_and_pages() {
    let dir = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(dir.path()).unwrap();
    let mut rows: Vec<_> = (0..137).map(row).collect();
    rows[0].binding_revision = u64::MAX;
    for (i, r) in rows.iter_mut().enumerate() {
        r.model = if i % 19 == 0 {
            None
        } else {
            Some(format!("model-{:02}", i % 43))
        };
        r.provider_id = None;
        if i % 7 == 0 {
            r.usage = Usage::default();
        }
        if i % 11 == 0 {
            r.started_at = "invalid".into();
        }
        if i % 9 == 0 {
            r.status = "error".into();
        }
        r.duration_ms = Some(i as f64);
        r.ttft_ms = Some(i as f64 / 3.0);
    }
    store.save_batch(&rows).unwrap();
    let mut sql = History::open(dir.path()).unwrap();
    sql.refresh(&[]).unwrap();
    for agent in [
        None,
        Some("claude"),
        Some("claude-code-cli"),
        Some("claude-unknown"),
    ] {
        for days in [None, Some(1)] {
            for model in [None, Some("model-01"), Some(""), Some("absent")] {
                let q = provider_usage::Query {
                    source: "gateway".into(),
                    provider_id: Some("_unknown".into()),
                    agent: agent.map(str::to_owned),
                    model: model.map(str::to_owned),
                    days,
                    ..Default::default()
                };
                let mut got = sql
                    .provider_usage(q.clone(), &[], "asof".into(), "ready".into(), now())
                    .unwrap();
                let mut old = value(
                    &provider_usage::aggregate(
                        q,
                        &[],
                        provider_usage::gateway_records(rows.clone(), &[]),
                        "asof".into(),
                        "ready".into(),
                        now(),
                    )
                    .unwrap(),
                )
                .unwrap();
                without_cursor(&mut got);
                without_cursor(&mut old);
                assert_close(&got, &old, "provider");
            }
        }
    }
    for agent in ["claude", "claude-code-cli", "claude-unknown"] {
        for model in [None, Some("MODEL-0")] {
            let q = crate::model_usage::UsageQuery {
                agent: agent.into(),
                source: "gateway".into(),
                days: None,
                provider: None,
                model: model.map(str::to_owned),
                cursor: None,
            };
            let mut got = sql
                .model_usage(q.clone(), "asof".into(), "ready".into(), now())
                .unwrap();
            let filtered: Vec<_> = rows
                .iter()
                .filter(|r| {
                    crate::agents::matches(Some(agent), &r.agent_id, r.agent_product_id.as_deref())
                })
                .cloned()
                .collect();
            let mut old = value(
                &crate::model_usage::aggregate(
                    q,
                    model_rows(&filtered),
                    "asof".into(),
                    "ready".into(),
                    now(),
                )
                .unwrap(),
            )
            .unwrap();
            without_cursor(&mut got);
            without_cursor(&mut old);
            assert_close(&got, &old, "model");
        }
    }
}

#[test]
fn fixed_pages_and_csv_survive_updates_deletes_and_expire_without_retaining_orphans() {
    let dir = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(dir.path()).unwrap();
    let rows: Vec<_> = (0..101).map(row).collect();
    store.save_batch(&rows).unwrap();
    let mut sql = History::open(dir.path()).unwrap();
    sql.refresh(&[]).unwrap();
    let q = requests::Query::default();
    let first = sql.request_page(&q, now(), Value::Null, None).unwrap();
    let id = first["snapshot"].as_str().unwrap().to_owned();
    let csv = sql.frozen_csv(&id, now()).unwrap();
    let oracle = requests::Snapshot::new(
        q.clone(),
        requests::logical_requests(rows, &HashMap::new()),
        now(),
        id.clone(),
        None,
    )
    .unwrap();
    assert_eq!(csv["csv"], oracle.csv());
    let mut update = row(3);
    update.usage.total_tokens = Some(777);
    store.save(&update).unwrap();
    store
        .0
        .execute("DELETE FROM model_observations WHERE id='r-000004'", [])
        .unwrap();
    store.save(&row(101)).unwrap();
    sql.refresh(&[]).unwrap();
    let mut frozen = q.clone();
    frozen.snapshot = Some(id.clone());
    frozen.cursor = first["nextCursor"].as_str().map(str::to_owned);
    let second = sql.request_page(&frozen, now(), Value::Null, None).unwrap();
    assert_eq!(second["total"], 101);
    assert_eq!(sql.frozen_csv(&id, now()).unwrap(), csv);
    let fresh = sql.request_page(&q, now(), Value::Null, None).unwrap();
    assert_eq!(fresh["summary"]["usage"]["totalTokens"], 101 * 110 + 667);
    assert!(
        sql.request_page(
            &frozen,
            now() + chrono::Duration::minutes(15),
            Value::Null,
            None
        )
        .is_err()
    );
    sql.prune().unwrap();
    let orphan: i64 = sql
        .db
        .query_row("SELECT count(*) FROM garbage", [], |r| r.get(0))
        .unwrap();
    assert_eq!(orphan, 0);
    let plans: Vec<String> = {
        let mut s=sql.db.prepare("EXPLAIN QUERY PLAN SELECT r.value FROM positions p JOIN records r ON r.v=p.v WHERE p.snapshot='x' AND p.pos>=80000 ORDER BY p.pos LIMIT 40").unwrap();
        s.query_map([], |r| r.get(3))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    };
    assert!(
        plans
            .iter()
            .any(|p| p.contains("PRIMARY KEY (snapshot=? AND pos>?)")),
        "{plans:?}"
    );
    assert!(!plans.iter().any(|p| p.contains("SCAN")), "{plans:?}");
}

#[test]
#[ignore = "explicit large-history benchmark; uses only a temporary database"]
fn large_history_bounded_decode_and_deep_page_benchmark() {
    let n = std::env::var("LOCAL_AGENT_HISTORY_ROWS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(100_000);
    let dir = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(dir.path()).unwrap();
    for start in (0..n).step_by(256) {
        store
            .save_batch(&(start..(start + 256).min(n)).map(row).collect::<Vec<_>>())
            .unwrap();
    }
    let mut sql = History::open(dir.path()).unwrap();
    let t = std::time::Instant::now();
    sql.refresh(&[]).unwrap();
    eprintln!("rows={n} cold={:?}", t.elapsed());
    assert_eq!(sql.decoded, n);
    let t = std::time::Instant::now();
    let q = requests::Query::default();
    let report = sql.source_report(&q, now(), vec![], None).unwrap();
    eprintln!("source_report={:?}", t.elapsed());
    assert_eq!(report["gateway"]["requests"], n);
    let t = std::time::Instant::now();
    let page = sql.request_page(&q, now(), report, None).unwrap();
    eprintln!("dashboard={:?}", t.elapsed());
    assert_eq!(page["total"], n);
    let mut q = q;
    q.snapshot = page["snapshot"].as_str().map(str::to_owned);
    q.cursor = Some(format!(
        "{}:{}",
        q.snapshot.as_deref().unwrap(),
        (n / 40 - 1) * 40
    ));
    let t = std::time::Instant::now();
    sql.refresh(&[]).unwrap();
    let deep = sql.request_page(&q, now(), Value::Null, None).unwrap();
    eprintln!("warm_refresh_deep={:?}", t.elapsed());
    assert_eq!(deep["items"].as_array().unwrap().len(), 40);
    assert_eq!(sql.decoded, n);
    let mut r = row(2);
    r.revision = 2;
    store.save(&r).unwrap();
    let t = std::time::Instant::now();
    sql.refresh(&[]).unwrap();
    eprintln!("delta={:?}", t.elapsed());
    assert_eq!(sql.decoded, n + 1);
    let q = provider_usage::Query {
        source: "gateway".into(),
        provider_id: Some("_unknown".into()),
        ..Default::default()
    };
    let t = std::time::Instant::now();
    let provider = sql
        .provider_usage(q.clone(), &[], "asof".into(), "ready".into(), now())
        .unwrap();
    eprintln!("provider={:?}", t.elapsed());
    assert_eq!(provider["totalCalls"], n);
    let mut deep_provider = q;
    let version = provider["nextCursor"]
        .as_str()
        .unwrap()
        .split_once(':')
        .unwrap()
        .0;
    deep_provider.cursor = Some(format!("{version}:{}", (n / 40 - 1) * 40));
    let t = std::time::Instant::now();
    let deep = sql
        .provider_usage(deep_provider, &[], "asof".into(), "ready".into(), now())
        .unwrap();
    eprintln!("provider_warm_deep={:?}", t.elapsed());
    assert_eq!(deep["calls"].as_array().unwrap().len(), 40);
    let q = crate::model_usage::UsageQuery {
        agent: "claude".into(),
        source: "gateway".into(),
        days: None,
        provider: None,
        model: None,
        cursor: None,
    };
    let t = std::time::Instant::now();
    let model = sql
        .model_usage(q.clone(), "asof".into(), "ready".into(), now())
        .unwrap();
    eprintln!("model={:?}", t.elapsed());
    assert_eq!(model["summary"]["attempts"], n);
    assert_eq!(model["summary"]["usage"]["totalTokens"], n * 110);
    let t = std::time::Instant::now();
    assert_eq!(
        sql.model_usage(q, "asof".into(), "ready".into(), now())
            .unwrap()["summary"],
        model["summary"]
    );
    eprintln!("model_warm_read={:?}", t.elapsed());
    let q = super::super::OverviewQuery {
        agent: None,
        range: 24,
        metric: "tokens".into(),
        dimension: "model".into(),
        source: "gateway".into(),
    };
    let t = std::time::Instant::now();
    let analysis = sql.analysis(&q, now()).unwrap();
    eprintln!("overview={:?}", t.elapsed());
    assert_eq!(analysis["samples"], n);
    assert_eq!(analysis["total"].as_f64().unwrap(), (n * 110) as f64);
}

#[test]
fn sql_associations_fees_revisions_and_revocation_match_canonical_global_evidence() {
    let mut catalog = crate::associated_cost_tests::catalog();
    let dir = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(dir.path()).unwrap();
    let mut sql = History::open(dir.path()).unwrap();
    let mut rows: Vec<_> = (0..91)
        .map(|i| {
            let mut r = row(i);
            r.session_id = Some("claude:one".into());
            r.agent_product_id = Some("claude-code-cli".into());
            r
        })
        .collect();
    rows[0].response_id = Some("response-one".into());
    rows[0].usage.input_tokens = Some(10);
    rows[0].usage.output_tokens = Some(5);
    rows[0].usage.total_tokens = Some(15);
    rows[0].usage.cached_input_tokens = Some(0);
    rows[1].response_id = Some("response-two".into());
    rows[1].session_id = None;
    rows[2].usage = Usage::default();
    for cycle in 0..6 {
        if cycle == 1 {
            rows[3].response_id = Some("response-one".into());
        }
        if cycle == 2 {
            rows[3].response_id = None;
            rows[0].session_id = Some("claude:two".into());
        }
        if cycle == 3 {
            rows[0].session_id = None;
        }
        if cycle == 4 {
            catalog.calls.clear();
            catalog.revision += 1;
        }
        if cycle == 5 {
            catalog.sessions.clear();
            catalog.revision += 1;
        }
        store.save_batch(&rows).unwrap();
        sql.refresh(&[]).unwrap();
        let epoch = sql.prepare_fees(&catalog).unwrap();
        assert_eq!(sql.prepare_fees(&catalog).unwrap(), epoch);
        let oracle = crate::associated_cost::Index::new(&catalog, rows.clone(), false);
        for agent in [
            None,
            Some("claude"),
            Some("claude-code-cli"),
            Some("claude-code-desktop"),
        ] {
            let ids = vec!["claude:one".into(), "claude:two".into()];
            let got = sql.fee_summary(&ids, agent, false).unwrap();
            assert_close(
                &got,
                &value(&oracle.summary(&ids, agent)).unwrap(),
                "fees summary",
            );
            let q = crate::associated_cost::detail::Query {
                scope: "project".into(),
                id: "project".into(),
                agent: agent.map(str::to_owned),
                ..Default::default()
            };
            let mut got = sql.fee_details(&ids, &q, false).unwrap();
            let mut expected = value(&oracle.details(&ids, &q).unwrap()).unwrap();
            without_cursor(&mut got);
            without_cursor(&mut expected);
            assert_close(&got, &expected, &format!("fees details cycle {cycle}"));
            for c in &catalog.calls {
                assert_close(
                    &value(&sql.response_cost(&c.id).unwrap()).unwrap(),
                    &value(&oracle.response_cost(&c.id)).unwrap(),
                    "response cost",
                );
            }
        }
        let native = requests::sources::native_records(catalog.calls.clone(), &HashMap::new());
        sql.sync_native_catalog(
            catalog.revision.to_string(),
            &HashMap::new(),
            &catalog.calls,
        )
        .unwrap();
        sql.refresh_native().unwrap();
        let mut gateway = requests::logical_requests(rows.clone(), &HashMap::new());
        let mut native = native;
        requests::sources::correlate(&mut gateway, &mut native);
        for n in &native {
            assert_close(
                &sql.request_detail(&n.id).unwrap().unwrap(),
                &value(n).unwrap(),
                "native detail",
            );
        }
        let q = requests::Query::default();
        let got = sql.source_report(&q, now(), vec![], None).unwrap();
        let expected = value(&requests::SourceReport::new(
            &gateway,
            &native,
            &q,
            now(),
            vec![],
            None,
        ))
        .unwrap();
        assert_close(&got, &expected, "source report");
    }
}
