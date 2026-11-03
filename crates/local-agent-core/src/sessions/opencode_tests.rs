use super::*;
use rusqlite::{Connection, params};
use serde_json::{Value, json};

pub(super) fn fixture(home: &Path) -> Connection {
    let path = opencode::path(home);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let db = Connection::open(path).unwrap();
    db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0;
        CREATE TABLE session(id TEXT PRIMARY KEY,title TEXT,directory TEXT,version TEXT,time_created INTEGER,time_updated INTEGER);
        CREATE TABLE message(id TEXT PRIMARY KEY,session_id TEXT,time_created INTEGER,time_updated INTEGER,data TEXT);
        CREATE TABLE part(id TEXT PRIMARY KEY,message_id TEXT,session_id TEXT,time_created INTEGER,time_updated INTEGER,data TEXT);
        CREATE INDEX message_session ON message(session_id);
        CREATE INDEX part_session ON part(session_id);").unwrap();
    db.execute(
        "INSERT INTO session VALUES('ses_test','真实标题',?1,'1.18.15',1000,6000)",
        [home.to_string_lossy()],
    )
    .unwrap();
    db
}
pub(super) fn message(db: &Connection, id: &str, created: i64, data: Value) {
    db.execute(
        "INSERT INTO message VALUES(?1,'ses_test',?2,?2,?3)",
        params![id, created, data.to_string()],
    )
    .unwrap();
}
pub(super) fn part(db: &Connection, id: &str, message: &str, created: i64, data: Value) {
    db.execute(
        "INSERT INTO part VALUES(?1,?2,'ses_test',?3,?3,?4)",
        params![id, message, created, data.to_string()],
    )
    .unwrap();
}
fn query(service: &NativeSessions) -> Page<SessionSummary, SessionQuery> {
    service
        .query(SessionQuery {
            agent: Some("opencode".into()),
            ..Default::default()
        })
        .unwrap()
}

fn live_tool(db: &Connection, time: i64, state: Value) {
    db.execute("UPDATE session SET time_updated=?1", [time])
        .unwrap();
    message(db, "user_live", time - 1000, json!({"role":"user"}));
    message(
        db,
        "assistant_live",
        time - 500,
        json!({"role":"assistant","parentID":"user_live","modelID":"actual-model"}),
    );
    part(
        db,
        "tool_live",
        "assistant_live",
        time,
        json!({"type":"tool","tool":"local_mcp_work","state":state}),
    );
}

#[test]
fn live_opencode_input_parallel_tools_and_thinking_are_bounded_and_separate_from_bodies() {
    let dir = tempfile::tempdir().unwrap();
    let db = fixture(dir.path());
    let now = chrono::Utc::now();
    let time = now.timestamp_millis();
    live_tool(
        &db,
        time - 100,
        json!({"status":"running","time":{"start":time-100},"input":{"secret":"private-arguments"}}),
    );
    part(
        &db,
        "synthetic_user_text",
        "user_live",
        time - 1200,
        json!({"type":"text","text":"private-synthetic-summary","synthetic":true}),
    );
    part(
        &db,
        "ignored_user_text",
        "user_live",
        time - 1150,
        json!({"type":"text","text":"private-ignored-summary","ignored":true}),
    );
    part(
        &db,
        "user_text",
        "user_live",
        time - 1100,
        json!({"type":"text","text":"请检查登录 api_key=private-credential"}),
    );
    part(
        &db,
        "second_tool",
        "assistant_live",
        time - 50,
        json!({"type":"tool","tool":"Grep","state":{"status":"running","time":{"start":time-50},"output":"private-results"}}),
    );
    let (rows, limited) = opencode_live::activities(dir.path(), now).unwrap();
    assert!(!limited);
    let a = &rows[0];
    let message_started_at = chrono::DateTime::<chrono::Utc>::from_timestamp_millis(time - 1100)
        .unwrap()
        .to_rfc3339();
    assert_eq!(
        a.message_started_at.as_deref(),
        Some(message_started_at.as_str())
    );
    assert_eq!(a.phase, "tool_requested");
    assert!(a.tool_names.contains(&"Grep".into()));
    assert!(a.tool_names.contains(&"local_mcp_work".into()));
    assert_eq!(
        a.user_input.as_ref().unwrap().text,
        "请检查登录 [已隐藏凭据]"
    );
    part(
        &db,
        "thinking",
        "assistant_live",
        time - 10,
        json!({"type":"reasoning","text":"private-thinking","time":{"start":time-10}}),
    );
    let (rows, _) = opencode_live::activities(dir.path(), now).unwrap();
    assert_eq!(rows[0].phase, "thinking");
    assert_eq!(
        rows[0].message_started_at.as_deref(),
        Some(message_started_at.as_str())
    );
    assert!(rows[0].tool_names.is_empty());
    assert!(!serde_json::to_string(&rows).unwrap().contains("private-"));
}

