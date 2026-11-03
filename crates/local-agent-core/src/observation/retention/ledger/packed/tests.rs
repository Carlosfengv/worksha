use super::*;
use crate::observation::{
    ObservationStore,
    retention::test_support::{STORE, canonical, directory, retry},
};
use serde_json::{Value, json};
mod capacity;

fn keys(db: &Connection, kind: PackedKind) -> Vec<String> {
    db.prepare(&format!("SELECT key FROM {} ORDER BY key", kind.table()))
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}
fn pack_all(ledger: &FactLedger<'_>, kind: PackedKind) -> PackedBatch {
    let ids = keys(ledger.db, kind);
    ledger
        .pack_staging_batch(kind, &ids.iter().map(String::as_str).collect::<Vec<_>>())
        .unwrap()
}
fn gateway(store: &ObservationStore) -> FactLedger<'_> {
    let (request, rows) = retry();
    store.save_batch(&rows).unwrap();
    let ledger = store.retention_ledger(STORE).unwrap();
    ledger
        .stage_attributed_batch(&[&request], directory(), &[])
        .unwrap();
    ledger
}
fn value(v: impl Serialize) -> Value {
    serde_json::to_value(v).unwrap()
}
fn tool_db(path: &std::path::Path) -> Connection {
    let db = Connection::open(path).unwrap();
    db.execute_batch("CREATE TABLE extension_version(version INTEGER NOT NULL);INSERT INTO extension_version VALUES(1);CREATE TABLE connector_calls(id INTEGER PRIMARY KEY,plugin_id TEXT NOT NULL,identity TEXT NOT NULL,record TEXT NOT NULL,UNIQUE(plugin_id,identity));").unwrap();
    db
}
fn tool(db: &Connection, id: i64, status: &str, duration: Option<f64>, time: &str) {
    let record = json!({"identity":format!("private-tool-{id}"),"rpcId":"private-rpc","mcpConnection":"private-connection","tool":if id%2==0{"search"}else{"read"},"status":status,"durationMs":duration,"observedAt":time,"agentSession":null,"content":"not-persisted","modelTokens":null});
    db.execute(
        "INSERT INTO connector_calls VALUES(?1,?2,?3,?4)",
        params![
            id,
            if id % 2 == 0 { "a" } else { "b" },
            record["identity"].as_str().unwrap(),
            record.to_string()
        ],
    )
    .unwrap();
}

#[test]
fn packed_gateway_bytes_restore_every_existing_contribution_without_private_source_identity() {
    let temp = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(temp.path()).unwrap();
    let ledger = gateway(&store);
    let original = store
        .load_all()
        .unwrap()
        .into_iter()
        .map(|r| serde_json::to_string(&r).unwrap())
        .collect::<Vec<_>>();
    let batch = pack_all(&ledger, PackedKind::Gateway);
    assert_eq!(batch.facts, 1);
    assert!(batch.compressed_payload_bytes < batch.frame_bytes);
    let preview = ledger.packed_preview(PackedKind::Gateway).unwrap();
    assert_eq!(preview.coverage().unpacked_facts, 0);
    assert_eq!(
        value(preview.gateway_summary().unwrap()),
        value(ledger.staging_summary().unwrap())
    );
    assert_eq!(
        value(preview.attribution_coverage().unwrap()),
        value(ledger.attribution_coverage().unwrap())
    );
    assert_eq!(
        value(
            preview
                .scope_summary(Some("private-session"), Some("private-project-id"))
                .unwrap()
        ),
        value(
            ledger
                .staging_scope_summary(Some("private-session"), Some("private-project-id"))
                .unwrap()
        )
    );
    for (dimension, id) in [
        (PartDimension::Provider, "provider-a"),
        (PartDimension::Model, "routed-b"),
    ] {
        assert_eq!(
            value(preview.part_summary(dimension, id).unwrap()),
            value(ledger.staging_part_summary(dimension, id).unwrap())
        );
    }
    for id in [None, Some("provider-a"), Some("_unknown")] {
        assert_eq!(
            preview.provider_contribution(id).unwrap(),
            ledger.staging_provider_contribution(id).unwrap()
        );
    }
    let (_, rows) = retry();
    let scope = crate::model_usage::provider_identity(
        Some(&rows[0].provider),
        Some(&rows[0].endpoint),
        Some(rows[0].binding_revision),
    );
    for scope in [None, Some(scope.as_str())] {
        assert_eq!(
            preview.model_contribution(scope).unwrap(),
            ledger.staging_model_contribution(scope).unwrap()
        );
    }
    assert_eq!(
        preview.tool_summary(&ToolFilter::default()).err(),
        Some(LedgerError::InvalidQuery)
    );
    let block = read_block(
        ledger.db,
        &unhex(&batch.block).unwrap(),
        STORE,
        &ledger.namespace,
        PackedKind::Gateway,
    )
    .unwrap();
    let source: String = ledger
        .db
        .query_row("SELECT value FROM retention_fact_staging", [], |r| r.get(0))
        .unwrap();
    assert_eq!(block.frame.entries[0].value, source);
    assert!(!source.contains("private-"));
    assert_eq!(
        store
            .load_all()
            .unwrap()
            .into_iter()
            .map(|r| serde_json::to_string(&r).unwrap())
            .collect::<Vec<_>>(),
        original
    );
}

