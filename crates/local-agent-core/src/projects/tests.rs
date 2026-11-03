use super::*;
use serde_json::json;
use std::{fs, process::Command};
use tempfile::tempdir;

#[test]
fn project_scope_excludes_system_temporary_and_own_verification_but_keeps_real_workspaces() {
    assert_eq!(
        project_kind(Path::new(
            "/private/var/folders/ab/opaque/T/bingo-mainline/files/project"
        )),
        "temporary"
    );
    assert_eq!(
        project_kind(Path::new("/var/folders/ab/opaque/T/pages % # 中文")),
        "temporary"
    );
    assert_eq!(
        project_kind(Path::new("/Users/fixture/projects/bingo-probe")),
        "workspace"
    );
    assert_eq!(
        project_kind(Path::new("/Users/fixture/projects/project")),
        "workspace"
    );
    let tmp = tempdir().unwrap();
    let home = tmp.path().join("home");
    let a = session(
        &home,
        Path::new("/Users/fixture/projects/work"),
        "codex",
        "work",
        None,
    );
    let b = session(&home, &tmp.path().join("probe"), "codex", "temp", None);
    let c = session(
        &home,
        Path::new(
            "/Users/fixture/Library/Application Support/app.localagent.desktop/programming-checks/check",
        ),
        "codex",
        "check",
        None,
    );
    let id = b.project_id.clone().unwrap();
    let catalog = ProjectCatalog {
        sessions: vec![a, b, c],
        calls: vec![].into(),
        writes: vec![],
        revision: 1,
        as_of: now(),
        coverage: "native_history".into(),
    };
    let q = |scope: &str| ProjectQuery {
        scope: Some(scope.into()),
        ..Default::default()
    };
    let snapshot = Default::default();
    assert_eq!(query(&catalog, q("workspace"), &snapshot).unwrap().total, 1);
    assert_eq!(
        query(&catalog, q("workspace"), &snapshot)
            .unwrap()
            .summary
            .session_count,
        1
    );
    assert_eq!(query(&catalog, q("temporary"), &snapshot).unwrap().total, 2);
    assert_eq!(query(&catalog, q("all"), &snapshot).unwrap().total, 3);
    assert_eq!(
        detail(&catalog, &id, q("workspace"), &snapshot)
            .unwrap()
            .project
            .identity
            .project_kind,
        "temporary"
    );
    assert_eq!(catalog.sessions.len(), 3);
    assert!(validate(&q("invalid")).is_err());
}

fn model_call(
    session: &SessionSummary,
    id: &str,
    product: Option<&str>,
    tokens: Option<u64>,
) -> CallSummary {
    CallSummary {
        response_id: None,
        native_agent_product_id: product.map(str::to_owned),
        agent_attribution: product.map(|_| "native_record"),
        agent_product_id: product.map(str::to_owned),
        id: id.into(),
        session_id: session.id.clone(),
        session_title: session.title.clone(),
        agent_id: session.agent_id.clone(),
        turn_id: "turn".into(),
        kind: "model".into(),
        name: "response".into(),
        status: "returned".into(),
        timestamp: session.created_at.clone(),
        model: Some("fixture".into()),
        provider: None,
        model_response_id: None,
        usage: Some(Usage {
            total_tokens: tokens,
            ..Default::default()
        }),
        server: None,
        tool_name: None,
        duration_ms: None,
        tool_source: None,
        failure: None,
    }
}