#[test]
fn live_opencode_tool_uses_fresh_wal_metadata_without_index_or_bodies() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().canonicalize().unwrap();
    let db = fixture(&home);
    let now = chrono::Utc::now();
    live_tool(
        &db,
        now.timestamp_millis() - 100,
        json!({"status":"running","time":{"start":now.timestamp_millis()-100},"input":{"secret":"do-not-load-arguments"},"output":"do-not-load-results"}),
    );
    let before: String = db
        .query_row("SELECT data FROM part", [], |r| r.get(0))
        .unwrap();
    let sessions = NativeSessions::new(home, &["opencode".into()]);
    let mut watch = live::NativeWatch::default();
    let a = watch.poll(&sessions).remove(0);
    assert_eq!(a.session_id, "opencode:ses_test");
    assert_eq!(a.agent_product_id.as_deref(), Some("opencode"));
    assert_eq!(a.agent_attribution.as_deref(), Some("native_record"));
    assert_eq!(a.model.as_deref(), Some("actual-model"));
    assert_eq!(a.phase, "tool_requested");
    assert!(a.step.contains("正在执行工具"));
    assert!(!a.stale);
    let text = serde_json::to_string(&a).unwrap();
    assert!(!text.contains("do-not-load"));
    assert_eq!(
        db.query_row::<String, _, _>("SELECT data FROM part", [], |r| r.get(0))
            .unwrap(),
        before
    );
    db.execute(
        "UPDATE part SET data=json_set(data,'$.state.status','completed','$.state.time.end',?1)",
        [now.timestamp_millis()],
    )
    .unwrap();
    let retained = watch.poll(&sessions);
    assert_eq!(retained.len(), 1);
    assert!(retained[0].stale);
    assert_eq!(retained[0].event_at, a.event_at);
    assert_eq!(retained[0].phase, "tool_requested");
    assert!(
        !serde_json::to_string(&retained)
            .unwrap()
            .contains("do-not-load")
    );
    db.execute("UPDATE part SET data=json_remove(json_set(data,'$.state.status','running'),'$.state.time.end')", []).unwrap();
    sessions.set_enabled("opencode", false).unwrap();
    assert!(watch.poll(&sessions).is_empty());
    db.execute(
        "UPDATE part SET data=json_set(data,'$.state.status','completed','$.state.time.end',?1)",
        [now.timestamp_millis()],
    )
    .unwrap();
    sessions.set_enabled("opencode", true).unwrap();
    // Authorized history is reread, while the old running-tool claim is cleared.
    let reread = watch.poll(&sessions);
    assert_eq!(reread.len(), 1);
    assert!(reread[0].stale);
    assert_ne!(reread[0].phase, "tool_requested");
    assert!(reread[0].tool_names.is_empty());
    db.execute("UPDATE part SET data=json_remove(json_set(data,'$.state.status','running'),'$.state.time.end')", []).unwrap();
    assert_eq!(watch.poll(&sessions).len(), 1);
}

#[test]
fn recent_opencode_messages_survive_restart_and_keep_repeated_inputs_separate() {
    let dir = tempfile::tempdir().unwrap();
    let db = fixture(dir.path());
    let now = chrono::Utc::now();
    let time = now.timestamp_millis();
    db.execute("UPDATE session SET time_updated=?1", [time])
        .unwrap();
    for (i, text) in [(1, "repeat"), (2, "second"), (3, "repeat")] {
        let user = format!("user_{i}");
        let at = time - (4 - i) * 1000;
        message(&db, &user, at, json!({"role":"user"}));
        part(
            &db,
            &format!("text_{i}"),
            &user,
            at,
            json!({"type":"text","text":text}),
        );
        for n in 0..80 {
            message(
                &db,
                &format!("assistant_{i}_{n:03}"),
                at + n + 1,
                json!({"role":"assistant","parentID":user,"modelID":"actual-model","time":{"completed":at+n+2},"content":"private-assistant-body"}),
            );
        }
    }
    message(
        &db,
        "summary",
        time - 100,
        json!({"role":"user","summary":true}),
    );
    message(&db, "ignored", time - 50, json!({"role":"user"}));
    part(
        &db,
        "ignored-text",
        "ignored",
        time - 50,
        json!({"type":"text","text":"private-ignored","synthetic":true}),
    );
    let before: String = db
        .query_row("SELECT data FROM message WHERE id='user_1'", [], |r| {
            r.get(0)
        })
        .unwrap();
    let sessions = NativeSessions::new(dir.path().canonicalize().unwrap(), &["opencode".into()]);
    let mut watch = live::NativeWatch::default();
    let rows = watch.poll(&sessions);
    assert_eq!(rows.len(), 3);
    let ids = rows
        .iter()
        .map(|a| a.turn_id.as_deref().unwrap())
        .collect::<std::collections::HashSet<_>>();
    assert_eq!(ids.len(), 3);
    assert_eq!(
        rows.iter()
            .filter(|a| a.user_input.as_ref().is_some_and(|p| p.text == "repeat"))
            .count(),
        2
    );
    assert!(rows.iter().all(|a| a.phase == "waiting_input" && !a.stale));
    assert!(
        !serde_json::to_string(&rows)
            .unwrap()
            .contains("private-assistant")
    );
    assert_eq!(live::NativeWatch::default().poll(&sessions).len(), 3);
    db.execute("DELETE FROM message WHERE id='user_2'", [])
        .unwrap();
    let remaining = watch.poll(&sessions);
    assert_eq!(remaining.len(), 2);
    assert!(
        remaining
            .iter()
            .all(|a| a.turn_id.as_deref() != Some("user_2"))
    );
    assert_eq!(
        db.query_row::<String, _, _>("SELECT data FROM message WHERE id='user_1'", [], |r| r
            .get(0))
            .unwrap(),
        before
    );
    sessions.set_enabled("opencode", false).unwrap();
    assert!(live::NativeWatch::default().poll(&sessions).is_empty());
}

