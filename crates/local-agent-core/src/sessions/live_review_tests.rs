use super::*;
use serde_json::{Value, json};
use std::io::Write;

fn append(path: &Path, value: Value) {
    writeln!(
        fs::OpenOptions::new().append(true).open(path).unwrap(),
        "{value}"
    )
    .unwrap();
}

#[test]
fn native_messages_keep_their_own_rows_and_revocation_clears_all_previews() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().canonicalize().unwrap();
    let root = home.join(".claude/projects/message-rows");
    fs::create_dir_all(&root).unwrap();
    let path = root.join("session.jsonl");
    fs::write(&path, "").unwrap();
    let base = chrono::Utc::now() - chrono::Duration::seconds(20);
    let at = |i| (base + chrono::Duration::seconds(i)).to_rfc3339();
    append(
        &path,
        json!({"type":"user","uuid":"user-1","sessionId":"s","timestamp":at(0),"message":{"content":"repeat"}}),
    );
    let sessions = NativeSessions::new(home, &["claude".into()]);
    let mut watch = live::NativeWatch::default();
    let first = watch.poll(&sessions).remove(0);
    assert_eq!(first.turn_id.as_deref(), Some("user-1"));
    append(
        &path,
        json!({"type":"assistant","sessionId":"s","timestamp":at(1),"message":{"content":[{"type":"tool_use","id":"call","name":"Read"}]}}),
    );
    let current = watch.poll(&sessions);
    assert_eq!(current.len(), 1);
    assert_eq!(current[0].id, first.id);
    append(
        &path,
        json!({"type":"user","uuid":"user-2","sessionId":"s","timestamp":at(2),"message":{"content":"repeat"}}),
    );
    let current = watch.poll(&sessions);
    assert_eq!(current.len(), 2);
    assert_ne!(current[0].id, current[1].id);
    assert!(
        current
            .iter()
            .find(|a| a.turn_id.as_deref() == Some("user-1"))
            .unwrap()
            .stale
    );
    let serialized = serde_json::to_value(&current).unwrap();
    assert_eq!(serialized[0]["messageId"], "user-2");
    let cold = live::NativeWatch::default().poll(&sessions);
    assert_eq!(cold.len(), 2);
    assert!(cold.iter().any(|a| a.turn_id.as_deref() == Some("user-1")));
    sessions.set_enabled("claude", false).unwrap();
    assert!(watch.poll(&sessions).is_empty());
}

#[test]
fn message_start_stays_on_the_user_turn_across_steps_completion_and_new_turns() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().canonicalize().unwrap();
    let root = home.join(".claude/projects/message-order");
    fs::create_dir_all(&root).unwrap();
    let path = root.join("session.jsonl");
    fs::write(&path, "").unwrap();
    let base = chrono::Utc::now() - chrono::Duration::seconds(20);
    let at = |seconds| (base + chrono::Duration::seconds(seconds)).to_rfc3339();
    append(
        &path,
        json!({"type":"user","sessionId":"order","timestamp":at(0),"message":{"content":"first message"}}),
    );
    let sessions = NativeSessions::new(home, &["claude".into()]);
    let mut watch = live::NativeWatch::default();
    for (seconds, event) in [
        (
            1,
            json!({"type":"assistant","message":{"content":[{"type":"thinking","thinking":"private"}]}}),
        ),
        (
            2,
            json!({"type":"assistant","message":{"content":[{"type":"tool_use","id":"read","name":"Read"}]}}),
        ),
        (
            3,
            json!({"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"read","content":"result"}]}}),
        ),
        (4, json!({"type":"system","subtype":"turn_duration"})),
    ] {
        let mut event = event;
        event["timestamp"] = json!(at(seconds));
        event["sessionId"] = json!("order");
        append(&path, event);
        let activity = watch.poll(&sessions).remove(0);
        assert_eq!(activity.message_started_at, Some(at(0)));
        assert_eq!(activity.event_at, at(seconds));
    }
    append(
        &path,
        json!({"type":"user","sessionId":"order","timestamp":at(5),"message":{"content":"next message"}}),
    );
    assert_eq!(watch.poll(&sessions)[0].message_started_at, Some(at(5)));
}

#[test]
fn codex_turn_start_is_available_without_input_and_never_uses_a_late_step_as_its_start() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().canonicalize().unwrap();
    let base = chrono::Utc::now() - chrono::Duration::seconds(20);
    let at = |seconds| (base + chrono::Duration::seconds(seconds)).to_rfc3339();
    let path = file(
        &home,
        &[
            json!({"type":"session_meta","timestamp":at(0),"payload":{"id":"order"}}),
            json!({"type":"event_msg","timestamp":at(1),"payload":{"type":"task_started","turn_id":"first"}}),
        ],
    );
    let sessions = NativeSessions::new(home, &["codex".into()]);
    let mut watch = live::NativeWatch::default();
    for (seconds, kind) in [
        (2, "reasoning"),
        (3, "function_call"),
        (4, "function_call_output"),
    ] {
        append(
            &path,
            json!({"type":"response_item","timestamp":at(seconds),"payload":{"type":kind,"name":"Read","call_id":"call"}}),
        );
        assert_eq!(watch.poll(&sessions)[0].message_started_at, Some(at(1)));
    }
    append(
        &path,
        json!({"type":"event_msg","timestamp":at(5),"payload":{"type":"task_complete","turn_id":"first"}}),
    );
    assert_eq!(watch.poll(&sessions)[0].message_started_at, Some(at(1)));
    append(
        &path,
        json!({"type":"event_msg","timestamp":at(6),"payload":{"type":"task_started","turn_id":"second"}}),
    );
    assert_eq!(watch.poll(&sessions)[0].message_started_at, Some(at(6)));
    append(
        &path,
        json!({"type":"event_msg","timestamp":at(7),"payload":{"type":"task_started","turn_id":"second"}}),
    );
    assert_eq!(watch.poll(&sessions)[0].message_started_at, Some(at(6)));
    append(
        &path,
        json!({"type":"event_msg","timestamp":at(8),"payload":{"type":"item_completed","turn_id":"unknown-start","item":{"type":"Reasoning","id":"reasoning"}}}),
    );
    assert!(watch.poll(&sessions)[0].message_started_at.is_none());
}