#[test]
fn packed_tools_keep_combined_p95_zero_unknown_and_exact_time_filtering() {
    let temp = tempfile::tempdir().unwrap();
    let db = tool_db(&temp.path().join("tools.db"));
    let ledger = FactLedger::for_connector_store(&db, STORE).unwrap();
    for (id, status, duration, time) in [
        (1, "success", Some(0.0), "2026-10-27T00:00:00Z"),
        (
            2,
            "tool_error",
            Some(1.25),
            "2026-10-27T08:00:00.000000001+08:00",
        ),
        (
            3,
            "protocol_error",
            Some(100.25),
            "2026-10-27T00:00:00.000000002Z",
        ),
        (4, "timeout", None, "unknown"),
    ] {
        tool(&db, id, status, duration, time);
    }
    ledger.stage_tool_batch(&[1, 2, 3, 4]).unwrap();
    pack_all(&ledger, PackedKind::ConnectorTool);
    let preview = ledger.packed_preview(PackedKind::ConnectorTool).unwrap();
    for filter in [
        ToolFilter::default(),
        ToolFilter {
            plugin_id: Some("a".into()),
            ..Default::default()
        },
        ToolFilter {
            tool: Some("search".into()),
            start: Some("2026-10-27T00:00:00.000000001Z".parse().unwrap()),
            end: Some("2026-10-27T00:00:00.000000002Z".parse().unwrap()),
            ..Default::default()
        },
    ] {
        let mut expected = ledger.tool_staging_summary(&filter).unwrap();
        expected.coverage = "packed_draft";
        assert_eq!(
            value(preview.tool_summary(&filter).unwrap()),
            value(expected)
        );
    }
    assert_eq!(
        preview.gateway_summary().err(),
        Some(LedgerError::InvalidQuery)
    );
    let raw = read_block(
        &db,
        &db.query_row("SELECT id FROM retention_packed_blocks", [], |r| {
            r.get::<_, Vec<u8>>(0)
        })
        .unwrap(),
        STORE,
        &ledger.namespace,
        PackedKind::ConnectorTool,
    )
    .unwrap();
    assert!(
        !serde_json::to_string(&raw.frame)
            .unwrap()
            .contains("private-")
    );
}

