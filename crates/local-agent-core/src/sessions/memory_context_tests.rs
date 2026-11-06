use super::*;
use serde_json::{Value, json};
use std::io::Write;

fn transcript(home: &Path, family: &str, rows: &[Value]) -> PathBuf {
    let dir = home.join(if family == "codex" {
        ".codex/sessions"
    } else {
        ".claude/projects/test"
    });
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("context.jsonl");
    let mut file = fs::File::create(&path).unwrap();
    for row in rows {
        writeln!(file, "{row}").unwrap();
    }
    path
}
fn parse_context(home: &Path, path: &Path, family: &str) -> SessionContext {
    parser::parse(
        home,
        path,
        family,
        Some(parser::Window {
            offset: 0,
            anchor: None,
        }),
    )
    .unwrap()
    .context
}
#[test]
fn live_context_reuses_index_and_tail_deduplicates_and_invalidates_after_compaction() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().canonicalize().unwrap();
    let ts = now();
    let rows = vec![
        json!({"type":"session_meta","timestamp":ts,"payload":{"id":"s"}}),
        json!({"type":"event_msg","timestamp":ts,"payload":{"type":"task_started"}}),
        json!({"type":"event_msg","timestamp":ts,"payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":100},"total_token_usage":{"input_tokens":999999},"model_context_window":1000}}}),
    ];
    let file = transcript(&home, "codex", &rows);
    let sessions = NativeSessions::new(home.clone(), &["codex".into()]);
    sessions.refresh().unwrap();
    let mut watch = live::NativeWatch::default();
    let ids = || vec!["codex:s".into(), "codex:s".into(), "codex:missing".into()];
    let samples = watch.context_samples(&sessions, ids()).unwrap();
    assert_eq!(samples.len(), 2);
    assert_eq!(samples[0].state, "unknown");
    assert_eq!(
        samples[1].context.as_ref().unwrap().used_percentage,
        Some(10.0)
    );
    let original = fs::read(&file).unwrap();
    watch.poll(&sessions);
    let sample = watch
        .context_samples(&sessions, vec!["codex:s".into()])
        .unwrap()
        .remove(0);
    assert_eq!(sample.context.unwrap().input_tokens, Some(100));
    assert_eq!(fs::read(&file).unwrap(), original);
    let append = |row: Value| {
        writeln!(
            fs::OpenOptions::new().append(true).open(&file).unwrap(),
            "{row}"
        )
        .unwrap()
    };
    append(
        json!({"type":"compacted","timestamp":now(),"payload":{"message":"private-summary-sentinel"}}),
    );
    assert_eq!(
        watch
            .context_samples(&sessions, vec!["codex:s".into()])
            .unwrap()[0]
            .state,
        "pending"
    );
    watch.poll(&sessions);
    let sample = watch
        .context_samples(&sessions, vec!["codex:s".into()])
        .unwrap()
        .remove(0);
    assert_eq!(sample.context.unwrap().input_tokens, None);
    append(
        json!({"type":"event_msg","timestamp":now(),"payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":0},"model_context_window":1000}}}),
    );
    watch.poll(&sessions);
    let samples = watch
        .context_samples(&sessions, vec!["codex:s".into()])
        .unwrap();
    assert_eq!(
        samples[0].context.as_ref().unwrap().used_percentage,
        Some(0.0)
    );
    assert!(
        samples[0]
            .context
            .as_ref()
            .unwrap()
            .sample_agent_product_id
            .is_none()
    );
    let encoded = serde_json::to_string(&samples).unwrap();
    assert!(!encoded.contains("private-summary-sentinel"));
    assert!(!encoded.contains("loaded"));
    sessions.set_enabled("codex", false).unwrap();
    let sample = watch
        .context_samples(&sessions, vec!["codex:s".into()])
        .unwrap()
        .remove(0);
    assert_eq!(sample.state, "unavailable");
    assert!(sample.context.is_none());
}