#[test]
fn hidden_projects_filter_catalogs_and_aliases_without_mutating_sessions_or_usage() {
    let tmp = tempdir().unwrap();
    let home = tmp.path().join("home");
    let a = session(&home, &tmp.path().join("a/same"), "codex", "a", None);
    let b = session(&home, &tmp.path().join("b/same"), "codex", "b", None);
    let mut a = a;
    a.project_path = Some(tmp.path().join("a/same/subdir").to_string_lossy().into());
    let mut identity = session_identity(&a).unwrap();
    identity.name = "same".into();
    let hidden: visibility::HiddenProject = identity.into();
    let catalog = ProjectCatalog {
        sessions: vec![a.clone(), b.clone()],
        calls: vec![
            model_call(&a, "a-call", None, Some(15)),
            model_call(&b, "b-call", None, Some(20)),
        ]
        .into(),
        writes: vec![],
        revision: 1,
        as_of: now(),
        coverage: "native_history".into(),
    };
    let paths = visibility::hidden_paths(&catalog, std::slice::from_ref(&hidden));
    assert!(paths.contains(a.project_path.as_ref().unwrap()));
    let visible = visibility::visible_catalog(catalog.clone(), std::slice::from_ref(&hidden));
    assert_eq!(visible.sessions.len(), 1);
    assert_eq!(visible.sessions[0].id, b.id);
    assert_eq!(
        query(&visible, Default::default(), &Default::default())
            .unwrap()
            .summary
            .usage
            .total_tokens,
        Some(20)
    );
    assert_eq!(catalog.sessions.len(), 2);
    assert_eq!(
        query(&catalog, Default::default(), &Default::default())
            .unwrap()
            .summary
            .usage
            .total_tokens,
        Some(35)
    );
    assert_ne!(
        visible.revision,
        visibility::visible_catalog(catalog.clone(), &[]).revision
    );
    assert_eq!(
        visibility::ensure_visible(&hidden.id, std::slice::from_ref(&hidden))
            .unwrap_err()
            .code,
        "project_hidden"
    );
    visibility::mark_session(&mut a, std::slice::from_ref(&hidden));
    assert!(a.project_hidden);
    visibility::mark_session(&mut a, &[]);
    assert!(!a.project_hidden);
}

#[test]
fn project_summary_reselection_keeps_duplicate_priority_after_filters_and_missing_projects() {
    let tmp = tempdir().unwrap();
    let omit = session(tmp.path(), &tmp.path().join("omit"), "codex", "omit", None);
    let keep = session(tmp.path(), &tmp.path().join("keep"), "codex", "keep", None);
    let mut catalog = ProjectCatalog {
        sessions: vec![omit.clone(), keep.clone()],
        calls: vec![
            model_call(&omit, "duplicate", Some("codex-cli"), Some(7)),
            model_call(&keep, "duplicate", Some("codex-cli"), Some(11)),
            model_call(&keep, "missing", None, None),
        ]
        .into(),
        writes: vec![],
        revision: 1,
        as_of: now(),
        coverage: "native_history".into(),
    };
    let snapshot = Default::default();
    let all = query(&catalog, Default::default(), &snapshot).unwrap();
    assert_eq!(all.summary.model_calls, 2);
    assert_eq!(all.summary.usage.total_tokens, Some(7));
    assert_eq!(all.summary.usage_missing_calls, 1);
    let filtered = query(
        &catalog,
        ProjectQuery {
            query: Some("keep".into()),
            ..Default::default()
        },
        &snapshot,
    )
    .unwrap();
    assert_eq!(filtered.summary.model_calls, 2);
    assert_eq!(filtered.summary.usage.total_tokens, Some(11));
    // Preserve the established per-project view's global first-ID selection.
    assert_eq!(filtered.items[0].totals.model_calls, 1);
    assert_eq!(filtered.items[0].totals.usage_missing_calls, 1);
    catalog.sessions[0].project_root = None;
    catalog.sessions[0].project_path = None;
    let assigned = query(&catalog, Default::default(), &snapshot).unwrap();
    assert_eq!(assigned.unassigned_sessions, 1);
    assert_eq!(assigned.summary.usage.total_tokens, Some(11));
    assert_eq!(assigned.summary.model_calls, 2);
    // Unique response sums still saturate, while duplicate evidence stays ignored.
    catalog.calls.chunks.push(std::sync::Arc::new(vec![
        model_call(&keep, "large", Some("codex-cli"), Some(u64::MAX)),
        model_call(&keep, "after-large", Some("codex-cli"), Some(1)),
    ]));
    let saturated = query(&catalog, Default::default(), &snapshot).unwrap();
    assert_eq!(saturated.summary.usage.total_tokens, Some(u64::MAX));
    assert_eq!(saturated.summary.model_calls, 4);
}

