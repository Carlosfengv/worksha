use super::*;
use std::fs;

const RUN: &str = "pid:123:00000000-0000-4000-8000-000000000001";
fn activity(session: &str, turn: &str, phase: &str) -> NativeActivity {
    NativeActivity {
        message_usage: None,
        turn_id: Some(turn.into()),
        call_id: Some("call-one".into()),
        agent_product_id: None,
        agent_attribution: None,
        id: format!("codex:{session}"),
        agent_id: "codex".into(),
        session_id: format!("codex:{session}"),
        title: "unchanged".into(),
        project: None,
        model: None,
        phase: phase.into(),
        step: "native progress".into(),
        user_input: None,
        tool_names: Vec::new(),
        message_started_at: None,
        event_at: Utc::now().to_rfc3339(),
        turn_ttft_ms: None,
        realtime: None,
        stale: false,
    }
}
fn database(home: &Path) -> Connection {
    fs::create_dir_all(home.join(".codex")).unwrap();
    let c = Connection::open(home.join(".codex/logs_2.sqlite")).unwrap();
    c.execute_batch("CREATE TABLE logs(id INTEGER PRIMARY KEY,ts INTEGER,ts_nanos INTEGER,thread_id TEXT,target TEXT,process_uuid TEXT,feedback_log_body TEXT); CREATE INDEX idx_logs_thread_id_ts ON logs(thread_id,ts DESC,ts_nanos DESC,id DESC);").unwrap();
    c
}
fn body(session: &str, turn: &str) -> String {
    format!(
        "session_loop{{thread_id={session}}}:submission_dispatch{{otel.name=\"op.dispatch.turn_input\" submission.id=\"{turn}\" codex.op=\"turn_input\"}}:codex.turn_input{{conversation.id={session} turn.id={turn}}}:received stream event"
    )
}
fn tool_body(session: &str, turn: &str, call: &str, started: bool) -> String {
    format!(
        "{}handle_output_item_done:handle_tool_call_with_source: tool call completed event.name=\"codex.tool_call\" trace_id= conversation.id={session} turn_id={turn} tool_name=read call_id={call} tool_source=\"direct\" execution_started={started} dispatch_duration_ms=0 handler_duration_ms=5 total_duration_ms=5",
        body(session, turn)
            .strip_suffix("received stream event")
            .unwrap()
    )
}
fn insert(c: &Connection, session: &str, target: &str, process: &str, body: &str) {
    c.execute("INSERT INTO logs(ts,ts_nanos,thread_id,target,process_uuid,feedback_log_body) VALUES(?1,0,?2,?3,?4,?5)",params![Utc::now().timestamp(),session,target,process,body]).unwrap();
}
fn processes(pid: i32) -> Option<Process> {
    Some(Process { path: match pid {
        123 => "/Applications/ChatGPT.app/Contents/Resources/codex-cli/CodexCLI.app/Contents/MacOS/codex",
        456 => "/Applications/ChatGPT.app/Contents/MacOS/ChatGPT", _ => return None,
    }.into(), parent: if pid == 123 {456} else {1}, uid: 501, started: 1_700_000_000, started_micros:0, tty:u32::MAX })
}

const TUI_START: &str = "tui startup initial frame scheduled duration_ms=42 bootstrap_ms=11 thread_and_widget_ms=2 initial_session_ms=0 event_stream_ms=0";
fn startup(c: &Connection, runtime: &str, body: &str) {
    c.execute_batch("CREATE INDEX IF NOT EXISTS idx_logs_process_uuid_threadless_ts ON logs(process_uuid,ts DESC,ts_nanos DESC,id DESC) WHERE thread_id IS NULL;").unwrap();
    c.execute("INSERT INTO logs(ts,ts_nanos,thread_id,target,process_uuid,feedback_log_body) VALUES(1700000001,0,NULL,'codex_tui::app::startup',?1,?2)",params![runtime,body]).unwrap();
}
fn tui_processes(pid: i32) -> Option<Process> {
    let mut p = processes(pid)?;
    if pid == 123 {
        p.tty = 42;
    }
    Some(p)
}

