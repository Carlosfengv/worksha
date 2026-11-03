use super::*;
use serde_json::{Value, json};
use std::io::Write;

#[test]
fn native_usage_is_bound_to_each_message_without_advancing_its_activity_time() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().canonicalize().unwrap();
    let path = home.join(".codex/sessions/usage.jsonl");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let session = "11111111-1111-4111-8111-111111111111";
    let turns = [
        "22222222-2222-4222-8222-222222222222",
        "33333333-3333-4333-8333-333333333333",
    ];
    let base = chrono::Utc::now() - chrono::Duration::seconds(40);
    let at = |n| (base + chrono::Duration::seconds(n)).to_rfc3339();
    append(
        &path,
        json!({"type":"session_meta","timestamp":at(0),"payload":{"id":session}}),
    );
    for (i, turn) in turns.iter().enumerate() {
        let n = i as i64 * 10;
        append(
            &path,
            json!({"type":"event_msg","timestamp":at(n+1),"payload":{"type":"task_started","turn_id":turn}}),
        );
        append(
            &path,
            json!({"type":"response_item","timestamp":at(n+2),"payload":{"type":"message","role":"user","content":[{"type":"input_text","text":format!("消息{i}")}]}}),
        );
        append(
            &path,
            json!({"type":"event_msg","timestamp":at(n+3),"payload":{"type":"task_complete","turn_id":turn}}),
        );
        append(
            &path,
            json!({"type":"token_usage_record","timestamp":at(n+4),"payload":{
                "thread_id":session,"turn_id":turn,"turn_token_usage":{"output_tokens":(i+1)*100,"reasoning_output_tokens":(i+1)*30},
                "thread_token_usage":{"output_tokens":999999},"usage":{"output_tokens":20}
            }}),
        );
    }
    let sessions = NativeSessions::new(home.clone(), &["codex".into()]);
    let mut watch = live::NativeWatch::with_cache_directory(home.join("app-data"));
    for indexed in [false, true] {
        if indexed {
            sessions.refresh().unwrap();
        }
        let rows = watch.poll(&sessions);
        assert_eq!(rows.len(), 2);
        for (i, turn) in turns.iter().enumerate() {
            let row = rows.iter().find(|r| r.message_id() == Some(turn)).unwrap();
            assert_eq!(
                row.message_usage.as_ref().unwrap().output_tokens,
                (i as u64 + 1) * 100
            );
            assert_eq!(
                row.message_usage.as_ref().unwrap().reasoning_tokens,
                Some((i as u64 + 1) * 30)
            );
            assert_eq!(row.event_at, at(i as i64 * 10 + 3));
        }
    }
}