#[test]
fn shared_project_clients_deduplicate_usage_and_never_allocate_creator_totals() {
    let tmp = tempdir().unwrap();
    let root = tmp.path().join("repo");
    fs::create_dir_all(&root).unwrap();
    let mut shared = session(tmp.path(), &root, "codex", "shared", Some(999));
    shared.agent_products = vec!["codex-cli".into(), "codex-desktop".into()];
    shared.agent_product_id = None;
    shared.has_unknown_client = true;
    let a = model_call(&shared, "a", Some("codex-cli"), Some(10));
    let b = model_call(&shared, "b", Some("codex-desktop"), Some(20));
    let c = model_call(&shared, "c", None, Some(30));
    let missing = model_call(&shared, "missing", None, None);
    let catalog = ProjectCatalog {
        sessions: vec![shared.clone(), shared.clone()],
        writes: vec![],
        calls: vec![a.clone(), a, b, c, missing].into(),
        revision: 1,
        as_of: now(),
        coverage: "native_history".into(),
    };
    let snapshot = listeners::ListenerSnapshot::default();
    let all = query(&catalog, Default::default(), &snapshot).unwrap();
    assert_eq!(all.summary.session_count, 1);
    assert_eq!(all.summary.disk_bytes, shared.disk_bytes);
    assert_eq!(all.summary.usage.total_tokens, Some(60));
    assert_eq!(all.summary.model_calls, 4);
    assert_eq!(all.summary.usage_missing_calls, 1);
    assert_eq!(
        all.items[0]
            .agents
            .iter()
            .filter_map(|a| a.totals.usage.total_tokens)
            .sum::<u64>(),
        60
    );
    for (agent, tokens, calls) in [
        ("codex-cli", 10, 1),
        ("codex-desktop", 20, 1),
        ("unknown", 30, 2),
    ] {
        let filter = ProjectQuery {
            agent: Some(agent.into()),
            ..Default::default()
        };
        let page = query(&catalog, filter.clone(), &snapshot).unwrap();
        assert_eq!(page.summary.session_count, 1);
        assert_eq!(page.summary.usage.total_tokens, Some(tokens));
        assert_eq!(page.summary.model_calls, calls);
        let detail = detail(&catalog, &all.items[0].identity.id, filter, &snapshot).unwrap();
        assert_eq!(detail.sessions.len(), 1);
        assert_eq!(detail.project.totals.usage.total_tokens, Some(tokens));
    }
    let filter = ProjectQuery {
        agent: Some("claude-code-cli".into()),
        ..Default::default()
    };
    assert_eq!(query(&catalog, filter.clone(), &snapshot).unwrap().total, 0);
    assert_eq!(
        detail(&catalog, &all.items[0].identity.id, filter, &snapshot)
            .unwrap()
            .project
            .totals
            .model_calls,
        0
    );
    let wrong_cursor = ProjectQuery {
        agent: Some("codex-cli".into()),
        cursor: Some(format!(
            "{}:40",
            hash(&json!([1, null, null, null]).to_string())
        )),
        ..Default::default()
    };
    assert_eq!(
        query(&catalog, wrong_cursor, &snapshot).err().unwrap().code,
        "stale_snapshot"
    );
}