fn world_body(session: &str, turn: &str) -> String {
    format!(
        "{}session_task.run:run_turn:world_state.build: building step world state selected_capability_root_count=0",
        body(session, turn)
            .strip_suffix("received stream event")
            .unwrap()
    )
}

#[test]
fn tui_verified_mode_survives_log_retention_but_never_substitutes_for_execution() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().canonicalize().unwrap();
    let c = database(&home);
    startup(&c, RUN, TUI_START);
    insert(
        &c,
        "tui",
        "codex_core::session::world_state",
        RUN,
        &world_body("tui", "first"),
    );
    let mut cache = RuntimeEvidence::default();
    let mut rows = vec![activity("tui", "first", "processing")];
    attribute_cached_with(&home, &mut rows, 501, &mut cache, tui_processes);
    assert_eq!(rows[0].agent_product_id.as_deref(), Some("codex-cli"));
    assert_eq!(cache.tui_modes.len(), 1);
    c.execute("DELETE FROM logs", []).unwrap();
    rows = vec![
        activity("tui", "next", "processing"),
        activity("other", "next", "processing"),
    ];
    attribute_cached_with(&home, &mut rows, 501, &mut cache, tui_processes);
    assert!(rows.iter().all(|r| r.agent_product_id.is_none()));
    insert(
        &c,
        "tui",
        "codex_core::stream_events_utils",
        RUN,
        &body("tui", "next"),
    );
    attribute_cached_with(&home, &mut rows, 501, &mut cache, tui_processes);
    assert_eq!(rows[0].agent_product_id.as_deref(), Some("codex-cli"));
    assert!(rows[1].agent_product_id.is_none());
    let mut cold = vec![activity("tui", "next", "processing")];
    attribute_with(&home, &mut cold, 501, tui_processes);
    assert!(cold[0].agent_product_id.is_none());
    rows[0].phase = "tool_requested".into();
    attribute_cached_with(&home, &mut rows, 501, &mut cache, tui_processes);
    assert!(rows.iter().all(|r| r.agent_product_id.is_none()));
    attribute_cached_with(&home, &mut rows, 501, &mut cache, |_| None);
    assert!(cache.tui_modes.is_empty());
}

#[test]
fn cached_tui_mode_is_revoked_for_identity_database_and_uid_changes() {
    for case in 0..5 {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().canonicalize().unwrap();
        let c = database(&home);
        startup(&c, RUN, TUI_START);
        insert(
            &c,
            "tui",
            "codex_core::stream_events_utils",
            RUN,
            &body("tui", "first"),
        );
        let mut cache = RuntimeEvidence::default();
        let mut rows = vec![activity("tui", "first", "processing")];
        attribute_cached_with(&home, &mut rows, 501, &mut cache, tui_processes);
        assert_eq!(cache.tui_modes.len(), 1);
        c.execute("DELETE FROM logs WHERE thread_id IS NULL", [])
            .unwrap();
        drop(c);
        if case == 3 {
            fs::rename(
                home.join(".codex/logs_2.sqlite"),
                home.join(".codex/old.sqlite"),
            )
            .unwrap();
            let c = database(&home);
            insert(
                &c,
                "tui",
                "codex_core::stream_events_utils",
                RUN,
                &body("tui", "first"),
            );
        }
        let inspect = |pid| {
            let mut p = tui_processes(pid)?;
            if pid == 123 {
                match case {
                    0 => p.started_micros += 1,
                    1 => p.tty = 99,
                    2 => p.parent = 1,
                    _ => {}
                }
            }
            Some(p)
        };
        attribute_cached_with(
            &home,
            &mut rows,
            if case == 4 { 502 } else { 501 },
            &mut cache,
            inspect,
        );
        assert!(rows[0].agent_product_id.is_none());
        assert!(cache.tui_modes.is_empty());
    }
}

