use super::*;
use rusqlite::params;
use serde_json::{Value, json};

fn now() -> DateTime<Utc> {
    "2026-10-27T00:00:00Z".parse().unwrap()
}
fn db(dir: &Path) -> Connection {
    let db = Connection::open(dir.join("plugin-extensions.sqlite3")).unwrap();
    db.execute_batch("PRAGMA journal_mode=WAL;CREATE TABLE extension_version(version INTEGER NOT NULL);INSERT INTO extension_version VALUES(1);CREATE TABLE connector_calls(id INTEGER PRIMARY KEY,plugin_id TEXT NOT NULL,identity TEXT NOT NULL,record TEXT NOT NULL,UNIQUE(plugin_id,identity));CREATE TRIGGER call_retention AFTER INSERT ON connector_calls BEGIN DELETE FROM connector_calls WHERE id<=NEW.id-10000;END;").unwrap();
    db
}
fn row(id: i64, status: &str, at: &str) -> Value {
    json!({"identity":format!("private-{id}"),"rpcId":{"private":"not-a-statistic"},"mcpConnection":"private-connection","tool":"search","status":status,"durationMs":null,"observedAt":at,"agentSession":null,"content":"not-persisted","modelTokens":null})
}
fn insert(db: &Connection, id: i64, value: &Value) {
    db.execute(
        "INSERT INTO connector_calls VALUES(?1,'fixture.plugin',?2,?3)",
        params![id, value["identity"].as_str().unwrap(), value.to_string()],
    )
    .unwrap();
}
fn raw(db: &Connection) -> Vec<(i64, String, String, String)> {
    db.prepare("SELECT id,plugin_id,identity,record FROM connector_calls ORDER BY id")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}

#[test]
fn all_tool_outcomes_and_nanosecond_boundaries_match_staged_candidate_rules_without_source_writes()
{
    use crate::observation::retention::{FactLedger, test_support::STORE};
    for days in [7, 30, 90] {
        let dir = tempfile::tempdir().unwrap();
        let db = db(dir.path());
        let cutoff = now() - Duration::days(days);
        let before = (cutoff - Duration::nanoseconds(1)).to_rfc3339();
        let equal = cutoff.to_rfc3339();
        for (id, status) in [
            (1, "success"),
            (2, "tool_error"),
            (3, "protocol_error"),
            (4, "transport_error"),
            (5, "timeout"),
            (6, "cancelled"),
            (7, "incomplete"),
            (8, "in_progress"),
        ] {
            insert(&db, id, &row(id, status, &before));
        }
        insert(&db, 9, &row(9, "success", &equal));
        insert(&db, 10, &row(10, "success", "unknown"));
        let source = raw(&db);
        let result = connector(dir.path(), days as u16, now()).unwrap();
        assert_eq!(result.records, 10);
        assert_eq!(result.expired, 3);
        assert_eq!(result.within_window, 1);
        assert_eq!(result.protected.active, 1);
        assert_eq!(result.protected.unknown_outcome, 4);
        assert_eq!(result.protected.unknown_time, 1);
        assert!(result.classification_complete);
        assert_eq!(result.capacity_limit, Some(10_000));
        assert_eq!(raw(&db), source);
        assert_eq!(
            db.query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name LIKE 'retention_%'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
        // Only the test oracle initializes staging, after proving preview did not.
        let ledger = FactLedger::for_connector_store(&db, STORE).unwrap();
        ledger
            .stage_tool_batch(&(1..=10).collect::<Vec<_>>())
            .unwrap();
        let expected = ledger.tool_expiration_counts(days as u16, now()).unwrap();
        assert_eq!(result.expired, expected.expired);
        assert_eq!(result.within_window, expected.within_window);
        assert_eq!(
            result.protected.active
                + result.protected.unknown_outcome
                + result.protected.unknown_time,
            expected.protected
        );
    }
}

#[test]
fn future_fields_unknown_status_private_content_or_invented_usage_remain_unclassified() {
    let dir = tempfile::tempdir().unwrap();
    let db = db(dir.path());
    for (i, (field, value)) in [
        ("future", Value::Null),
        ("status", json!("future")),
        ("agentSession", json!("private-session")),
        ("modelTokens", json!(0)),
        ("content", json!("private-content")),
        ("identity", json!("reused")),
    ]
    .into_iter()
    .enumerate()
    {
        let id = i as i64 + 1;
        let mut value_row = row(id, "success", "2026-01-01T00:00:00Z");
        value_row[field] = value;
        db.execute(
            "INSERT INTO connector_calls VALUES(?1,'fixture.plugin',?2,?3)",
            params![id, format!("private-{id}"), value_row.to_string()],
        )
        .unwrap();
    }
    let before = raw(&db);
    let result = connector(dir.path(), 30, now()).unwrap();
    assert_eq!(result.records, 6);
    assert_eq!(result.protected.unsupported_source, 6);
    assert_eq!(result.expired, 0);
    assert!(!result.classification_complete);
    assert_eq!(raw(&db), before);
}

#[test]
fn large_or_wrongly_typed_columns_are_counted_without_loading_unbounded_values() {
    let dir = tempfile::tempdir().unwrap();
    let db = db(dir.path());
    db.execute(
        "INSERT INTO connector_calls VALUES(1,'fixture.plugin','oversize',?1)",
        ["x".repeat(MAX_ROW_BYTES as usize + 1)],
    )
    .unwrap();
    db.execute(
        "INSERT INTO connector_calls VALUES(2,?1,'oversize-plugin','{}')",
        ["p".repeat(257)],
    )
    .unwrap();
    db.execute(
        "INSERT INTO connector_calls VALUES(3,'fixture.plugin','broken','{')",
        [],
    )
    .unwrap();
    let result = connector(dir.path(), 30, now()).unwrap();
    assert_eq!(result.records, 3);
    assert_eq!(result.unreadable_records, 2);
    assert_eq!(result.protected.unsupported_source, 1);
    assert_eq!(result.expired, 0);
    assert!(!result.classification_complete);
}

#[test]
fn source_absence_is_not_confirmed_empty_and_preview_does_not_initialize_any_database() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(
        connector(dir.path(), 30, now()).unwrap_err(),
        Error::SourceAbsent
    );
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    let _db = db(dir.path());
    let result = connector(dir.path(), 30, now()).unwrap();
    assert_eq!(result.records, 0);
    assert!(result.classification_complete);
}