#[test]
fn activity_input_and_tool_names_follow_the_turn_without_exposing_thinking_or_tool_bodies() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().canonicalize().unwrap();
    let root = home.join(".claude/projects/input-preview");
    fs::create_dir_all(&root).unwrap();
    let path = root.join("session.jsonl");
    fs::write(&path, "").unwrap();
    append(
        &path,
        json!({"type":"user","sessionId":"input-preview","timestamp":now(),"message":{"content":"检查登录 Bearer abcdefghijklmnop"}}),
    );
    append(
        &path,
        json!({"type":"assistant","sessionId":"input-preview","timestamp":now(),"message":{"model":"actual-model","content":[{"type":"thinking","thinking":"private-thinking-sentinel"}]}}),
    );
    let sessions = NativeSessions::new(home, &["claude".into()]);
    let mut watch = live::NativeWatch::default();
    let a = watch.poll(&sessions).remove(0);
    assert_eq!(a.phase, "thinking");
    assert_eq!(a.model.as_deref(), Some("actual-model"));
    assert_eq!(a.user_input.as_ref().unwrap().text, "检查登录 [已隐藏凭据]");
    assert!(
        !serde_json::to_string(&a)
            .unwrap()
            .contains("private-thinking")
    );
    append(
        &path,
        json!({"type":"assistant","sessionId":"input-preview","timestamp":now(),"message":{"model":"actual-model","content":[{"type":"tool_use","id":"read-a","name":"Read","input":{"secret":"private-arguments-sentinel"}},{"type":"tool_use","id":"grep-b","name":"Grep","input":{}}]}}),
    );
    let a = watch.poll(&sessions).remove(0);
    assert_eq!(a.phase, "tool_requested");
    assert_eq!(a.tool_names, vec!["Read", "Grep"]);
    append(
        &path,
        json!({"type":"user","sessionId":"input-preview","timestamp":now(),"message":{"content":[{"type":"tool_result","tool_use_id":"read-a","content":"private-results-sentinel"}]}}),
    );
    let a = watch.poll(&sessions).remove(0);
    assert_eq!(a.phase, "tool_returned");
    assert_eq!(a.tool_names, vec!["Read"]);
    assert!(a.step.contains("Grep"));
    assert!(a.step.contains("仍待返回"));
    assert_eq!(a.user_input.as_ref().unwrap().text, "检查登录 [已隐藏凭据]");
    let output = serde_json::to_string(&a).unwrap();
    assert!(!output.contains("private-") && !output.contains("abcdefghijklmnop"));
    append(
        &path,
        json!({"type":"user","sessionId":"input-preview","timestamp":now(),"message":{"content":"改为检查退出按钮"}}),
    );
    let a = watch.poll(&sessions).remove(0);
    assert_eq!(a.phase, "processing");
    assert_eq!(a.user_input.as_ref().unwrap().text, "改为检查退出按钮");
    assert!(a.tool_names.is_empty());
    sessions.set_enabled("claude", false).unwrap();
    assert!(watch.poll(&sessions).is_empty());
}