fn append(path: &Path, value: Value) {
    writeln!(
        fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap(),
        "{value}"
    )
    .unwrap();
}
fn fixture(home: &Path, agent: &str) -> PathBuf {
    let path = if agent == "codex" {
        home.join(".codex/sessions/2026/10/24/history.jsonl")
    } else {
        home.join(".claude/projects/history/session.jsonl")
    };
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let base = chrono::Utc::now() - chrono::Duration::seconds(20);
    let at = |n| (base + chrono::Duration::seconds(n)).to_rfc3339();
    if agent == "codex" {
        append(
            &path,
            json!({"type":"session_meta","timestamp":at(0),"payload":{"id":"history"}}),
        );
    }
    for (n, text) in [(1, "第一条输入 Bearer abcdefghijklmnop"), (2, "第二条输入")] {
        let id = format!("user-{n}");
        if agent == "codex" {
            append(
                &path,
                json!({"type":"event_msg","timestamp":at(n*4),"payload":{"type":"task_started","turn_id":id}}),
            );
            append(
                &path,
                json!({"type":"response_item","timestamp":at(n*4),"payload":{"type":"message","role":"user","content":[{"type":"input_text","text":text}]}}),
            );
            append(
                &path,
                json!({"type":"response_item","timestamp":at(n*4+1),"payload":{"type":"function_call","call_id":format!("call-{n}"),"name":"Read","arguments":"tool-body-sentinel".repeat(180_000)}}),
            );
            append(
                &path,
                json!({"type":"event_msg","timestamp":at(n*4+2),"payload":{"type":"task_complete","turn_id":id,"time_to_first_token_ms":462}}),
            );
        } else {
            append(
                &path,
                json!({"type":"user","uuid":id,"sessionId":"history","timestamp":at(n*4),"message":{"content":text}}),
            );
            append(
                &path,
                json!({"type":"assistant","uuid":format!("assistant-{n}"),"sessionId":"history","timestamp":at(n*4+1),"message":{"model":"fixture-model","content":[{"type":"tool_use","id":format!("call-{n}"),"name":"Read","input":{"body":"tool-body-sentinel".repeat(180_000)}}]}}),
            );
            append(
                &path,
                json!({"type":"system","sessionId":"history","subtype":"turn_duration","timestamp":at(n*4+2)}),
            );
        }
    }
    path
}
#[test]
fn old_inputs_outside_the_tail_are_backfilled_and_both_messages_survive_restart() {
    for agent in ["codex", "claude"] {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().canonicalize().unwrap();
        let path = fixture(&home, agent);
        assert!(fs::metadata(&path).unwrap().len() > 2 * 1024 * 1024);
        let data = home.join("app-data");
        let sessions = NativeSessions::new(home.clone(), &[agent.into()]);
        let mut watch = live::NativeWatch::with_cache_directory(data.clone());
        assert!(
            watch
                .poll(&sessions)
                .iter()
                .all(|row| row.user_input.is_none())
        );
        sessions.refresh().unwrap();
        let rows = watch.poll(&sessions);
        assert_eq!(rows.len(), 2, "{agent}: {rows:?}");
        assert!(rows.iter().all(|row| row.user_input.is_some()));
        assert!(
            rows.iter()
                .any(|row| row.user_input.as_ref().unwrap().text == "第一条输入 [已隐藏凭据]")
        );
        assert!(rows.iter().all(|row| row.phase == "waiting_input"));
        let ids = rows
            .iter()
            .map(|row| (row.id.clone(), row.message_started_at.clone()))
            .collect::<Vec<_>>();
        drop(watch);
        drop(sessions);
        let restarted = NativeSessions::new(home.clone(), &[agent.into()]);
        let mut watch = live::NativeWatch::with_cache_directory(data.clone());
        let cold = watch.poll(&restarted);
        assert_eq!(cold.len(), 2, "cold {agent}: {cold:?}");
        assert!(cold.iter().all(|row| row.user_input.is_some()));
        assert_eq!(
            cold.iter()
                .map(|row| (row.id.clone(), row.message_started_at.clone()))
                .collect::<Vec<_>>(),
            ids
        );
        restarted.refresh().unwrap();
        assert_eq!(watch.poll(&restarted).len(), 2);
        let cache = fs::read_to_string(data.join("native-activity-inputs.json")).unwrap();
        assert!(!cache.contains("tool-body-sentinel"));
        assert!(!cache.contains("abcdefghijklmnop"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(data.join("native-activity-inputs.json"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        restarted.set_enabled(agent, false).unwrap();
        assert!(watch.poll(&restarted).is_empty());
        let cache = fs::read_to_string(data.join("native-activity-inputs.json")).unwrap();
        assert!(!cache.contains("第一条输入"));
        assert!(!cache.contains("第二条输入"));
    }
}

#[test]
fn rewritten_or_deleted_sources_do_not_restore_old_cached_messages() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().canonicalize().unwrap();
    let path = fixture(&home, "claude");
    let data = home.join("app-data");
    let sessions = NativeSessions::new(home.clone(), &["claude".into()]);
    sessions.refresh().unwrap();
    let mut watch = live::NativeWatch::with_cache_directory(data.clone());
    assert_eq!(watch.poll(&sessions).len(), 2);
    fs::write(&path, "").unwrap();
    append(
        &path,
        json!({"type":"user","uuid":"new","sessionId":"history","timestamp":chrono::Utc::now().to_rfc3339(),"message":{"content":"重写后的输入"}}),
    );
    let rows = watch.poll(&sessions);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].user_input.as_ref().unwrap().text, "重写后的输入");
    fs::remove_file(&path).unwrap();
    assert!(watch.poll(&sessions).is_empty());
    assert!(
        !fs::read_to_string(data.join("native-activity-inputs.json"))
            .unwrap()
            .contains("重写后的输入")
    );
}

#[test]
fn same_length_rewrite_with_restored_mtime_does_not_keep_an_old_message_summary() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().canonicalize().unwrap();
    let path = home.join(".claude/projects/rewrite/session.jsonl");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    append(
        &path,
        json!({"type":"user","uuid":"user","sessionId":"rewrite","timestamp":chrono::Utc::now().to_rfc3339(),"message":{"content":"旧用户消息"}}),
    );
    let modified = fs::metadata(&path).unwrap().modified().unwrap();
    let sessions = NativeSessions::new(home.clone(), &["claude".into()]);
    sessions.refresh().unwrap();
    let mut watch = live::NativeWatch::with_cache_directory(home.join("app-data"));
    assert_eq!(
        watch.poll(&sessions)[0].user_input.as_ref().unwrap().text,
        "旧用户消息"
    );
    let bytes = fs::read_to_string(&path)
        .unwrap()
        .replace("旧用户消息", "新用户消息");
    fs::write(&path, bytes).unwrap();
    std::fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(modified)
        .unwrap();
    assert_eq!(
        watch.poll(&sessions)[0].user_input.as_ref().unwrap().text,
        "新用户消息"
    );
}