#[test]
fn future_or_incomplete_source_schemas_are_rejected_instead_of_becoming_empty() {
    for sql in [
        "UPDATE extension_version SET version=2",
        "INSERT INTO extension_version VALUES(1)",
        "DELETE FROM extension_version",
        "PRAGMA user_version=1",
        "ALTER TABLE connector_calls ADD COLUMN future TEXT",
        "DROP TABLE connector_calls;CREATE TABLE connector_calls(id INTEGER PRIMARY KEY,plugin_id TEXT NOT NULL,identity TEXT NOT NULL,record TEXT NOT NULL)",
        "ALTER TABLE connector_calls ADD COLUMN generated TEXT GENERATED ALWAYS AS ('future') VIRTUAL",
    ] {
        let dir = tempfile::tempdir().unwrap();
        let db = db(dir.path());
        db.execute_batch(sql).unwrap();
        assert_eq!(
            connector(dir.path(), 30, now()).unwrap_err(),
            Error::UnsupportedSchema,
            "{sql}"
        );
    }
}

#[test]
fn current_saved_collection_is_complete_without_claiming_history_removed_by_the_capacity_cap() {
    let dir = tempfile::tempdir().unwrap();
    let db = db(dir.path());
    db.execute_batch("BEGIN").unwrap();
    for id in 1..=12_005 {
        insert(&db, id, &row(id, "success", "2026-01-01T00:00:00Z"));
    }
    db.execute_batch("COMMIT").unwrap();
    let result = connector(dir.path(), 30, now()).unwrap();
    assert_eq!(result.records, 10_000);
    assert_eq!(result.expired, 10_000);
    assert_eq!(result.capacity_limit, Some(10_000));
    db.execute_batch("DROP TRIGGER call_retention").unwrap();
    let unknown = connector(dir.path(), 30, now()).unwrap();
    assert_eq!(unknown.records, 10_000);
    assert_eq!(unknown.capacity_limit, None);
}

#[test]
fn limits_or_links_never_yield_a_partial_or_aliased_inventory() {
    let dir = tempfile::tempdir().unwrap();
    let db = db(dir.path());
    insert(&db, 1, &row(1, "success", "2026-01-01T00:00:00Z"));
    assert_eq!(
        scan(dir.path(), 30, now(), 0, Budget::from_secs(1)).unwrap_err(),
        Error::Limit
    );
    assert_eq!(
        scan(dir.path(), 30, now(), 1, Budget::ZERO).unwrap_err(),
        Error::Limit
    );
    assert_eq!(
        connector(dir.path(), 1, now()).unwrap_err(),
        Error::InvalidInput
    );
    let linked = tempfile::tempdir().unwrap();
    std::fs::hard_link(
        dir.path().join("plugin-extensions.sqlite3"),
        linked.path().join("plugin-extensions.sqlite3"),
    )
    .unwrap();
    assert_eq!(
        connector(dir.path(), 30, now()).unwrap_err(),
        Error::SourceChanged
    );
}