#[test]
fn codex_input_preview_ignores_context_and_resets_with_a_replaced_tail() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().canonicalize().unwrap();
    let path = file(
        &home,
        &[
            json!({"type":"session_meta","timestamp":now(),"payload":{"id":"preview"}}),
            json!({"type":"response_item","timestamp":now(),"payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"# AGENTS.md instructions context-only-sentinel"}]}}),
            json!({"type":"response_item","timestamp":now(),"payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"最近的真实输入"}]}}),
            json!({"type":"response_item","timestamp":now(),"payload":{"type":"reasoning","text":"private-thinking-sentinel"}}),
        ],
    );
    let sessions = NativeSessions::new(home, &["codex".into()]);
    let mut watch = live::NativeWatch::default();
    let a = watch.poll(&sessions).remove(0);
    assert_eq!(a.phase, "thinking");
    assert_eq!(a.user_input.unwrap().text, "最近的真实输入");
    fs::write(
        path,
        format!(
            "{}\n{}\n",
            json!({"type":"session_meta","timestamp":now(),"payload":{"id":"preview"}}),
            json!({"type":"response_item","timestamp":now(),"payload":{"type":"reasoning"}})
        ),
    )
    .unwrap();
    let a = watch.poll(&sessions).remove(0);
    assert!(a.user_input.is_none());
    assert!(a.tool_names.is_empty());
}

#[test]
fn native_task_input_updates_live_without_indexing_and_never_carries_into_an_unobserved_turn() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().canonicalize().unwrap();
    let a = "11111111-1111-4111-8111-111111111111";
    let b = "22222222-2222-4222-8222-222222222222";
    let base = chrono::Utc::now() - chrono::Duration::seconds(15);
    let at = |n| (base + chrono::Duration::seconds(n)).to_rfc3339();
    let path = file(
        &home,
        &[
            json!({"type":"session_meta","timestamp":at(0),"payload":{"id":"live-tasks"}}),
            json!({"type":"event_msg","timestamp":at(1),"payload":{"type":"task_started","turn_id":a}}),
            json!({"type":"response_item","timestamp":at(2),"payload":{"type":"function_call_output","id":format!("fco_{a}"),"namespace":"codex_app","name":"send_message_to_thread","output":"<codex_delegation><source_thread_id>44444444-4444-4444-8444-444444444444</source_thread_id><input>检查第一条消息</input></codex_delegation>","internal_chat_message_metadata_passthrough":{"turn_id":a}}}),
        ],
    );
    let sessions = NativeSessions::new(home, &["codex".into()]);
    let mut watch = live::NativeWatch::default();
    let rows = watch.poll(&sessions);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].user_input.as_ref().unwrap().text, "检查第一条消息");
    assert_eq!(rows[0].phase, "processing");
    append(
        &path,
        json!({"type":"response_item","timestamp":at(3),"payload":{"type":"function_call_output","call_id":"ordinary-tool","output":"not a message","internal_chat_message_metadata_passthrough":{"turn_id":b}}}),
    );
    let rows = watch.poll(&sessions);
    assert!(
        rows.iter()
            .find(|r| r.message_id() == Some(b))
            .unwrap()
            .user_input
            .is_none()
    );
    assert_eq!(
        rows.iter()
            .find(|r| r.message_id() == Some(a))
            .unwrap()
            .user_input
            .as_ref()
            .unwrap()
            .text,
        "检查第一条消息"
    );
    append(
        &path,
        json!({"type":"response_item","timestamp":at(4),"payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"第二条真实用户输入"}],"internal_chat_message_metadata_passthrough":{"turn_id":b}}}),
    );
    let rows = watch.poll(&sessions);
    let row = rows.iter().find(|r| r.message_id() == Some(b)).unwrap();
    assert_eq!(row.user_input.as_ref().unwrap().text, "第二条真实用户输入");
    assert_eq!(row.message_started_at.as_deref(), Some(at(4).as_str()));
}
fn file(home: &Path, rows: &[Value]) -> PathBuf {
    let root = home.join(".codex/sessions/2026/10/19");
    fs::create_dir_all(&root).unwrap();
    let path = root.join("live.jsonl");
    fs::write(
        &path,
        rows.iter().map(|v| format!("{v}\n")).collect::<String>(),
    )
    .unwrap();
    path
}

#[test]
fn claude_activity_uses_current_response_model_before_index_and_resets_on_new_turn() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().canonicalize().unwrap();
    let root = home.join(".claude/projects/live-model");
    fs::create_dir_all(&root).unwrap();
    let path = root.join("session.jsonl");
    fs::write(&path, "").unwrap();
    append(
        &path,
        json!({"type":"user","entrypoint":"cli","sessionId":"live-model","timestamp":now(),"message":{"role":"user","content":"Indexed title"}}),
    );
    append(
        &path,
        json!({"type":"assistant","entrypoint":"cli","sessionId":"live-model","timestamp":now(),"message":{"id":"a","role":"assistant","model":"old-model","content":[{"type":"text","text":"first"}]}}),
    );
    let sessions = NativeSessions::new(home, &["claude".into()]);
    let mut watch = live::NativeWatch::default();
    // No full index is needed to identify the model of this exact response.
    assert_eq!(watch.poll(&sessions)[0].model.as_deref(), Some("old-model"));
    sessions.refresh().unwrap();
    // No file append is needed for a cold activity's indexed title to appear.
    assert_eq!(watch.poll(&sessions)[0].title, "Indexed title");
    append(
        &path,
        json!({"type":"assistant","entrypoint":"cli","sessionId":"live-model","timestamp":now(),"message":{"id":"b","role":"assistant","model":"new-model","content":[{"type":"tool_use","id":"call-b","name":"Read","input":{}}]}}),
    );
    let activity = watch.poll(&sessions).remove(0);
    assert_eq!(activity.model.as_deref(), Some("new-model"));
    assert_eq!(activity.phase, "tool_requested");
    append(
        &path,
        json!({"type":"user","entrypoint":"cli","sessionId":"live-model","timestamp":now(),"message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"call-b","content":"result"}]}}),
    );
    assert_eq!(watch.poll(&sessions)[0].model.as_deref(), Some("new-model"));
    append(
        &path,
        json!({"type":"user","entrypoint":"cli","sessionId":"live-model","timestamp":now(),"message":{"role":"user","content":"another turn"}}),
    );
    assert!(watch.poll(&sessions)[0].model.is_none());
    append(
        &path,
        json!({"type":"assistant","entrypoint":"cli","isApiErrorMessage":true,"sessionId":"live-model","timestamp":now(),"message":{"id":"error","role":"assistant","model":"<synthetic>","content":[]}}),
    );
    let before_poll = fs::read(&path).unwrap();
    let activity = watch.poll(&sessions).remove(0);
    assert_eq!(activity.phase, "error");
    assert!(activity.model.is_none());
    assert_eq!(fs::read(&path).unwrap(), before_poll);
}

#[test]
fn claude_native_interruption_closes_activity_without_synthetic_context_usage() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().canonicalize().unwrap();
    let root = home.join(".claude/projects/live-cancel");
    fs::create_dir_all(&root).unwrap();
    let path = root.join("session.jsonl");
    fs::write(&path, "").unwrap();
    append(
        &path,
        json!({"type":"assistant","entrypoint":"cli","sessionId":"live-cancel","timestamp":now(),"message":{"id":"real","role":"assistant","model":"real-model","usage":{"input_tokens":600,"output_tokens":20},"content":[]}}),
    );
    let sessions = NativeSessions::new(home, &["claude".into()]);
    let mut watch = live::NativeWatch::default();
    watch.poll(&sessions);
    append(
        &path,
        json!({"type":"assistant","entrypoint":"cli","sessionId":"live-cancel","timestamp":now(),"isApiErrorMessage":false,"message":{"id":"local","role":"assistant","model":"<synthetic>","usage":{"input_tokens":0,"output_tokens":0},"content":[{"type":"thinking","thinking":"local partial response"}]}}),
    );
    let activity = watch.poll(&sessions).remove(0);
    assert_eq!(activity.model.as_deref(), Some("real-model"));
    assert_eq!(activity.phase, "reply_recorded");
    append(
        &path,
        json!({"type":"user","entrypoint":"cli","sessionId":"live-cancel","timestamp":now(),"interruptedMessageId":"real","message":{"role":"user","content":[{"type":"text","text":"[Request interrupted by user]"}]}}),
    );
    let before = fs::read(&path).unwrap();
    let activity = watch.poll(&sessions).remove(0);
    assert_eq!(activity.phase, "cancelled");
    assert!(activity.model.is_none());
    let sample = watch
        .context_samples(&sessions, vec!["claude:live-cancel".into()])
        .unwrap()
        .remove(0)
        .context
        .unwrap();
    assert_eq!(sample.input_tokens, Some(600));
    assert_eq!(sample.model.as_deref(), Some("real-model"));
    sessions.refresh().unwrap();
    let conversation = sessions
        .conversation("claude:live-cancel", None, None)
        .unwrap();
    assert_eq!(conversation.context.input_tokens, Some(600));
    assert_eq!(conversation.context.model.as_deref(), Some("real-model"));
    assert_eq!(fs::read(&path).unwrap(), before);
    // A quoted interruption message without the native marker proves no cancellation.
    append(
        &path,
        json!({"type":"user","entrypoint":"cli","sessionId":"live-cancel","timestamp":now(),"message":{"role":"user","content":"[Request interrupted by user]"}}),
    );
    assert_eq!(watch.poll(&sessions)[0].phase, "processing");
    append(
        &path,
        json!({"type":"assistant","entrypoint":"cli","sessionId":"live-cancel","timestamp":now(),"message":{"id":"zero","role":"assistant","model":"real-model","usage":{"input_tokens":0,"output_tokens":1},"content":[]}}),
    );
    watch.poll(&sessions);
    assert_eq!(
        watch
            .context_samples(&sessions, vec!["claude:live-cancel".into()])
            .unwrap()[0]
            .context
            .as_ref()
            .unwrap()
            .input_tokens,
        Some(0)
    );
}

#[test]
fn long_transcript_has_live_identity_and_context_before_full_index() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().canonicalize().unwrap();
    let ts = now();
    let path = file(
        &home,
        &[
            json!({"type":"session_meta","timestamp":ts,"payload":{"id":"long","originator":"codex_work_desktop"}}),
        ],
    );
    for n in 0..40 {
        append(
            &path,
            json!({"type":"padding","payload":{"n":n,"text":"x".repeat(65536)}}),
        );
    }
    append(
        &path,
        json!({"type":"event_msg","timestamp":ts,"payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":100},"model_context_window":1000}}}),
    );
    append(
        &path,
        json!({"type":"event_msg","timestamp":ts,"payload":{"type":"item_completed","turn_id":"turn-a","item":{"type":"Reasoning","id":"reasoning-a"}}}),
    );
    let original = fs::read(&path).unwrap();
    let sessions = NativeSessions::new(home, &["codex".into()]);
    let mut watch = live::NativeWatch::default();
    let activity = watch.poll(&sessions);
    assert_eq!(activity.len(), 1);
    assert_eq!(activity[0].session_id, "codex:long");
    assert_eq!(activity[0].phase, "thinking");
    assert!(activity[0].agent_product_id.is_none());
    assert!(activity[0].agent_attribution.is_none());
    let context = watch
        .context_samples(&sessions, vec!["codex:long".into()])
        .unwrap()
        .remove(0)
        .context
        .unwrap();
    assert_eq!(context.used_percentage, Some(10.0));
    assert!(context.sample_agent_product_id.is_none());
    assert_eq!(fs::read(path).unwrap(), original);
}

