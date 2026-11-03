use super::*;
use crate::domain as d;
use serde_json::{Value, json};
use std::{
    io::Write,
    sync::atomic::{AtomicBool, Ordering},
};

fn fixture(home: &Path, messages: usize) -> PathBuf {
    let root = home.join(".claude/projects/project");
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("fixture.jsonl");
    let mut out = std::fs::File::create(&path).unwrap();
    for n in 0..messages {
        let row = if n % 2 == 0 {
            json!({"type":"user","sessionId":"fixture","uuid":format!("u{n}"),"promptId":format!("t{n}"),"cwd":"/projects/domain-fixture","entrypoint":"cli","timestamp":"2026-10-30T10:00:00Z","message":{"role":"user","content":format!("body {n}")}})
        } else {
            json!({"type":"assistant","sessionId":"fixture","uuid":format!("a{n}"),"timestamp":"2026-10-30T10:00:01Z","message":{"id":format!("r{n}"),"model":"fixture","content":[{"type":"text","text":format!("body {n}")}],"usage":{"input_tokens":10,"output_tokens":3,"cache_read_input_tokens":2}}})
        };
        writeln!(out, "{row}").unwrap();
    }
    path
}
fn stable<T: serde::Serialize>(value: T) -> Value {
    let mut value = serde_json::to_value(value).unwrap();
    fn visit(v: &mut Value) {
        match v {
            Value::Object(map) => {
                map.remove("asOf");
                map.remove("lastScan");
                map.remove("lastSuccessfulScan");
                for v in map.values_mut() {
                    visit(v);
                }
            }
            Value::Array(items) => {
                for v in items {
                    visit(v);
                }
            }
            _ => {}
        }
    }
    visit(&mut value);
    value
}
fn legacy(home: &Path) -> NativeSessions {
    let sources: Vec<Arc<dyn ReadOnlySessionSource>> = vec![
        Arc::new(super::super::jsonl::JsonlSource("claude")),
        Arc::new(super::super::jsonl::JsonlSource("codex")),
        Arc::new(super::super::opencode::OpenCodeSource),
        Arc::new(super::super::antigravity::AntigravitySource),
    ];
    NativeSessions::with_source_plugins(
        home.into(),
        &["claude".into()],
        NativeSourcePlugins::mount(sources).unwrap(),
    )
}

fn assert_native_domain_equivalence(home: &Path, source: &str) {
    let before = legacy(home);
    let after = NativeSessions::new(home.into(), &["claude".into()]);
    for native in [&before, &after] {
        native.set_enabled(source, true).unwrap();
        native.refresh().unwrap();
    }
    let query = super::super::SessionQuery {
        agent: Some(source.into()),
        ..Default::default()
    };
    let original = before.query(query.clone()).unwrap();
    assert!(original.total > 0);
    assert_eq!(stable(&original), stable(after.query(query).unwrap()));
    let calls = super::super::CallQuery {
        agent: Some(source.into()),
        ..Default::default()
    };
    assert_eq!(
        stable(before.calls(calls.clone()).unwrap()),
        stable(after.calls(calls).unwrap())
    );
    let (items, summary) = after
        .domain_sessions_for_source(source, &d::SessionFilter::default(), None, None, &[])
        .unwrap();
    assert_eq!(summary.sessions.value(), original.total as u64);
    assert_eq!(items.len(), original.items.len());
    for item in items {
        let old = original
            .items
            .iter()
            .find(|s| s.id == item.record.id)
            .unwrap();
        assert_eq!(item.record, super::super::domain_query::record(old));
        assert_eq!(
            stable(before.conversation(&old.id, None, None).unwrap()),
            stable(after.conversation(&old.id, None, None).unwrap())
        );
    }
    after.set_enabled(source, false).unwrap();
    assert!(
        after
            .domain_sessions_for_source(source, &d::SessionFilter::default(), None, None, &[])
            .unwrap()
            .0
            .is_empty()
    );
}