#[test]
fn indexed_history_and_native_tail_keep_one_message_row_with_offset_timestamps() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().canonicalize().unwrap();
    let path = fixture(&home, "claude");
    let text = fs::read_to_string(&path).unwrap();
    let mut rewritten = String::new();
    for line in text.lines() {
        let mut value: Value = serde_json::from_str(line).unwrap();
        let at =
            chrono::DateTime::parse_from_rfc3339(value["timestamp"].as_str().unwrap()).unwrap();
        value["timestamp"] = json!(
            at.with_timezone(&chrono::FixedOffset::east_opt(8 * 3600).unwrap())
                .to_rfc3339()
        );
        rewritten.push_str(&value.to_string());
        rewritten.push('\n');
    }
    fs::write(&path, rewritten).unwrap();
    let sessions = NativeSessions::new(home, &["claude".into()]);
    sessions.refresh().unwrap();
    let rows = live::NativeWatch::default().poll(&sessions);
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert!(
        rows.iter()
            .all(|row| row.turn_id.is_some() && row.user_input.is_some())
    );
}

#[test]
fn ongoing_appends_do_not_block_backfilling_inputs_outside_the_live_tail() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().canonicalize().unwrap();
    let path = fixture(&home, "codex");
    let sessions = NativeSessions::new(home.clone(), &["codex".into()]);
    sessions.refresh().unwrap();
    for _ in 0..3 {
        append(
            &path,
            json!({"type":"turn_context","timestamp":chrono::Utc::now().to_rfc3339(),"payload":{"turn_id":"user-2","model":"fixture-model"}}),
        );
        let rows = live::NativeWatch::default().poll(&sessions);
        assert_eq!(rows.len(), 2, "{rows:?}");
        assert!(
            rows.iter()
                .all(|row| row.user_input.is_some() && row.message_started_at.is_some())
        );
    }
}