#[test]
fn explicit_progress_is_live_but_closed_turn_does_not_reactivate() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().canonicalize().unwrap();
    let path = file(
        &home,
        &[
            json!({"type":"session_meta","timestamp":now(),"payload":{"id":"s","originator":"codex_work_desktop"}}),
            json!({"type":"event_msg","timestamp":now(),"payload":{"type":"task_started","turn_id":"turn-a"}}),
            json!({"type":"response_item","timestamp":now(),"payload":{"type":"message","role":"assistant","phase":"commentary"}}),
        ],
    );
    let sessions = NativeSessions::new(home, &["codex".into()]);
    let mut watch = live::NativeWatch::default();
    assert_eq!(watch.poll(&sessions)[0].phase, "processing");
    append(
        &path,
        json!({"type":"event_msg","timestamp":now(),"payload":{"type":"task_complete","turn_id":"turn-a"}}),
    );
    assert_eq!(watch.poll(&sessions)[0].phase, "waiting_input");
    append(
        &path,
        json!({"type":"event_msg","timestamp":now(),"payload":{"type":"item_completed","turn_id":"turn-a","item":{"type":"Reasoning","id":"late"}}}),
    );
    assert_eq!(watch.poll(&sessions)[0].phase, "waiting_input");
    append(
        &path,
        json!({"type":"event_msg","timestamp":now(),"payload":{"type":"item_completed","turn_id":"turn-b","item":{"type":"Reasoning","id":"new"}}}),
    );
    let active = watch.poll(&sessions).remove(0);
    assert_eq!(active.phase, "thinking");
    assert!(active.agent_product_id.is_none());
}