fn git_at(root: &Path, args: &[&str]) {
    let result = Command::new("/usr/bin/git")
        .args(args)
        .current_dir(root)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(result.status.success(), "fixture git failed");
}
fn repo(root: &Path) {
    fs::create_dir_all(root).unwrap();
    git_at(root, &["init", "-q"]);
    fs::write(root.join("file.txt"), "before\n").unwrap();
    git_at(root, &["add", "file.txt"]);
    git_at(
        root,
        &[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "-qm",
            "initial fixture",
        ],
    );
}
fn session(
    home: &Path,
    project: &Path,
    agent: &str,
    id: &str,
    usage: Option<u64>,
) -> SessionSummary {
    let source = home.join(if agent == "claude" {
        ".claude/projects/p"
    } else {
        ".codex/sessions/2026/10/18"
    });
    fs::create_dir_all(&source).unwrap();
    let row = if agent == "claude" {
        json!({"type":"user","sessionId":id,"uuid":id,"cwd":project,"timestamp":Utc::now().to_rfc3339(),"message":{"content":"fixture title"}})
    } else {
        json!({"type":"session_meta","payload":{"id":id,"cwd":project},"timestamp":Utc::now().to_rfc3339()})
    };
    fs::write(source.join(format!("{id}.jsonl")), format!("{row}\n")).unwrap();
    let native = crate::sessions::NativeSessions::new(home.to_path_buf(), &[agent.into()]);
    native.refresh().unwrap();
    let mut summary = native
        .project_catalog()
        .unwrap()
        .sessions
        .into_iter()
        .find(|s| s.id == format!("{agent}:{id}"))
        .unwrap();
    summary.usage = Usage {
        input_tokens: usage,
        output_tokens: usage,
        total_tokens: usage.map(|n| n * 2),
        ..Default::default()
    };
    summary.usage_quality = if usage.is_some() {
        "native_cumulative"
    } else {
        "unknown"
    };
    summary
}
#[test]
fn canonical_projects_worktrees_cross_agent_unknown_usage_and_calendar_are_distinct() {
    let tmp = tempdir().unwrap();
    let root = tmp.path().join("a/same");
    repo(&root);
    let nested = root.join("nested");
    fs::create_dir(&nested).unwrap();
    let other = tmp.path().join("b/same");
    fs::create_dir_all(&other).unwrap();
    let worktree = tmp.path().join("worktree");
    git_at(
        &root,
        &[
            "worktree",
            "add",
            "-qb",
            "fixture-worktree",
            worktree.to_str().unwrap(),
        ],
    );
    assert_eq!(
        identity(root.to_str().unwrap()).unwrap().id,
        identity(nested.to_str().unwrap()).unwrap().id
    );
    assert_ne!(
        identity(root.to_str().unwrap()).unwrap().id,
        identity(worktree.to_str().unwrap()).unwrap().id
    );
    assert_ne!(
        identity(root.to_str().unwrap()).unwrap().id,
        identity(other.to_str().unwrap()).unwrap().id
    );
    let home = tmp.path().join("home");
    let a = session(&home, &root, "claude", "c", Some(10));
    let b = session(&home, &nested, "codex", "x", None);
    let call = model_call(&a, "response", None, Some(20));
    let catalog = ProjectCatalog {
        sessions: vec![a, b],
        writes: vec![],
        calls: vec![call].into(),
        revision: 1,
        as_of: now(),
        coverage: "native_history".into(),
    };
    let page = query(
        &catalog,
        ProjectQuery::default(),
        &listeners::ListenerSnapshot {
            state: "ready".into(),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].totals.session_count, 2);
    assert_eq!(page.summary.usage.total_tokens, Some(20));
    assert_eq!(page.summary.usage_unknown_sessions, 1);
    assert_eq!(page.items[0].agents.len(), 2);
    assert_eq!(
        page.items[0]
            .agents
            .iter()
            .filter_map(|a| a.totals.usage.total_tokens)
            .sum::<u64>(),
        20
    );
    assert!(in_range(Some(&Utc::now().to_rfc3339()), Some(1)));
    assert!(!in_range(Some("2020-01-01T00:00:00Z"), Some(7)));
    assert_eq!(aggregate(&[], &[]).disk_allocated_bytes, Some(0));
    let mut partial = Usage {
        input_tokens: Some(10),
        ..Default::default()
    };
    partial.add(&Usage {
        output_tokens: Some(20),
        ..Default::default()
    });
    assert_eq!(partial.total_tokens, None);
    let main = git::snapshot(&identity(root.to_str().unwrap()).unwrap());
    let branch = git::snapshot(&identity(worktree.to_str().unwrap()).unwrap());
    assert_eq!(main.repository_id, branch.repository_id);
    assert_ne!(main.worktree_id, branch.worktree_id);
    fs::rename(&root, tmp.path().join("moved-repo")).unwrap();
    let historical = query(
        &catalog,
        ProjectQuery::default(),
        &listeners::ListenerSnapshot::default(),
    )
    .unwrap();
    assert_eq!(historical.items.len(), 1);
    assert_eq!(historical.items[0].identity.id, page.items[0].identity.id);
    assert_eq!(historical.items[0].identity.location_state, "unavailable");
}

