use super::*;
use crate::observation::retention::test_support::STORE;
use chrono::TimeDelta;
use serde_json::{Value, json};

fn source_db(path: &std::path::Path) -> Connection {
    let db = Connection::open(path).unwrap();
    db.execute_batch("PRAGMA journal_mode=WAL;CREATE TABLE extension_version(version INTEGER NOT NULL);INSERT INTO extension_version VALUES(1);CREATE TABLE connector_calls(id INTEGER PRIMARY KEY,plugin_id TEXT NOT NULL,identity TEXT NOT NULL,record TEXT NOT NULL,UNIQUE(plugin_id,identity));CREATE TRIGGER call_retention AFTER INSERT ON connector_calls BEGIN DELETE FROM connector_calls WHERE id<=NEW.id-10000;END;").unwrap();
    db
}
fn raw(id: i64, state: &str, duration: Option<f64>, at: &str) -> Value {
    json!({"identity":format!("private-call-{id}"),"rpcId":"private-rpc-id","mcpConnection":"private-connection","tool":"search","status":state,"durationMs":duration,"observedAt":at,"agentSession":null,"content":"not-persisted","modelTokens":null})
}
fn insert(db: &Connection, id: i64, plugin: &str, value: &Value) {
    db.execute(
        "INSERT INTO connector_calls VALUES(?1,?2,?3,?4)",
        params![
            id,
            plugin,
            value["identity"].as_str().unwrap(),
            value.to_string()
        ],
    )
    .unwrap();
}
fn sources(db: &Connection) -> Vec<(i64, String, String, String)> {
    db.prepare("SELECT id,plugin_id,identity,record FROM connector_calls ORDER BY id")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}
fn count(db: &Connection) -> u64 {
    db.query_row("SELECT count(*) FROM retention_tool_staging", [], |r| {
        count_column(r, 0)
    })
    .unwrap()
}

#[test]
fn tool_outcomes_have_distinct_denominators_unknown_usage_and_exact_combined_p95() {
    let temp = tempfile::tempdir().unwrap();
    let db = source_db(&temp.path().join("plugin.db"));
    let ledger = FactLedger::for_connector_store(&db, STORE).unwrap();
    for (i, (state, duration)) in [
        ("success", Some(0.0)),
        ("tool_error", Some(1.5)),
        ("protocol_error", Some(100.25)),
        ("transport_error", None),
        ("timeout", None),
        ("cancelled", None),
        ("incomplete", None),
        ("in_progress", None),
        ("success", None),
    ]
    .into_iter()
    .enumerate()
    {
        insert(
            &db,
            i as i64 + 1,
            "private.plugin.a",
            &raw(i as i64, state, duration, "2026-10-01T00:00:00Z"),
        );
    }
    let original = sources(&db);
    ledger
        .stage_tool_batch(&(1..=9).collect::<Vec<_>>())
        .unwrap();
    let summary = ledger.tool_staging_summary(&ToolFilter::default()).unwrap();
    assert_eq!(
        (
            summary.calls,
            summary.returned,
            summary.tool_errors,
            summary.protocol_errors,
            summary.transport_errors,
            summary.timeouts,
            summary.cancelled,
            summary.incomplete,
            summary.in_progress
        ),
        (9, 2, 1, 1, 1, 1, 1, 1, 1)
    );
    assert_eq!(summary.error_denominator, 5);
    assert_eq!(summary.error_rate, Some(0.6));
    assert_eq!(summary.duration_samples, 3);
    assert_eq!(summary.p95_ms, Some(100.25));
    assert_eq!(summary.unknown_agent_calls, 9);
    assert_eq!(summary.unknown_session_calls, 9);
    assert_eq!(summary.model_tokens, None);
    assert_eq!(ledger.staging_summary().unwrap().requests, 0);
    for value in db
        .prepare("SELECT value FROM retention_tool_staging")
        .unwrap()
        .query_map([], |r| r.get::<_, String>(0))
        .unwrap()
    {
        let value = value.unwrap();
        assert!(!value.contains("private-"));
        assert!(!value.contains("private.plugin"));
        for field in [
            "rpcId",
            "mcpConnection",
            "agentSession",
            "content",
            "modelTokens",
        ] {
            assert!(!value.contains(field));
        }
    }
    assert_eq!(sources(&db), original);
}