#[test]
fn native_proposal_does_not_prove_unidentified_execution_or_later_progress() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().canonicalize().unwrap();
    let path = file(
        &home,
        &[
            json!({"type":"session_meta","timestamp":now(),"payload":{"id":"s","originator":"codex_work_desktop"}}),
            json!({"type":"event_msg","timestamp":now(),"payload":{"type":"task_started","turn_id":"t"}}),
            json!({"type":"response_item","timestamp":now(),"originator":"codex_cli_rs","payload":{"type":"function_call","id":"item-a","call_id":"call-a","name":"read","arguments":"{}"}}),
            json!({"type":"response_item","timestamp":now(),"payload":{"type":"function_call_output","id":"item-b","call_id":"call-a","output":"ok"}}),
        ],
    );
    let sessions = NativeSessions::new(home, &["codex".into()]);
    sessions.refresh().unwrap();
    let mut watch = live::NativeWatch::default();
    let active = watch.poll(&sessions).remove(0);
    assert!(active.agent_product_id.is_none());
    assert!(active.agent_attribution.is_none());
    append(
        &path,
        json!({"type":"response_item","timestamp":now(),"payload":{"type":"reasoning","id":"r"}}),
    );
    let active = watch.poll(&sessions).remove(0);
    assert_eq!(active.phase, "thinking");
    assert!(active.agent_product_id.is_none());
}