#[test]
fn world_state_requires_exact_turn_execution_and_numeric_fixed_event() {
    let value = world_body("one", "turn-one");
    assert!(world_state(&value));
    assert!(!world_state(&value.replace("count=0", "count=unknown")));
    assert!(!world_state(&format!("{value} injected=1")));
    assert!(!world_state(&body("one", "turn-one")));
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().canonicalize().unwrap();
    let c = database(&home);
    startup(&c, RUN, TUI_START);
    insert(
        &c,
        "tui",
        "codex_core::session::world_state",
        RUN,
        &world_body("tui", "other-turn"),
    );
    let mut rows = vec![activity("tui", "first", "processing")];
    attribute_with(&home, &mut rows, 501, tui_processes);
    assert!(rows[0].agent_product_id.is_none());
}

#[test]
fn interactive_tui_requires_the_same_live_terminal_runtime_and_exact_execution() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().canonicalize().unwrap();
    let c = database(&home);
    startup(&c, RUN, TUI_START);
    insert(
        &c,
        "tui",
        "codex_core::stream_events_utils",
        RUN,
        &body("tui", "turn-one"),
    );
    insert(
        &c,
        "desktop",
        "codex_core::stream_events_utils",
        "pid:321:00000000-0000-4000-8000-000000000002",
        &body("desktop", "turn-two"),
    );
    drop(c);
    let original = fs::read(home.join(".codex/logs_2.sqlite")).unwrap();
    let inspect = |pid| {
        if pid == 321 {
            processes(123)
        } else {
            tui_processes(pid)
        }
    };
    let mut rows = vec![
        activity("tui", "turn-one", "processing"),
        activity("desktop", "turn-two", "processing"),
        activity("tui", "other-turn", "processing"),
    ];
    attribute_with(&home, &mut rows, 501, inspect);
    assert_eq!(rows[0].agent_product_id.as_deref(), Some("codex-cli"));
    assert_eq!(
        rows[0].agent_attribution.as_deref(),
        Some("cli_runtime_execution")
    );
    assert_eq!(rows[1].agent_product_id.as_deref(), Some("codex-desktop"));
    assert!(rows[2].agent_product_id.is_none());
    assert_eq!(
        fs::read(home.join(".codex/logs_2.sqlite")).unwrap(),
        original
    );
    attribute_with(&home, &mut rows, 501, |_| None);
    assert!(rows.iter().all(|a| a.agent_product_id.is_none()));
}

#[test]
fn tui_startup_is_not_execution_and_requires_supported_index_complete_marker_and_terminal() {
    for case in 0..8 {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().canonicalize().unwrap();
        let c = database(&home);
        startup(&c, RUN, TUI_START);
        if case != 0 {
            insert(
                &c,
                "a",
                "codex_core::stream_events_utils",
                RUN,
                &body("a", "turn-a"),
            );
        }
        match case {
            1 => {
                c.execute_batch("DROP INDEX idx_logs_process_uuid_threadless_ts")
                    .unwrap();
            }
            2 => {
                c.execute("UPDATE logs SET process_uuid='pid:222:00000000-0000-4000-8000-000000000001' WHERE thread_id IS NULL",[]).unwrap();
            }
            3 => {
                c.execute("UPDATE logs SET feedback_log_body='received tui startup initial frame scheduled' WHERE thread_id IS NULL",[]).unwrap();
            }
            4 => {
                c.execute("UPDATE logs SET ts=1700000031 WHERE thread_id IS NULL", [])
                    .unwrap();
            }
            5 => {
                c.execute("UPDATE logs SET ts=1699999999 WHERE thread_id IS NULL", [])
                    .unwrap();
            }
            6 => {
                c.execute(
                    "UPDATE logs SET ts_nanos=1000000000 WHERE thread_id IS NULL",
                    [],
                )
                .unwrap();
            }
            7 => {
                c.execute(
                    "UPDATE logs SET feedback_log_body=?1 WHERE thread_id IS NULL",
                    [format!("{TUI_START} event_stream_ms=0")],
                )
                .unwrap();
            }
            _ => {}
        }
        let mut rows = vec![activity("a", "turn-a", "processing")];
        attribute_with(&home, &mut rows, 501, tui_processes);
        assert!(rows[0].agent_product_id.is_none(), "case {case}");
    }
    assert!(!tui_start(&format!("{TUI_START} content=untrusted")));
}

