//! Preexisting raw corpus: reuse can only be measured while that corpus stays
//! fixed. The original interleaved ingest/stage experiment is retained separately.
use super::*;
use rusqlite::trace::{TraceEvent, TraceEventCodes};
use std::cell::Cell;

thread_local! {
    static SOURCE_SCANS: Cell<usize> = const { Cell::new(0) };
}
fn trace(event: TraceEvent<'_>) {
    if let TraceEvent::Stmt(statement, _) = event {
        let sql = statement.sql();
        if sql.contains("model_observations") && sql.contains("ORDER BY rowid LIMIT") {
            SOURCE_SCANS.with(|count| count.set(count.get() + 1));
        }
    }
}

fn measure(count: usize) -> Value {
    let temp = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(temp.path()).unwrap();
    let mut requests = Vec::with_capacity(count);
    for first in (0..count).step_by(64) {
        let mut batch = Vec::new();
        for index in first..(first + 64).min(count) {
            let (_, mut rows) = retry();
            for row in &mut rows {
                row.id.push_str(&format!("-{index}"));
                row.logical_id = Some(format!("model-service-private-{index}"));
                row.session_id = Some(format!("private-session-{}", index / 10));
                let input = 100 + index as u64 % 1000;
                row.usage.input_tokens = Some(input);
                row.usage.total_tokens = row
                    .usage
                    .total_tokens
                    .map(|_| input + row.usage.output_tokens.unwrap_or(0));
            }
            let mut request = canonical(rows.clone());
            request.project_id = Some(format!("private-project-{}", index % 100));
            batch.extend(rows);
            requests.push(request);
        }
        store.save_batch(&batch).unwrap();
    }
    let source = source_hash(&store.0, PackedKind::Gateway);
    SOURCE_SCANS.with(|n| n.set(0));
    store
        .0
        .trace_v2(TraceEventCodes::SQLITE_TRACE_STMT, Some(trace));
    let started = Instant::now();
    let ledger = store.retention_ledger(STORE).unwrap();
    for batch in requests.chunks(64) {
        ledger
            .stage_attributed_batch(&batch.iter().collect::<Vec<_>>(), directory(), &[])
            .unwrap();
    }
    let staging_ms = started.elapsed().as_millis();
    let scans = SOURCE_SCANS.with(Cell::get);
    if std::env::var_os("SOURCE_SNAPSHOT_REQUIRE_REUSE").is_some() {
        assert_eq!(scans, 1, "unchanged corpus must be decoded once");
    }
    assert_eq!(source_hash(&store.0, PackedKind::Gateway), source);
    let summary = ledger.staging_summary().unwrap();
    assert_eq!(summary.requests, count);
    assert_eq!(summary.attempts, count * 4);
    let summary_digest = digest(&serde_json::to_string(&summary).unwrap());
    json!({"facts":count,"attempts":count*4,"sourceMode":"preexisting_fixed_corpus","stagingMs":staging_ms,"fullSourceScans":scans,"sourceByteDigest":source,"summaryDigest":summary_digest,"sourceBytesUnchanged":true,"fixtureOnly":true,"productCleanup":false})
}

#[test]
#[ignore = "explicit fixed-source staging experiment; private synthetic corpus only"]
fn fixed_source_snapshot_capacity() {
    let counts = std::env::var("SOURCE_SNAPSHOT_CASES")
        .unwrap_or_else(|_| "2000,10000".into())
        .split(',')
        .map(|n| n.parse::<usize>().unwrap())
        .collect::<Vec<_>>();
    assert!(counts.iter().all(|n| [2000, 10000].contains(n)));
    let cases = counts.into_iter().map(measure).collect::<Vec<_>>();
    if let Some(baseline) = std::env::var_os("SOURCE_SNAPSHOT_BASELINE_REPORT") {
        let old: Value = serde_json::from_slice(&std::fs::read(baseline).unwrap()).unwrap();
        for old in old["cases"].as_array().unwrap() {
            let current = cases.iter().find(|c| c["facts"] == old["facts"]).unwrap();
            assert_eq!(current["sourceByteDigest"], old["sourceByteDigest"]);
            assert_eq!(current["summaryDigest"], old["summaryDigest"]);
        }
    }
    let report = json!({"cases":cases,"fixtureOnly":true});
    println!("{report}");
    if let Some(path) = std::env::var_os("SOURCE_SNAPSHOT_REPORT") {
        std::fs::write(path, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    }
}
