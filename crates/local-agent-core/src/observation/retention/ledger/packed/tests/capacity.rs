//! Explicit offline storage experiment. Only synthetic files in a TempDir are
//! removed in the packed-only measurement copy; no product cleanup API is called.
use super::*;
use std::{path::Path, time::Instant};
mod fixed_source;

fn size(path: &Path) -> u64 {
    std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}
fn wal(path: &Path) -> u64 {
    size(&std::path::PathBuf::from(format!("{}-wal", path.display())))
}
fn checkpoint(db: &Connection) {
    db.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
        .unwrap();
}
fn raw_copy_bytes(db: &Connection, path: &Path) -> u64 {
    db.execute("VACUUM INTO ?1", [path.to_str().unwrap()])
        .unwrap();
    let copy = Connection::open(path).unwrap();
    copy.execute_batch("DROP TRIGGER IF EXISTS retention_source_insert_v1;DROP TRIGGER IF EXISTS retention_source_update_v1;DROP TRIGGER IF EXISTS retention_source_delete_v1;DROP TABLE IF EXISTS retention_source_clock;DROP TABLE IF EXISTS retention_packed_members;DROP TABLE IF EXISTS retention_packed_blocks;DROP TABLE IF EXISTS retention_packed_watermark;DROP TABLE IF EXISTS retention_fact_staging;DROP TABLE IF EXISTS retention_tool_staging;DROP TABLE IF EXISTS retention_fact_scope;VACUUM;").unwrap();
    drop(copy);
    size(path)
}
fn packed_copy_bytes(db: &Connection, path: &Path, kind: PackedKind) -> u64 {
    db.execute("VACUUM INTO ?1", [path.to_str().unwrap()])
        .unwrap();
    let copy = Connection::open(path).unwrap();
    tools::create_staging(&copy).unwrap();
    // Measurement copy only. Keep empty staging tables so this cannot be
    // mistaken for a published artifact: the normal draft reader must reject it.
    copy.execute_batch("DELETE FROM retention_fact_staging;DELETE FROM retention_tool_staging;DROP TABLE IF EXISTS model_observations;DROP TABLE IF EXISTS retention_source_clock;DROP TABLE IF EXISTS connector_calls;VACUUM;").unwrap();
    assert_eq!(
        copy.query_row("PRAGMA integrity_check", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "ok"
    );
    let namespace = copy
        .query_row("SELECT namespace FROM retention_fact_scope", [], |r| {
            r.get::<_, String>(0)
        })
        .unwrap();
    let ledger = FactLedger {
        db: &copy,
        store_id: STORE.into(),
        namespace,
        immutable_snapshot: false,
        source: None,
        validation_cache: RefCell::default(),
    };
    assert_eq!(
        ledger.packed_preview(kind).err().unwrap(),
        LedgerError::SourceChanged
    );
    drop(ledger);
    drop(copy);
    size(path)
}
fn source_hash(db: &Connection, kind: PackedKind) -> String {
    let sql = match kind {
        PackedKind::Gateway => "SELECT id,value FROM model_observations ORDER BY id",
        PackedKind::ConnectorTool => {
            "SELECT CAST(id AS TEXT),plugin_id || char(0) || identity || char(0) || record FROM connector_calls ORDER BY id"
        }
    };
    let mut stmt = db.prepare(sql).unwrap();
    let mut rows = stmt.query([]).unwrap();
    let mut hash = Sha256::new();
    while let Some(row) = rows.next().unwrap() {
        for column in [0, 1] {
            let text: String = row.get(column).unwrap();
            hash.update((text.len() as u64).to_be_bytes());
            hash.update(text.as_bytes());
        }
    }
    hex(&hash.finalize())
}
fn working_bytes(preview: &PackedPreview) -> i64 {
    let pages: i64 = preview
        .db
        .query_row("PRAGMA page_count", [], |r| r.get(0))
        .unwrap();
    let size: i64 = preview
        .db
        .query_row("PRAGMA page_size", [], |r| r.get(0))
        .unwrap();
    pages * size
}
fn pack_keys(ledger: &FactLedger<'_>, kind: PackedKind, ids: Vec<String>) {
    for chunk in ids.chunks(64) {
        ledger
            .pack_staging_batch(kind, &chunk.iter().map(String::as_str).collect::<Vec<_>>())
            .unwrap();
    }
}
fn measure_tools(count: i64) -> Value {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("tools.db");
    let db = tool_db(&path);
    db.execute_batch(
        "PRAGMA journal_mode=WAL;PRAGMA wal_autocheckpoint=1000;PRAGMA cache_size=-32768;",
    )
    .unwrap();
    let start = Instant::now();
    let base: chrono::DateTime<chrono::Utc> = "2026-10-27T00:00:00Z".parse().unwrap();
    for chunk in (1..=count).collect::<Vec<_>>().chunks(64) {
        let tx = db.unchecked_transaction().unwrap();
        for id in chunk {
            let states = [
                "success",
                "tool_error",
                "protocol_error",
                "transport_error",
                "timeout",
                "cancelled",
                "incomplete",
            ];
            let state = states[*id as usize % states.len()];
            let duration = matches!(state, "success" | "tool_error" | "protocol_error")
                .then_some((*id as f64 * 1.61803398875) % 30000.0);
            let at = (base + chrono::Duration::nanoseconds(*id * 10000003)).to_rfc3339();
            tool(&tx, *id, state, duration, &at);
        }
        tx.commit().unwrap();
    }
    checkpoint(&db);
    let raw_bytes = size(&path);
    let raw_wal = wal(&path);
    let before = source_hash(&db, PackedKind::ConnectorTool);
    let ledger = FactLedger::for_connector_store(&db, STORE).unwrap();
    let mut wal_peak = raw_wal;
    let staging_started = Instant::now();
    for chunk in (1..=count).collect::<Vec<_>>().chunks(64) {
        ledger.stage_tool_batch(chunk).unwrap();
        wal_peak = wal_peak.max(wal(&path));
    }
    checkpoint(&db);
    let with_stage = size(&path);
    let staging_ms = staging_started.elapsed().as_millis();
    let packing_started = Instant::now();
    for chunk in keys(&db, PackedKind::ConnectorTool).chunks(64) {
        ledger
            .pack_staging_batch(
                PackedKind::ConnectorTool,
                &chunk.iter().map(String::as_str).collect::<Vec<_>>(),
            )
            .unwrap();
        wal_peak = wal_peak.max(wal(&path));
    }
    checkpoint(&db);
    let migration_peak = size(&path);
    let packing_ms = packing_started.elapsed().as_millis();
    let read_started = Instant::now();
    let preview = ledger.packed_preview(PackedKind::ConnectorTool).unwrap();
    let mut expected = ledger.tool_staging_summary(&ToolFilter::default()).unwrap();
    expected.coverage = "packed_draft";
    assert_eq!(
        value(preview.tool_summary(&ToolFilter::default()).unwrap()),
        value(expected)
    );
    assert_eq!(preview.coverage().unpacked_facts, 0);
    let preview_ms = read_started.elapsed().as_millis();
    let restored_bytes = working_bytes(&preview);
    drop(preview);
    let packed_bytes = packed_copy_bytes(
        &db,
        &temp.path().join("packed-only-measurement.db"),
        PackedKind::ConnectorTool,
    );
    assert_eq!(source_hash(&db, PackedKind::ConnectorTool), before);
    let result = json!({"kind":"connector_tool","facts":count,"rawDatabaseBytes":raw_bytes,"rawWalAfterCheckpoint":raw_wal,"databaseWithStagingBytes":with_stage,"databaseWithStagingAndPackedBytes":migration_peak,"sampledWalPeakBytes":wal_peak,"packedOnlyMeasurementCopyBytes":packed_bytes,"restoredWorkingDatabaseLogicalBytes":restored_bytes,"sourceRowsByteHashUnchanged":true,"packedToRawRatio":packed_bytes as f64/raw_bytes as f64,"stagingMs":staging_ms,"packingMs":packing_ms,"restoredSummaryMs":preview_ms,"totalInstrumentedMs":start.elapsed().as_millis(),"allSummaryFieldsExact":true,"originalSourceRows":db.query_row("SELECT count(*) FROM connector_calls",[],|r|r.get::<_,i64>(0)).unwrap(),"sourceAndStagingDeletedOnlyInMeasurementCopy":true,"publishedArtifact":false,"sourceFixtureHasCapacityTrigger":false});
    assert_eq!(
        db.query_row("SELECT count(*) FROM connector_calls", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        count
    );
    drop(ledger);
    drop(db);
    drop(temp);
    result
}
fn measure_gateway(count: usize) -> Value {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("observations.sqlite3");
    let store = ObservationStore::open(temp.path()).unwrap();
    let ledger = store.retention_ledger(STORE).unwrap();
    let start = Instant::now();
    let mut all_keys = Vec::new();
    let mut wal_peak = 0;
    for first in (0..count).step_by(64) {
        let mut requests = Vec::new();
        let mut source = Vec::new();
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
            source.extend(rows);
            requests.push(request);
        }
        store.save_batch(&source).unwrap();
        ledger
            .stage_attributed_batch(&requests.iter().collect::<Vec<_>>(), directory(), &[])
            .unwrap();
        wal_peak = wal_peak.max(wal(&path));
    }
    checkpoint(ledger.db);
    let with_stage = size(&path);
    let before = source_hash(ledger.db, PackedKind::Gateway);
    let raw_bytes = raw_copy_bytes(ledger.db, &temp.path().join("raw-only.db"));
    let pack_started = Instant::now();
    all_keys.extend(keys(ledger.db, PackedKind::Gateway));
    pack_keys(&ledger, PackedKind::Gateway, all_keys);
    wal_peak = wal_peak.max(wal(&path));
    checkpoint(ledger.db);
    let migration_peak = size(&path);
    let packing_ms = pack_started.elapsed().as_millis();
    let read_started = Instant::now();
    let preview = ledger.packed_preview(PackedKind::Gateway).unwrap();
    assert_eq!(
        value(preview.gateway_summary().unwrap()),
        value(ledger.staging_summary().unwrap())
    );
    assert_eq!(
        preview.provider_contribution(None).unwrap(),
        ledger.staging_provider_contribution(None).unwrap()
    );
    assert_eq!(
        preview.model_contribution(None).unwrap(),
        ledger.staging_model_contribution(None).unwrap()
    );
    let preview_ms = read_started.elapsed().as_millis();
    let restored_bytes = working_bytes(&preview);
    drop(preview);
    let packed_bytes = packed_copy_bytes(
        ledger.db,
        &temp.path().join("packed-only-measurement.db"),
        PackedKind::Gateway,
    );
    assert_eq!(source_hash(ledger.db, PackedKind::Gateway), before);
    let source_rows: i64 = ledger
        .db
        .query_row("SELECT count(*) FROM model_observations", [], |r| r.get(0))
        .unwrap();
    let result = json!({"kind":"gateway_four_attempts","facts":count,"rawDatabaseBytes":raw_bytes,"databaseWithStagingBytes":with_stage,"databaseWithStagingAndPackedBytes":migration_peak,"sampledWalPeakBytes":wal_peak,"packedOnlyMeasurementCopyBytes":packed_bytes,"restoredWorkingDatabaseLogicalBytes":restored_bytes,"sourceRowsByteHashUnchanged":true,"packedToRawRatio":packed_bytes as f64/raw_bytes as f64,"packingMs":packing_ms,"restoredSummariesMs":preview_ms,"totalInstrumentedMs":start.elapsed().as_millis(),"logicalProviderModelSummaryFieldsExact":true,"originalSourceRows":source_rows,"sourceAndStagingDeletedOnlyInMeasurementCopy":true,"publishedArtifact":false});
    assert_eq!(source_rows, count as i64 * 4);
    drop(ledger);
    drop(store);
    drop(temp);
    result
}

#[test]
#[ignore = "explicit offline storage experiment; bounded synthetic fixtures only"]
fn packed_capacity_benchmark() {
    let mut reports = Vec::new();
    for n in [2000, 10000] {
        let report = measure_gateway(n);
        println!("{report}");
        reports.push(report);
    }
    for n in [10000, 100000] {
        let report = measure_tools(n);
        println!("{report}");
        reports.push(report);
    }
    if let Some(path) = std::env::var_os("LOCAL_AGENT_PACKED_CAPACITY_REPORT") {
        std::fs::write(
            path,
            serde_json::to_vec_pretty(&json!({"schema":1,"fixturesOnly":true,"cases":reports}))
                .unwrap(),
        )
        .unwrap();
    }
}