#[test]
fn tui_runtime_rejects_unknown_host_ownership_changes_conflicts_and_incomplete_tools() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().canonicalize().unwrap();
    let c = database(&home);
    startup(&c, RUN, TUI_START);
    insert(
        &c,
        "a",
        "codex_core::stream_events_utils",
        RUN,
        &body("a", "turn-a"),
    );
    for case in 0..4 {
        let mut rows = vec![activity("a", "turn-a", "processing")];
        attribute_with(&home, &mut rows, 501, |pid| {
            let mut p = tui_processes(pid)?;
            if pid == 123 {
                match case {
                    0 => p.tty = u32::MAX,
                    1 => p.uid = 999,
                    2 => p.path = "/unverified/codex".into(),
                    _ => p.started = Utc::now().timestamp() + 1,
                }
            }
            // An unrecognised parent must not accidentally prove desktop mode.
            if pid == 456 {
                p.path = "/unverified/launcher".into();
            }
            Some(p)
        });
        assert!(rows[0].agent_product_id.is_none(), "case {case}");
    }
    let mut rows = vec![activity("a", "turn-a", "tool_requested")];
    attribute_with(&home, &mut rows, 501, tui_processes);
    assert!(rows[0].agent_product_id.is_none());
    insert(
        &c,
        "a",
        "codex_core::tools::parallel",
        RUN,
        &tool_body("a", "turn-a", "call-one", true),
    );
    rows[0].agent_product_id = Some("codex-desktop".into());
    rows[0].agent_attribution = Some("native_record".into());
    attribute_with(&home, &mut rows, 501, tui_processes);
    assert!(rows[0].agent_product_id.is_none());
    assert_eq!(
        rows[0].agent_attribution.as_deref(),
        Some("conflicting_evidence")
    );
    let mut changed = vec![activity("a", "turn-a", "processing")];
    let mut reads = 0;
    attribute_with(&home, &mut changed, 501, |pid| {
        let mut p = tui_processes(pid)?;
        if pid == 123 {
            reads += 1;
            if reads > 1 {
                p.tty = 77;
            }
        }
        Some(p)
    });
    assert!(changed[0].agent_product_id.is_none());
}

#[test]
fn tui_startup_budget_fails_closed_without_losing_desktop_attribution() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().canonicalize().unwrap();
    let c = database(&home);
    startup(&c, RUN, TUI_START);
    insert(
        &c,
        "a",
        "codex_core::stream_events_utils",
        RUN,
        &body("a", "turn-a"),
    );
    for _ in 0..256 {
        c.execute("INSERT INTO logs(ts,ts_nanos,thread_id,target,process_uuid,feedback_log_body) VALUES(1700000002,0,NULL,'other',?1,'bounded')",[RUN]).unwrap();
    }
    let mut rows = vec![activity("a", "turn-a", "processing")];
    attribute_with(&home, &mut rows, 501, tui_processes);
    assert_eq!(
        rows[0].agent_product_id.as_deref(),
        Some("codex-cli"),
        "later rendering does not revoke a verified mode"
    );
    c.execute("UPDATE logs SET ts=1700000000 WHERE target='other'", [])
        .unwrap();
    attribute_with(&home, &mut rows, 501, tui_processes);
    assert!(rows[0].agent_product_id.is_none());
    // Existing desktop evidence does not depend on the optional TUI startup index.
    c.execute_batch("DROP INDEX idx_logs_process_uuid_threadless_ts")
        .unwrap();
    attribute_with(&home, &mut rows, 501, processes);
    assert_eq!(rows[0].agent_product_id.as_deref(), Some("codex-desktop"));
}

