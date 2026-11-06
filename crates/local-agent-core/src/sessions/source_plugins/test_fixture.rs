use std::path::PathBuf;

pub(super) struct Fixture {
    _home: tempfile::TempDir,
    _db: rusqlite::Connection,
    pub(super) home: PathBuf,
}

impl Fixture {
    pub(super) fn new() -> Self {
        use super::super::opencode_tests::{fixture, message, part};
        use serde_json::json;

        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().canonicalize().unwrap();
        let db = fixture(&home);
        let time = 1_790_000_000_000i64;
        db.execute("UPDATE session SET time_created=?1,time_updated=?1", [time])
            .unwrap();
        message(&db, "user", time, json!({"role":"user"}));
        part(
            &db,
            "input",
            "user",
            time,
            json!({"type":"text","text":"source plugin fixture task"}),
        );
        message(
            &db,
            "assistant",
            time + 1000,
            json!({"role":"assistant","parentID":"user","providerID":"fixture","modelID":"fixture-model","tokens":{"input":60,"output":20,"cache":{"read":40,"write":0}},"finish":"stop"}),
        );
        part(
            &db,
            "output",
            "assistant",
            time + 1000,
            json!({"type":"text","text":"visible fixture reply"}),
        );
        part(
            &db,
            "tool",
            "assistant",
            time + 2000,
            json!({"type":"tool","tool":"fixture_mcp_lookup","callID":"call-native","state":{"status":"completed","input":{"query":"safe","api_key":"hidden-fixture-secret"},"output":"tool-result-sentinel","time":{"start":time+1000,"end":time+2000}}}),
        );
        std::fs::write(
            home.join("opencode.json"),
            r#"{"mcp":{"fixture_mcp":{"type":"local","command":["never-run"]}}}"#,
        )
        .unwrap();
        Self {
            _home: temp,
            _db: db,
            home,
        }
    }
}