#[test]
fn live_claude_context_keeps_cache_once_and_resets_on_file_replacement() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().canonicalize().unwrap();
    let rows = vec![
        json!({"type":"assistant","timestamp":now(),"sessionId":"s","message":{"usage":{"input_tokens":10,"cache_read_input_tokens":20,"cache_creation_input_tokens":30}}}),
    ];
    transcript(&home, "claude", &rows);
    let sessions = NativeSessions::new(home.clone(), &["claude".into()]);
    sessions.refresh().unwrap();
    let mut watch = live::NativeWatch::default();
    watch.poll(&sessions);
    let sample = watch
        .context_samples(&sessions, vec!["claude:s".into()])
        .unwrap()
        .remove(0);
    assert_eq!(sample.context.unwrap().input_tokens, Some(60));
    transcript(
        &home,
        "claude",
        &[json!({"type":"user","timestamp":now(),"sessionId":"s","message":{"content":"new"}})],
    );
    watch.poll(&sessions);
    let sample = watch
        .context_samples(&sessions, vec!["claude:s".into()])
        .unwrap()
        .remove(0);
    assert!(sample.context.unwrap().input_tokens.is_none());
}
#[test]
fn codex_last_input_not_cumulative_and_context_is_not_page_limited() {
    let home = tempfile::tempdir().unwrap();
    let mut rows = vec![
        json!({"type":"session_meta","payload":{"id":"s","base_instructions":{"text":"api_key=abcdefghijklmnop"}}}),
    ];
    for n in 0..70 {
        rows.push(json!({"type":"response_item","payload":{"id":format!("m{n}"),"type":"message","role":"user","content":[{"type":"input_text","text":"hello"}]}}));
    }
    rows.push(json!({"type":"turn_context","originator":"codex_cli_rs","payload":{"model":"known","developer_instructions":"shared guidance"}}));
    rows.push(json!({"type":"event_msg","timestamp":"2026-10-19T01:00:00Z","payload":{"type":"token_count","info":{"model_context_window":1000,"last_token_usage":{"input_tokens":100},"total_token_usage":{"input_tokens":9999999}}}}));
    let path = transcript(home.path(), "codex", &rows);
    let original = fs::read(&path).unwrap();
    let data = parse_context(home.path(), &path, "codex");
    assert_eq!(data.input_tokens, Some(100));
    assert_eq!(data.used_percentage, Some(10.0));
    assert_eq!(data.loaded_count, 2);
    assert!(data.loaded[0].text.as_ref().unwrap().contains("已隐藏凭据"));
    assert_eq!(original, fs::read(&path).unwrap());
    let encoded = serde_json::to_value(&data).unwrap();
    assert_eq!(encoded["windowTokens"], 1000);
    assert!(encoded["input_tokens"].is_null());
    rows.push(json!({"type":"compacted","payload":{"message":"keep this summary"}}));
    let path = transcript(home.path(), "codex", &rows);
    let data = parse_context(home.path(), &path, "codex");
    assert_eq!(data.input_tokens, None);
    assert_eq!(data.used_percentage, None);
    assert_eq!(data.compaction_count, 1);
    assert_eq!(
        data.compactions[0].text.as_deref(),
        Some("keep this summary")
    );
}
#[test]
fn claude_cache_is_counted_once_failed_reads_and_title_summaries_are_not_context() {
    let home = tempfile::tempdir().unwrap();
    let rows = vec![
        json!({"type":"assistant","sessionId":"s","uuid":"a","message":{"model":"m","usage":{"input_tokens":0,"cache_read_input_tokens":30,"cache_creation_input_tokens":20},"content":[{"type":"tool_use","id":"read","name":"Read","input":{"file_path":"/project/CLAUDE.md"}}]}}),
        json!({"type":"user","sessionId":"s","uuid":"u","message":{"content":[{"type":"tool_result","tool_use_id":"read","is_error":true,"content":"denied"}]}}),
        json!({"type":"summary","summary":"A short session title"}),
        json!({"type":"assistant","sessionId":"foreign","message":{"usage":{"input_tokens":9999}}}),
        json!({"type":"assistant","isSidechain":true,"message":{"usage":{"input_tokens":8888}}}),
    ];
    let path = transcript(home.path(), "claude", &rows);
    let data = parse_context(home.path(), &path, "claude");
    assert_eq!(data.input_tokens, Some(50));
    assert_eq!(data.window_tokens, None);
    assert_eq!(data.loaded_count, 0);
    assert_eq!(data.compaction_count, 0);
    let mut rows = rows;
    rows.push(
        json!({"type":"system","sessionId":"s","subtype":"compact_boundary","content":"summary"}),
    );
    let path = transcript(home.path(), "claude", &rows);
    assert_eq!(
        parse_context(home.path(), &path, "claude").input_tokens,
        None
    );
    rows.push(json!({"type":"assistant","sessionId":"s","uuid":"zero","message":{"usage":{"input_tokens":0}}}));
    let path = transcript(home.path(), "claude", &rows);
    assert_eq!(
        parse_context(home.path(), &path, "claude").input_tokens,
        Some(0)
    );
}
#[test]
fn opencode_completed_loads_and_explicit_compaction_keep_unknown_limits() {
    let mut c = context::Collector::new(true);
    c.opencode_message(&json!({"role":"assistant","tokens":{"input":10,"cache":{"read":20,"write":30}},"modelID":"m"}), None, 1);
    assert_eq!(c.data.input_tokens, Some(60));
    c.opencode_part(
        &json!({"type":"tool","tool":"skill","state":{"status":"running","output":"not yet"}}),
        None,
        2,
    );
    assert_eq!(c.data.loaded_count, 0);
    c.opencode_part(&json!({"type":"tool","tool":"skill","state":{"status":"completed","output":"actual skill"}}), None, 3);
    assert_eq!(c.data.loaded_count, 1);
    c.opencode_part(&json!({"type":"compaction","auto":true}), None, 4);
    let data = c.finish();
    assert_eq!(data.input_tokens, None);
    assert_eq!(data.used_percentage, None);
    assert!(data.compactions[0].label.contains("完成状态未知"));
}