#[test]
fn tool_ranges_are_half_open_nanosecond_exact_and_unknown_dates_are_visible() {
    let temp = tempfile::tempdir().unwrap();
    let db = source_db(&temp.path().join("plugin.db"));
    let ledger = FactLedger::for_connector_store(&db, STORE).unwrap();
    for (id, time, plugin, tool) in [
        (1, "2026-10-27T00:00:00Z", "a", "search"),
        (2, "2026-10-27T08:00:00.000000001+08:00", "a", "other"),
        (3, "2026-10-27T00:00:00.000000002Z", "b", "search"),
        (4, "missing-time", "a", "search"),
    ] {
        let mut value = raw(id, "success", Some(0.0), time);
        value["tool"] = json!(tool);
        insert(&db, id, plugin, &value);
    }
    ledger.stage_tool_batch(&[1, 2, 3, 4]).unwrap();
    let selected = ToolFilter {
        plugin_id: Some("a".into()),
        ..Default::default()
    };
    assert_eq!(ledger.tool_staging_summary(&selected).unwrap().calls, 3);
    let s = ledger
        .tool_staging_summary(&ToolFilter {
            plugin_id: Some("a".into()),
            tool: Some("search".into()),
            start: Some("2026-10-27T00:00:00Z".parse().unwrap()),
            end: Some("2026-10-27T00:00:00.000000001Z".parse().unwrap()),
        })
        .unwrap();
    assert_eq!(
        (s.calls, s.unknown_time_calls, s.excluded_unknown_time_calls),
        (1, 0, 1)
    );
    let empty = ledger
        .tool_staging_summary(&ToolFilter {
            plugin_id: Some("' OR 1=1 --".into()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(empty.calls, 0);
    assert_eq!(empty.error_rate, None);
    assert_eq!(empty.p95_ms, None);
    assert_eq!(empty.model_tokens, None);
    let reversed = ToolFilter {
        start: Some("2026-10-28T00:00:00Z".parse().unwrap()),
        end: Some("2026-10-27T00:00:00Z".parse().unwrap()),
        ..Default::default()
    };
    assert_eq!(
        ledger.tool_staging_summary(&reversed).err(),
        Some(LedgerError::InvalidQuery)
    );
}

#[test]
fn tool_expiration_protects_active_incomplete_and_unknown_time_without_deleting_anything() {
    let now: DateTime<Utc> = "2026-10-27T00:00:00Z".parse().unwrap();
    for days in [7, 30, 90] {
        let temp = tempfile::tempdir().unwrap();
        let db = source_db(&temp.path().join("plugin.db"));
        let ledger = FactLedger::for_connector_store(&db, STORE).unwrap();
        let cutoff = now - Duration::days(i64::from(days));
        for (i, (state, at)) in [
            ("success", (cutoff - TimeDelta::nanoseconds(1)).to_rfc3339()),
            ("success", cutoff.to_rfc3339()),
            ("success", (cutoff + TimeDelta::nanoseconds(1)).to_rfc3339()),
            ("timeout", (cutoff - TimeDelta::seconds(1)).to_rfc3339()),
            ("cancelled", (cutoff - TimeDelta::seconds(1)).to_rfc3339()),
            (
                "transport_error",
                (cutoff - TimeDelta::seconds(1)).to_rfc3339(),
            ),
            ("incomplete", (cutoff - TimeDelta::seconds(1)).to_rfc3339()),
            ("in_progress", (cutoff - TimeDelta::seconds(1)).to_rfc3339()),
            ("success", "bad".into()),
            ("success", (now + TimeDelta::days(1)).to_rfc3339()),
        ]
        .into_iter()
        .enumerate()
        {
            insert(&db, i as i64 + 1, "a", &raw(i as i64, state, None, &at));
        }
        let original = sources(&db);
        ledger
            .stage_tool_batch(&(1..=10).collect::<Vec<_>>())
            .unwrap();
        assert_eq!(
            ledger.tool_expiration_counts(days, now).unwrap(),
            ToolExpiry {
                expired: 1,
                within_window: 3,
                protected: 6
            }
        );
        assert_eq!(
            ledger.tool_expiration_counts(1, now),
            Err(LedgerError::InvalidQuery)
        );
        assert_eq!(sources(&db), original);
        assert!(exists(&db, "connector_calls").unwrap());
    }
}

#[test]
fn tool_facts_reopen_idempotently_and_follow_exact_raw_bytes_without_recounting() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("plugin.db");
    let db = source_db(&path);
    insert(
        &db,
        1,
        "a",
        &raw(1, "success", Some(0.0), "2026-10-27T00:00:00Z"),
    );
    let ledger = FactLedger::for_connector_store(&db, STORE).unwrap();
    let namespace = ledger.namespace.clone();
    assert_eq!(ledger.stage_tool_batch(&[1]).unwrap().inserted, 1);
    assert_eq!(ledger.stage_tool_batch(&[1, 1]).unwrap().unchanged, 2);
    let before: String = db
        .query_row(
            "SELECT source_fingerprint FROM retention_tool_staging",
            [],
            |r| r.get(0),
        )
        .unwrap();
    db.execute("UPDATE connector_calls SET record=record || ' '", [])
        .unwrap();
    assert_eq!(ledger.stage_tool_batch(&[1]).unwrap().updated, 1);
    let after: String = db
        .query_row(
            "SELECT source_fingerprint FROM retention_tool_staging",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_ne!(before, after);
    drop(ledger);
    drop(db);
    let db = Connection::open(path).unwrap();
    let ledger = FactLedger::for_connector_store(&db, STORE).unwrap();
    assert_eq!(namespace, ledger.namespace);
    assert_eq!(
        ledger
            .tool_staging_summary(&ToolFilter::default())
            .unwrap()
            .calls,
        1
    );
    assert_eq!(
        FactLedger::for_connector_store(&db, "00000000-0000-4000-8000-000000000002")
            .err()
            .unwrap(),
        LedgerError::WrongStore
    );
}

#[test]
fn absent_or_reused_source_locators_never_create_partial_or_duplicate_tool_facts() {
    let temp = tempfile::tempdir().unwrap();
    let db = source_db(&temp.path().join("plugin.db"));
    let ledger = FactLedger::for_connector_store(&db, STORE).unwrap();
    insert(
        &db,
        1,
        "a",
        &raw(1, "success", Some(0.0), "2026-10-27T00:00:00Z"),
    );
    assert_eq!(
        ledger.stage_tool_batch(&[1, 2]),
        Err(LedgerError::SourceChanged)
    );
    assert_eq!(count(&db), 0);
    ledger.stage_tool_batch(&[1]).unwrap();
    let mut changed = raw(2, "success", Some(0.0), "2026-10-27T00:00:00Z");
    changed["tool"] = json!("other");
    db.execute(
        "UPDATE connector_calls SET identity=?1,record=?2 WHERE id=1",
        params![changed["identity"].as_str().unwrap(), changed.to_string()],
    )
    .unwrap();
    assert_eq!(
        ledger.stage_tool_batch(&[1]),
        Err(LedgerError::SourceChanged)
    );
    assert_eq!(count(&db), 1);
    assert_eq!(
        ledger
            .tool_staging_summary(&ToolFilter::default())
            .unwrap()
            .calls,
        1
    );
}

#[test]
fn tool_batches_roll_back_on_ignored_and_failed_inserts_and_updates() {
    let temp = tempfile::tempdir().unwrap();
    let db = source_db(&temp.path().join("plugin.db"));
    let ledger = FactLedger::for_connector_store(&db, STORE).unwrap();
    for id in [1, 2] {
        let mut r = raw(id, "success", Some(0.0), "2026-10-27T00:00:00Z");
        r["tool"] = json!(if id == 1 { "first" } else { "second" });
        insert(&db, id, "a", &r);
    }
    let original = sources(&db);
    for action in ["IGNORE", "ABORT,'fixture'"] {
        db.execute_batch(&format!("CREATE TRIGGER reject_tool BEFORE INSERT ON retention_tool_staging WHEN json_extract(NEW.value,'$.toolLabel')='second' BEGIN SELECT RAISE({action});END;")).unwrap();
        assert_eq!(
            ledger.stage_tool_batch(&[1, 2]),
            Err(LedgerError::Unavailable)
        );
        assert_eq!(count(&db), 0);
        db.execute_batch("DROP TRIGGER reject_tool").unwrap();
    }
    ledger.stage_tool_batch(&[1]).unwrap();
    let old: String = db
        .query_row("SELECT value FROM retention_tool_staging", [], |r| r.get(0))
        .unwrap();
    db.execute(
        "UPDATE connector_calls SET record=replace(record,'success','tool_error') WHERE id=1",
        [],
    )
    .unwrap();
    db.execute_batch("CREATE TRIGGER reject_tool BEFORE UPDATE ON retention_tool_staging BEGIN SELECT RAISE(IGNORE);END;").unwrap();
    assert_eq!(
        ledger.stage_tool_batch(&[2, 1]),
        Err(LedgerError::Unavailable)
    );
    assert_eq!(count(&db), 1);
    assert_eq!(
        db.query_row("SELECT value FROM retention_tool_staging", [], |r| r
            .get::<_, String>(0))
            .unwrap(),
        old
    );
    assert_eq!(sources(&db).len(), original.len());
}

#[test]
fn unknown_source_fields_content_or_usage_are_preserved_instead_of_silently_discarded() {
    let temp = tempfile::tempdir().unwrap();
    let db = source_db(&temp.path().join("plugin.db"));
    let ledger = FactLedger::for_connector_store(&db, STORE).unwrap();
    insert(
        &db,
        1,
        "a",
        &raw(1, "success", Some(0.0), "2026-10-27T00:00:00Z"),
    );
    for (field, value) in [
        ("future", json!("private-text")),
        ("agentSession", json!("session")),
        ("modelTokens", json!(1)),
        ("content", json!("private-body")),
        ("status", json!("future_status")),
        ("durationMs", json!(-1)),
    ] {
        let mut r = raw(2, "success", Some(0.0), "2026-10-27T00:00:00Z");
        r[field] = value;
        db.execute("DELETE FROM connector_calls WHERE id=2", [])
            .unwrap();
        insert(&db, 2, "a", &r);
        let original = sources(&db);
        assert_eq!(
            ledger.stage_tool_batch(&[1, 2]),
            Err(LedgerError::InvalidRecord)
        );
        assert_eq!(count(&db), 0);
        assert_eq!(sources(&db), original);
    }
}

#[test]
fn malformed_future_or_sealed_tool_facts_are_never_skipped_or_overwritten() {
    let temp = tempfile::tempdir().unwrap();
    let db = source_db(&temp.path().join("plugin.db"));
    let ledger = FactLedger::for_connector_store(&db, STORE).unwrap();
    insert(
        &db,
        1,
        "a",
        &raw(1, "success", Some(1.5), "2026-10-27T00:00:00Z"),
    );
    ledger.stage_tool_batch(&[1]).unwrap();
    db.execute("UPDATE retention_tool_staging SET state='sealed'", [])
        .unwrap();
    assert_eq!(
        ledger.stage_tool_batch(&[1]),
        Err(LedgerError::ProtectedFact)
    );
    assert_eq!(
        ledger
            .tool_staging_summary(&ToolFilter::default())
            .unwrap()
            .calls,
        0
    );
    let original: String = db
        .query_row("SELECT value FROM retention_tool_staging", [], |r| r.get(0))
        .unwrap();
    for (field, value, error) in [
        ("future", json!("must-keep"), LedgerError::InvalidRecord),
        ("version", json!(2), LedgerError::UnsupportedVersion),
        ("durationMs", json!(-1), LedgerError::InvalidRecord),
    ] {
        let mut damaged: Value = serde_json::from_str(&original).unwrap();
        damaged[field] = value;
        let text = damaged.to_string();
        db.execute(
            "UPDATE retention_tool_staging SET value=?1,projection_digest=?2,state='staged'",
            params![text, digest(&text)],
        )
        .unwrap();
        assert_eq!(
            ledger
                .tool_staging_summary(&ToolFilter::default())
                .err()
                .unwrap(),
            error
        );
        assert_eq!(ledger.stage_tool_batch(&[1]), Err(error));
        assert_eq!(
            db.query_row("SELECT value FROM retention_tool_staging", [], |r| r
                .get::<_, String>(0))
                .unwrap(),
            text
        );
    }
}

#[test]
fn unsupported_connector_schema_and_orphan_facts_never_get_a_replacement_identity() {
    let temp = tempfile::tempdir().unwrap();
    let db = source_db(&temp.path().join("plugin.db"));
    db.execute("UPDATE extension_version SET version=2", [])
        .unwrap();
    assert_eq!(
        FactLedger::for_connector_store(&db, STORE).err().unwrap(),
        LedgerError::UnsupportedVersion
    );
    assert!(!exists(&db, "retention_fact_scope").unwrap());
    db.execute("UPDATE extension_version SET version=1", [])
        .unwrap();
    let ledger = FactLedger::for_connector_store(&db, STORE).unwrap();
    let namespace = ledger.namespace.clone();
    drop(ledger);
    db.execute("UPDATE retention_fact_scope SET version=3", [])
        .unwrap();
    assert_eq!(
        FactLedger::for_connector_store(&db, STORE).err().unwrap(),
        LedgerError::UnsupportedVersion
    );
    assert_eq!(
        db.query_row("SELECT namespace FROM retention_fact_scope", [], |r| r
            .get::<_, String>(
            0
        ))
        .unwrap(),
        namespace
    );
    db.execute_batch("DROP TABLE retention_fact_scope;DROP TABLE retention_fact_staging;")
        .unwrap();
    assert_eq!(
        FactLedger::for_connector_store(&db, STORE).err().unwrap(),
        LedgerError::InvalidRecord
    );
    assert!(!exists(&db, "retention_fact_scope").unwrap());
}

#[test]
fn tool_staging_limits_count_and_bytes_without_partially_applying_a_batch() {
    let temp = tempfile::tempdir().unwrap();
    let db = source_db(&temp.path().join("plugin.db"));
    let ledger = FactLedger::for_connector_store(&db, STORE).unwrap();
    insert(
        &db,
        1,
        "a",
        &raw(1, "success", Some(0.0), "2026-10-27T00:00:00Z"),
    );
    assert_eq!(
        ledger.stage_tool_batch(&vec![1; 65]),
        Err(LedgerError::BatchLimit)
    );
    assert_eq!(
        ledger.stage_tool_batch(&[0]),
        Err(LedgerError::InvalidQuery)
    );
    db.execute(
        "UPDATE connector_calls SET record=record || ?1",
        [" ".repeat(8 * 1024 * 1024)],
    )
    .unwrap();
    assert_eq!(ledger.stage_tool_batch(&[1]), Err(LedgerError::BatchLimit));
    assert_eq!(count(&db), 0);
}