#[test]
fn packed_numeric_json_spelling_and_u64_precision_are_not_roundtripped() {
    let temp = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(temp.path()).unwrap();
    let ledger = gateway(&store);
    let mut v: Value = serde_json::from_str(
        &ledger
            .db
            .query_row("SELECT value FROM retention_fact_staging", [], |r| {
                r.get::<_, String>(0)
            })
            .unwrap(),
    )
    .unwrap();
    v["stats"]["usage"]["inputTokens"] = json!(u64::MAX);
    v["stats"]["duration"]["p50"] = serde_json::from_str("1.0e3").unwrap();
    let exact = v.to_string();
    ledger
        .db
        .execute(
            "UPDATE retention_fact_staging SET value=?1,projection_digest=?2",
            params![exact, digest(&exact)],
        )
        .unwrap();
    let batch = pack_all(&ledger, PackedKind::Gateway);
    let block = read_block(
        ledger.db,
        &unhex(&batch.block).unwrap(),
        STORE,
        &ledger.namespace,
        PackedKind::Gateway,
    )
    .unwrap();
    assert_eq!(block.frame.entries[0].value, exact);
    assert!(exact.contains("18446744073709551615"));
    assert_eq!(
        value(
            ledger
                .packed_preview(PackedKind::Gateway)
                .unwrap()
                .gateway_summary()
                .unwrap()
        ),
        value(ledger.staging_summary().unwrap())
    );
}

#[test]
fn partial_packing_reports_coverage_and_old_summary_only_facts_stay_summary_only() {
    let temp = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(temp.path()).unwrap();
    let ledger = store.retention_ledger(STORE).unwrap();
    let (_, mut rows) = retry();
    rows.truncate(1);
    rows[0].logical_id = None;
    for id in ["a", "b"] {
        rows[0].id = id.into();
        let r = canonical(rows.clone());
        ledger.stage_batch(&[(&r, [1; 32])], directory()).unwrap();
    }
    let ids = keys(ledger.db, PackedKind::Gateway);
    ledger
        .pack_staging_batch(PackedKind::Gateway, &[&ids[0]])
        .unwrap();
    let preview = ledger.packed_preview(PackedKind::Gateway).unwrap();
    assert_eq!(
        (
            preview.coverage().packed_facts,
            preview.coverage().staged_facts,
            preview.coverage().unpacked_facts
        ),
        (1, 2, 1)
    );
    assert_eq!(preview.gateway_summary().unwrap().requests, 1);
    assert_eq!(
        preview.provider_contribution(None).err(),
        Some(LedgerError::IncompleteFacts)
    );
}