#[test]
fn recent_opencode_message_reads_have_an_explicit_row_budget() {
    let dir = tempfile::tempdir().unwrap();
    let db = fixture(dir.path());
    let now = chrono::Utc::now();
    db.execute(
        "UPDATE session SET time_updated=?1",
        [now.timestamp_millis()],
    )
    .unwrap();
    db.execute_batch("BEGIN").unwrap();
    for i in 0..140 {
        message(
            &db,
            &format!("user_{i}"),
            now.timestamp_millis() - i,
            json!({"role":"user"}),
        );
    }
    db.execute_batch("COMMIT").unwrap();
    let (rows, limited) = opencode_messages::recent(&db, now).unwrap();
    assert_eq!(rows.len(), 128);
    assert!(limited);
}

#[test]
fn live_opencode_malformed_or_oversized_metadata_stays_unknown() {
    for data in [
        "{bad-json".to_owned(),
        json!({"role":"assistant","parentID":"user_live","body":"x".repeat(8*1024*1024)})
            .to_string(),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let db = fixture(dir.path());
        live_tool(&db, 2000, json!({"status":"running","time":{"start":2000}}));
        db.execute(
            "UPDATE message SET data=?1 WHERE id='assistant_live'",
            [data],
        )
        .unwrap();
        let (rows, limited) = opencode_live::activities(
            dir.path(),
            chrono::DateTime::from_timestamp_millis(2500).unwrap(),
        )
        .unwrap();
        assert!(rows.is_empty());
        assert!(limited);
    }
}

#[test]
fn live_opencode_rejects_closed_old_future_and_unknown_tool_states() {
    for state in [
        json!({"status":"running"}),
        json!({"status":"future-state","time":{"start":1000}}),
        json!({"status":"completed","time":{"start":1000}}),
        json!({"status":"running","time":{"start":1000,"end":2000}}),
        json!({"status":"running","time":{"start":100000}}),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let db = fixture(dir.path());
        live_tool(&db, 2000, state);
        let now = chrono::DateTime::from_timestamp_millis(2000).unwrap();
        assert!(
            opencode_live::activities(dir.path(), now)
                .unwrap()
                .0
                .is_empty()
        );
    }
    let dir = tempfile::tempdir().unwrap();
    let db = fixture(dir.path());
    live_tool(&db, 2000, json!({"status":"running","time":{"start":2000}}));
    assert!(
        opencode_live::activities(
            dir.path(),
            chrono::DateTime::from_timestamp_millis(32001).unwrap()
        )
        .unwrap()
        .0
        .is_empty()
    );
    for terminal in [
        json!({"completed":2100}),
        json!({"error":{"name":"Aborted"}}),
    ] {
        db.execute(
            "UPDATE message SET data=?1 WHERE id='assistant_live'",
            [
                json!({"role":"assistant","parentID":"user_live","modelID":"actual-model"})
                    .to_string(),
            ],
        )
        .unwrap();
        db.execute(
            "UPDATE message SET data=json_patch(data,?1) WHERE id='assistant_live'",
            [if terminal.get("completed").is_some() {
                json!({"time":terminal}).to_string()
            } else {
                terminal.to_string()
            }],
        )
        .unwrap();
        assert!(
            opencode_live::activities(
                dir.path(),
                chrono::DateTime::from_timestamp_millis(2200).unwrap()
            )
            .unwrap()
            .0
            .is_empty()
        );
    }
}

#[test]
fn live_opencode_new_turn_or_newer_completed_response_cannot_revive_old_tool() {
    for newer in ["user", "assistant"] {
        let dir = tempfile::tempdir().unwrap();
        let db = fixture(dir.path());
        live_tool(&db, 2000, json!({"status":"pending"}));
        let now = chrono::DateTime::from_timestamp_millis(2500).unwrap();
        assert_eq!(
            opencode_live::activities(dir.path(), now).unwrap().0.len(),
            1
        );
        message(
            &db,
            "new",
            2300,
            json!({"role":newer,"parentID":"user_live","time":{"completed":2400}}),
        );
        assert!(
            opencode_live::activities(dir.path(), now)
                .unwrap()
                .0
                .is_empty()
        );
    }
}

#[test]
fn overview_context_uses_opencode_index_metadata_without_conversation_body() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().canonicalize().unwrap();
    let db = fixture(&home);
    message(
        &db,
        "m",
        2000,
        json!({"role":"assistant","modelID":"m","tokens":{"input":10,"cache":{"read":20,"write":30}}}),
    );
    let sessions = NativeSessions::new(home, &["opencode".into()]);
    sessions.refresh().unwrap();
    let watch = live::NativeWatch::default();
    let samples = watch
        .context_samples(&sessions, vec!["opencode:ses_test".into()])
        .unwrap();
    let context = samples[0].context.as_ref().unwrap();
    assert_eq!(context.input_tokens, Some(60));
    assert!(context.window_tokens.is_none());
    assert!(context.used_percentage.is_none());
    assert_eq!(context.sample_agent_product_id.as_deref(), Some("opencode"));
}

