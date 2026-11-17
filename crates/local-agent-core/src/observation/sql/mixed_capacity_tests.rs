use super::request_cells_tests::catalog;
use super::*;
use std::{sync::Arc, time::Instant};

#[test]
#[ignore = "explicit mixed-source capacity benchmark with generated history"]
fn mixed_observation_capacity() {
    let setting = |name, default| {
        std::env::var(name)
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(default)
    };
    let files = setting("LOCAL_AGENT_NATIVE_FILES", 1000);
    let calls = setting("LOCAL_AGENT_NATIVE_CALLS", 950);
    let gateway_count = setting("LOCAL_AGENT_MIXED_GATEWAY_ROWS", 50_000);
    let dir = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(dir.path()).unwrap();
    let mut catalog = catalog(files, calls);
    for (file, batch) in catalog.calls.chunks.iter_mut().enumerate() {
        for (index, call) in Arc::make_mut(batch).iter_mut().enumerate() {
            call.provider = Some(format!("native-fixture-{}", file % 5));
            if index % 17 == 16 {
                call.usage = None;
            }
        }
    }
    let at = now();
    let native_agent = catalog.calls[0].agent_id.clone();
    let product = catalog_product(&native_agent);
    let start = at - chrono::Duration::hours(6);
    let end = at;
    let request = requests::Query {
        start: Some(start.to_rfc3339()),
        end: Some(end.to_rfc3339()),
        ..Default::default()
    };
    let overview = crate::observation::OverviewQuery {
        agent: Some(native_agent.clone()),
        range: 6,
        metric: "calls".into(),
        dimension: "model".into(),
        source: String::new(),
    };
    let model = crate::model_usage::UsageQuery {
        agent: native_agent.clone(),
        source: String::new(),
        days: None,
        provider: None,
        model: None,
        cursor: None,
    };
    let gateway = |number: usize, linked: Option<&crate::sessions::CallSummary>| {
        let mut row = row(number);
        row.agent_id = native_agent.clone();
        row.agent_product_id = product.clone();
        row.started_at = (at - chrono::Duration::minutes((number % 300 + 1) as i64)).to_rfc3339();
        row.ended_at = Some(at.to_rfc3339());
        row.model = Some(format!("model-{}", number % 31));
        row.provider = format!("gateway-fixture-{}", number % 24);
        row.provider_id = Some(row.provider.clone());
        if number % 13 == 12 {
            row.ttft_ms = None;
            row.duration_ms = None;
        }
        if let Some(call) = linked {
            row.agent_product_id = call.agent_product_id.clone();
            row.session_id = Some(call.session_id.clone());
            row.response_id = call.response_id.clone();
        }
        row
    };
    for first in (0..gateway_count).step_by(256) {
        let rows: Vec<_> = (first..(first + 256).min(gateway_count))
            .map(|i| gateway(i, (i == 0).then_some(&catalog.calls[0])))
            .collect();
        store.save_batch(&rows).unwrap();
    }
    let mut h = History::open(dir.path()).unwrap();
    h.refresh(&[]).unwrap();
    h.sync_native_catalog("first".into(), &HashMap::new(), &catalog.calls)
        .unwrap();
    h.sync_native_analysis(&catalog).unwrap();
    let query = |h: &mut History, catalog: &crate::projects::ProjectCatalog, source: &str| {
        let q = requests::Query {
            source: Some(source.into()),
            ..request.clone()
        };
        let started = Instant::now();
        let report = h.source_report(&q, at, vec![], None).unwrap();
        let page = h.request_page(&q, at, report, None).unwrap();
        let request_ms = started.elapsed().as_secs_f64() * 1000.0;
        let started = Instant::now();
        let analysis = h
            .analysis(
                &crate::observation::OverviewQuery {
                    source: source.into(),
                    ..overview.clone()
                },
                at,
            )
            .unwrap();
        let overview_ms = started.elapsed().as_secs_f64() * 1000.0;
        let started = Instant::now();
        if source == "unified" {
            h.sync_unified_native_models(catalog).unwrap();
        } else if source == "native" {
            h.sync_native_models(catalog, &native_agent).unwrap();
        }
        let models = h
            .model_usage(
                crate::model_usage::UsageQuery {
                    source: source.into(),
                    ..model.clone()
                },
                at.to_rfc3339(),
                "ready".into(),
                at,
            )
            .unwrap();
        let model_ms = started.elapsed().as_secs_f64() * 1000.0;
        (
            (page, analysis, models),
            [request_ms, overview_ms, model_ms],
        )
    };
    let ((legacy_gateway, _, _), gateway_ms) = query(&mut h, &catalog, "gateway");
    let reset_request_queries = |h: &mut History| {
        // A source tab can warm request report blocks for the other tabs.
        // Each cold measurement starts without those disposable aggregates.
        h.request_reports.borrow_mut().clear();
        h.request_templates.clear();
        h.snapshots.clear();
        h.db.execute_batch(
            "DELETE FROM positions;DELETE FROM native_request_cells;
             DELETE FROM native_request_dirty;DELETE FROM native_request_clock;",
        )
        .unwrap();
    };
    reset_request_queries(&mut h);
    let ((legacy_native, _, _), native_ms) = query(&mut h, &catalog, "native");
    reset_request_queries(&mut h);
    let ((first, analytics, models), cold_ms) = query(&mut h, &catalog, "unified");
    let expected = files * calls + gateway_count - 1;
    assert_eq!(first["total"], expected);
    assert_eq!(
        first["total"].as_u64().unwrap() + 1,
        legacy_gateway["total"].as_u64().unwrap() + legacy_native["total"].as_u64().unwrap()
    );
    assert_eq!(analytics["samples"], expected);
    assert_eq!(models["summary"]["attempts"], expected);
    h.db.execute_batch(
        "CREATE TEMP TABLE capacity_blocks(kind TEXT,bucket INTEGER,PRIMARY KEY(kind,bucket));
         CREATE TEMP TRIGGER capacity_request_blocks AFTER INSERT ON native_request_cells BEGIN
           INSERT OR IGNORE INTO capacity_blocks VALUES('request',new.bucket);
         END;
         CREATE TEMP TRIGGER capacity_model_blocks AFTER INSERT ON native_model_blocks BEGIN
           INSERT OR IGNORE INTO capacity_blocks VALUES('model',new.bucket);
         END;",
    )
    .unwrap();
    let changed_blocks = |h: &History, kind: &str| {
        h.db.query_row(
            "SELECT count(*) FROM capacity_blocks WHERE kind=?1",
            [kind],
            |row| sql_count(row, 0),
        )
        .unwrap()
    };
    let clock = |h: &History| {
        h.db.query_row("SELECT seq FROM position_clock", [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap()
    };
    let position = clock(&h);
    let mut warm_ms = vec![];
    for _ in 0..20 {
        let ((page, overview, model), elapsed) = query(&mut h, &catalog, "unified");
        assert_eq!(page["summary"], first["summary"]);
        assert_eq!(overview["total"], analytics["total"]);
        assert_eq!(model["summary"], models["summary"]);
        warm_ms.push(elapsed);
    }
    assert_eq!(clock(&h), position);
    assert_eq!(changed_blocks(&h, "request"), 0);
    assert_eq!(changed_blocks(&h, "model"), 0);
    let mut appended = catalog.calls[0].clone();
    appended.id = "mixed-new-response".into();
    appended.response_id = Some(appended.id.clone());
    Arc::make_mut(catalog.calls.chunks.last_mut().unwrap()).push(appended.clone());
    catalog.revision += 1;
    h.sync_native_catalog("append".into(), &HashMap::new(), &catalog.calls)
        .unwrap();
    let mut updated = vec![];
    let mut updated_blocks = vec![];
    let mut rebuilt_positions = vec![];
    for phase in ["native", "late-gateway", "ambiguity"] {
        h.db.execute_batch("DELETE FROM capacity_blocks").unwrap();
        let position = clock(&h);
        let expected = if phase == "ambiguity" {
            expected + 3
        } else {
            expected + 1
        };
        if phase != "native" {
            let number = gateway_count + usize::from(phase == "ambiguity");
            store.save(&gateway(number, Some(&appended))).unwrap();
            h.refresh(&[]).unwrap();
            h.refresh_native().unwrap();
        }
        let ((page, overview, model), elapsed) = query(&mut h, &catalog, "unified");
        assert_eq!(page["total"], expected, "{phase}");
        assert_eq!(overview["samples"], expected, "{phase}");
        assert_eq!(model["summary"]["attempts"], expected, "{phase}");
        assert!(clock(&h) - position <= 1);
        rebuilt_positions.push(clock(&h) - position);
        updated_blocks.push(json!({
            "phase": phase,
            "request": changed_blocks(&h, "request"),
            "model": changed_blocks(&h, "model"),
        }));
        updated.push(elapsed);
    }
    println!(
        "MIXED_OBSERVATION {}",
        json!({
            "nativeRows":files*calls, "gatewayRows":gateway_count,
            "coldMs":cold_ms, "legacyGatewayMs":gateway_ms, "legacyNativeMs":native_ms,
            "warmMs":warm_ms, "updatedMs":updated,
            "updatedBlocks":updated_blocks, "rebuiltPositions":rebuilt_positions
        })
    );
}

fn catalog_product(family: &str) -> Option<String> {
    crate::agents::builtin_directory()
        .products()
        .iter()
        .find(|product| product.family == family)
        .map(|product| product.id.clone())
}