#[test]
fn old_native_desktop_task_inputs_backfill_each_turn_and_survive_a_cold_restart() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().canonicalize().unwrap();
    let path = home.join(".codex/sessions/2026/10/25/tasks.jsonl");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let base = chrono::Utc::now() - chrono::Duration::seconds(90);
    let at = |n| (base + chrono::Duration::seconds(n)).to_rfc3339();
    append(
        &path,
        json!({"type":"session_meta","timestamp":at(0),"payload":{"id":"desktop-tasks"}}),
    );
    let ids = [
        "11111111-1111-4111-8111-111111111111",
        "22222222-2222-4222-8222-222222222222",
        "33333333-3333-4333-8333-333333333333",
    ];
    for (i, id) in ids.iter().enumerate() {
        let n = i as i64 * 10;
        append(
            &path,
            json!({"type":"event_msg","timestamp":at(n+1),"payload":{"type":"task_started","turn_id":id}}),
        );
        append(
            &path,
            json!({"type":"response_item","timestamp":at(n+2),"payload":{
                "type":"function_call_output","id":format!("fco_{id}"),"namespace":"codex_app","name":"send_message_to_thread",
                "internal_chat_message_metadata_passthrough":{"turn_id":id},
                "output":format!("<codex_delegation><source_thread_id>44444444-4444-4444-8444-444444444444</source_thread_id><input>第{}条真实任务输入 Bearer abcdefghijklmnop</input></codex_delegation>",i+1)
            }}),
        );
        append(
            &path,
            json!({"type":"response_item","timestamp":at(n+3),"payload":{"type":"function_call_output","call_id":"ordinary-tool","output":"TOOL-RESULT-MUST-NOT-BECOME-TITLE".repeat(80_000)}}),
        );
        append(
            &path,
            json!({"type":"event_msg","timestamp":at(n+4),"payload":{"type":"task_complete","turn_id":id,"time_to_first_token_ms":(i+1)*100}}),
        );
    }
    assert!(fs::metadata(&path).unwrap().len() > 2 * 1024 * 1024);
    let data = home.join("app-data");
    let sessions = NativeSessions::new(home.clone(), &["codex".into()]);
    let mut watch = live::NativeWatch::with_cache_directory(data.clone());
    assert!(watch.poll(&sessions).iter().all(|r| r.user_input.is_none()));
    sessions.refresh().unwrap();
    let rows = watch.poll(&sessions);
    assert_eq!(rows.len(), 3);
    for (i, id) in ids.iter().enumerate() {
        let row = rows.iter().find(|r| r.message_id() == Some(id)).unwrap();
        assert_eq!(
            row.user_input.as_ref().unwrap().text,
            format!("第{}条真实任务输入 [已隐藏凭据]", i + 1)
        );
        assert_eq!(row.phase, "waiting_input");
        assert_eq!(row.turn_ttft_ms, Some((i + 1) as f64 * 100.0));
    }
    let old = rows
        .iter()
        .map(|r| {
            (
                r.id.clone(),
                r.user_input.clone(),
                r.message_started_at.clone(),
            )
        })
        .collect::<Vec<_>>();
    drop(watch);
    drop(sessions);
    let restarted = NativeSessions::new(home.clone(), &["codex".into()]);
    let mut watch = live::NativeWatch::with_cache_directory(data.clone());
    let cold = watch.poll(&restarted);
    assert_eq!(cold.len(), 3);
    assert!(old.iter().all(|(id, input, started)| {
        cold.iter()
            .any(|r| r.id == *id && r.user_input == *input && r.message_started_at == *started)
    }));
    let bytes = fs::read_to_string(data.join("native-activity-inputs.json")).unwrap();
    assert!(
        !bytes.contains("TOOL-RESULT")
            && !bytes.contains("abcdefghijklmnop")
            && !bytes.contains("codex_delegation")
    );
    restarted.set_enabled("codex", false).unwrap();
    assert!(watch.poll(&restarted).is_empty());
    assert!(
        !fs::read_to_string(data.join("native-activity-inputs.json"))
            .unwrap()
            .contains("真实任务输入")
    );
}