#[test]
fn model_failure_without_parts_keeps_cause_and_unknown_placeholder_usage() {
    let dir = tempfile::tempdir().unwrap();
    let db = fixture(dir.path());
    for (id, error) in [
        (
            "connection",
            json!({"name":"APIError","data":{"message":"Cannot connect to API: AggregateError apiKey=private-fixture-secret","isRetryable":true,"responseHeaders":{"authorization":"do-not-expose"}}}),
        ),
        (
            "auth",
            json!({"name":"APIError","data":{"message":"Access denied","statusCode":401,"isRetryable":false,"responseBody":"private-response-body"}}),
        ),
        (
            "abort",
            json!({"name":"MessageAbortedError","data":{"message":"Aborted"}}),
        ),
    ] {
        message(
            &db,
            id,
            2000,
            json!({"role":"assistant","modelID":"model","providerID":"provider","time":{"created":2000,"completed":3000},"tokens":{"input":0,"output":0},"error":error}),
        );
    }
    let service = NativeSessions::new(dir.path().into(), &["opencode".into()]);
    service.refresh().unwrap();
    let summary = query(&service).items.remove(0);
    assert_eq!(summary.model_calls, 3);
    assert_eq!(summary.model_error_count, 3);
    assert!(summary.usage.total_tokens.is_none());
    let page = service.conversation(&summary.id, None, None).unwrap();
    let connection = page
        .items
        .iter()
        .find(|e| e.native_id == "model:connection")
        .unwrap();
    assert_eq!(connection.status, "error");
    let failure = connection.failure.as_ref().unwrap();
    assert_eq!(failure.category, "connection");
    assert_eq!(failure.retryable, Some(true));
    assert!(failure.http_status.is_none());
    assert!(
        failure
            .message
            .as_deref()
            .unwrap()
            .contains("Cannot connect to API")
    );
    let auth = page
        .items
        .iter()
        .find(|e| e.native_id == "model:auth")
        .unwrap();
    assert_eq!(auth.failure.as_ref().unwrap().http_status, Some(401));
    assert_eq!(auth.failure.as_ref().unwrap().category, "authentication");
    assert_eq!(
        page.items
            .iter()
            .find(|e| e.native_id == "model:abort")
            .unwrap()
            .status,
        "cancelled"
    );
    let encoded = serde_json::to_string(&page).unwrap();
    for secret in [
        "private-fixture-secret",
        "do-not-expose",
        "private-response-body",
    ] {
        assert!(!encoded.contains(secret));
    }
}

fn log_error(session: &str, timestamp: &str, message: &str) -> String {
    format!(
        "timestamp={timestamp} level=ERROR message=\"stream error\" providerID=privatemode-ai modelID=glm-flash-latest session.id={session} error.error={}\n",
        serde_json::to_string(message).unwrap()
    )
}