#[test]
fn opencode_inflight_zero_is_not_observed_context_and_cannot_replace_a_sample() {
    let mut c = context::Collector::new(false);
    let zero = json!({"role":"assistant","tokens":{"input":0,"output":0,"cache":{"read":0,"write":0}},"modelID":"model"});
    c.opencode_message(&zero, None, 1);
    assert!(c.data.input_tokens.is_none());
    c.opencode_message(
        &json!({"role":"assistant","tokens":{"input":30,"cache":{"read":20}}}),
        None,
        2,
    );
    assert_eq!(c.data.input_tokens, Some(50));
    c.opencode_message(&zero, None, 3);
    c.opencode_message(&json!({"role":"assistant"}), None, 4);
    assert_eq!(c.data.input_tokens, Some(50));
    let mut completed = zero;
    completed["time"] = json!({"completed":1792461000000u64});
    completed["error"] = json!({"name":"APIError"});
    c.opencode_message(&completed, None, 5);
    assert_eq!(c.data.input_tokens, Some(50));
    completed.as_object_mut().unwrap().remove("error");
    c.opencode_message(&completed, None, 6);
    assert_eq!(c.data.input_tokens, Some(0));
}
#[test]
fn builtin_agents_memory_is_read_only_shared_and_requires_consent() {
    let home = tempfile::tempdir().unwrap();
    for (file, content) in [
        (
            ".codex/memories/MEMORY.md",
            "api_key=abcdefghijklmno\nnotes",
        ),
        (".claude/CLAUDE.md", "rules"),
        (".config/opencode/AGENTS.md", "open rules"),
    ] {
        let path = home.path().join(file);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }
    let sessions = NativeSessions::new(
        home.path().canonicalize().unwrap(),
        &["codex".into(), "claude".into(), "opencode".into()],
    );
    let mut codex_id = String::new();
    for (agent, _, family, _) in crate::agents::PRODUCTS.iter().copied() {
        let snapshot = sessions.memory(agent, None, &HashSet::new()).unwrap();
        assert!(!snapshot.files.is_empty());
        assert!(snapshot.selected.is_none());
        let id = &snapshot.files[0].id;
        let preview = sessions
            .memory(agent, Some(id), &HashSet::new())
            .unwrap()
            .selected
            .unwrap();
        if family == "codex" {
            assert!(preview.text.contains("已隐藏凭据"));
            if codex_id.is_empty() {
                codex_id = id.clone();
            } else {
                assert_eq!(codex_id, *id);
            }
            assert_eq!(snapshot.shared_agents.len(), 2);
        }
    }
    assert_eq!(
        fs::read_to_string(home.path().join(".codex/memories/MEMORY.md")).unwrap(),
        "api_key=abcdefghijklmno\nnotes"
    );
    assert!(
        sessions
            .memory("codex-cli", Some("../../etc/passwd"), &HashSet::new())
            .is_err()
    );
    sessions.set_enabled("codex", false).unwrap();
    assert_eq!(
        sessions
            .memory("codex-desktop", None, &HashSet::new())
            .unwrap_err()
            .code,
        "unauthorized"
    );
}
#[test]
fn memory_does_not_follow_symlinks_and_reports_preview_and_index_limits() {
    use std::os::unix::fs::symlink;
    let home = tempfile::tempdir().unwrap();
    let root = home.path().join(".codex/memories");
    fs::create_dir_all(&root).unwrap();
    let outside = home.path().join("outside.md");
    fs::write(&outside, "private").unwrap();
    symlink(&outside, root.join("linked.md")).unwrap();
    for n in 0..510 {
        fs::write(root.join(format!("{n:04}.md")), "x".repeat(70000)).unwrap();
    }
    let sessions = NativeSessions::new(home.path().canonicalize().unwrap(), &["codex".into()]);
    let snapshot = sessions.memory("codex-cli", None, &HashSet::new()).unwrap();
    assert!(snapshot.limited);
    assert_eq!(snapshot.files.len(), 500);
    assert!(!snapshot.files.iter().any(|f| f.name == "linked.md"));
    let preview = sessions
        .memory("codex-cli", Some(&snapshot.files[0].id), &HashSet::new())
        .unwrap()
        .selected
        .unwrap();
    assert!(preview.truncated);
    assert_eq!(preview.text.len(), 16384);
}

#[test]
fn memory_previews_reject_hardlinks_and_nonfiles() {
    let home = tempfile::tempdir().unwrap();
    let root = home.path().join(".codex/memories");
    fs::create_dir_all(&root).unwrap();
    let ordinary = root.join("ordinary.md");
    fs::write(&ordinary, "read only").unwrap();
    let sessions = NativeSessions::new(home.path().canonicalize().unwrap(), &["codex".into()]);
    let snapshot = sessions.memory("codex-cli", None, &HashSet::new()).unwrap();
    let id = &snapshot.files[0].id;
    fs::hard_link(&ordinary, root.join("hard.md")).unwrap();
    assert_eq!(
        sessions
            .memory("codex-cli", Some(id), &HashSet::new())
            .unwrap_err()
            .code,
        "unauthorized"
    );
    fs::create_dir(root.join("folder.md")).unwrap();
    assert!(
        !sessions
            .memory("codex-cli", None, &HashSet::new())
            .unwrap()
            .files
            .iter()
            .any(|f| f.name == "folder.md")
    );
}