#[test]
fn opencode_domain_adapter_keeps_sqlite_legacy_sessions_calls_and_bodies() {
    let home = tempfile::tempdir().unwrap();
    let db = super::super::opencode_tests::fixture(home.path());
    super::super::opencode_tests::message(&db, "user", 1000, json!({"role":"user"}));
    super::super::opencode_tests::part(
        &db,
        "input",
        "user",
        1000,
        json!({"type":"text","text":"domain source input"}),
    );
    super::super::opencode_tests::message(
        &db,
        "assistant",
        2000,
        json!({"role":"assistant","parentID":"user","providerID":"fixture","modelID":"fixture","tokens":{"input":11,"output":4,"cache":{"read":2,"write":3}},"finish":"stop"}),
    );
    super::super::opencode_tests::part(
        &db,
        "output",
        "assistant",
        2000,
        json!({"type":"text","text":"domain source output"}),
    );
    assert_native_domain_equivalence(home.path(), "opencode");
}

#[test]
fn antigravity_domain_adapter_keeps_protobuf_legacy_sessions_calls_and_bodies() {
    let fixture = super::super::antigravity_tests::Fixture::new();
    assert_native_domain_equivalence(&fixture.home, "antigravity");
}

#[test]
fn codex_domain_adapter_keeps_shared_clients_unknown_attribution_and_complete_legacy_results() {
    let home = tempfile::tempdir().unwrap();
    let root = home.path().join(".codex/sessions/2026/10/31");
    std::fs::create_dir_all(&root).unwrap();
    let mut rows = vec![];
    for (n, originator, input) in [
        (1, "codex_cli_rs", 10),
        (2, "codex_work_desktop", 20),
        (3, "unknown-client", 30),
    ] {
        rows.push(json!({"type":"session_meta","timestamp":"2026-10-31T00:00:00Z","payload":{"id":"domain-shared","originator":originator,"source":"vscode"}}));
        rows.push(json!({"type":"turn_context","timestamp":"2026-10-31T00:00:00Z","payload":{"turn_id":format!("t{n}"),"model":"fixture"}}));
        rows.push(json!({"type":"token_usage_record","originator":originator,"timestamp":"2026-10-31T00:00:00Z","payload":{"response_id":format!("r{n}"),"usage":{"input_tokens":input,"output_tokens":1},"thread_token_usage":{"input_tokens":999,"output_tokens":1}}}));
    }
    let bytes = rows.iter().map(|r| format!("{r}\n")).collect::<String>();
    for name in ["original", "copy"] {
        std::fs::write(root.join(format!("{name}.jsonl")), &bytes).unwrap();
    }
    let before = legacy(home.path());
    before.set_enabled("codex", true).unwrap();
    let after = NativeSessions::new(home.path().into(), &["claude".into()]);
    after.set_enabled("codex", true).unwrap();
    before.refresh().unwrap();
    after.refresh().unwrap();
    for agent in [
        "codex",
        "codex-cli",
        "codex-desktop",
        "codex-unknown",
        "unknown",
    ] {
        let query = super::super::SessionQuery {
            agent: Some(agent.into()),
            ..Default::default()
        };
        assert_eq!(
            stable(before.query(query.clone()).unwrap()),
            stable(after.query(query).unwrap())
        );
    }
    assert_eq!(
        stable(before.calls(Default::default()).unwrap()),
        stable(after.calls(Default::default()).unwrap())
    );
    let query = super::super::SessionQuery {
        agent: Some("codex".into()),
        ..Default::default()
    };
    let original = before.query(query).unwrap();
    assert_eq!(original.total, 1);
    let (items, summary) = after
        .domain_sessions_for_source("codex", &d::SessionFilter::default(), None, None, &[])
        .unwrap();
    assert_eq!(summary.sessions.value(), 1);
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].record.id, original.items[0].id);
    assert_eq!(items[0].record.product_ids, ["codex-cli", "codex-desktop"]);
    assert!(items[0].record.unknown_client);
    assert_eq!(
        stable(
            before
                .conversation(&original.items[0].id, None, None)
                .unwrap()
        ),
        stable(
            after
                .conversation(&original.items[0].id, None, None)
                .unwrap()
        )
    );
    after.set_enabled("codex", false).unwrap();
    assert_eq!(
        after
            .domain_sessions_for_source("codex", &d::SessionFilter::default(), None, None, &[])
            .unwrap()
            .0
            .len(),
        0
    );
    assert!(
        after
            .domain_conversation(&original.items[0].id, None, None)
            .is_err()
    );
}
#[test]
fn domain_pilot_preserves_legacy_sessions_statistics_cursors_and_full_conversation() {
    let home = tempfile::tempdir().unwrap();
    let path = fixture(home.path(), 96);
    let before = legacy(home.path());
    let after = NativeSessions::new(home.path().into(), &["claude".into()]);
    assert!(
        !before
            .domain_catalog()
            .unwrap()
            .sources
            .iter()
            .any(|s| s.id == "claude")
    );
    before.refresh().unwrap();
    after.refresh().unwrap();
    for agent in [
        None,
        Some("claude"),
        Some("claude-code-cli"),
        Some("claude-code-desktop"),
        Some("unknown"),
    ] {
        let query = super::super::SessionQuery {
            agent: agent.map(Into::into),
            ..Default::default()
        };
        assert_eq!(
            stable(before.query(query.clone()).unwrap()),
            stable(after.query(query).unwrap())
        );
    }
    assert_eq!(
        stable(before.calls(Default::default()).unwrap()),
        stable(after.calls(Default::default()).unwrap())
    );
    let id = before.query(Default::default()).unwrap().items[0]
        .id
        .clone();
    let mut cursor = None;
    loop {
        let old = before.conversation(&id, cursor.as_deref(), None).unwrap();
        let new = after.conversation(&id, cursor.as_deref(), None).unwrap();
        assert_eq!(stable(&old), stable(new));
        cursor = old.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    let cursor = before
        .conversation(&id, None, None)
        .unwrap()
        .next_cursor
        .unwrap();
    let old_body = std::fs::read_to_string(&path).unwrap();
    std::fs::write(&path, old_body.replace("body", "edit")).unwrap();
    for native in [&before, &after] {
        assert_eq!(
            native
                .conversation(&id, Some(&cursor), None)
                .unwrap_err()
                .code,
            "stale_snapshot"
        );
        native.set_enabled("claude", false).unwrap();
        assert_eq!(native.query(Default::default()).unwrap().total, 0);
        assert!(native.conversation(&id, None, None).is_err());
    }
}
#[test]
fn partial_unread_and_revoked_catalog_states_do_not_claim_current_data() {
    let home = tempfile::tempdir().unwrap();
    let native = NativeSessions::new(home.path().into(), &[]);
    let initial = native.domain_catalog().unwrap();
    assert_eq!(initial.providers.len(), 4);
    assert_eq!(initial.products.len(), 6);
    assert_eq!(
        initial
            .sources
            .iter()
            .find(|s| s.id == "claude")
            .unwrap()
            .status
            .freshness,
        d::Freshness::Unread
    );
    native.set_enabled("claude", true).unwrap();
    native.refresh().unwrap();
    let failed = native.domain_catalog().unwrap();
    assert_eq!(
        failed
            .sources
            .iter()
            .find(|s| s.id == "claude")
            .unwrap()
            .status
            .freshness,
        d::Freshness::Failed
    );
    let raw = serde_json::to_string(&failed).unwrap();
    assert!(!raw.contains(home.path().to_str().unwrap()));
    assert!(!raw.contains("sourcePath"));
    native.set_enabled("claude", false).unwrap();
    let revoked = native.domain_catalog().unwrap();
    assert_eq!(
        revoked
            .sources
            .iter()
            .find(|s| s.id == "claude")
            .unwrap()
            .status
            .authorization,
        d::Authorization::Revoked
    );
    assert_eq!(
        revoked
            .sources
            .iter()
            .find(|s| s.id == "claude")
            .unwrap()
            .status
            .freshness,
        d::Freshness::Unread
    );
    native.stop_source_plugins().unwrap();
    assert!(
        !native
            .domain_catalog()
            .unwrap()
            .sources
            .iter()
            .find(|s| s.id == "claude")
            .unwrap()
            .status
            .runtime_active
    );
}
#[test]
fn pilot_does_not_return_native_artifacts_after_consent_changes_on_last_progress() {
    let home = tempfile::tempdir().unwrap();
    fixture(home.path(), 4);
    let adapter = super::super::domain_source::BuiltinSessionAdapter::claude();
    let host = PluginHost::mount(&[&adapter]).unwrap();
    let active = AtomicBool::new(true);
    let cache = std::sync::Mutex::new(super::super::parser::IndexCache::default());
    let progress = |total, scanned| {
        if total > 0 && total == scanned {
            active.store(false, Ordering::SeqCst);
        }
        Ok(())
    };
    let authorized = || Ok(active.load(Ordering::SeqCst));
    let context = SourceContext {
        home: home.path(),
        directory: crate::agents::builtin_directory(),
        index_cache: &cache,
        lease: host.acquire(&capability("claude")).unwrap(),
        grant_revision: 1,
        authorized: &authorized,
        progress: &progress,
        cancelled: &|| false,
    };
    assert_eq!(
        adapter.scan(&context, &HashMap::new()).err().unwrap().code,
        "unauthorized"
    );
}

#[test]
fn pilot_crosses_batch_boundaries_without_losing_legacy_files_or_pagination() {
    let home = tempfile::tempdir().unwrap();
    let root = home.path().join(".claude/projects/project");
    std::fs::create_dir_all(&root).unwrap();
    for n in 0..257 {
        let row = json!({"type":"user","sessionId":format!("batch-{n:03}"),"uuid":format!("u{n}"),"entrypoint":"cli","timestamp":"2026-10-30T10:00:00Z","message":{"content":format!("batch {n}")}});
        std::fs::write(root.join(format!("{n}.jsonl")), format!("{row}\n")).unwrap();
    }
    let before = legacy(home.path());
    let after = NativeSessions::new(home.path().into(), &["claude".into()]);
    before.refresh().unwrap();
    after.refresh().unwrap();
    let mut cursor = None;
    let mut ids = std::collections::HashSet::new();
    loop {
        let query = super::super::SessionQuery {
            cursor: cursor.clone(),
            ..Default::default()
        };
        let old = before.query(query.clone()).unwrap();
        let new = after.query(query).unwrap();
        assert_eq!(old.total, 257);
        assert_eq!(stable(&old), stable(new));
        for item in old.items {
            assert!(ids.insert(item.id));
        }
        cursor = old.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(ids.len(), 257);
}

#[test]
fn six_products_keep_complete_legacy_memory_metadata_preview_and_errors() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().canonicalize().unwrap();
    let content = format!(
        "api_key=fixturesecret123456789\n{}",
        "记忆内容🙂".repeat(5000)
    );
    let files = [
        ".claude/CLAUDE.md",
        ".codex/memories/MEMORY.md",
        ".config/opencode/AGENTS.md",
        ".gemini/config/GEMINI.md",
    ];
    for name in files {
        let path = home.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, &content).unwrap();
    }
    let before = legacy(&home);
    for source in ["codex", "opencode", "antigravity"] {
        before.set_enabled(source, true).unwrap();
    }
    let after = NativeSessions::new(
        home.clone(),
        &[
            "claude".into(),
            "codex".into(),
            "opencode".into(),
            "antigravity".into(),
        ],
    );
    let hidden = std::collections::HashSet::new();
    for (product, _, _, _) in crate::agents::PRODUCTS.iter().copied() {
        let original = before.memory(product, None, &hidden).unwrap();
        let ids = original
            .files
            .iter()
            .map(|f| f.id.clone())
            .collect::<Vec<_>>();
        assert!(!ids.is_empty());
        assert_eq!(
            stable(original),
            stable(after.memory(product, None, &hidden).unwrap())
        );
        for id in ids {
            let original = before.memory(product, Some(&id), &hidden).unwrap();
            let actual = after.memory(product, Some(&id), &hidden).unwrap();
            let preview = actual.selected.as_ref().unwrap();
            assert!(preview.truncated);
            assert_eq!(preview.text.chars().count(), 16384);
            assert!(!preview.text.contains("fixturesecret123456789"));
            assert_eq!(stable(original), stable(actual));
        }
        let original = before
            .memory(product, Some("missing"), &hidden)
            .unwrap_err();
        let actual = after.memory(product, Some("missing"), &hidden).unwrap_err();
        assert_eq!(
            (original.code, original.message),
            (actual.code, actual.message)
        );
    }
    for name in files {
        assert_eq!(std::fs::read_to_string(home.join(name)).unwrap(), content);
    }
    after.set_enabled("codex", false).unwrap();
    assert_eq!(
        after
            .memory("codex-desktop", None, &hidden)
            .unwrap_err()
            .code,
        "unauthorized"
    );
}