#[test]
fn execution_scope_and_direct_desktop_runtime_move_only_the_current_activity_readonly() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().canonicalize().unwrap();
    let c = database(&home);
    insert(
        &c,
        "a",
        "codex_core::stream_events_utils",
        RUN,
        &body("a", "turn-a"),
    );
    drop(c);
    let db = home.join(".codex/logs_2.sqlite");
    let original = fs::read(&db).unwrap();
    let mut rows = vec![
        activity("a", "turn-a", "processing"),
        activity("a", "turn-b", "processing"),
        activity("b", "turn-a", "processing"),
    ];
    attribute_with(&home, &mut rows, 501, processes);
    assert_eq!(rows[0].agent_product_id.as_deref(), Some("codex-desktop"));
    assert_eq!(
        rows[0].agent_attribution.as_deref(),
        Some("desktop_runtime_execution")
    );
    assert!(rows[1..].iter().all(|a| a.agent_product_id.is_none()));
    assert_eq!(fs::read(db).unwrap(), original);
    let json = serde_json::to_value(&rows[0]).unwrap();
    assert!(json.get("turnId").is_none() && json.get("callId").is_none());
    attribute_with(&home, &mut rows, 501, |_| None);
    assert!(
        rows[0].agent_product_id.is_none(),
        "exited runtime proof must be withdrawn"
    );
}

#[test]
fn tool_evidence_requires_the_exact_execution_not_a_receipt_or_neighbor_call() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().canonicalize().unwrap();
    let c = database(&home);
    insert(
        &c,
        "a",
        "codex_core::tools::parallel",
        RUN,
        &tool_body("a", "turn-a", "call-one", false),
    );
    insert(
        &c,
        "a",
        "codex_core::stream_events_utils",
        RUN,
        &body("a", "turn-a"),
    );
    let mut rows = vec![activity("a", "turn-a", "tool_requested")];
    attribute_with(&home, &mut rows, 501, processes);
    assert!(rows[0].agent_product_id.is_none());
    insert(
        &c,
        "a",
        "codex_core::tools::parallel",
        RUN,
        &tool_body("a", "turn-a", "other", true),
    );
    attribute_with(&home, &mut rows, 501, processes);
    assert!(rows[0].agent_product_id.is_none());
    insert(
        &c,
        "a",
        "codex_core::tools::parallel",
        RUN,
        &tool_body("a", "turn-a", "call-one", true),
    );
    attribute_with(&home, &mut rows, 501, processes);
    assert_eq!(rows[0].agent_product_id.as_deref(), Some("codex-desktop"));
    assert!(!executed_tool(
        &(tool_body("a", "turn-a", "call-one", true) + " arguments=untrusted"),
        "a",
        "turn-a",
        "call-one"
    ));
}

#[test]
fn confirmed_desktop_turn_stays_visible_across_later_steps_without_tool_evidence() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().canonicalize().unwrap();
    let c = database(&home);
    insert(
        &c,
        "a",
        "codex_core::tools::parallel",
        RUN,
        &tool_body("a", "turn-a", "call-one", true),
    );
    let mut cache = RuntimeEvidence::default();
    let mut confirmed = activity("a", "turn-a", "tool_returned");
    attribute_cached_with(
        &home,
        std::slice::from_mut(&mut confirmed),
        501,
        &mut cache,
        processes,
    );
    assert_eq!(confirmed.agent_product_id.as_deref(), Some("codex-desktop"));
    c.execute("DELETE FROM logs", []).unwrap();

    for phase in ["thinking", "tool_requested", "streaming", "waiting_input"] {
        let mut step = activity("a", "turn-a", phase);
        step.call_id = (phase == "tool_requested").then(|| "call-two".into());
        step.stale = phase == "thinking";
        let mut rows = [
            step,
            activity("a", "turn-b", phase),
            activity("b", "turn-a", phase),
        ];
        attribute_cached_with(&home, &mut rows, 501, &mut cache, processes);
        assert_eq!(
            rows[0].agent_product_id.as_deref(),
            Some("codex-desktop"),
            "{phase}"
        );
        assert_eq!(rows[0].phase, phase);
        assert_eq!(rows[0].stale, phase == "thinking");
        assert!(rows[1..].iter().all(|row| row.agent_product_id.is_none()));
    }
}