#[test]
fn duplicate_live_files_use_one_canonical_source_for_activity_and_context() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().canonicalize().unwrap();
    let rows = |input| {
        vec![
            json!({"type":"session_meta","timestamp":now(),"payload":{"id":"shared"}}),
            json!({"type":"event_msg","timestamp":now(),"payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":input},"model_context_window":1000}}}),
            json!({"type":"event_msg","timestamp":now(),"payload":{"type":"item_completed","turn_id":"t","item":{"type":"Reasoning","id":"r"}}}),
        ]
    };
    let original = file(&home, &rows(100));
    let newer = original.with_file_name("newer.jsonl");
    fs::write(
        &newer,
        rows(200)
            .iter()
            .map(|v| format!("{v}\n"))
            .collect::<String>(),
    )
    .unwrap();
    let sessions = NativeSessions::new(home, &["codex".into()]);
    let mut watch = live::NativeWatch::default();
    assert_eq!(watch.poll(&sessions).len(), 1);
    let contexts = watch
        .context_samples(
            &sessions,
            vec!["codex:shared".into(), "codex:shared".into()],
        )
        .unwrap();
    assert_eq!(contexts.len(), 1);
    assert_eq!(
        contexts[0].context.as_ref().unwrap().input_tokens,
        Some(200)
    );
}