#[test]
fn desktop_question_answers_backfill_old_previews_and_survive_restart() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().canonicalize().unwrap();
    let path = fixture(&home, "codex");
    let body = fs::read_to_string(&path)
        .unwrap()
        .lines()
        .map(|line| {
            let mut value: Value = serde_json::from_str(line).unwrap();
            if value["payload"]["role"] == "user" {
                let answer = value["payload"]["content"][0]["text"].clone();
                value["payload"]["content"][0]["text"] = json!(format!(
                    "<send_user_message_question_reply>{}</send_user_message_question_reply>",
                    json!([{"question":"PRIVATE-QUESTION-METADATA".repeat(40),"questionItemId":"internal-id","answer":answer}])
                ));
            }
            format!("{value}\n")
        })
        .collect::<String>();
    fs::write(&path, body).unwrap();
    let data = home.join("app-data");
    let sessions = NativeSessions::new(home.clone(), &["codex".into()]);
    sessions.refresh().unwrap();
    let mut watch = live::NativeWatch::with_cache_directory(data.clone());
    let rows = watch.poll(&sessions);
    assert_eq!(rows.len(), 2);
    assert_eq!(
        rows.iter()
            .find(|row| row.message_id() == Some("user-1"))
            .unwrap()
            .user_input
            .as_ref()
            .unwrap()
            .text,
        "第一条输入 [已隐藏凭据]"
    );
    drop(watch);
    // A previous version could have saved a truncated transport envelope.
    let cache_path = data.join("native-activity-inputs.json");
    let mut cache: Value = serde_json::from_str(&fs::read_to_string(&cache_path).unwrap()).unwrap();
    for row in cache["rows"].as_array_mut().unwrap() {
        row["input"]["text"] = json!("<send_user_message_question_reply>old truncated envelope…");
    }
    fs::write(&cache_path, cache.to_string()).unwrap();
    let restarted = NativeSessions::new(home.clone(), &["codex".into()]);
    let mut watch = live::NativeWatch::with_cache_directory(data.clone());
    watch.poll(&restarted);
    restarted.refresh().unwrap();
    for row in watch.poll(&restarted) {
        let old = rows.iter().find(|old| old.id == row.id).unwrap();
        assert_eq!(row.user_input, old.user_input);
        assert_eq!(row.turn_ttft_ms, old.turn_ttft_ms);
    }
    let saved = fs::read_to_string(cache_path).unwrap();
    assert!(!saved.contains("send_user_message_question_reply"));
    assert!(!saved.contains("PRIVATE-QUESTION-METADATA"));
    assert!(!saved.contains("abcdefghijklmnop"));
    drop(watch);
    let mut cold = live::NativeWatch::with_cache_directory(data);
    let no_index = NativeSessions::new(home, &["codex".into()]);
    let restored = cold.poll(&no_index);
    assert_eq!(restored.len(), 2);
    assert!(restored.iter().all(|row| {
        rows.iter()
            .any(|old| old.id == row.id && old.user_input == row.user_input)
    }));
}

#[test]
fn an_inactive_sessions_latest_input_is_still_recoverable_without_reopening_old_history() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().canonicalize().unwrap();
    let path = fixture(&home, "codex");
    let content = fs::read_to_string(&path)
        .unwrap()
        .lines()
        .map(|line| {
            let mut v: Value = serde_json::from_str(line).unwrap();
            let at =
                chrono::DateTime::parse_from_rfc3339(v["timestamp"].as_str().unwrap()).unwrap();
            v["timestamp"] = json!((at - chrono::Duration::days(2)).to_rfc3339());
            format!("{v}\n")
        })
        .collect::<String>();
    fs::write(&path, content).unwrap();
    let sessions = NativeSessions::new(home.clone(), &["codex".into()]);
    sessions.refresh().unwrap();
    let mut watch = live::NativeWatch::with_cache_directory(home.join("app-data"));
    let rows = watch.poll(&sessions);
    let latest = rows
        .iter()
        .find(|r| r.message_id() == Some("user-2"))
        .unwrap();
    assert_eq!(latest.user_input.as_ref().unwrap().text, "第二条输入");
    assert_eq!(latest.phase, "waiting_input");
    assert!(rows.iter().all(|r| r.message_id() != Some("user-1")));
}
