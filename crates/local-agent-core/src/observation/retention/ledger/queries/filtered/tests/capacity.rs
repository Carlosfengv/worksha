//! Explicit synthetic query experiment; no original rows are deleted.
use super::*;
use std::time::Instant;

fn source_hash(db: &Connection) -> String {
    let mut hash = Sha256::new();
    let mut stmt = db
        .prepare("SELECT id,started_at,revision,value FROM main.model_observations ORDER BY id")
        .unwrap();
    let mut rows = stmt.query([]).unwrap();
    while let Some(row) = rows.next().unwrap() {
        for col in [0, 1, 3] {
            let value: String = row.get(col).unwrap();
            hash.update((value.len() as u64).to_be_bytes());
            hash.update(value.as_bytes());
        }
        hash.update(row.get::<_, i64>(2).unwrap().to_be_bytes());
    }
    format!("{:x}", hash.finalize())
}

#[test]
#[ignore = "explicit v3 filtered-statistics experiment; bounded private synthetic corpus"]
fn filtered_statistics_capacity() {
    let temp = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(temp.path()).unwrap();
    let base = at("2026-10-27T00:00:00Z");
    let mut data = vec![];
    for i in 0..5000 {
        let time = (base + chrono::Duration::seconds(i)).to_rfc3339();
        let product = if i % 2 == 0 {
            Some("codex-cli")
        } else {
            Some("codex-desktop")
        };
        let mut item = fixture(
            &format!("capacity-{i}"),
            "codex",
            product,
            &format!("session-{}", i % 100),
            &format!("project-{}", i % 50),
            &[&time],
        );
        if i % 33 == 0 {
            item.1[0].usage.total_tokens = None;
            item.0 = crate::requests::logical_requests(
                item.1.clone(),
                &std::collections::HashMap::new(),
            )
            .remove(0);
            item.0.project_id = Some(format!("project-{}", i % 50));
        }
        data.push(item);
    }
    for batch in data.chunks(64) {
        store
            .save_batch(
                &batch
                    .iter()
                    .flat_map(|(_, rows)| rows.iter().cloned())
                    .collect::<Vec<_>>(),
            )
            .unwrap();
    }
    let raw = source_hash(&store.0);
    let begin = Instant::now();
    let ledger = store.retention_ledger(STORE).unwrap();
    for batch in data.chunks(64) {
        ledger
            .stage_queryable_batch(
                &batch.iter().map(|(r, _)| r).collect::<Vec<_>>(),
                directory(),
                &[],
            )
            .unwrap();
    }
    let staging_ms = begin.elapsed().as_millis();
    ledger.release_source_snapshot();
    let filter = GatewayFilter {
        start: Some(base + chrono::Duration::minutes(10)),
        end: Some(base + chrono::Duration::minutes(50)),
        agent: Some(AgentFilter::new(directory(), "codex").unwrap()),
        session: Some("session-1".into()),
        project: Some("project-1".into()),
    };
    let selected = data
        .iter()
        .filter(|(r, _)| {
            in_window(&r.started_at, &filter)
                && r.session_id.as_deref() == Some("session-1")
                && r.project_id.as_deref() == Some("project-1")
                && directory().matches(Some("codex"), &r.agent_id, r.agent_product_id.as_deref())
        })
        .collect::<Vec<_>>();
    assert_eq!(selected.len(), 24);
    let expected = serde_json::to_value(Summary::of(selected.iter().map(|(r, _)| r))).unwrap();
    let selected_rows = selected
        .iter()
        .flat_map(|(_, rows)| rows.iter().cloned())
        .collect::<Vec<_>>();
    let provider = provider_total(&selected_rows, None);
    let model = model_total(&selected_rows, None);
    let begin = Instant::now();
    same(
        serde_json::to_value(ledger.staging_summary_filtered(&filter).unwrap()).unwrap(),
        expected.clone(),
    );
    same(
        ledger
            .staging_provider_contribution_filtered(None, &filter)
            .unwrap(),
        provider.clone(),
    );
    same(
        ledger
            .staging_model_contribution_filtered(None, &filter)
            .unwrap(),
        model.clone(),
    );
    let staging_query_ms = begin.elapsed().as_millis();
    let (_, cold_hash_checks, cold_full_checks) = ledger.validation_cache.borrow().counts();
    assert_eq!((cold_hash_checks, cold_full_checks), (15_000, 5000));
    let begin = Instant::now();
    same(
        serde_json::to_value(ledger.staging_summary_filtered(&filter).unwrap()).unwrap(),
        expected.clone(),
    );
    same(
        ledger
            .staging_provider_contribution_filtered(None, &filter)
            .unwrap(),
        provider.clone(),
    );
    same(
        ledger
            .staging_model_contribution_filtered(None, &filter)
            .unwrap(),
        model.clone(),
    );
    let warm_query_ms = begin.elapsed().as_millis();
    let (entries, all_hash_checks, all_full_checks) = ledger.validation_cache.borrow().counts();
    assert_eq!(entries, 5000);
    assert_eq!((all_hash_checks, all_full_checks), (30_000, 5000));
    ledger.release_query_validation_cache();
    assert_eq!(ledger.validation_cache.borrow().counts().0, 0);
    let begin = Instant::now();
    let preview = pack(&ledger);
    let packing_and_restore_ms = begin.elapsed().as_millis();
    let begin = Instant::now();
    same(
        serde_json::to_value(preview.gateway_summary_filtered(&filter).unwrap()).unwrap(),
        expected,
    );
    same(
        preview
            .provider_contribution_filtered(None, &filter)
            .unwrap(),
        provider,
    );
    same(
        preview.model_contribution_filtered(None, &filter).unwrap(),
        model,
    );
    let restored_query_ms = begin.elapsed().as_millis();
    assert_eq!(source_hash(&store.0), raw);
    if let Some(path) = std::env::var_os("FILTERED_STATISTICS_BASELINE_REPORT") {
        let old: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        assert_eq!(old["sourceByteDigest"], raw);
        assert_eq!(old["facts"], 5000);
        assert_eq!(old["selectedFacts"], 24);
    }
    let report = serde_json::json!({"facts":5000,"attempts":5000,"selectedFacts":24,"projectionVersion":3,"sourceMode":"preexisting_fixed_corpus","stagingMs":staging_ms,"stagingQueriesMs":staging_query_ms,"warmStagingQueriesMs":warm_query_ms,"coldProjectionHashChecks":cold_hash_checks,"coldFullSemanticChecks":cold_full_checks,"warmProjectionHashChecks":all_hash_checks-cold_hash_checks,"warmFullSemanticChecks":all_full_checks-cold_full_checks,"retainedValidationEntries":entries,"validationCacheExplicitlyReleasedBeforePacking":true,"packingAndRestoreMs":packing_and_restore_ms,"restoredQueriesMs":restored_query_ms,"sourceByteDigest":raw,"sourceBytesUnchanged":true,"logicalProviderModelAllFieldsReconciled":true,"fixtureOnly":true,"productDeletion":false,"realModelCalls":0,"releasePerformanceAcceptance":false});
    println!("{report}");
    if let Some(path) = std::env::var_os("FILTERED_STATISTICS_REPORT") {
        std::fs::write(path, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    }
}