#[test]
fn live_progress_retains_indexed_title_while_catalog_is_refreshing() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().canonicalize().unwrap();
    let path = file(
        &home,
        &[
            json!({"type":"session_meta","timestamp":now(),"payload":{"id":"s","cwd":home.join("workspace")}}),
            json!({"type":"response_item","timestamp":now(),"payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"验证活动标题"}]}}),
        ],
    );
    let sessions = NativeSessions::new(home, &["codex".into()]);
    sessions.refresh().unwrap();
    let mut watch = live::NativeWatch::default();
    append(
        &path,
        json!({"type":"event_msg","timestamp":now(),"payload":{"type":"task_started","turn_id":"t"}}),
    );
    let indexed = watch.poll(&sessions).remove(0);
    assert_eq!(indexed.title, "验证活动标题");
    assert_eq!(indexed.project.as_deref(), Some("workspace"));
    sessions.lock().unwrap().files.clear();
    append(
        &path,
        json!({"type":"response_item","timestamp":now(),"payload":{"type":"reasoning","id":"r"}}),
    );
    let progress = watch.poll(&sessions).remove(0);
    assert_eq!(progress.title, indexed.title);
    assert_eq!(progress.project, indexed.project);
    assert_eq!(progress.phase, "thinking");
    assert!(progress.agent_product_id.is_none());
}

fn exec_start(turn: &str) -> Value {
    json!({"type":"event_msg","timestamp":now(),"payload":{
        "type":"task_started","turn_id":turn,"root_turn_id":turn,
        "turn_attribution":{"turn_id":turn,"turn_trigger":"exec",
            "root_turn_id":turn,"parent_turn_id":null,"initiating_agent_path":null}
    }})
}

#[test]
fn root_exec_turn_identifies_only_its_live_activity_not_context_or_historical_usage() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().canonicalize().unwrap();
    let path = file(
        &home,
        &[
            // The creator may be desktop: it is the current turn marker that proves exec.
            json!({"type":"session_meta","timestamp":now(),"payload":{"id":"cli","originator":"codex_work_desktop"}}),
            exec_start("exec-one"),
            json!({"type":"event_msg","timestamp":now(),"payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":100},"model_context_window":1000}}}),
            json!({"type":"token_usage_record","timestamp":now(),"payload":{"turn_id":"exec-one","response_id":"response-one","usage":{"input_tokens":100,"output_tokens":2}}}),
            json!({"type":"response_item","timestamp":now(),"payload":{"type":"custom_tool_call","call_id":"read-one","name":"read","input":"readonly"}}),
        ],
    );
    fs::write(path.with_file_name("desktop.jsonl"), format!("{}\n{}\n",
        json!({"type":"session_meta","timestamp":now(),"payload":{"id":"desktop","originator":"codex_exec"}}),
        json!({"type":"event_msg","timestamp":now(),"originator":"codex_work_desktop","payload":{"type":"task_started","turn_id":"desktop-one"}})
    )).unwrap();
    let original = fs::read(&path).unwrap();
    let sessions = NativeSessions::new(home, &["codex".into()]);
    sessions.refresh().unwrap();
    let mut watch = live::NativeWatch::default();
    let rows = watch.poll(&sessions);
    let cli = rows.iter().find(|a| a.session_id == "codex:cli").unwrap();
    let desktop = rows
        .iter()
        .find(|a| a.session_id == "codex:desktop")
        .unwrap();
    assert_eq!(cli.agent_product_id.as_deref(), Some("codex-cli"));
    assert_eq!(cli.agent_attribution.as_deref(), Some("native_exec_turn"));
    assert_eq!(cli.phase, "tool_requested");
    assert_eq!(desktop.agent_product_id.as_deref(), Some("codex-desktop"));
    let context = watch
        .context_samples(&sessions, vec![cli.session_id.clone()])
        .unwrap()
        .remove(0)
        .context
        .unwrap();
    assert_eq!(context.input_tokens, Some(100));
    assert!(context.sample_agent_product_id.is_none());
    assert_eq!(
        sessions
            .calls(CallQuery {
                agent: Some("codex-cli".into()),
                ..Default::default()
            })
            .unwrap()
            .total,
        0
    );
    assert!(
        sessions
            .calls(CallQuery {
                agent: Some("unknown".into()),
                ..Default::default()
            })
            .unwrap()
            .total
            > 0
    );
    assert_eq!(fs::read(path).unwrap(), original);
}

#[test]
fn partial_exec_metadata_and_other_triggers_never_infer_cli_from_a_session_creator() {
    for case in 0..10 {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().canonicalize().unwrap();
        let mut start = exec_start("one");
        let p = &mut start["payload"];
        match case {
            0 => p["turn_attribution"]["turn_trigger"] = json!("composer"),
            1 => p["turn_attribution"]["turn_id"] = json!("other"),
            2 => p["turn_attribution"]["root_turn_id"] = json!("other"),
            3 => p["root_turn_id"] = json!("other"),
            4 => p["turn_attribution"]["parent_turn_id"] = json!("parent"),
            5 => p["turn_attribution"]["initiating_agent_path"] = json!("/child"),
            6 => {
                p["turn_attribution"]
                    .as_object_mut()
                    .unwrap()
                    .remove("parent_turn_id");
            }
            7 => {
                p["turn_attribution"]
                    .as_object_mut()
                    .unwrap()
                    .remove("initiating_agent_path");
            }
            8 => p["turn_attribution"] = Value::Null,
            _ => {
                p["turn_id"] = json!("");
                p["turn_attribution"]["turn_id"] = json!("");
            }
        }
        file(
            &home,
            &[
                json!({"type":"session_meta","timestamp":now(),"payload":{"id":"s","originator":"codex_exec"}}),
                start,
                json!({"type":"response_item","timestamp":now(),"payload":{"type":"reasoning","text":"turn_trigger=exec"}}),
            ],
        );
        let sessions = NativeSessions::new(home, &["codex".into()]);
        let active = live::NativeWatch::default().poll(&sessions).remove(0);
        assert!(active.agent_product_id.is_none(), "case {case}");
    }
}