#[test]
fn codex_structured_and_patch_success_are_deduplicated_and_anchor_to_native_calls() {
    let tmp = tempdir().unwrap();
    let root = tmp.path().join("project");
    fs::create_dir(&root).unwrap();
    let source = tmp.path().join("home/.codex/sessions/2026/10/18");
    fs::create_dir_all(&source).unwrap();
    let rows = [
        json!({"type":"session_meta","timestamp":"2026-10-18T00:00:00Z","payload":{"id":"codex-writes","cwd":root,"originator":"codex_work_desktop"}}),
        json!({"type":"event_msg","originator":"codex_cli_rs","timestamp":"2026-10-18T00:00:01Z","payload":{"type":"item_completed","item":{"type":"FileChange","id":"native-change","status":"completed","changes":[{"path":"x.txt","kind":"update"}]}}}),
        json!({"type":"response_item","timestamp":"2026-10-18T00:00:02Z","payload":{"type":"custom_tool_call","id":"p","call_id":"patch","name":"apply_patch","input":"*** Begin Patch\n*** Update File: x.txt\n@@\n-old\n+new\n*** End Patch"}}),
        json!({"type":"response_item","timestamp":"2026-10-18T00:00:03Z","payload":{"type":"custom_tool_call_output","id":"r","call_id":"patch","output":"{\"output\":\"Success. Updated the following files\",\"metadata\":{\"exit_code\":0}}"}}),
    ];
    fs::write(
        source.join("s.jsonl"),
        rows.iter().map(|v| format!("{v}\n")).collect::<String>(),
    )
    .unwrap();
    let native = crate::sessions::NativeSessions::new(tmp.path().join("home"), &["codex".into()]);
    native.refresh().unwrap();
    let catalog = native.project_catalog().unwrap();
    assert_eq!(catalog.writes.len(), 2);
    assert_eq!(
        catalog
            .writes
            .iter()
            .filter(|w| w.agent_product_id.as_deref() == Some("codex-cli"))
            .count(),
        1
    );
    assert_eq!(
        catalog
            .writes
            .iter()
            .filter(|w| w.agent_product_id.is_none())
            .count(),
        1
    );
    let scoped = detail(
        &catalog,
        &catalog.writes[0].project_id,
        ProjectQuery {
            agent: Some("codex-cli".into()),
            ..Default::default()
        },
        &Default::default(),
    )
    .unwrap();
    assert_eq!(scoped.writes.len(), 1);
    assert_eq!(scoped.project.totals.write_count, 1);
    let unknown = detail(
        &catalog,
        &catalog.writes[0].project_id,
        ProjectQuery {
            agent: Some("unknown".into()),
            ..Default::default()
        },
        &Default::default(),
    )
    .unwrap();
    assert_eq!(unknown.writes.len(), 1);
    assert_eq!(
        aggregate(&catalog.sessions, &catalog.writes).distinct_files,
        1
    );
    for write in &catalog.writes {
        assert!(
            native
                .conversation(&write.session_id, None, Some(&write.id))
                .unwrap()
                .items
                .iter()
                .any(|e| e.id == write.id)
        );
    }
    assert!(!writes::explicit_success(
        "apply_patch",
        &json!("{\"output\":\"Success then failed\",\"metadata\":{\"exit_code\":1}}"),
        false
    ));
}
#[test]
fn git_snapshots_are_read_only_handle_renames_secrets_binary_and_missing_directories() {
    let tmp = tempdir().unwrap();
    let root = tmp.path().join("repo");
    repo(&root);
    let project = identity(root.to_str().unwrap()).unwrap();
    fs::write(root.join("file.txt"), "after\n").unwrap();
    fs::write(root.join(".env"), "TOKEN=secret-value\n").unwrap();
    let before = fs::read(root.join(".git/index")).unwrap();
    let snapshot = git::snapshot(&project);
    assert_eq!(snapshot.state, "ready");
    assert_eq!(snapshot.commits.len(), 1);
    assert_eq!(fs::read(root.join(".git/index")).unwrap(), before);
    let tracked = snapshot
        .files
        .iter()
        .find(|f| f.path == "file.txt")
        .unwrap();
    assert!(
        git::diff(&project, &tracked.id)
            .text
            .unwrap()
            .contains("+after")
    );
    let env = snapshot.files.iter().find(|f| f.path == ".env").unwrap();
    assert!(git::diff(&project, &env.id).text.is_none());
    git_at(&root, &["mv", "file.txt", "renamed file.txt"]);
    let snapshot = git::snapshot(&project);
    assert!(
        snapshot
            .files
            .iter()
            .any(|f| f.previous_path.as_deref() == Some("file.txt"))
    );
    fs::write(root.join("renamed file.txt"), b"binary\0value").unwrap();
    assert_eq!(
        git::diff(
            &project,
            &snapshot
                .files
                .iter()
                .find(|f| f.path == "renamed file.txt")
                .unwrap()
                .id
        )
        .state,
        "binary"
    );
    let missing = identity(tmp.path().join("missing").to_str().unwrap()).unwrap();
    assert_eq!(git::snapshot(&missing).state, "unavailable");
}
#[test]
fn successful_write_evidence_excludes_proposals_failures_and_keeps_repeated_writes() {
    let tmp = tempdir().unwrap();
    let root = tmp.path().join("project");
    fs::create_dir(&root).unwrap();
    let home = tmp.path().join("home");
    let source = home.join(".claude/projects/p");
    fs::create_dir_all(&source).unwrap();
    let rows = [
        json!({"type":"user","sessionId":"write-session","uuid":"u","cwd":root,"timestamp":"2026-10-18T00:00:00Z","message":{"content":"fixture"}}),
        json!({"type":"assistant","sessionId":"write-session","uuid":"a","timestamp":"2026-10-18T00:00:01Z","message":{"id":"m","content":[{"type":"tool_use","id":"1","name":"Write","input":{"file_path":"x.txt","content":"one"}},{"type":"tool_use","id":"2","name":"Edit","input":{"file_path":"x.txt"}},{"type":"tool_use","id":"3","name":"Write","input":{"file_path":"bad.txt"}},{"type":"tool_use","id":"4","name":"Write","input":{"file_path":"unexecuted.txt"}}]}}),
        json!({"type":"user","sessionId":"write-session","uuid":"r","timestamp":"2026-10-18T00:00:02Z","message":{"content":[{"type":"tool_result","tool_use_id":"1","is_error":false,"content":"written"},{"type":"tool_result","tool_use_id":"2","is_error":false,"content":"edited"},{"type":"tool_result","tool_use_id":"3","is_error":true,"content":"failed"}]}}),
    ];
    fs::write(
        source.join("s.jsonl"),
        rows.iter().map(|v| format!("{v}\n")).collect::<String>(),
    )
    .unwrap();
    let native = crate::sessions::NativeSessions::new(home, &["claude".into()]);
    native.refresh().unwrap();
    let catalog = native.project_catalog().unwrap();
    assert_eq!(catalog.writes.len(), 2);
    assert!(
        catalog
            .writes
            .iter()
            .all(|w| w.files[0].path.ends_with("x.txt"))
    );
    let totals = aggregate(&catalog.sessions, &catalog.writes);
    assert_eq!(totals.write_count, 2);
    assert_eq!(totals.distinct_files, 1);
    for write in &catalog.writes {
        let conversation = native
            .conversation(&write.session_id, None, Some(&write.id))
            .unwrap();
        assert!(
            conversation
                .items
                .iter()
                .any(|e| e.id == write.id && e.status == "returned")
        );
    }
    let parsed = writes::proposed(
        "functions.apply_patch",
        &json!("*** Begin Patch\n*** Update File: x.txt\n@@\n-old\n+new\n*** End Patch"),
        root.to_str().unwrap(),
    );
    assert_eq!(parsed[0].added, Some(1));
    assert_eq!(parsed[0].removed, Some(1));
    native.set_enabled("claude", false).unwrap();
    assert!(native.project_catalog().unwrap().sessions.is_empty());
}