#[test]
fn stale_drafts_are_rejected_and_repacking_collects_only_unreferenced_blocks() {
    let temp = tempfile::tempdir().unwrap();
    let db = tool_db(&temp.path().join("tools.db"));
    let ledger = FactLedger::for_connector_store(&db, STORE).unwrap();
    for id in [1, 2] {
        tool(&db, id, "success", Some(0.0), "2026-10-27T00:00:00Z");
    }
    ledger.stage_tool_batch(&[1, 2]).unwrap();
    let first = pack_all(&ledger, PackedKind::ConnectorTool);
    let old = ledger.packed_preview(PackedKind::ConnectorTool).unwrap();
    assert_eq!(
        pack_all(&ledger, PackedKind::ConnectorTool).block,
        first.block
    );
    db.execute(
        "UPDATE connector_calls SET record=replace(record,'success','tool_error') WHERE id=1",
        [],
    )
    .unwrap();
    ledger.stage_tool_batch(&[1]).unwrap();
    assert_eq!(
        ledger
            .packed_preview(PackedKind::ConnectorTool)
            .err()
            .unwrap(),
        LedgerError::SourceChanged
    );
    assert_eq!(
        old.tool_summary(&ToolFilter::default()).unwrap().returned,
        2
    );
    let ids = keys(&db, PackedKind::ConnectorTool);
    ledger
        .pack_staging_batch(PackedKind::ConnectorTool, &[&ids[0]])
        .unwrap();
    ledger
        .pack_staging_batch(PackedKind::ConnectorTool, &[&ids[1]])
        .unwrap();
    assert_eq!(
        db.query_row("SELECT count(*) FROM retention_packed_blocks", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        2
    );
    assert_eq!(
        ledger
            .packed_preview(PackedKind::ConnectorTool)
            .unwrap()
            .tool_summary(&ToolFilter::default())
            .unwrap()
            .tool_errors,
        1
    );
    pack_all(&ledger, PackedKind::ConnectorTool);
    assert_eq!(
        db.query_row("SELECT count(*) FROM retention_packed_blocks", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
}

#[test]
fn zlib_stream_limits_require_end_checksum_and_no_trailing_or_concatenated_stream() {
    use std::io::Write;
    let raw = b"bounded statistical payload";
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
    encoder.write_all(raw).unwrap();
    let payload = encoder.finish().unwrap();
    assert_eq!(codec::decode(&payload, raw.len()).unwrap(), raw);
    for n in [0, 1, payload.len() - 1] {
        assert!(codec::decode(&payload[..n], raw.len()).is_err());
    }
    let mut trailing = payload.clone();
    trailing.push(0);
    assert!(codec::decode(&trailing, raw.len()).is_err());
    let mut joined = payload.clone();
    joined.extend(&payload);
    assert!(codec::decode(&joined, raw.len()).is_err());
    let mut broken = payload.clone();
    let last = broken.len() - 1;
    broken[last] ^= 1;
    assert!(codec::decode(&broken, raw.len()).is_err());
    assert!(codec::decode(&payload, raw.len() - 1).is_err());
    assert!(codec::decode(&payload, raw.len() + 1).is_err());
    assert!(codec::decode(&payload, MAX_RAW + 1).is_err());
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
    encoder.write_all(&vec![0; 1024 * 1024]).unwrap();
    let bomb = encoder.finish().unwrap();
    assert!(codec::decode(&bomb, 16).is_err());
}

#[test]
fn packing_failures_roll_back_block_and_membership_without_touching_staging() {
    let temp = tempfile::tempdir().unwrap();
    let db = tool_db(&temp.path().join("tools.db"));
    let ledger = FactLedger::for_connector_store(&db, STORE).unwrap();
    for id in [1, 2] {
        tool(&db, id, "success", None, "2026-10-27T00:00:00Z");
    }
    ledger.stage_tool_batch(&[1, 2]).unwrap();
    prepare(&db).unwrap();
    for (table, when) in [
        ("retention_packed_blocks", ""),
        ("retention_packed_members", "WHEN NEW.ordinal=1"),
    ] {
        db.execute_batch(&format!("CREATE TRIGGER reject_pack BEFORE INSERT ON {table} {when} BEGIN SELECT RAISE(IGNORE);END;")).unwrap();
        let ids = keys(&db, PackedKind::ConnectorTool);
        assert_eq!(
            ledger
                .pack_staging_batch(
                    PackedKind::ConnectorTool,
                    &ids.iter().map(String::as_str).collect::<Vec<_>>()
                )
                .err(),
            Some(LedgerError::Unavailable)
        );
        assert_eq!(
            db.query_row("SELECT count(*) FROM retention_packed_blocks", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert_eq!(
            db.query_row("SELECT count(*) FROM retention_packed_members", [], |r| r
                .get::<_, i64>(
                0
            ))
            .unwrap(),
            0
        );
        db.execute_batch("DROP TRIGGER reject_pack").unwrap();
    }
    assert_eq!(
        ledger
            .tool_staging_summary(&ToolFilter::default())
            .unwrap()
            .calls,
        2
    );
}

#[test]
fn damaged_future_or_orphan_packed_data_is_never_silently_skipped_or_replaced() {
    let temp = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(temp.path()).unwrap();
    let ledger = gateway(&store);
    let info = pack_all(&ledger, PackedKind::Gateway);
    ledger
        .db
        .execute("UPDATE retention_packed_blocks SET version=2", [])
        .unwrap();
    assert_eq!(
        ledger.packed_preview(PackedKind::Gateway).err().unwrap(),
        LedgerError::UnsupportedVersion
    );
    let ids = keys(ledger.db, PackedKind::Gateway);
    assert_eq!(
        ledger
            .pack_staging_batch(PackedKind::Gateway, &[&ids[0]])
            .err()
            .unwrap(),
        LedgerError::UnsupportedVersion
    );
    ledger
        .db
        .execute("UPDATE retention_packed_blocks SET version=1", [])
        .unwrap();
    ledger
        .db
        .execute("UPDATE retention_packed_members SET ordinal=63", [])
        .unwrap();
    assert_eq!(
        ledger.packed_preview(PackedKind::Gateway).err().unwrap(),
        LedgerError::InvalidRecord
    );
    ledger
        .db
        .execute("UPDATE retention_packed_members SET ordinal=0", [])
        .unwrap();
    ledger
        .db
        .execute("DELETE FROM retention_packed_blocks", [])
        .unwrap();
    assert_eq!(
        ledger.packed_preview(PackedKind::Gateway).err().unwrap(),
        LedgerError::InvalidRecord
    );
    assert_eq!(info.facts, 1);
    ledger
        .db
        .execute_batch("DROP TABLE retention_fact_scope;DROP TABLE retention_fact_staging")
        .unwrap();
    drop(ledger);
    assert_eq!(
        store.retention_ledger(STORE).err().unwrap(),
        LedgerError::InvalidRecord
    );
}

#[test]
fn packing_rejects_empty_duplicate_missing_sealed_and_oversized_inputs() {
    let temp = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(temp.path()).unwrap();
    let ledger = gateway(&store);
    let ids = keys(ledger.db, PackedKind::Gateway);
    assert_eq!(
        ledger.pack_staging_batch(PackedKind::Gateway, &[]).err(),
        Some(LedgerError::InvalidQuery)
    );
    assert_eq!(
        ledger
            .pack_staging_batch(PackedKind::Gateway, &[&ids[0], &ids[0]])
            .err(),
        Some(LedgerError::InvalidQuery)
    );
    assert_eq!(
        ledger
            .pack_staging_batch(PackedKind::Gateway, &vec![ids[0].as_str(); 65])
            .err(),
        Some(LedgerError::BatchLimit)
    );
    assert_eq!(
        ledger
            .pack_staging_batch(PackedKind::Gateway, &[&"00".repeat(32)])
            .err(),
        Some(LedgerError::SourceChanged)
    );
    ledger
        .db
        .execute("UPDATE retention_fact_staging SET state='sealed'", [])
        .unwrap();
    assert_eq!(
        ledger
            .pack_staging_batch(PackedKind::Gateway, &[&ids[0]])
            .err(),
        Some(LedgerError::ProtectedFact)
    );
    ledger
        .db
        .execute(
            "UPDATE retention_fact_staging SET state='staged',value=?1",
            [" ".repeat(MAX_RAW)],
        )
        .unwrap();
    assert_eq!(
        ledger
            .pack_staging_batch(PackedKind::Gateway, &[&ids[0]])
            .err(),
        Some(LedgerError::BatchLimit)
    );
}

#[test]
fn packed_preview_is_a_disk_backed_statistical_snapshot_after_source_connection_closes() {
    let temp = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(temp.path()).unwrap();
    let ledger = gateway(&store);
    pack_all(&ledger, PackedKind::Gateway);
    let preview = ledger.packed_preview(PackedKind::Gateway).unwrap();
    let expected = value(preview.gateway_summary().unwrap());
    drop(ledger);
    drop(store);
    assert_eq!(value(preview.gateway_summary().unwrap()), expected);
    assert_eq!(
        preview
            .db
            .query_row("PRAGMA temp_store", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert!(
        preview
            .db
            .execute("DELETE FROM retention_fact_staging", [])
            .is_err()
    );
}

#[test]
fn ignored_old_block_cleanup_rolls_back_replacement_instead_of_leaking_a_new_draft() {
    let temp = tempfile::tempdir().unwrap();
    let db = tool_db(&temp.path().join("tools.db"));
    let ledger = FactLedger::for_connector_store(&db, STORE).unwrap();
    tool(&db, 1, "success", Some(0.0), "2026-10-27T00:00:00Z");
    ledger.stage_tool_batch(&[1]).unwrap();
    let original = pack_all(&ledger, PackedKind::ConnectorTool);
    db.execute(
        "UPDATE connector_calls SET record=replace(record,'success','tool_error')",
        [],
    )
    .unwrap();
    ledger.stage_tool_batch(&[1]).unwrap();
    db.execute_batch("CREATE TRIGGER refuse_block_gc BEFORE DELETE ON retention_packed_blocks BEGIN SELECT RAISE(IGNORE);END;").unwrap();
    let ids = keys(&db, PackedKind::ConnectorTool);
    assert_eq!(
        ledger
            .pack_staging_batch(PackedKind::ConnectorTool, &[&ids[0]])
            .err(),
        Some(LedgerError::Unavailable)
    );
    assert_eq!(
        hex(&db
            .query_row("SELECT id FROM retention_packed_blocks", [], |r| r
                .get::<_, Vec<u8>>(0))
            .unwrap()),
        original.block
    );
    assert_eq!(
        ledger
            .packed_preview(PackedKind::ConnectorTool)
            .err()
            .unwrap(),
        LedgerError::SourceChanged
    );
}

#[test]
fn valid_compression_with_wrong_scope_or_future_frame_is_still_rejected() {
    for future in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let store = ObservationStore::open(temp.path()).unwrap();
        let ledger = gateway(&store);
        let info = pack_all(&ledger, PackedKind::Gateway);
        let old_id = unhex(&info.block).unwrap();
        let mut block = read_block(
            ledger.db,
            &old_id,
            STORE,
            &ledger.namespace,
            PackedKind::Gateway,
        )
        .unwrap();
        if future {
            block.frame.version = 2;
        } else {
            block.frame.namespace = "00000000-0000-4000-8000-000000000002".into();
        }
        let (raw, payload) = codec::encode(&block.frame).unwrap();
        let id = block_id(&raw);
        ledger.db.execute("UPDATE retention_packed_blocks SET id=?1,raw_bytes=?2,raw_digest=?3,payload_digest=?4,payload=?5",params![id,raw.len() as i64,bytes_hash(&raw).as_slice(),bytes_hash(&payload).as_slice(),payload]).unwrap();
        let expected = if future {
            LedgerError::UnsupportedVersion
        } else {
            LedgerError::WrongStore
        };
        assert_eq!(
            ledger.packed_preview(PackedKind::Gateway).err().unwrap(),
            expected
        );
    }
}

#[test]
fn draft_arithmetic_keeps_recorded_contribution_order_across_packed_rebuilds() {
    let temp = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(temp.path()).unwrap();
    let ledger = store.retention_ledger(STORE).unwrap();
    let (_, mut rows) = retry();
    rows.truncate(1);
    rows[0].logical_id = None;
    let mut facts = Vec::new();
    for id in ["a", "b", "c"] {
        rows[0].id = id.into();
        facts.push(
            GatewayFact::from_request(&canonical(rows.clone()), &ledger.namespace, directory())
                .unwrap(),
        );
    }
    facts.sort_by(|a, b| a.key.cmp(&b.key));
    let mut stored = Vec::new();
    for (index, fact) in facts.into_iter().enumerate() {
        let mut v = value(&fact);
        v["stats"]["cost"] = json!({"actual":[{"currency":"USD","amount":if index==2{1.0e16}else{1.0},"calls":1}],"estimated":[],"unknownCalls":0});
        let text = v.to_string();
        stored.push((fact.key, text));
    }
    for (key, text) in stored.into_iter().rev() {
        ledger
            .db
            .execute(
                "INSERT INTO retention_fact_staging VALUES(?1,?2,?3,'staged',?4)",
                params![key, "01".repeat(32), digest(&text), text],
            )
            .unwrap();
    }
    let expected = value(ledger.staging_summary().unwrap());
    assert_eq!(
        expected["cost"]["actual"][0]["amount"].as_f64(),
        Some(1.0e16)
    );
    pack_all(&ledger, PackedKind::Gateway);
    assert_eq!(
        value(
            ledger
                .packed_preview(PackedKind::Gateway)
                .unwrap()
                .gateway_summary()
                .unwrap()
        ),
        expected
    );
}

#[test]
fn private_contribution_sequence_is_not_reused_after_staged_rows_are_removed() {
    let temp = tempfile::tempdir().unwrap();
    let db = tool_db(&temp.path().join("tools.db"));
    let ledger = FactLedger::for_connector_store(&db, STORE).unwrap();
    tool(&db, 1, "success", Some(0.0), "2026-10-27T00:00:00Z");
    ledger.stage_tool_batch(&[1]).unwrap();
    pack_all(&ledger, PackedKind::ConnectorTool);
    // Synthetic staging removal only: this is not a product cleanup operation.
    db.execute("DELETE FROM retention_tool_staging", [])
        .unwrap();
    tool(&db, 2, "success", Some(0.0), "2026-10-27T00:00:00Z");
    ledger.stage_tool_batch(&[2]).unwrap();
    assert_eq!(
        db.query_row("SELECT rowid FROM retention_tool_staging", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        2
    );
    pack_all(&ledger, PackedKind::ConnectorTool);
    assert_eq!(
        db.query_row("SELECT count(*) FROM retention_packed_members", [], |r| r
            .get::<_, i64>(
            0
        ))
        .unwrap(),
        2
    );
    assert_eq!(
        ledger
            .packed_preview(PackedKind::ConnectorTool)
            .err()
            .unwrap(),
        LedgerError::SourceChanged
    );
}

#[test]
fn damaged_or_ignored_sequence_watermark_never_resets_or_partially_publishes_drafts() {
    let temp = tempfile::tempdir().unwrap();
    let db = tool_db(&temp.path().join("tools.db"));
    let ledger = FactLedger::for_connector_store(&db, STORE).unwrap();
    tool(&db, 1, "success", None, "2026-10-27T00:00:00Z");
    ledger.stage_tool_batch(&[1]).unwrap();
    pack_all(&ledger, PackedKind::ConnectorTool);
    tool(&db, 2, "success", None, "2026-10-27T00:00:00Z");
    ledger.stage_tool_batch(&[2]).unwrap();
    db.execute_batch("CREATE TRIGGER refuse_mark BEFORE UPDATE ON retention_packed_watermark BEGIN SELECT RAISE(IGNORE);END;").unwrap();
    let ids = keys(&db, PackedKind::ConnectorTool);
    assert_eq!(
        ledger
            .pack_staging_batch(
                PackedKind::ConnectorTool,
                &ids.iter().map(String::as_str).collect::<Vec<_>>()
            )
            .err(),
        Some(LedgerError::Unavailable)
    );
    assert_eq!(
        ledger
            .packed_preview(PackedKind::ConnectorTool)
            .unwrap()
            .coverage()
            .packed_facts,
        1
    );
    db.execute_batch(
        "DROP TRIGGER refuse_mark;UPDATE retention_packed_watermark SET last_order=99;",
    )
    .unwrap();
    assert_eq!(
        ledger
            .packed_preview(PackedKind::ConnectorTool)
            .err()
            .unwrap(),
        LedgerError::InvalidRecord
    );
    tool(&db, 3, "success", None, "2026-10-27T00:00:00Z");
    assert_eq!(
        ledger.stage_tool_batch(&[3]),
        Err(LedgerError::InvalidRecord)
    );
}