#[test]
fn exec_turn_proof_is_cleared_by_new_turns_conflicts_and_unrecognised_clients() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().canonicalize().unwrap();
    let path = file(
        &home,
        &[
            json!({"type":"session_meta","timestamp":now(),"payload":{"id":"s"}}),
            exec_start("one"),
        ],
    );
    let sessions = NativeSessions::new(home, &["codex".into()]);
    let mut watch = live::NativeWatch::default();
    assert_eq!(
        watch.poll(&sessions)[0].agent_product_id.as_deref(),
        Some("codex-cli")
    );
    append(
        &path,
        json!({"type":"event_msg","timestamp":now(),"payload":{"type":"task_started","turn_id":"two"}}),
    );
    assert!(watch.poll(&sessions)[0].agent_product_id.is_none());
    append(&path, exec_start("three"));
    assert_eq!(
        watch.poll(&sessions)[0].agent_product_id.as_deref(),
        Some("codex-cli")
    );
    append(
        &path,
        json!({"type":"response_item","timestamp":now(),"originator":"codex_work_desktop","payload":{"type":"reasoning"}}),
    );
    let conflict = watch.poll(&sessions).remove(0);
    assert!(conflict.agent_product_id.is_none());
    assert_eq!(
        conflict.agent_attribution.as_deref(),
        Some("conflicting_evidence")
    );
    append(
        &path,
        json!({"type":"response_item","timestamp":now(),"payload":{"type":"reasoning"}}),
    );
    assert!(watch.poll(&sessions)[0].agent_product_id.is_none());
    append(&path, exec_start("four"));
    assert_eq!(
        watch.poll(&sessions)[0].agent_product_id.as_deref(),
        Some("codex-cli")
    );
    append(
        &path,
        json!({"type":"response_item","timestamp":now(),"originator":"unrecognised-client","payload":{"type":"reasoning"}}),
    );
    assert_eq!(
        watch.poll(&sessions)[0].agent_attribution.as_deref(),
        Some("unrecognized_client")
    );
    append(
        &path,
        json!({"type":"response_item","timestamp":now(),"payload":{"type":"reasoning"}}),
    );
    assert!(watch.poll(&sessions)[0].agent_product_id.is_none());
    append(&path, exec_start("five"));
    append(
        &path,
        json!({"type":"event_msg","timestamp":now(),"payload":{"type":"task_complete","turn_id":"five"}}),
    );
    assert_eq!(watch.poll(&sessions)[0].phase, "waiting_input");
    append(
        &path,
        json!({"type":"response_item","timestamp":now(),"payload":{"type":"reasoning"}}),
    );
    assert_eq!(watch.poll(&sessions)[0].phase, "waiting_input");
}

#[test]
fn exec_proof_does_not_survive_truncation_catchup_gaps_or_source_revocation() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().canonicalize().unwrap();
    let header = json!({"type":"session_meta","timestamp":now(),"payload":{"id":"s","originator":"codex_exec"}});
    let path = file(&home, &[header.clone(), exec_start("one")]);
    let sessions = NativeSessions::new(home.clone(), &["codex".into()]);
    let mut watch = live::NativeWatch::default();
    assert_eq!(
        watch.poll(&sessions)[0].agent_product_id.as_deref(),
        Some("codex-cli")
    );
    file(
        &home,
        &[
            header.clone(),
            json!({"type":"response_item","timestamp":now(),"payload":{"type":"reasoning"}}),
        ],
    );
    assert!(watch.poll(&sessions)[0].agent_product_id.is_none());
    append(&path, exec_start("two"));
    assert_eq!(
        watch.poll(&sessions)[0].agent_product_id.as_deref(),
        Some("codex-cli")
    );
    for _ in 0..9 {
        append(
            &path,
            json!({"type":"padding","payload":{"text":"x".repeat(1024*1024)}}),
        );
    }
    append(
        &path,
        json!({"type":"response_item","timestamp":now(),"payload":{"type":"reasoning"}}),
    );
    assert!(watch.poll(&sessions)[0].agent_product_id.is_none());
    append(&path, exec_start("three"));
    assert_eq!(
        watch.poll(&sessions)[0].agent_product_id.as_deref(),
        Some("codex-cli")
    );
    sessions.set_enabled("codex", false).unwrap();
    assert!(watch.poll(&sessions).is_empty());
    sessions.set_enabled("codex", true).unwrap();
    let active = watch.poll(&sessions).remove(0);
    assert_eq!(active.agent_product_id.as_deref(), Some("codex-cli"));
    // The fresh bounded tail contains this new explicit marker; it is re-read,
    // rather than carried over from the revoked source.
}
