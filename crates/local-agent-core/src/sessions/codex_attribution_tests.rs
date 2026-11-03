use super::*;
use serde_json::json;
use std::fs;

fn log_line(event: &str, call: &str, tool: &str, remote: bool) -> String {
    format!(
        "{} info [electron-message-handler] {event} callId={call} isRemoteHost={remote} tool={tool} turnId=turn-one{}\n",
        now(),
        if event == "Sending server response" {
            " success=true"
        } else {
            ""
        }
    )
}
#[test]
fn desktop_execution_is_call_scoped_revocable_and_consistent_in_every_native_view() {
    let home = tempfile::tempdir().unwrap();
    let source = home.path().join(".codex/sessions/source.jsonl");
    fs::create_dir_all(source.parent().unwrap()).unwrap();
    let ts = now();
    let rows = [
        json!({"type":"session_meta","timestamp":ts,"payload":{"id":"thread-one","cwd":home.path().join("project"),"originator":"codex_work_desktop"}}),
        json!({"type":"turn_context","timestamp":ts,"payload":{"turn_id":"turn-one","model":"fixture"}}),
        json!({"type":"response_item","timestamp":ts,"payload":{"type":"function_call","call_id":"call-one","name":"mcp__fixture__read","arguments":"{}"}}),
        json!({"type":"event_msg","timestamp":ts,"payload":{"type":"item_completed","turn_id":"turn-one","item":{"type":"McpToolCall","id":"call-one","server":"fixture","tool":"read","status":"completed","result":{"ok":true},"duration":{"secs":1,"nanos":0}}}}),
        json!({"type":"token_usage_record","timestamp":ts,"payload":{"turn_id":"turn-one","response_id":"response-one","usage":{"input_tokens":10,"output_tokens":2}}}),
    ];
    fs::write(
        &source,
        rows.iter().map(|r| format!("{r}\n")).collect::<String>(),
    )
    .unwrap();
    let logs = home.path().join("Library/Logs/com.openai.codex/2026/10/19");
    fs::create_dir_all(&logs).unwrap();
    let log =
        logs.join("codex-desktop-00000000-0000-4000-8000-000000000001-123-t0-i1-000000-0.log");
    fs::write(
        &log,
        log_line(
            "dynamic_app_tool_renderer_execution_started",
            "call-one",
            "read",
            false,
        ) + &log_line("Sending server response", "call-one", "read", false),
    )
    .unwrap();
    let sessions = NativeSessions::new(home.path().into(), &["codex".into()]);
    sessions.refresh().unwrap();
    let call_query = |agent: &str| CallQuery {
        agent: Some(agent.into()),
        ..Default::default()
    };
    let desktop = sessions.calls(call_query("codex-desktop")).unwrap();
    assert_eq!(desktop.total, 1);
    assert_eq!(
        desktop.items[0].agent_attribution,
        Some("desktop_tool_execution")
    );
    assert_eq!(sessions.calls(call_query("codex-cli")).unwrap().total, 0);
    let unknown = sessions.calls(call_query("unknown")).unwrap();
    assert_eq!(unknown.total, 1);
    assert_eq!(unknown.items[0].kind, "model");
    let native_id = &desktop.items[0].session_id;
    let conversation = sessions
        .conversation(native_id, None, Some(&desktop.items[0].id))
        .unwrap();
    assert_eq!(
        conversation
            .items
            .iter()
            .find(|e| e.id == desktop.items[0].id)
            .unwrap()
            .agent_product_id
            .as_deref(),
        Some("codex-desktop")
    );
    let mcp = sessions
        .mcp(McpQuery {
            agent: Some("codex-desktop".into()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(mcp.total_calls, 1);
    assert_eq!(mcp.groups[0].p95_ms, Some(1000));
    assert_eq!(mcp.usage_summary.related_tokens, None);
    let catalog = sessions.project_catalog().unwrap();
    let project = crate::projects::query(
        &catalog,
        crate::projects::ProjectQuery {
            agent: Some("codex-desktop".into()),
            ..Default::default()
        },
        &Default::default(),
    )
    .unwrap();
    assert_eq!(project.summary.model_calls, 0);
    assert_eq!(project.summary.usage.total_tokens, Some(0));
    let unknown_project = crate::projects::query(
        &catalog,
        crate::projects::ProjectQuery {
            agent: Some("codex-unknown".into()),
            ..Default::default()
        },
        &Default::default(),
    )
    .unwrap();
    assert_eq!(unknown_project.summary.usage.total_tokens, Some(12));
    let analysis = sessions.analysis_rows().unwrap();
    assert_eq!(analysis[0].agent, "codex-unknown");
    let mut watch = live::NativeWatch::default();
    assert_eq!(
        watch.poll(&sessions)[0].agent_product_id.as_deref(),
        Some("codex-desktop")
    );
    assert_eq!(
        sessions
            .sources()
            .unwrap()
            .iter()
            .find(|s| s.agent_id == "codex")
            .unwrap()
            .client_attribution
            .as_ref()
            .unwrap()
            .matched_calls,
        1
    );
    // Contradicting per-record native identity is unknown, never overwritten as desktop.
    let mut explicit = rows.clone();
    for row in &mut explicit {
        if row["type"] == "response_item" || row["payload"]["type"] == "item_completed" {
            row["originator"] = json!("codex_cli_rs");
        }
    }
    fs::write(
        &source,
        explicit
            .iter()
            .map(|r| format!("{r}\n"))
            .collect::<String>(),
    )
    .unwrap();
    sessions.refresh().unwrap();
    assert_eq!(
        sessions.calls(call_query("codex-desktop")).unwrap().total,
        0
    );
    assert_eq!(sessions.calls(call_query("codex-cli")).unwrap().total, 0);
    assert!(
        sessions
            .calls(call_query("unknown"))
            .unwrap()
            .items
            .iter()
            .any(|c| c.agent_attribution == Some("conflicting_evidence"))
    );
    assert!(watch.poll(&sessions)[0].agent_product_id.is_none());
    fs::write(
        &source,
        rows.iter().map(|r| format!("{r}\n")).collect::<String>(),
    )
    .unwrap();
    sessions.refresh().unwrap();
    // Duplicated physical transcript is still one execution; another thread is ambiguous.
    let copy = home.path().join(".codex/sessions/copy.jsonl");
    fs::copy(&source, &copy).unwrap();
    sessions.refresh().unwrap();
    assert_eq!(
        sessions.calls(call_query("codex-desktop")).unwrap().total,
        1
    );
    // An older paginated copy may contain extra IDs; source counters use visible versions.
    let original_log = fs::read_to_string(&log).unwrap();
    fs::write(
        &log,
        original_log.clone()
            + &log_line(
                "dynamic_app_tool_renderer_execution_started",
                "hidden-call",
                "read",
                false,
            )
            + &log_line("Sending server response", "hidden-call", "read", false),
    )
    .unwrap();
    {
        let mut state = sessions.lock().unwrap();
        let file = state.files.get_mut(&source).unwrap();
        let mut hidden = file.calls.iter().find(|c| c.kind == "mcp").unwrap().clone();
        hidden.id = hidden.id.replace("call-one", "hidden-call");
        std::sync::Arc::make_mut(&mut file.calls).push(hidden);
        file.stamp.1 = 0;
        let evidence = codex_attribution::EvidenceCache::default().snapshot(home.path(), true);
        codex_attribution::apply(&mut state, evidence);
        assert_eq!(
            state
                .sources
                .iter()
                .find(|s| s.agent_id == "codex")
                .unwrap()
                .client_attribution
                .as_ref()
                .unwrap()
                .matched_calls,
            1
        );
    }
    fs::write(&log, original_log).unwrap();
    sessions.refresh().unwrap();
    // Copies with contradictory per-record identity cannot win by dedup order.
    fs::write(
        &copy,
        explicit
            .iter()
            .map(|r| format!("{r}\n"))
            .collect::<String>(),
    )
    .unwrap();
    sessions.refresh().unwrap();
    assert_eq!(
        sessions.calls(call_query("codex-desktop")).unwrap().total,
        0
    );
    assert!(
        sessions
            .calls(call_query("unknown"))
            .unwrap()
            .items
            .iter()
            .any(|c| c.agent_attribution == Some("conflicting_evidence"))
    );
    fs::copy(&source, &copy).unwrap();
    sessions.refresh().unwrap();
    let fork = home.path().join(".codex/sessions/fork.jsonl");
    fs::write(
        &fork,
        fs::read_to_string(&source)
            .unwrap()
            .replace("thread-one", "thread-two"),
    )
    .unwrap();
    sessions.refresh().unwrap();
    assert_eq!(
        sessions.calls(call_query("codex-desktop")).unwrap().total,
        0
    );
    assert!(
        sessions
            .calls(call_query("unknown"))
            .unwrap()
            .items
            .iter()
            .any(|c| c.agent_attribution == Some("ambiguous_evidence"))
    );
    fs::remove_file(fork).unwrap();
    sessions.refresh().unwrap();
    assert_eq!(
        sessions.calls(call_query("codex-desktop")).unwrap().total,
        1
    );
    fs::remove_file(&log).unwrap();
    sessions.refresh().unwrap();
    assert_eq!(
        sessions.calls(call_query("codex-desktop")).unwrap().total,
        0
    );
    assert!(watch.poll(&sessions)[0].agent_product_id.is_none());
    sessions.set_enabled("codex", false).unwrap();
    sessions.refresh().unwrap();
    assert_eq!(sessions.calls(Default::default()).unwrap().total, 0);
    assert!(
        sessions
            .sources()
            .unwrap()
            .iter()
            .find(|s| s.agent_id == "codex")
            .unwrap()
            .client_attribution
            .is_none()
    );
}

#[test]
fn remote_incomplete_receipt_and_mismatched_tool_logs_do_not_prove_local_execution() {
    let home = tempfile::tempdir().unwrap();
    let dir = home.path().join("Library/Logs/com.openai.codex/2026/10/19");
    fs::create_dir_all(&dir).unwrap();
    let path =
        dir.join("codex-desktop-00000000-0000-4000-8000-000000000001-123-t0-i1-000000-0.log");
    let mut text = log_line(
        "dynamic_app_tool_renderer_execution_started",
        "remote",
        "read",
        true,
    ) + &log_line("Sending server response", "remote", "read", true);
    text += &log_line(
        "dynamic_app_tool_renderer_execution_started",
        "unfinished",
        "read",
        false,
    );
    text += &log_line("Sending server response", "unfinished", "write", false);
    text += &log_line("Reasoning summary item completed", "receipt", "read", false);
    text += &log_line("Sending server response", "receipt", "read", false);
    fs::write(&path, &text).unwrap();
    let mut cache = codex_attribution::EvidenceCache::default();
    assert_eq!(
        cache
            .snapshot(home.path(), true)
            .status
            .confirmed_tool_executions,
        0
    );
    text += &log_line("Sending server response", "unfinished", "read", false);
    fs::write(&path, text).unwrap();
    assert_eq!(
        cache
            .snapshot(home.path(), true)
            .status
            .confirmed_tool_executions,
        1
    );
    fs::remove_file(&path).unwrap();
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(home.path().join("elsewhere"), &path).unwrap();
        assert_eq!(
            cache
                .snapshot(home.path(), true)
                .status
                .confirmed_tool_executions,
            0
        );
    }
}