#[test]
fn desktop_turn_proof_is_discarded_on_runtime_database_or_attribution_conflict() {
    for case in [
        "process-reused",
        "database-replaced",
        "competing-runtime",
        "explicit-conflict",
    ] {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().canonicalize().unwrap();
        let c = database(&home);
        insert(
            &c,
            "a",
            "codex_core::stream_events_utils",
            RUN,
            &body("a", "turn-a"),
        );
        let mut cache = RuntimeEvidence::default();
        let mut rows = [activity("a", "turn-a", "processing")];
        attribute_cached_with(&home, &mut rows, 501, &mut cache, processes);
        assert_eq!(rows[0].agent_product_id.as_deref(), Some("codex-desktop"));
        rows[0] = activity("a", "turn-a", "thinking");
        match case {
            "database-replaced" => {
                drop(c);
                fs::rename(
                    home.join(".codex/logs_2.sqlite"),
                    home.join(".codex/previous.sqlite"),
                )
                .unwrap();
                database(&home);
            }
            "competing-runtime" => {
                rows[0].phase = "tool_returned".into();
                for run in [RUN, "pid:789:00000000-0000-4000-8000-000000000002"] {
                    insert(
                        &c,
                        "a",
                        "codex_core::tools::parallel",
                        run,
                        &tool_body("a", "turn-a", "call-one", true),
                    );
                }
            }
            "explicit-conflict" => rows[0].agent_attribution = Some("conflicting_evidence".into()),
            _ => {}
        }
        attribute_cached_with(&home, &mut rows, 501, &mut cache, |pid| {
            let mut process = processes(pid)?;
            if case == "process-reused" && pid == 123 {
                process.started_micros += 1;
            }
            Some(process)
        });
        assert!(rows[0].agent_product_id.is_none(), "{case}");
        assert!(cache.desktop_turns.is_empty(), "{case}");
    }
}

#[test]
fn runtime_must_be_current_owned_direct_and_stable() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().canonicalize().unwrap();
    let c = database(&home);
    insert(
        &c,
        "a",
        "codex_core::stream_events_utils",
        RUN,
        &body("a", "turn-a"),
    );
    for case in ["reused", "uid", "parent", "path", "exited", "changed"] {
        let mut reads = 0;
        let mut rows = vec![activity("a", "turn-a", "processing")];
        attribute_with(&home, &mut rows, 501, |pid| {
            reads += 1;
            let mut p = processes(pid)?;
            match case {
                "reused" if pid == 123 => p.started = Utc::now().timestamp() + 1,
                "uid" => p.uid = 502,
                "parent" if pid == 123 => p.parent = 999,
                "path" if pid == 123 => p.path = "/tmp/codex".into(),
                "exited" => return None,
                "changed" if reads > 2 => p.started_micros = 1,
                _ => {}
            }
            Some(p)
        });
        assert!(rows[0].agent_product_id.is_none(), "{case}");
    }
}

#[test]
fn competing_runtimes_and_explicit_conflicts_stay_unknown() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().canonicalize().unwrap();
    let c = database(&home);
    insert(
        &c,
        "a",
        "codex_core::stream_events_utils",
        RUN,
        &body("a", "turn-a"),
    );
    let mut rows = vec![activity("a", "turn-a", "processing")];
    rows[0].agent_product_id = Some("codex-cli".into());
    rows[0].agent_attribution = Some("native_record".into());
    attribute_with(&home, &mut rows, 501, processes);
    assert!(rows[0].agent_product_id.is_none());
    assert_eq!(
        rows[0].agent_attribution.as_deref(),
        Some("conflicting_evidence")
    );
    let mut unrecognized = vec![activity("a", "turn-a", "processing")];
    unrecognized[0].agent_attribution = Some("unrecognized_client".into());
    attribute_with(&home, &mut unrecognized, 501, processes);
    assert!(unrecognized[0].agent_product_id.is_none());
    insert(
        &c,
        "a",
        "codex_core::stream_events_utils",
        "pid:789:00000000-0000-4000-8000-000000000002",
        &body("a", "turn-a"),
    );
    let mut rows = vec![activity("a", "turn-a", "processing")];
    attribute_with(&home, &mut rows, 501, processes);
    assert!(rows[0].agent_product_id.is_none());
}