#[test]
fn log_only_errors_refresh_without_database_changes_anchor_and_never_count_as_requests() {
    use std::io::Write;
    let dir = tempfile::tempdir().unwrap();
    let db = fixture(dir.path());
    message(&db, "user", 1000, json!({"role":"user"}));
    message(
        &db,
        "pending",
        2000,
        json!({"role":"assistant","parentID":"user","providerID":"privatemode-ai","modelID":"glm-flash-latest","time":{"created":2000},"tokens":{"input":0,"output":0}}),
    );
    for n in 0..45 {
        part(
            &db,
            &format!("text{n:02}"),
            "user",
            1000 + n,
            json!({"type":"text","text":"prompt"}),
        );
    }
    let log = opencode_log::path(dir.path());
    fs::create_dir_all(log.parent().unwrap()).unwrap();
    let error = "AI_APICallError: Cannot connect to API: (cause: AggregateError) Authorization=private-header";
    let mut lines = log_error("ses_test", "2026-10-18T15:00:00Z", error);
    lines += &log_error("ses_test", "2026-10-18T15:01:00Z", error);
    lines += &log_error("another_session", "2026-10-18T15:02:00Z", error);
    lines += &log_error("", "2026-10-18T15:03:00Z", error);
    lines +=
        &log_error("ses_test", "2026-10-18T15:04:00Z", error).replace("level=ERROR", "level=INFO");
    fs::write(&log, lines).unwrap();
    let service = NativeSessions::new(dir.path().into(), &["opencode".into()]);
    service.refresh().unwrap();
    let summary = query(&service).items.remove(0);
    assert_eq!(summary.model_error_count, 1);
    assert_eq!(summary.model_calls, 1);
    assert!(summary.usage.total_tokens.is_none());
    let calls = service
        .calls(CallQuery {
            agent: Some("opencode".into()),
            kind: Some("diagnostic".into()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(calls.total, 1);
    assert_eq!(calls.items[0].failure.as_ref().unwrap().occurrences, 2);
    assert!(calls.items[0].failure.as_ref().unwrap().message.is_none());
    assert_eq!(
        calls.items[0].timestamp.as_deref(),
        Some("2026-10-18T15:01:00+00:00")
    );
    let first = service.conversation(&summary.id, None, None).unwrap();
    let cursor = first.next_cursor.unwrap();
    let anchored = service
        .conversation(&summary.id, None, summary.latest_failure_id.as_deref())
        .unwrap();
    let event = anchored
        .items
        .iter()
        .find(|e| e.kind == "diagnostic")
        .unwrap();
    assert!(event.turn_id.ends_with(":turn:unknown"));
    assert_eq!(event.failure.as_ref().unwrap().category, "connection");
    assert!(
        event
            .failure
            .as_ref()
            .unwrap()
            .message
            .as_deref()
            .unwrap()
            .contains("Cannot connect")
    );
    assert!(
        !serde_json::to_string(&anchored)
            .unwrap()
            .contains("private-header")
    );
    writeln!(
        fs::OpenOptions::new().append(true).open(&log).unwrap(),
        "{}",
        log_error("ses_test", "2026-10-18T15:05:00Z", error).trim()
    )
    .unwrap();
    assert_eq!(
        service
            .conversation(&summary.id, Some(&cursor), None)
            .unwrap_err()
            .code,
        "stale_snapshot"
    );
    service.refresh().unwrap();
    let refreshed = query(&service).items.remove(0);
    assert_eq!(refreshed.latest_failure_id, summary.latest_failure_id);
    let calls = service
        .calls(CallQuery {
            kind: Some("diagnostic".into()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(calls.items[0].failure.as_ref().unwrap().occurrences, 3);
    service.set_enabled("opencode", false).unwrap();
    assert_eq!(service.calls(CallQuery::default()).unwrap().total, 0);
    assert!(service.conversation(&summary.id, None, None).is_err());
}

#[test]
fn diagnostic_log_bounds_latest_errors_partial_lines_and_unsafe_paths() {
    let dir = tempfile::tempdir().unwrap();
    let log = opencode_log::path(dir.path());
    fs::create_dir_all(log.parent().unwrap()).unwrap();
    let mut lines = String::new();
    for n in 0..1003 {
        lines += &log_error(
            &format!("session{n}"),
            "2026-10-18T15:00:00Z",
            "AI_APICallError: Cannot connect",
        );
    }
    lines += log_error("unfinished", "2026-10-18T15:00:00Z", "failure").trim_end();
    fs::write(&log, lines).unwrap();
    let logs = opencode_log::load(dir.path());
    assert_eq!(logs.sessions.len(), 1000);
    assert!(!logs.sessions.contains_key("session0"));
    assert!(logs.sessions.contains_key("session1002"));
    assert!(!logs.sessions.contains_key("unfinished"));
    assert!(!logs.warnings.is_empty());
    let mut lines = log_error("old", "2026-10-18T15:00:00Z", "old");
    lines += &"padding\n".repeat(400000);
    lines += &log_error("latest", "2026-10-18T15:00:00Z", "Cannot connect");
    fs::write(&log, lines).unwrap();
    let logs = opencode_log::load(dir.path());
    assert!(!logs.sessions.contains_key("old"));
    assert!(logs.sessions.contains_key("latest"));
    assert!(logs.warnings.iter().any(|s| s.contains("2 MiB")));
    #[cfg(unix)]
    {
        fs::remove_file(&log).unwrap();
        let external = dir.path().join("outside");
        fs::write(&external, "private").unwrap();
        std::os::unix::fs::symlink(&external, &log).unwrap();
        let logs = opencode_log::load(dir.path());
        assert!(logs.sessions.is_empty());
        assert!(!logs.warnings.is_empty());
        fs::remove_file(&log).unwrap();
        fs::hard_link(&external, &log).unwrap();
        assert!(opencode_log::load(dir.path()).sessions.is_empty());
    }
}

#[test]
fn bounded_diagnostics_keep_complete_sessions_and_mcp_readable() {
    let dir = tempfile::tempdir().unwrap();
    let db = fixture(dir.path());
    message(&db, "assistant", 1000, json!({"role":"assistant"}));
    for n in 0..45 {
        part(
            &db,
            &format!("part{n:02}"),
            "assistant",
            1000 + n,
            json!({"type":"tool","tool":"local_check_read","state":{"status":"completed","input":{"query":"contract"},"output":"result","time":{"start":1000+n,"end":1004+n}}}),
        );
    }
    let config = dir.path().join("opencode.json");
    let config_bytes = br#"{"mcp":{"local_check":{"type":"local","command":["never-run"]}}}"#;
    fs::write(&config, config_bytes).unwrap();
    let log = opencode_log::path(dir.path());
    fs::create_dir_all(log.parent().unwrap()).unwrap();
    let mut lines = "padding\n".repeat(400000);
    for n in 0..1003 {
        lines += &log_error(
            &format!("other_session{n}"),
            "2026-10-18T15:00:00Z",
            "Cannot connect",
        );
    }
    fs::write(&log, lines).unwrap();
    let main_before = fs::read(opencode::path(dir.path())).unwrap();
    let service = NativeSessions::new(dir.path().into(), &["opencode".into()]);
    service.refresh().unwrap();
    let session = query(&service).items.remove(0);
    assert!(!session.partial);
    assert_eq!(session.mcp_calls, 45);
    let source = service.sources().unwrap().remove(2);
    assert_eq!(source.status, "ready");
    assert!(source.error.is_none());
    assert_eq!(source.skipped_files, 0);
    let first = service.conversation(&session.id, None, None).unwrap();
    assert!(first.warnings.iter().any(|s| s.contains("2 MiB")));
    assert!(first.warnings.iter().any(|s| s.contains("1000")));
    let tail = service
        .conversation(&session.id, first.next_cursor.as_deref(), None)
        .unwrap();
    assert!(tail.next_cursor.is_none());
    let anchor = &tail.items.iter().rfind(|e| e.kind == "mcp").unwrap().id;
    let anchored = service
        .conversation(&session.id, None, Some(anchor))
        .unwrap();
    assert!(anchored.warnings.iter().any(|s| s.contains("2 MiB")));
    let call = anchored.items.iter().find(|e| &e.id == anchor).unwrap();
    assert_eq!(call.server.as_deref(), Some("local_check"));
    assert_eq!(
        call.tool_source.as_ref().unwrap().tool_name.as_deref(),
        Some("read")
    );
    assert_eq!(
        serde_json::from_str::<Value>(call.input.as_ref().unwrap()).unwrap()["query"],
        "contract"
    );
    assert_eq!(call.output.as_deref(), Some("result"));
    assert_eq!(call.duration_ms, Some(4));
    assert_eq!(fs::read(opencode::path(dir.path())).unwrap(), main_before);
    assert_eq!(fs::read(config).unwrap(), config_bytes);
}

#[test]
fn sqlite_mcp_config_mapping_refresh_paging_and_unknown_evidence() {
    let dir = tempfile::tempdir().unwrap();
    let db = fixture(dir.path());
    message(&db, "msg_reply", 1000, json!({"role":"assistant"}));
    for n in 0..45 {
        part(
            &db,
            &format!("prt_{n:03}"),
            "msg_reply",
            1000 + n,
            json!({
                "type":"tool","tool":if n == 44 { "removed_old_read" } else { "local_agent_check_get_contract" },
                "callID":format!("upstream-{n}"),"state":{"status":"completed","input":{"query":"contract","apiKey":"secret-fixture-key"},"output":"contract-result","time":{"start":1000+n,"end":1004+n}}
            }),
        );
    }
    let config = dir.path().join("opencode.jsonc");
    fs::write(
        &config,
        r#"{"mcp":{"local_agent_check":{"type":"local","command":["never-run"]}}}"#,
    )
    .unwrap();
    let service = NativeSessions::new(dir.path().into(), &["opencode".into()]);
    service.refresh().unwrap();
    let session = query(&service).items.remove(0);
    assert_eq!(session.mcp_calls, 44);
    assert_eq!(session.tool_calls, 1);
    let first = service.conversation(&session.id, None, None).unwrap();
    let event = first.items.iter().find(|e| e.kind == "mcp").unwrap();
    assert_eq!(event.server.as_deref(), Some("local_agent_check"));
    assert_eq!(event.duration_ms, Some(4));
    assert_eq!(event.output.as_deref(), Some("contract-result"));
    let source = event.tool_source.as_ref().unwrap();
    assert_eq!(source.status, "configuration");
    assert_eq!(source.tool_name.as_deref(), Some("get_contract"));
    assert_eq!(source.sources[0].scope, "project");
    let encoded = serde_json::to_string(&first).unwrap();
    assert!(!encoded.contains("secret-fixture-key"));
    assert!(!encoded.contains("never-run"));
    let cursor = first.next_cursor.unwrap();
    let last = service
        .conversation(&session.id, Some(&cursor), None)
        .unwrap();
    let unknown = last
        .items
        .iter()
        .find(|e| e.name == "removed_old_read")
        .unwrap();
    assert_eq!(unknown.tool_source.as_ref().unwrap().status, "unknown");
    assert!(unknown.server.is_none());
    // The database is unchanged; configuration changes must still invalidate
    // paging and the metadata/call index together.
    fs::write(&config, "{}").unwrap();
    assert_eq!(
        service
            .conversation(&session.id, Some(&cursor), None)
            .unwrap_err()
            .code,
        "stale_snapshot"
    );
    service.refresh().unwrap();
    assert_eq!(query(&service).items[0].mcp_calls, 0);
    assert_eq!(query(&service).items[0].tool_calls, 45);
    assert!(
        service
            .conversation(&session.id, None, None)
            .unwrap()
            .items
            .iter()
            .all(|e| e.kind != "mcp")
    );
    // An unreadable configuration affects attribution, not transcript integrity.
    fs::write(&config, "{invalid json").unwrap();
    service.refresh().unwrap();
    let session = query(&service).items.remove(0);
    assert!(!session.partial);
    assert_eq!(session.tool_calls, 45);
    let source = service.sources().unwrap().remove(2);
    assert_eq!(source.status, "ready");
    assert!(source.error.is_none());
    let conversation = service.conversation(&session.id, None, None).unwrap();
    assert!(!conversation.warnings.is_empty());
    assert!(conversation.items.iter().all(|e| {
        e.tool_source
            .as_ref()
            .is_some_and(|s| s.status == "unknown")
            && e.server.is_none()
    }));
}

#[test]
fn sqlite_wal_consent_messages_usage_calls_writes_and_redaction() {
    let dir = tempfile::tempdir().unwrap();
    let db = fixture(dir.path());
    message(&db, "msg_user", 1000, json!({"role":"user"}));
    message(
        &db,
        "msg_assistant",
        2000,
        json!({"role":"assistant","parentID":"msg_user","modelID":"native-model","providerID":"local-connection","tokens":{"input":10,"output":3,"reasoning":2,"cache":{"read":20,"write":5}}}),
    );
    part(
        &db,
        "prt_user",
        "msg_user",
        1000,
        json!({"type":"text","text":"native-opencode-body-sentinel"}),
    );
    part(
        &db,
        "prt_reply",
        "msg_assistant",
        2000,
        json!({"type":"text","text":"检查完成"}),
    );
    part(
        &db,
        "prt_skill",
        "msg_assistant",
        2100,
        json!({"type":"tool","tool":"skill","callID":"same-upstream-call","state":{"status":"completed","input":{"name":"review"},"output":"skill loaded","time":{"start":2100,"end":2200}}}),
    );
    part(
        &db,
        "prt_mcp",
        "msg_assistant",
        2300,
        json!({"type":"tool","tool":"mcp__local__read","callID":"same-upstream-call","state":{"status":"error","input":{"password":"secret with spaces"},"error":"tool-error-sentinel","time":{"start":2300,"end":2500}}}),
    );
    part(
        &db,
        "prt_custom",
        "msg_assistant",
        2600,
        json!({"type":"tool","tool":"my_server_custom","state":{"status":"running","input":{}}}),
    );
    let target = dir.path().join("test.txt");
    fs::write(&target, "fixture").unwrap();
    part(
        &db,
        "prt_write",
        "msg_assistant",
        2700,
        json!({"type":"tool","tool":"write","callID":"write-call","state":{"status":"completed","input":{"filePath":target},"output":"wrote file","time":{"start":2700,"end":2800}}}),
    );
    // Step-level usage duplicates the assistant snapshot; never sum both sources.
    part(
        &db,
        "prt_step",
        "msg_assistant",
        2900,
        json!({"type":"step-finish","tokens":{"input":10,"output":3,"reasoning":2,"cache":{"read":20,"write":5}}}),
    );
    part(
        &db,
        "prt_public_reasoning",
        "msg_assistant",
        3000,
        json!({"type":"reasoning","text":"原生公开推理记录"}),
    );
    part(
        &db,
        "prt_empty_reasoning",
        "msg_assistant",
        3100,
        json!({"type":"reasoning","text":""}),
    );
    part(
        &db,
        "prt_notice",
        "msg_assistant",
        3200,
        json!({"type":"provider-notice","message":"retained as native detail"}),
    );
    let main_before = fs::read(opencode::path(dir.path())).unwrap();
    let service = NativeSessions::new(dir.path().into(), &[]);
    service.refresh().unwrap();
    assert_eq!(query(&service).total, 0);
    service.set_enabled("opencode", true).unwrap();
    service.refresh().unwrap();
    let page = query(&service);
    assert_eq!(page.total, 1, "{:?}", service.sources().unwrap());
    let session = &page.items[0];
    assert_eq!(session.title, "真实标题");
    assert_eq!(session.agent_version.as_deref(), Some("1.18.15"));
    assert_eq!(session.model_calls, 1);
    assert_eq!(session.message_count, 2);
    assert_eq!(session.skill_calls, 1);
    assert_eq!(session.mcp_calls, 1);
    assert_eq!(session.tool_calls, 2);
    assert_eq!(session.usage.total_tokens, Some(40));
    assert_eq!(session.usage.input_tokens, Some(35));
    assert_eq!(session.usage.output_tokens, Some(5));
    assert_eq!(session.disk_scope, "native_database_payload");
    assert_eq!(session.disk_allocated_bytes, None);
    assert!(
        !serde_json::to_string(&page)
            .unwrap()
            .contains("native-opencode-body-sentinel")
    );
    let conversation = service.conversation(&session.id, None, None).unwrap();
    assert!(
        conversation
            .items
            .iter()
            .any(|e| e.text.as_deref() == Some("native-opencode-body-sentinel"))
    );
    let mcp = conversation.items.iter().find(|e| e.kind == "mcp").unwrap();
    assert_eq!(mcp.status, "error");
    assert_eq!(mcp.duration_ms, Some(200));
    assert_eq!(mcp.call_id.as_deref(), Some("same-upstream-call"));
    assert_eq!(mcp.tool_source.as_ref().unwrap().status, "native");
    let unknown = conversation
        .items
        .iter()
        .find(|e| e.name == "my_server_custom")
        .unwrap();
    assert_eq!(unknown.tool_source.as_ref().unwrap().status, "unknown");
    assert!(unknown.duration_ms.is_none());
    assert!(unknown.output.is_none());
    assert_eq!(unknown.status, "missing_result");
    assert!(
        !serde_json::to_string(&conversation)
            .unwrap()
            .contains("secret with spaces")
    );
    assert!(
        conversation
            .items
            .iter()
            .any(|e| e.name == "Skill · review")
    );
    assert!(
        conversation
            .items
            .iter()
            .all(|e| e.kind != "context" && !e.name.contains("step-"))
    );
    assert_eq!(
        conversation
            .items
            .iter()
            .filter(|e| e.name == "公开推理记录")
            .count(),
        1
    );
    assert!(
        conversation
            .items
            .iter()
            .any(|e| e.name == "原生事件 · provider-notice"
                && e.kind == "event"
                && e.text.is_some())
    );
    let reused: Vec<_> = conversation
        .items
        .iter()
        .filter(|e| e.call_id.as_deref() == Some("same-upstream-call"))
        .collect();
    assert_eq!(reused.len(), 2);
    assert_ne!(reused[0].id, reused[1].id);
    assert_eq!(service.project_catalog().unwrap().writes.len(), 1);
    assert_eq!(
        service
            .calls(CallQuery {
                agent: Some("opencode".into()),
                ..Default::default()
            })
            .unwrap()
            .total,
        5
    );
    assert_eq!(
        fs::read(opencode::path(dir.path())).unwrap(),
        main_before,
        "read-only import must not checkpoint or rewrite native database"
    );
    service.set_enabled("opencode", false).unwrap();
    assert_eq!(query(&service).total, 0);
    assert!(service.conversation(&session.id, None, None).is_err());
}

#[test]
fn sqlite_pagination_anchor_refresh_and_in_place_update() {
    let dir = tempfile::tempdir().unwrap();
    let db = fixture(dir.path());
    for i in 0..45 {
        let id = format!("msg_{i:03}");
        message(&db, &id, 1000 + i, json!({"role":"user"}));
        part(
            &db,
            &format!("prt_{i:03}"),
            &id,
            1000 + i,
            json!({"type":"text","text":format!("body-{i}")}),
        );
    }
    let service = NativeSessions::new(dir.path().into(), &["opencode".into()]);
    service.refresh().unwrap();
    let session = query(&service).items.remove(0);
    let first = service.conversation(&session.id, None, None).unwrap();
    assert_eq!(first.items.len(), 40);
    let cursor = first.next_cursor.unwrap();
    let tail = service
        .conversation(&session.id, Some(&cursor), None)
        .unwrap();
    assert_eq!(tail.items.len(), 5);
    assert!(tail.next_cursor.is_none());
    let anchor = &tail.items[4].id;
    assert!(
        service
            .conversation(&session.id, None, Some(anchor))
            .unwrap()
            .items
            .iter()
            .any(|e| &e.id == anchor && e.text.is_some())
    );
    assert!(
        service
            .conversation(&session.id, None, Some("opencode:other:part:prt_044"))
            .is_err()
    );
    // A native part update can happen without touching session.time_updated.
    db.execute(
        "UPDATE part SET data=?1,time_updated=9000 WHERE id='prt_001'",
        [json!({"type":"text","text":"changed"}).to_string()],
    )
    .unwrap();
    assert_eq!(
        service
            .conversation(&session.id, Some(&cursor), None)
            .unwrap_err()
            .code,
        "stale_snapshot"
    );
    service.refresh().unwrap();
    assert!(
        service
            .conversation(&session.id, None, None)
            .unwrap()
            .items
            .iter()
            .any(|e| e.text.as_deref() == Some("changed"))
    );
    let second = NativeSessions::new(dir.path().into(), &["opencode".into()]);
    second.refresh().unwrap();
    assert_eq!(query(&second).total, 1);
}

#[test]
fn sqlite_missing_bad_schema_malformed_records_and_symlinks_are_explicit() {
    let dir = tempfile::tempdir().unwrap();
    let service = NativeSessions::new(dir.path().into(), &["opencode".into()]);
    service.refresh().unwrap();
    assert!(!opencode::path(dir.path()).exists());
    assert_eq!(service.sources().unwrap()[2].status, "partial");
    let db = fixture(dir.path());
    message(&db, "msg_user", 1000, json!({"role":"user"}));
    part(
        &db,
        "prt_good",
        "msg_user",
        1000,
        json!({"type":"text","text":"kept"}),
    );
    db.execute(
        "INSERT INTO part VALUES('prt_broken','msg_user','ses_test',2000,2000,'not json')",
        [],
    )
    .unwrap();
    part(
        &db,
        "prt_orphan",
        "unknown_message",
        3000,
        json!({"type":"text","text":"not merged"}),
    );
    service.refresh().unwrap();
    let session = query(&service).items.remove(0);
    assert!(session.partial);
    let source = service.sources().unwrap().remove(2);
    assert_eq!(source.status, "partial");
    assert!(source.error.is_some());
    assert!(source.skipped_files > 0);
    let c = service.conversation(&session.id, None, None).unwrap();
    assert!(c.items.iter().any(|e| e.text.as_deref() == Some("kept")));
    assert!(
        !c.items
            .iter()
            .any(|e| e.text.as_deref() == Some("not merged"))
    );
    drop(db);
    #[cfg(unix)]
    {
        let moved = opencode::path(dir.path()).with_extension("outside");
        fs::rename(opencode::path(dir.path()), &moved).unwrap();
        std::os::unix::fs::symlink(&moved, opencode::path(dir.path())).unwrap();
        assert!(opencode::scan(dir.path(), &HashMap::new()).is_err());
        service.refresh().unwrap();
        let source = service.sources().unwrap().remove(2);
        assert_eq!(source.status, "partial");
        assert!(source.error.is_some());
        assert_eq!(source.scanned_files, 0);
        assert_eq!(source.skipped_files, 0);
        assert_eq!(query(&service).total, 1, "keep the previous readable index");
        assert!(service.conversation(&session.id, None, None).is_err());
    }
    let other = tempfile::tempdir().unwrap();
    let db = fixture(other.path());
    db.execute_batch("DROP TABLE part").unwrap();
    assert!(opencode::scan(other.path(), &HashMap::new()).is_err());
}