#[test]
fn absent_unsafe_schema_and_inactive_rows_never_gain_identity() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().canonicalize().unwrap();
    let c = database(&home);
    insert(
        &c,
        "a",
        "codex_core::stream_events_utils",
        RUN,
        &body("a", "turn-a"),
    );
    let mut stale = activity("a", "turn-a", "processing");
    stale.event_at = (Utc::now() - chrono::Duration::seconds(31)).to_rfc3339();
    let mut rows = vec![stale, activity("a", "turn-a", "waiting_input")];
    attribute_with(&home, &mut rows, 501, |_| {
        panic!("inactive source must not inspect processes")
    });
    assert!(rows.iter().all(|a| a.agent_product_id.is_none()));
    c.execute("DROP INDEX idx_logs_thread_id_ts", []).unwrap();
    let mut rows = vec![activity("a", "turn-a", "processing")];
    attribute_with(&home, &mut rows, 501, processes);
    assert!(rows[0].agent_product_id.is_none());
    drop(c);
    #[cfg(unix)]
    {
        let db = home.join(".codex/logs_2.sqlite");
        fs::rename(&db, home.join(".codex/original.sqlite")).unwrap();
        std::os::unix::fs::symlink(home.join(".codex/original.sqlite"), db).unwrap();
        attribute_with(&home, &mut rows, 501, processes);
        assert!(rows[0].agent_product_id.is_none());
    }
    assert!(scope(&format!("untrusted message {}", body("a", "turn-a"))).is_none());
    assert!(process_id("pid:123:not-a-run").is_none());
}

#[test]
fn row_budget_and_reused_pid_within_the_same_second_fail_closed() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().canonicalize().unwrap();
    let c = database(&home);
    insert(
        &c,
        "a",
        "codex_core::stream_events_utils",
        RUN,
        &body("a", "turn-a"),
    );
    let ts: i64 = c
        .query_row("SELECT ts FROM logs LIMIT 1", [], |r| r.get(0))
        .unwrap();
    let mut rows = vec![activity("a", "turn-a", "processing")];
    attribute_with(&home, &mut rows, 501, |pid| {
        let mut p = processes(pid)?;
        if pid == 123 {
            p.started = ts;
            p.started_micros = 1;
        }
        Some(p)
    });
    assert!(rows[0].agent_product_id.is_none());
    for _ in 0..256 {
        insert(
            &c,
            "a",
            "codex_core::stream_events_utils",
            RUN,
            &body("a", "turn-a"),
        );
    }
    attribute_with(&home, &mut rows, 501, |_| {
        panic!("truncated evidence must stay unknown")
    });
    assert!(rows[0].agent_product_id.is_none());
}

#[test]
fn code_mode_host_completion_is_execution_evidence_for_its_exact_call() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().canonicalize().unwrap();
    let c = database(&home);
    let log = format!(
        "{} code-mode host operation completed event.name=\"codex.code_mode.host_timing\" conversation_id=a turn_id=turn-a call_id=call-one cell_id=abc tool_name=exec code_mode_host_duration_ns=120000",
        body("a", "turn-a")
            .strip_suffix("received stream event")
            .unwrap()
    );
    insert(&c, "a", "codex_code_mode::timing", RUN, &log);
    let mut rows = vec![activity("a", "turn-a", "tool_returned")];
    attribute_with(&home, &mut rows, 501, processes);
    assert_eq!(rows[0].agent_product_id.as_deref(), Some("codex-desktop"));
    assert!(!executed_tool(&log, "a", "turn-a", "other"));
    assert!(!executed_tool(
        &log.replace(
            "code-mode host operation completed",
            "received notification"
        ),
        "a",
        "turn-a",
        "call-one"
    ));
}
