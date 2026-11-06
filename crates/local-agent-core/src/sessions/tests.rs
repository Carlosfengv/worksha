use super::*;
use serde_json::{Value, json};
use std::io::Write;

#[test]
fn object_summaries_read_only_indexed_scalars_and_distinguish_unread_from_zero() {
    use crate::domain::{self as d, object_summary as o};
    let home = tempfile::tempdir().unwrap();
    let sessions = NativeSessions::new(home.path().into(), &["claude".into()]);
    let agent = o::ObjectSelector::Agent {
        id: "claude".into(),
    };
    let fields = [o::SummaryField::SessionCount];
    let unread = sessions.object_summary(&agent, &fields, &[]).unwrap();
    assert_eq!(unread.summary.freshness, d::Freshness::Unread);
    assert!(unread.summary.metrics[0].value.is_none());
    sessions.refresh().unwrap();
    assert!(
        sessions
            .object_summary(&agent, &fields, &[])
            .unwrap()
            .summary
            .metrics[0]
            .value
            .is_none()
    );
    fs::create_dir_all(home.path().join(".claude/projects")).unwrap();
    sessions.refresh().unwrap();
    let empty = sessions.object_summary(&agent, &fields, &[]).unwrap();
    assert_eq!(
        empty.summary.metrics[0].value,
        Some(o::SummaryValue::Count {
            value: 0_u64.into()
        })
    );
    assert_ne!(empty.read_identity, unread.read_identity);
    let file = write_rows(home.path(), "claude", "object-summary", &claude_rows());
    sessions.refresh().unwrap();
    let session = sessions.project_catalog().unwrap().sessions.remove(0);
    let selector = o::ObjectSelector::Session {
        id: session.id,
        origin: o::ObjectOrigin::Builtin {},
    };
    let fields = [
        o::SummaryField::InputTokens,
        o::SummaryField::OutputTokens,
        o::SummaryField::TotalTokens,
    ];
    let before = sessions.object_summary(&selector, &fields, &[]).unwrap();
    // Successful no-change collection updates clocks, not the retained object.
    sessions.refresh().unwrap();
    {
        let mut state = sessions.lock().unwrap();
        state.as_of = "2099-01-01T00:00:00Z".into();
        state
            .sources
            .iter_mut()
            .find(|source| source.agent_id == "claude")
            .unwrap()
            .last_successful_scan = Some("2099-01-01T00:00:00Z".into());
    }
    let refreshed = sessions.object_summary(&selector, &fields, &[]).unwrap();
    assert_eq!(refreshed.read_identity, before.read_identity);
    assert_ne!(refreshed.summary.captured_at, before.summary.captured_at);
    // A summary cannot reopen the source, even after its body becomes unreadable.
    fs::write(&file, "private new text; deliberately invalid JSON").unwrap();
    let after = sessions.object_summary(&selector, &fields, &[]).unwrap();
    assert_eq!(after, refreshed);
    let raw = serde_json::to_string(&after.summary).unwrap();
    assert!(!raw.contains("private"));
    assert!(!raw.contains(&home.path().display().to_string()));
    assert!(!raw.contains("sourcePath"));
    sessions.set_enabled("claude", false).unwrap();
    assert!(sessions.object_summary(&selector, &fields, &[]).is_err());
    sessions.set_enabled("claude", true).unwrap();
    assert_ne!(
        sessions
            .object_summary(&agent, &[o::SummaryField::SessionCount], &[])
            .unwrap()
            .read_identity,
        empty.read_identity
    );
}

#[test]
fn object_project_summary_honors_hidden_projects_and_stopped_sources() {
    use crate::domain::{self as d, object_summary as o};
    let home = tempfile::tempdir().unwrap();
    write_rows(home.path(), "claude", "object-project", &claude_rows());
    let sessions = NativeSessions::new(home.path().into(), &["claude".into()]);
    sessions.refresh().unwrap();
    let id = sessions.project_catalog().unwrap().sessions[0]
        .project_id
        .clone()
        .unwrap();
    let selector = o::ObjectSelector::Project {
        id: id.clone(),
        origin: o::ObjectOrigin::Builtin {},
    };
    let fields = [o::SummaryField::SessionCount];
    let visible = sessions.object_summary(&selector, &fields, &[]).unwrap();
    assert_eq!(
        visible.summary.metrics[0].value,
        Some(o::SummaryValue::Count {
            value: 1_u64.into()
        })
    );
    assert_eq!(
        sessions
            .object_summary(&selector, &fields, &[id])
            .unwrap_err()
            .code,
        d::ErrorCode::NotFound
    );
    sessions.stop_source_plugin("claude").unwrap();
    assert!(sessions.object_summary(&selector, &fields, &[]).is_err());
    assert_eq!(
        sessions
            .object_summary(
                &o::ObjectSelector::Agent {
                    id: "claude".into()
                },
                &fields,
                &[]
            )
            .unwrap_err()
            .code,
        d::ErrorCode::GrantRevoked
    );
}

#[test]
fn domain_invocations_keep_each_mounted_sources_actual_implementation_identity() {
    let home = tempfile::tempdir().unwrap();
    let native = NativeSessions::new(
        home.path().into(),
        &["claude".into(), "codex".into(), "opencode".into()],
    );
    let mut identities = std::collections::BTreeSet::new();
    for source in ["claude", "codex", "opencode"] {
        native
            .with_source_context(source, None, true, |plugin, context| {
                let provider = plugin.domain_provider().unwrap();
                let invocation = context.domain_invocation(&provider);
                assert_eq!(invocation.provider_key, provider.provider_key);
                assert_eq!(
                    invocation.implementation_identity,
                    provider.implementation_identity
                );
                assert!(
                    invocation
                        .implementation_identity
                        .ends_with(&format!(":{source}"))
                );
                identities.insert(invocation.implementation_identity);
                Ok(())
            })
            .unwrap();
    }
    assert_eq!(identities.len(), 3);
}

#[test]
fn domain_native_fact_queries_preserve_links_hide_project_evidence_and_only_count_successful_writes()
 {
    use crate::domain::{
        self as d,
        facts::{FactCandidate, FactKind, query::FactQuery},
    };
    use std::collections::BTreeSet;
    let home = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let file_path = workspace.path().join("example.txt");
    fs::write(&file_path, "original").unwrap();
    let mut rows = claude_rows();
    for row in &mut rows {
        row["cwd"] = json!(workspace.path());
    }
    for (id, failed) in [("write-ok", false), ("write-failed", true)] {
        rows.push(json!({"type":"assistant","sessionId":"same-native-id","uuid":format!("a-{id}"),"cwd":workspace.path(),"timestamp":"2026-10-17T10:01:00Z","message":{"id":format!("response-{id}"),"model":"fixture","content":[{"type":"tool_use","id":id,"name":"Write","input":{"file_path":file_path,"content":"proposed"}}],"usage":{"input_tokens":1,"output_tokens":1}}}));
        rows.push(json!({"type":"user","sessionId":"same-native-id","uuid":format!("r-{id}"),"timestamp":"2026-10-17T10:01:01Z","message":{"content":[{"type":"tool_result","tool_use_id":id,"is_error":failed,"content":if failed { "failed" } else { "success" }}]}}));
    }
    write_rows(home.path(), "claude", "domain-facts", &rows);
    let sessions = NativeSessions::new(home.path().into(), &["claude".into()]);
    sessions.refresh().unwrap();
    let catalog = sessions.project_catalog().unwrap();
    let native_session = &catalog.sessions[0];
    let project_id = native_session.project_id.clone().unwrap();
    let mut query = FactQuery {
        scope: d::QueryScope {
            kind: d::ScopeKind::Builtin,
            source_ids: vec!["claude".into()],
        },
        kind: FactKind::Response,
        project_id: Some(project_id.clone()),
        session_id: None,
        response_id: None,
        snapshot_token: None,
        cursor: None,
    };
    let read = |query: &FactQuery, hidden: &[String]| {
        sessions
            .domain_facts_for_source("claude", query, None, hidden, d::Coverage::Complete)
            .unwrap()
    };
    let (responses, summary) = read(&query, &[]);
    let native_responses: BTreeSet<_> = catalog
        .calls
        .iter()
        .filter(|c| c.kind == "model")
        .map(|c| c.id.clone())
        .collect();
    assert_eq!(
        responses
            .iter()
            .map(|r| r.id.clone())
            .collect::<BTreeSet<_>>(),
        native_responses
    );
    assert_eq!(
        summary.responses.records.unwrap().value() as usize,
        native_responses.len()
    );
    assert_eq!(summary.successful_write_reports.unwrap().value(), 1);
    for record in responses {
        let FactCandidate::Response(r) = record.value else {
            panic!("response");
        };
        assert_eq!(r.session_ref, native_session.id);
        assert_eq!(
            r.reported_response_id,
            catalog
                .calls
                .iter()
                .find(|c| c.id == record.id)
                .unwrap()
                .response_id
        );
    }
    query.kind = FactKind::Write;
    let (writes, _) = read(&query, &[]);
    assert_eq!(writes.len(), 1);
    let FactCandidate::Write(write) = &writes[0].value else {
        panic!("write");
    };
    assert_eq!(write.relative_path, "example.txt");
    assert_eq!(write.project_ref, project_id);
    let expected_write_response = native_responses
        .iter()
        .find(|id| id.ends_with("model:response-write-ok"))
        .unwrap();
    assert_eq!(write.response_ref.as_ref(), Some(expected_write_response));
    query.kind = FactKind::McpExecution;
    let (mcp, _) = read(&query, &[]);
    assert_eq!(mcp.len(), 1);
    let FactCandidate::McpExecution(execution) = &mcp[0].value else {
        panic!("MCP execution");
    };
    let expected_mcp_response = native_responses
        .iter()
        .find(|id| id.ends_with("model:msg1"))
        .unwrap();
    assert_eq!(execution.response_ref.as_ref(), Some(expected_mcp_response));
    query.response_id = Some(expected_write_response.clone());
    query.kind = FactKind::Write;
    assert_eq!(read(&query, &[]).0.len(), 1);
    query.kind = FactKind::McpExecution;
    assert!(read(&query, &[]).0.is_empty());
    query.response_id = Some(expected_mcp_response.clone());
    assert_eq!(read(&query, &[]).0.len(), 1);
    query.response_id = None;
    query.project_id = None;
    query.kind = FactKind::Session;
    let (visible, summary) = read(&query, std::slice::from_ref(&project_id));
    let FactCandidate::Session(s) = &visible[0].value else {
        panic!("session");
    };
    assert_eq!(visible[0].id, native_session.id);
    assert!(s.project_ref.is_none());
    assert_eq!(summary.projects.unwrap().value(), 0);
    assert_eq!(summary.write_reports.unwrap().value(), 0);
    assert_eq!(
        summary.responses.records.unwrap().value() as usize,
        native_responses.len()
    );
    sessions.set_enabled("claude", false).unwrap();
    assert_eq!(
        sessions
            .domain_facts_for_source("claude", &query, None, &[], d::Coverage::Complete)
            .unwrap_err()
            .code,
        "permission_denied"
    );
    assert_eq!(fs::read_to_string(file_path).unwrap(), "original");
}

#[test]
fn domain_native_mcp_links_require_the_exact_indexed_parent_not_a_reported_id() {
    use crate::domain::{
        self as d,
        facts::{FactCandidate, FactKind, query::FactQuery},
    };
    let home = tempfile::tempdir().unwrap();
    write_rows(home.path(), "claude", "parent-evidence", &claude_rows());
    let sessions = NativeSessions::new(home.path().into(), &["claude".into()]);
    sessions.refresh().unwrap();
    let catalog = sessions.project_catalog().unwrap();
    let parent = catalog
        .calls
        .iter()
        .find(|c| c.kind == "model")
        .unwrap()
        .id
        .clone();
    let mut query = FactQuery {
        scope: d::QueryScope {
            kind: d::ScopeKind::Builtin,
            source_ids: vec!["claude".into()],
        },
        kind: FactKind::McpExecution,
        project_id: None,
        session_id: None,
        response_id: None,
        snapshot_token: None,
        cursor: None,
    };
    // A raw upstream claim, a dangling parent and another session's parent
    // cannot establish a relationship, even when response_id claims a real ID.
    for reference in [
        None,
        Some("msg1".into()),
        Some("claude:foreign:model:msg1".into()),
        Some("claude:same-native-id:model:absent".into()),
    ] {
        {
            let mut state = sessions.lock().unwrap();
            let file = state.files.values_mut().next().unwrap();
            let calls = std::sync::Arc::make_mut(&mut file.calls);
            let tool = calls.iter_mut().find(|c| c.kind == "mcp").unwrap();
            tool.model_response_id = reference;
            tool.response_id = Some(parent.clone());
            file.fact_calls = domain_facts::CallIndex::new(calls);
        }
        query.response_id = None;
        let (records, _) = sessions
            .domain_facts_for_source("claude", &query, None, &[], d::Coverage::Complete)
            .unwrap();
        assert_eq!(records.len(), 1);
        let FactCandidate::McpExecution(execution) = &records[0].value else {
            panic!("MCP execution")
        };
        assert!(execution.response_ref.is_none());
        query.response_id = Some(parent.clone());
        assert!(
            sessions
                .domain_facts_for_source("claude", &query, None, &[], d::Coverage::Complete)
                .unwrap()
                .0
                .is_empty()
        );
    }
}

#[test]
fn domain_native_activity_facts_keep_distinct_recent_messages_and_bound_coverage() {
    use crate::domain::{
        self as d,
        facts::{FactCandidate, FactKind, query::FactQuery},
    };
    let home = tempfile::tempdir().unwrap();
    let now = chrono::Utc::now() - chrono::Duration::minutes(2);
    let rows = (0..2).flat_map(|i| [
        json!({"type":"user","sessionId":"activity-facts","uuid":format!("u{i}"),"timestamp":(now+chrono::Duration::seconds(i*2)).to_rfc3339(),"message":{"role":"user","content":format!("fixture {i}")}}),
        json!({"type":"assistant","sessionId":"activity-facts","uuid":format!("a{i}"),"timestamp":(now+chrono::Duration::seconds(i*2+1)).to_rfc3339(),"message":{"id":format!("r{i}"),"model":"fixture","content":[{"type":"text","text":"done"}],"usage":{"input_tokens":1,"output_tokens":1}}})
    ]).collect::<Vec<_>>();
    write_rows(home.path(), "claude", "activity-facts", &rows);
    let sessions = NativeSessions::new(home.path().into(), &["claude".into()]);
    sessions.refresh().unwrap();
    sessions.refresh_domain_mcp_catalog(&|| true);
    let revision = sessions
        .sources()
        .unwrap()
        .into_iter()
        .find(|s| s.agent_id == "claude")
        .unwrap()
        .authorization_revision;
    assert_eq!(
        sessions
            .mcp_catalog
            .lock()
            .unwrap()
            .view("claude", revision)
            .unwrap()
            .coverage,
        d::Coverage::Complete
    );
    let query = FactQuery {
        scope: d::QueryScope {
            kind: d::ScopeKind::Builtin,
            source_ids: vec!["claude".into()],
        },
        kind: FactKind::Activity,
        project_id: None,
        session_id: None,
        response_id: None,
        snapshot_token: None,
        cursor: None,
    };
    let (records, summary) = sessions
        .domain_facts_for_source("claude", &query, None, &[], d::Coverage::Complete)
        .unwrap();
    assert_eq!(records.len(), 2);
    assert_ne!(records[0].id, records[1].id);
    assert_eq!(summary.activities.unwrap().value(), 2);
    assert_eq!(summary.coverage, d::Coverage::Partial);
    assert_eq!(summary.responses.coverage, d::Coverage::Complete);
    for kind in [FactKind::Response, FactKind::McpCatalog] {
        let independent = FactQuery {
            kind,
            ..query.clone()
        };
        let (_, summary) = sessions
            .domain_facts_for_source("claude", &independent, None, &[], d::Coverage::Complete)
            .unwrap();
        assert_eq!(summary.coverage, d::Coverage::Complete);
        assert!(summary.activities.is_none());
    }
    for record in records {
        let FactCandidate::Activity(value) = record.value else {
            panic!("activity")
        };
        assert_eq!(value.session_ref, "claude:activity-facts");
        assert!(value.response_ref.is_none());
        assert_eq!(value.native_id, record.id);
    }
}

#[test]
fn session_page_and_fee_catalog_remain_one_generation_during_append_and_revocation() {
    let home = tempfile::tempdir().unwrap();
    let rows = claude_rows();
    let path = write_rows(home.path(), "claude", "coherent", &rows);
    let sessions = NativeSessions::new(home.path().into(), &["claude".into()]);
    sessions.refresh().unwrap();
    let authorizations = |sources: Vec<SourceStatus>| {
        sources
            .into_iter()
            .map(|s| (s.agent_id, s.enabled, s.authorization_revision))
            .collect::<Vec<_>>()
    };
    let consent = authorizations(sessions.sources().unwrap());
    let (before, catalog) = sessions
        .query_with_catalog(SessionQuery::default())
        .unwrap();
    let before_calls = before.summary.as_ref().unwrap().model_calls;
    let mut next = rows[1].clone();
    next["uuid"] = json!("new-event");
    next["message"]["id"] = json!("new-response");
    writeln!(
        fs::OpenOptions::new().append(true).open(path).unwrap(),
        "{next}"
    )
    .unwrap();
    sessions.refresh().unwrap();
    let (after, latest) = sessions
        .query_with_catalog(SessionQuery::default())
        .unwrap();
    assert_eq!(
        after.summary.as_ref().unwrap().model_calls,
        before_calls + 1
    );
    assert!(latest.revision > catalog.revision);
    assert_eq!(authorizations(sessions.sources().unwrap()), consent);
    let data = tempfile::tempdir().unwrap();
    let mut history = crate::observation::sql::History::open(data.path()).unwrap();
    history.refresh(&[]).unwrap();
    for (page, catalog) in [(&before, &catalog), (&after, &latest), (&before, &catalog)] {
        history.prepare_fees(catalog).unwrap();
        let fees = history
            .fee_summary(&page.selected_session_ids, None, false)
            .unwrap();
        assert_eq!(
            fees["nativeResponses"],
            page.summary.as_ref().unwrap().model_calls
        );
        assert_eq!(fees["unlinkedNativeResponses"], fees["nativeResponses"]);
    }
    sessions.set_enabled("claude", false).unwrap();
    assert_ne!(authorizations(sessions.sources().unwrap()), consent);
    let (empty, revoked) = sessions
        .query_with_catalog(SessionQuery::default())
        .unwrap();
    assert_eq!(empty.total, 0);
    assert!(revoked.calls.is_empty());
}

#[test]
fn cold_directory_builds_only_bounded_continuations_and_evicted_files_still_refresh() {
    let home = tempfile::tempdir().unwrap();
    let mut paths = vec![];
    for i in 0..65 {
        let mut rows = claude_rows();
        for row in &mut rows {
            row["sessionId"] = json!(format!("bounded-{i}"));
        }
        let path = write_rows(home.path(), "claude", &format!("bounded-{i:03}"), &rows);
        fs::File::open(&path)
            .unwrap()
            .set_times(
                fs::FileTimes::new()
                    .set_modified(UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000 + i)),
            )
            .unwrap();
        paths.push(path);
    }
    let sessions = NativeSessions::new(home.path().into(), &["claude".into()]);
    sessions.refresh().unwrap();
    let before = sessions.project_catalog().unwrap();
    assert_eq!(before.sessions.len(), 65);
    sessions.refresh().unwrap();
    let unchanged = sessions.project_catalog().unwrap();
    assert_eq!(unchanged.revision, before.revision);
    assert!(
        before
            .calls
            .chunks
            .iter()
            .zip(&unchanged.calls.chunks)
            .all(|(a, b)| Arc::ptr_eq(a, b))
    );
    assert_eq!(sessions.index_cache.lock().unwrap().snapshots, 8);
    assert_eq!(
        sessions
            .index_cache
            .lock()
            .unwrap()
            .decoded_lines(&paths[0]),
        0
    );
    assert!(
        sessions
            .index_cache
            .lock()
            .unwrap()
            .decoded_lines(&paths[64])
            > 0
    );
    assert_eq!(sessions.index_cache.lock().unwrap().retained_bodies(), 0);
    // An older file outside the bounded continuation cache remains authoritative.
    let mut rows = claude_rows();
    for row in &mut rows {
        row["sessionId"] = json!("bounded-0");
    }
    rows[1]["message"]["usage"]["output_tokens"] = json!(198);
    write_rows(home.path(), "claude", "bounded-000", &rows);
    sessions.refresh().unwrap();
    let full = parser::parse(home.path(), &paths[0], "claude", None).unwrap();
    let after = sessions.project_catalog().unwrap();
    let session = after
        .sessions
        .iter()
        .find(|s| s.native_id == "bounded-0")
        .unwrap();
    assert_eq!(
        serde_json::to_value(session).unwrap(),
        serde_json::to_value(full.summary).unwrap()
    );
    fs::remove_file(&paths[1]).unwrap();
    sessions.refresh().unwrap();
    assert_eq!(sessions.project_catalog().unwrap().sessions.len(), 64);
    sessions.set_enabled("claude", false).unwrap();
    assert!(sessions.project_catalog().unwrap().calls.is_empty());
    assert_eq!(
        sessions
            .index_cache
            .lock()
            .unwrap()
            .decoded_lines(&paths[0]),
        0
    );
}

#[test]
fn scan_project_identity_cache_expires_before_the_next_refresh() {
    let home = tempfile::tempdir().unwrap();
    let project = home.path().join("workspace/subdir");
    fs::create_dir_all(home.path().join("workspace/.git")).unwrap();
    fs::create_dir_all(&project).unwrap();
    let write = |name: &str| {
        let mut rows = claude_rows();
        for row in &mut rows {
            row["sessionId"] = json!(name);
            row["cwd"] = json!(project);
        }
        write_rows(home.path(), "claude", name, &rows)
    };
    write("first");
    let sessions = NativeSessions::new(home.path().into(), &["claude".into()]);
    sessions.refresh().unwrap();
    let first = sessions.project_catalog().unwrap().sessions[0]
        .project_id
        .clone();
    fs::create_dir_all(project.join(".git")).unwrap();
    let second = write("second");
    sessions.refresh().unwrap();
    let catalog = sessions.project_catalog().unwrap();
    let second = catalog
        .sessions
        .iter()
        .find(|s| s.source_path == second.to_string_lossy())
        .unwrap();
    assert_ne!(first, second.project_id);
    assert_eq!(
        second.project_id,
        crate::projects::identity(project.to_str().unwrap()).map(|p| p.id)
    );
}

#[test]
fn revocation_does_not_wait_for_a_parser_holding_the_incremental_cache() {
    let home = tempfile::tempdir().unwrap();
    let sessions = std::sync::Arc::new(NativeSessions::new(home.path().into(), &["claude".into()]));
    let held = sessions.index_cache.lock().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let worker = sessions.clone();
    let thread = std::thread::spawn(move || {
        let result = worker.set_enabled("claude", false);
        tx.send(result).unwrap();
    });
    let result = rx.recv_timeout(std::time::Duration::from_secs(1));
    drop(held);
    thread.join().unwrap();
    result
        .expect("revocation must finish independently of the parser")
        .unwrap();
    assert!(!sessions.sources().unwrap()[0].enabled);
}

#[test]
fn temporary_native_parse_failure_keeps_last_good_usage_and_recovery_replaces_it() {
    let home = tempfile::tempdir().unwrap();
    let rows = claude_rows();
    let path = write_rows(home.path(), "claude", "transient", &rows);
    let sessions = NativeSessions::new(home.path().into(), &["claude".into()]);
    sessions.refresh().unwrap();
    let before = sessions.project_catalog().unwrap();
    assert_eq!(before.sessions.len(), 1);
    fs::write(&path, b"{\"temporary_rewrite\":").unwrap();
    sessions.refresh().unwrap();
    let stale = sessions.project_catalog().unwrap();
    assert_eq!(stale.sessions.len(), 1);
    assert_eq!(
        stale.sessions[0].usage.total_tokens,
        before.sessions[0].usage.total_tokens
    );
    assert!(stale.sessions[0].partial);
    assert!(stale.revision > before.revision);
    assert_eq!(sessions.sources().unwrap()[0].status, "partial");
    write_rows(home.path(), "claude", "transient", &rows);
    sessions.refresh().unwrap();
    assert!(!sessions.project_catalog().unwrap().sessions[0].partial);
    assert_eq!(sessions.sources().unwrap()[0].status, "ready");
    sessions.set_enabled("claude", false).unwrap();
    assert!(sessions.project_catalog().unwrap().sessions.is_empty());
}

#[test]
fn incremental_index_matches_full_parse_after_append_rewrite_partial_tail_and_rotation() {
    let home = tempfile::tempdir().unwrap();
    let mut rows = claude_rows();
    let path = write_rows(home.path(), "claude", "incremental", &rows);
    let sessions = NativeSessions::new(home.path().into(), &["claude".into()]);
    sessions.refresh().unwrap();
    assert_eq!(
        sessions.index_cache.lock().unwrap().decoded_lines(&path),
        rows.len()
    );
    let mut next = rows[1].clone();
    next["uuid"] = json!("append-message");
    next["message"]["id"] = json!("new-response");
    rows.push(next.clone());
    writeln!(
        fs::OpenOptions::new().append(true).open(&path).unwrap(),
        "{next}"
    )
    .unwrap();
    sessions.refresh().unwrap();
    assert_eq!(sessions.index_cache.lock().unwrap().decoded_lines(&path), 1);
    assert_eq!(sessions.index_cache.lock().unwrap().retained_bodies(), 0);
    let assert_parity = || {
        let full = parser::parse(home.path(), &path, "claude", None).unwrap();
        let catalog = sessions.project_catalog().unwrap();
        assert_eq!(
            serde_json::to_value(&catalog.sessions[0]).unwrap(),
            serde_json::to_value(full.summary).unwrap()
        );
        assert_eq!(
            serde_json::to_value(&catalog.calls).unwrap(),
            serde_json::to_value(full.calls).unwrap()
        );
    };
    assert_parity();
    // A modified prefix plus append invalidates continuation even on the same inode.
    rows[1]["message"]["usage"]["output_tokens"] = json!(99);
    next["uuid"] = json!("rewritten-prefix");
    rows.push(next.clone());
    write_rows(home.path(), "claude", "incremental", &rows);
    sessions.refresh().unwrap();
    assert_eq!(
        sessions.index_cache.lock().unwrap().decoded_lines(&path),
        rows.len()
    );
    assert_parity();
    let mut append = fs::OpenOptions::new().append(true).open(&path).unwrap();
    append.write_all(b"{\"type\":").unwrap();
    sessions.refresh().unwrap();
    assert!(sessions.project_catalog().unwrap().sessions[0].partial);
    append.write_all(b"\"progress\"}\n").unwrap();
    drop(append);
    sessions.refresh().unwrap();
    assert_parity();
    let rotated = path.with_extension("replacement");
    fs::write(
        &rotated,
        rows.iter().map(|v| format!("{v}\n")).collect::<String>(),
    )
    .unwrap();
    fs::rename(rotated, &path).unwrap();
    sessions.refresh().unwrap();
    assert_eq!(
        sessions.index_cache.lock().unwrap().decoded_lines(&path),
        rows.len()
    );
    assert_parity();
    sessions.set_enabled("claude", false).unwrap();
    assert_eq!(sessions.index_cache.lock().unwrap().decoded_lines(&path), 0);
}

fn write_rows(home: &Path, agent: &str, name: &str, rows: &[Value]) -> PathBuf {
    let root = if agent == "claude" {
        home.join(".claude/projects/project")
    } else {
        home.join(".codex/sessions/2026/10/17")
    };
    fs::create_dir_all(&root).unwrap();
    let file = root.join(format!("{name}.jsonl"));
    let mut f = fs::File::create(&file).unwrap();
    for row in rows {
        writeln!(f, "{row}").unwrap();
    }
    file
}

#[test]
fn cross_client_session_is_unique_and_usage_is_attributed_per_response() {
    let home = tempfile::tempdir().unwrap();
    let ts = now();
    let mut rows = vec![];
    for (index, originator, input) in [
        (1, "codex_cli_rs", 10),
        (2, "codex_work_desktop", 20),
        (3, "unrecognised_host", 30),
    ] {
        rows.push(json!({"type":"session_meta","timestamp":ts,"payload":{"id":"shared","originator":originator,"source":"vscode"}}));
        rows.push(json!({"type":"turn_context","timestamp":ts,"payload":{"turn_id":format!("turn{index}"),"model":"test-model"}}));
        rows.push(json!({"type":"token_usage_record","originator":originator,"timestamp":ts,"payload":{"response_id":format!("response{index}"),"usage":{"input_tokens":input,"output_tokens":1},"thread_token_usage":{"input_tokens":999,"output_tokens":1}}}));
    }
    write_rows(home.path(), "codex", "shared", &rows);
    // A second copy with the same native identity must not double any totals.
    write_rows(home.path(), "codex", "shared-copy", &rows);
    let sessions = NativeSessions::new(home.path().into(), &["codex".into()]);
    sessions.refresh().unwrap();
    let all = sessions.query(SessionQuery::default()).unwrap();
    assert_eq!(all.total, 1);
    assert_eq!(all.items[0].agent_products, ["codex-cli", "codex-desktop"]);
    assert!(all.items[0].has_unknown_client);
    assert!(all.items[0].agent_product_id.is_none());
    for (agent, total) in [
        ("codex-cli", 11),
        ("codex-desktop", 21),
        ("codex-unknown", 31),
    ] {
        let filtered = sessions
            .query(SessionQuery {
                agent: Some(agent.into()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(
            filtered.total, 1,
            "same Session can be associated with multiple clients"
        );
        let summary = filtered.summary.unwrap();
        assert_eq!(summary.usage_scope, "matched_calls");
        assert_eq!(summary.model_calls, 1);
        assert_eq!(summary.usage.total_tokens, Some(total));
        let calls = sessions
            .calls(CallQuery {
                agent: Some(agent.into()),
                kind: Some("model".into()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(calls.total, 1);
        let (usage, _, _) = sessions.usage_records(agent).unwrap();
        assert_eq!(usage.len(), 1);
        assert_eq!(usage[0].usage.total_tokens, Some(total));
    }
    assert_eq!(
        sessions
            .query(SessionQuery {
                agent: Some("claude-code-cli".into()),
                ..Default::default()
            })
            .unwrap()
            .total,
        0
    );
    sessions.set_enabled("codex", false).unwrap();
    for agent in ["codex-cli", "codex-desktop", "codex-unknown"] {
        assert_eq!(
            sessions
                .query(SessionQuery {
                    agent: Some(agent.into()),
                    ..Default::default()
                })
                .unwrap()
                .total,
            0
        );
        assert!(sessions.usage_records(agent).unwrap().0.is_empty());
    }
}

#[test]
fn legacy_history_does_not_become_cli_and_claude_entrypoint_can_change() {
    let home = tempfile::tempdir().unwrap();
    let mut rows = claude_rows();
    write_rows(home.path(), "claude", "legacy", &rows);
    let sessions = NativeSessions::new(home.path().into(), &["claude".into()]);
    sessions.refresh().unwrap();
    assert_eq!(
        sessions
            .query(SessionQuery {
                agent: Some("claude-code-cli".into()),
                ..Default::default()
            })
            .unwrap()
            .total,
        0
    );
    assert_eq!(
        sessions
            .query(SessionQuery {
                agent: Some("unknown".into()),
                ..Default::default()
            })
            .unwrap()
            .total,
        1
    );
    assert_eq!(sessions.usage_records("claude-unknown").unwrap().0.len(), 2);
    for row in &mut rows {
        row["entrypoint"] = json!("cli");
    }
    let last = rows
        .iter_mut()
        .rev()
        .find(|row| row["type"] == "assistant")
        .unwrap();
    last["entrypoint"] = json!("claude-desktop-3p");
    write_rows(home.path(), "claude", "legacy", &rows);
    sessions.refresh().unwrap();
    let all = sessions.query(SessionQuery::default()).unwrap();
    assert_eq!(all.total, 1);
    assert_eq!(
        all.items[0].agent_products,
        ["claude-code-cli", "claude-code-desktop"]
    );
}

#[test]
fn claude_sdk_reason_and_explicit_cli_are_scoped_per_record_and_context() {
    let home = tempfile::tempdir().unwrap();
    let mut rows = claude_rows();
    for row in &mut rows {
        row["entrypoint"] = json!("sdk-cli");
    }
    let file = write_rows(home.path(), "claude", "sdk-scope", &rows);
    let original = fs::read(&file).unwrap();
    let sessions = NativeSessions::new(home.path().into(), &["claude".into()]);
    sessions.refresh().unwrap();
    let calls = sessions
        .calls(CallQuery {
            agent: Some("claude-unknown".into()),
            kind: Some("model".into()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(calls.total, 2);
    assert!(
        calls
            .items
            .iter()
            .all(|c| c.agent_product_id.is_none() && c.agent_attribution == Some("sdk_client"))
    );
    let mut watch = live::NativeWatch::default();
    let activity = watch.poll(&sessions);
    let id = activity[0].session_id.clone();
    assert_eq!(activity[0].agent_attribution.as_deref(), Some("sdk_client"));
    let samples = watch.context_samples(&sessions, vec![id.clone()]).unwrap();
    assert!(
        samples[0]
            .context
            .as_ref()
            .unwrap()
            .sample_agent_product_id
            .is_none()
    );
    assert_eq!(fs::read(&file).unwrap(), original);
    let mut next = rows
        .iter()
        .rev()
        .find(|r| r["type"] == "assistant")
        .unwrap()
        .clone();
    next["entrypoint"] = json!("cli");
    next["uuid"] = json!("next-cli");
    next["message"]["id"] = json!("next-response");
    next["timestamp"] = json!(now());
    writeln!(
        fs::OpenOptions::new().append(true).open(&file).unwrap(),
        "{next}"
    )
    .unwrap();
    let activity = watch.poll(&sessions);
    assert_eq!(
        activity[0].agent_product_id.as_deref(),
        Some("claude-code-cli")
    );
    sessions.refresh().unwrap();
    assert_eq!(
        sessions
            .calls(CallQuery {
                agent: Some("claude-unknown".into()),
                kind: Some("model".into()),
                ..Default::default()
            })
            .unwrap()
            .total,
        2
    );
    assert_eq!(
        sessions
            .calls(CallQuery {
                agent: Some("claude-code-cli".into()),
                kind: Some("model".into()),
                ..Default::default()
            })
            .unwrap()
            .total,
        1
    );
}

#[test]
fn session_creator_does_not_claim_later_calls_or_live_activity() {
    let home = tempfile::tempdir().unwrap();
    let ts = now();
    write_rows(
        home.path(),
        "codex",
        "created-on-desktop",
        &[
            json!({"type":"session_meta","timestamp":ts,"payload":{"id":"creator-only","originator":"codex_work_desktop"}}),
            json!({"type":"turn_context","timestamp":ts,"payload":{"turn_id":"later-turn","model":"m"}}),
            json!({"type":"token_usage_record","timestamp":ts,"payload":{"response_id":"r","usage":{"input_tokens":10,"output_tokens":5}}}),
            json!({"type":"event_msg","timestamp":ts,"payload":{"type":"task_started"}}),
        ],
    );
    let sessions = NativeSessions::new(home.path().into(), &["codex".into()]);
    sessions.refresh().unwrap();
    let desktop = sessions
        .query(SessionQuery {
            agent: Some("codex-desktop".into()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(desktop.total, 1, "creation proves participation only");
    assert_eq!(desktop.summary.unwrap().model_calls, 0);
    assert!(
        sessions
            .usage_records("codex-desktop")
            .unwrap()
            .0
            .is_empty()
    );
    assert_eq!(sessions.usage_records("codex-unknown").unwrap().0.len(), 1);
    let activities = live::NativeWatch::default().poll(&sessions);
    assert!(activities.iter().all(|a| a.agent_product_id.is_none()));
}

#[test]
fn model_usage_metadata_keeps_recorded_provider_and_respects_source_revocation() {
    let home = tempfile::tempdir().unwrap();
    let rows = vec![
        json!({"type":"session_meta","timestamp":now(),"payload":{"id":"usage-native","model_provider":"historical-gateway"}}),
        json!({"type":"turn_context","timestamp":now(),"payload":{"turn_id":"t","model":"recorded-model"}}),
        json!({"type":"token_usage_record","timestamp":now(),"payload":{"response_id":"response1","usage":{"input_tokens":10,"output_tokens":5},"thread_token_usage":{"input_tokens":990,"output_tokens":10}}}),
    ];
    write_rows(home.path(), "codex", "usage", &rows);
    let sessions = NativeSessions::new(home.path().into(), &["codex".into()]);
    sessions.refresh().unwrap();
    let (records, _, source) = sessions.usage_records("codex").unwrap();
    assert_eq!(source, "ready");
    assert!(!records.is_empty());
    assert_eq!(
        records[0].usage.total_tokens,
        Some(15),
        "session cumulative usage is never allocated to a model response"
    );
    assert!(records.iter().all(
        |record| record.provider.as_deref() == Some("historical-gateway")
            && record.endpoint.is_none()
            && record.route_revision.is_none()
    ));
    let (other, _, _) = sessions.usage_records("claude").unwrap();
    assert!(other.is_empty());
    sessions.set_enabled("codex", false).unwrap();
    let (revoked, _, source) = sessions.usage_records("codex").unwrap();
    assert_eq!(source, "disabled");
    assert!(revoked.is_empty());
}

#[test]
fn claude_explicit_api_errors_are_visible_without_counting_synthetic_responses() {
    let dir = tempfile::tempdir().unwrap();
    let ts = now();
    let mut rows = claude_rows();
    rows.push(json!({"type":"system","subtype":"api_error","sessionId":"same-native-id","uuid":"retry-error","timestamp":ts,"error":{"name":"APIConnectionError","message":"Connection failed apiKey=private-error-key","headers":{"authorization":"must-not-leak"}},"retryAttempt":1}));
    rows.push(rows.last().unwrap().clone());
    rows.push(json!({"type":"assistant","sessionId":"same-native-id","uuid":"api-error","timestamp":ts,"isApiErrorMessage":true,"apiErrorStatus":401,"error":"authentication_failed","message":{"id":"synthetic-error","model":"<synthetic>","content":[{"type":"text","text":"API Error: invalid credentials Bearer private-long-token"}],"usage":{"input_tokens":0,"output_tokens":0}}}));
    rows.push(json!({"type":"user","sessionId":"same-native-id","uuid":"discussion","timestamp":ts,"message":{"content":"Explain Cannot connect to API and API Error; this is user text, not an error event."}}));
    let file = write_rows(dir.path(), "claude", "failure", &rows);
    let parsed = parser::parse(
        dir.path(),
        &file,
        "claude",
        Some(parser::Window {
            offset: 0,
            anchor: None,
        }),
    )
    .unwrap();
    assert_eq!(parsed.summary.model_calls, 2);
    assert_eq!(parsed.summary.model_error_count, 2);
    assert_eq!(parsed.summary.usage.total_tokens, Some(58));
    let failures: Vec<_> = parsed
        .events
        .iter()
        .filter(|e| e.failure.is_some())
        .collect();
    assert_eq!(failures.len(), 2);
    assert!(
        failures
            .iter()
            .all(|e| e.kind == "diagnostic" && e.status == "error")
    );
    assert!(failures.iter().all(|e| e.model.is_none()));
    assert_eq!(failures[0].failure.as_ref().unwrap().category, "connection");
    assert_eq!(failures[1].failure.as_ref().unwrap().http_status, Some(401));
    assert_eq!(
        failures[1].failure.as_ref().unwrap().category,
        "authentication"
    );
    assert_eq!(
        parsed.summary.latest_failure_id.as_deref(),
        Some(failures[1].id.as_str())
    );
    let encoded = serde_json::to_string(&parsed.events).unwrap();
    for secret in ["private-error-key", "private-long-token", "must-not-leak"] {
        assert!(!encoded.contains(secret));
    }
    let service = NativeSessions::new(dir.path().into(), &["claude".into()]);
    service.refresh().unwrap();
    let mut watch = live::NativeWatch::default();
    // A final explicit failure must not show as a model reply.
    let failure = rows[rows.len() - 2].clone();
    writeln!(
        fs::OpenOptions::new().append(true).open(&file).unwrap(),
        "{failure}"
    )
    .unwrap();
    assert_eq!(watch.poll(&service)[0].phase, "error");
}

#[test]
fn codex_error_stream_and_unsuccessful_turns_keep_native_context_and_live_failure() {
    let dir = tempfile::tempdir().unwrap();
    let ts = now();
    let rows = vec![
        json!({"type":"session_meta","timestamp":ts,"payload":{"id":"codex-failure","cwd":"/projects/test","model_provider":"provider"}}),
        json!({"type":"turn_context","timestamp":ts,"payload":{"turn_id":"turn1","model":"model"}}),
        json!({"type":"event_msg","timestamp":ts,"payload":{"type":"stream_error","message":"Reconnecting","additional_details":"Connection failed Bearer private-long-token","codex_error_info":{"http_connection_failed":{"http_status_code":503}}}}),
        json!({"type":"event_msg","timestamp":ts,"payload":{"type":"error","message":"Invalid credentials","codex_error_info":"unauthorized"}}),
        json!({"type":"event_msg","timestamp":ts,"payload":{"type":"task_complete","turn_id":"turn1","error":{"message":"Connection failed","statusCode":502,"codex_error_info":"other"},"time_to_first_token_ms":123}}),
    ];
    let file = write_rows(dir.path(), "codex", "failures", &rows);
    let parsed = parser::parse(
        dir.path(),
        &file,
        "codex",
        Some(parser::Window {
            offset: 0,
            anchor: None,
        }),
    )
    .unwrap();
    assert_eq!(parsed.summary.model_calls, 0);
    assert_eq!(parsed.summary.model_error_count, 3);
    assert!(parsed.summary.usage.total_tokens.is_none());
    let failures: Vec<_> = parsed
        .events
        .iter()
        .filter(|e| e.failure.is_some())
        .collect();
    assert_eq!(failures[0].failure.as_ref().unwrap().http_status, Some(503));
    assert_eq!(
        failures[1].failure.as_ref().unwrap().category,
        "authentication"
    );
    assert_eq!(failures[2].failure.as_ref().unwrap().http_status, Some(502));
    assert_eq!(
        failures[2].failure.as_ref().unwrap().code.as_deref(),
        Some("other")
    );
    assert!(failures.iter().all(|e| e.model.as_deref() == Some("model")
        && e.provider.as_deref() == Some("provider")
        && e.turn_id.ends_with(":turn:turn1")));
    assert!(
        !serde_json::to_string(&parsed.events)
            .unwrap()
            .contains("private-long-token")
    );
    assert!(
        !parsed
            .events
            .iter()
            .any(|e| e.name.contains("等待后续输入"))
    );
    let service = NativeSessions::new(dir.path().into(), &["codex".into()]);
    service.refresh().unwrap();
    let mut watch = live::NativeWatch::default();
    let activity = watch.poll(&service);
    assert_eq!(activity[0].phase, "error");
    assert!(activity[0].turn_ttft_ms.is_none());
}
#[test]
fn native_activity_follows_complete_rows_and_distinguishes_turn_ttft() {
    let dir = tempfile::tempdir().unwrap();
    let now = now();
    let rows = vec![
        json!({"type":"session_meta","timestamp":now,"payload":{"id":"live-native","cwd":"/projects/test"}}),
        json!({"type":"event_msg","timestamp":now,"payload":{"type":"task_started","turn_id":"turn-live"}}),
    ];
    let file = write_rows(dir.path(), "codex", "live", &rows);
    let service = NativeSessions::new(dir.path().into(), &["codex".into()]);
    service.refresh().unwrap();
    let mut watch = live::NativeWatch::default();
    assert_eq!(watch.poll(&service)[0].phase, "processing");
    let tool=serde_json::to_string(&json!({"type":"response_item","timestamp":now,"payload":{"type":"function_call","name":"mcp__test__inspect","call_id":"tool1"}})).unwrap();
    fs::OpenOptions::new()
        .append(true)
        .open(&file)
        .unwrap()
        .write_all(tool.as_bytes())
        .unwrap();
    assert_eq!(watch.poll(&service)[0].phase, "processing");
    fs::OpenOptions::new()
        .append(true)
        .open(&file)
        .unwrap()
        .write_all(b"\n")
        .unwrap();
    let activity = watch.poll(&service);
    assert_eq!(activity[0].phase, "tool_requested");
    assert!(activity[0].turn_ttft_ms.is_none());
    let complete = json!({"type":"event_msg","timestamp":now,"payload":{"type":"task_complete","turn_id":"turn-live","time_to_first_token_ms":1234,"duration_ms":5000}});
    writeln!(
        fs::OpenOptions::new().append(true).open(&file).unwrap(),
        "{complete}"
    )
    .unwrap();
    let activities = watch.poll(&service);
    assert_eq!(activities[0].phase, "waiting_input");
    assert_eq!(activities[0].turn_ttft_ms, Some(1234.0));
    service.set_enabled("codex", false).unwrap();
    assert!(watch.poll(&service).is_empty());
}
#[test]
fn parallel_native_activity_survives_the_recent_history_limit() {
    let dir = tempfile::tempdir().unwrap();
    let current = chrono::Utc::now();
    for n in 0..40 {
        let id = format!("completed-{n}");
        write_rows(
            dir.path(),
            "codex",
            &id,
            &[
                json!({"type":"session_meta","timestamp":current.to_rfc3339(),"payload":{"id":id}}),
                json!({"type":"event_msg","timestamp":current.to_rfc3339(),"originator":"codex_cli_rs","payload":{"type":"task_complete"}}),
            ],
        );
    }
    for n in 0..35 {
        let id = format!("running-{n}");
        let time = (current - chrono::Duration::seconds(5)).to_rfc3339();
        write_rows(
            dir.path(),
            "codex",
            &id,
            &[
                json!({"type":"session_meta","timestamp":time,"payload":{"id":id}}),
                json!({"type":"event_msg","timestamp":time,"originator":"codex_cli_rs","payload":{"type":"task_started"}}),
            ],
        );
    }
    let service = NativeSessions::new(dir.path().into(), &["codex".into()]);
    service.refresh().unwrap();
    let activity = live::NativeWatch::default().poll(&service);
    let running: Vec<_> = activity
        .iter()
        .filter(|a| a.phase == "processing")
        .collect();
    assert_eq!(running.len(), 35);
    assert!(running.iter().all(|a| !a.stale));
    assert_eq!(activity.len(), 75);
    assert_eq!(
        activity
            .iter()
            .filter(|a| a.phase == "waiting_input")
            .count(),
        40
    );
}
fn claude_rows() -> Vec<Value> {
    vec![
        json!({"type":"user","sessionId":"same-native-id","uuid":"u1","promptId":"turn1","cwd":"/projects/test","version":"2.1.286","timestamp":"2026-10-17T10:00:00Z","message":{"role":"user","content":"检查服务状态"}}),
        json!({"type":"assistant","sessionId":"same-native-id","uuid":"a1","timestamp":"2026-10-17T10:00:01Z","message":{"id":"msg1","model":"claude-test","content":[{"type":"text","text":"开始检查"},{"type":"tool_use","id":"call1","name":"mcp__server__status","input":{"password":"secret-password with spaces","value":"sk-ant-abcdefghijklmnop"}}],"usage":{"input_tokens":10,"cache_read_input_tokens":20,"cache_creation_input_tokens":5,"output_tokens":2}}}),
        json!({"type":"assistant","sessionId":"same-native-id","uuid":"a2","timestamp":"2026-10-17T10:00:02Z","message":{"id":"msg1","model":"claude-test","content":[{"type":"text","text":"继续检查"}],"usage":{"input_tokens":10,"cache_read_input_tokens":20,"cache_creation_input_tokens":5,"output_tokens":8}}}),
        json!({"type":"user","sessionId":"same-native-id","uuid":"r1","timestamp":"2026-10-17T10:00:03Z","message":{"content":[{"type":"tool_result","tool_use_id":"call1","is_error":true,"content":"服务返回错误"}]}}),
        json!({"type":"assistant","sessionId":"same-native-id","uuid":"a3","timestamp":"2026-10-17T10:00:04Z","message":{"id":"msg2","model":"claude-test","content":[{"type":"tool_use","id":"skill1","name":"Skill","input":{"skill":"review"}}],"usage":{"input_tokens":12,"output_tokens":3}}}),
    ]
}
#[test]
fn claude_turns_usage_and_tool_results_are_deduplicated_and_redacted() {
    let dir = tempfile::tempdir().unwrap();
    let mut rows = claude_rows();
    rows.push(rows[1].clone());
    let file = write_rows(dir.path(), "claude", "main", &rows);
    let parsed = parser::parse(
        dir.path(),
        &file,
        "claude",
        Some(parser::Window {
            offset: 0,
            anchor: None,
        }),
    )
    .unwrap();
    assert_eq!(parsed.summary.title, "检查服务状态");
    assert_eq!(parsed.summary.model_calls, 2);
    assert_eq!(parsed.summary.mcp_calls, 1);
    assert_eq!(parsed.summary.skill_calls, 1);
    assert_eq!(parsed.summary.usage.input_tokens, Some(47));
    assert_eq!(parsed.summary.usage.total_tokens, Some(58));
    let call = parsed
        .events
        .iter()
        .find(|e| e.call_id.as_deref() == Some("call1"))
        .unwrap();
    assert_eq!(call.status, "error");
    assert_eq!(call.turn_id, "claude:same-native-id:turn:turn1");
    assert!(!call.input.as_deref().unwrap().contains("secret-password"));
    assert!(!call.input.as_deref().unwrap().contains("with spaces"));
    assert!(!call.input.as_deref().unwrap().contains("sk-ant-"));
    assert!(call.output.as_deref().unwrap().contains("服务返回错误"));
    assert!(call.duration_ms.is_none());
}
#[test]
fn codex_mirrors_and_cumulative_usage_do_not_inflate_counts() {
    let dir = tempfile::tempdir().unwrap();
    let rows = vec![
        json!({"type":"session_meta","timestamp":"2026-10-17T10:00:00Z","payload":{"id":"same-native-id","cwd":"/projects/test","cli_version":"0.163.0-alpha.10.2","model_provider":"openai"}}),
        json!({"type":"turn_context","payload":{"turn_id":"turn1","model":"model-test"}}),
        json!({"type":"response_item","payload":{"type":"message","id":"u1","role":"user","content":[{"type":"input_text","text":"# AGENTS.md instructions\ncontext"}]}}),
        json!({"type":"response_item","payload":{"type":"message","id":"context-page","role":"user","content":[{"type":"input_text","text":"<external_codex_apps_open_page>{}</external_codex_apps_open_page>"}]}}),
        json!({"type":"response_item","payload":{"type":"message","id":"u2","role":"user","content":[{"type":"input_text","text":"检查端口"}]}}),
        json!({"type":"event_msg","payload":{"type":"user_message","message":"检查端口"}}),
        json!({"type":"response_item","payload":{"type":"function_call","id":"f1","call_id":"call1","name":"mcp__server__tool","arguments":"{\"port\":3000}"}}),
        json!({"type":"response_item","payload":{"type":"function_call_output","id":"f2","call_id":"call1","output":"ok"}}),
        json!({"type":"event_msg","payload":{"type":"item_completed","item":{"type":"McpToolCall","id":"call1","server":"server","tool":"tool","status":"completed","result":{"isError":false},"duration":{"secs":1,"nanos":500000000}}}}),
        json!({"type":"token_usage_record","payload":{"turn_id":"turn1","response_id":"response1","usage":{"input_tokens":100,"cached_input_tokens":50,"output_tokens":20},"thread_token_usage":{"input_tokens":100,"cached_input_tokens":50,"output_tokens":20}}}),
        json!({"type":"token_usage_record","payload":{"turn_id":"turn1","response_id":"response1","usage":{"input_tokens":100,"cached_input_tokens":50,"output_tokens":20},"thread_token_usage":{"input_tokens":100,"cached_input_tokens":50,"output_tokens":20}}}),
        json!({"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":100,"output_tokens":20}}}}),
        json!({"type":"response_item","payload":{"type":"message","id":"a1","role":"assistant","content":[{"type":"output_text","text":"检查完成"}]}}),
        json!({"type":"event_msg","payload":{"type":"agent_message","message":"检查完成"}}),
    ];
    let file = write_rows(dir.path(), "codex", "rollout-test", &rows);
    let parsed = parser::parse(
        dir.path(),
        &file,
        "codex",
        Some(parser::Window {
            offset: 0,
            anchor: None,
        }),
    )
    .unwrap();
    assert_eq!(parsed.summary.title, "检查端口");
    assert_eq!(parsed.summary.message_count, 2);
    assert_eq!(parsed.summary.model_calls, 1);
    assert_eq!(parsed.summary.mcp_calls, 1);
    assert_eq!(parsed.summary.usage.total_tokens, Some(120));
    let call = parsed.events.iter().find(|e| e.kind == "mcp").unwrap();
    assert_eq!(call.duration_ms, Some(1500));
    assert_eq!(call.status, "returned");
}
#[test]
fn consent_disconnect_and_namespaces_bound_read_access() {
    let dir = tempfile::tempdir().unwrap();
    write_rows(dir.path(), "claude", "a", &claude_rows());
    let service = NativeSessions::new(dir.path().into(), &[]);
    service.refresh().unwrap();
    assert_eq!(service.query(SessionQuery::default()).unwrap().total, 0);
    service.set_enabled("claude", true).unwrap();
    service.refresh().unwrap();
    let result = service.query(SessionQuery::default()).unwrap();
    assert_eq!(result.total, 1);
    assert!(service.conversation("../../outside", None, None).is_err());
    assert!(
        service
            .query(SessionQuery {
                agent: Some("other".into()),
                ..Default::default()
            })
            .is_err()
    );
    assert_eq!(
        service
            .query(SessionQuery {
                query: Some("/projects/test".into()),
                ..Default::default()
            })
            .unwrap()
            .total,
        1
    );
    service.set_enabled("claude", false).unwrap();
    assert_eq!(
        service
            .conversation(&result.items[0].id, None, None)
            .unwrap_err()
            .code,
        "not_found"
    );
}
#[test]
fn conversation_paging_anchor_and_partial_tail_are_explicit() {
    let dir = tempfile::tempdir().unwrap();
    let mut rows = claude_rows();
    for i in 0..90 {
        rows.push(json!({"type":"user","sessionId":"same-native-id","uuid":format!("extra{i}"),"message":{"content":format!("用户内容 {i}")}}));
    }
    let file = write_rows(dir.path(), "claude", "a", &rows);
    fs::OpenOptions::new()
        .append(true)
        .open(&file)
        .unwrap()
        .write_all(b"{broken}\n{\"unfinished\":")
        .unwrap();
    let service = NativeSessions::new(dir.path().into(), &["claude".into()]);
    service.refresh().unwrap();
    let id = &service.query(SessionQuery::default()).unwrap().items[0].id;
    let first = service.conversation(id, None, None).unwrap();
    assert_eq!(first.items.len(), 40);
    assert_eq!(first.warnings.len(), 2);
    let second = service
        .conversation(id, first.next_cursor.as_deref(), None)
        .unwrap();
    assert!(
        first
            .items
            .iter()
            .all(|a| second.items.iter().all(|b| a.id != b.id))
    );
    let anchor = &second.items[2].id;
    let anchored = service.conversation(id, None, Some(anchor)).unwrap();
    assert!(
        anchored
            .items
            .iter()
            .any(|e| &e.id == anchor && e.text.is_some())
    );
    fs::OpenOptions::new()
        .append(true)
        .open(&file)
        .unwrap()
        .write_all(b"0}\n")
        .unwrap();
    let appended = service
        .conversation(id, first.next_cursor.as_deref(), None)
        .unwrap();
    assert_eq!(appended.total, second.total);
    assert_eq!(
        appended
            .items
            .iter()
            .map(|event| &event.id)
            .collect::<Vec<_>>(),
        second
            .items
            .iter()
            .map(|event| &event.id)
            .collect::<Vec<_>>()
    );
    assert!(
        appended
            .warnings
            .iter()
            .any(|warning| warning.contains("快照"))
    );
    let rewritten = fs::read_to_string(&file)
        .unwrap()
        .replace("用户内容 0", "更改内容 0");
    fs::write(&file, rewritten).unwrap();
    assert_eq!(
        service
            .conversation(id, first.next_cursor.as_deref(), None)
            .unwrap_err()
            .code,
        "stale_snapshot"
    );
}
#[test]
fn duplicate_files_share_identity_and_deleted_files_leave_the_index() {
    let dir = tempfile::tempdir().unwrap();
    let a = write_rows(dir.path(), "claude", "a", &claude_rows());
    let b = a.with_file_name("copy.jsonl");
    fs::copy(&a, &b).unwrap();
    let service = NativeSessions::new(dir.path().into(), &["claude".into()]);
    service.refresh().unwrap();
    assert_eq!(service.query(SessionQuery::default()).unwrap().total, 1);
    fs::remove_file(&a).unwrap();
    fs::remove_file(&b).unwrap();
    service.refresh().unwrap();
    assert_eq!(service.query(SessionQuery::default()).unwrap().total, 0);
}

#[test]
fn native_title_indexes_are_preferred_and_refresh_without_transcript_changes() {
    let dir = tempfile::tempdir().unwrap();
    let rows = vec![
        json!({"type":"session_meta","payload":{"id":"native-title","cwd":"/projects/test"}}),
        json!({"type":"response_item","payload":{"type":"message","role":"user","id":"user1","content":[{"type":"input_text","text":"首条提问摘要"}]}}),
    ];
    write_rows(dir.path(), "codex", "rollout-title", &rows);
    let index = dir.path().join(".codex/session_index.jsonl");
    fs::write(
        &index,
        "{\"id\":\"native-title\",\"thread_name\":\"原生会话标题\"}\n",
    )
    .unwrap();
    let service = NativeSessions::new(dir.path().into(), &["codex".into()]);
    service.refresh().unwrap();
    let first = service.query(SessionQuery::default()).unwrap();
    assert_eq!(first.items[0].title, "原生会话标题");
    assert_eq!(first.items[0].title_source, "native_title");
    let detail = service
        .conversation(&first.items[0].id, None, None)
        .unwrap();
    assert_eq!(detail.session.title, "原生会话标题");
    assert!(detail.session.created_at.is_none());
    assert!(detail.session.updated_at.is_none());
    fs::write(
        index,
        "{\"id\":\"native-title\",\"thread_name\":\"更新后的标题\"}\n",
    )
    .unwrap();
    service.refresh().unwrap();
    assert_eq!(
        service.query(SessionQuery::default()).unwrap().items[0].title,
        "更新后的标题"
    );
}

#[test]
fn mixed_session_rows_are_not_merged_into_one_conversation() {
    let dir = tempfile::tempdir().unwrap();
    let mut rows = claude_rows();
    rows.push(json!({"type":"user","sessionId":"another-session","uuid":"foreign1","message":{"content":"foreign-body-must-not-appear"}}));
    let path = write_rows(dir.path(), "claude", "main", &rows);
    let parsed = parser::parse(
        dir.path(),
        &path,
        "claude",
        Some(parser::Window {
            offset: 0,
            anchor: None,
        }),
    )
    .unwrap();
    assert_eq!(parsed.summary.message_count, 3);
    assert!(parsed.summary.partial);
    assert!(parsed.events.iter().all(|e| {
        !e.text
            .as_deref()
            .unwrap_or("")
            .contains("foreign-body-must-not-appear")
    }));
    assert!(parsed.warnings.iter().any(|s| s.contains("会话身份不一致")));
}
#[cfg(unix)]
#[test]
fn symlinks_are_never_followed_and_source_consent_persists_without_content() {
    let dir = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let target = write_rows(outside.path(), "claude", "outside", &claude_rows());
    let root = dir.path().join(".claude/projects/project");
    fs::create_dir_all(&root).unwrap();
    std::os::unix::fs::symlink(target, root.join("linked.jsonl")).unwrap();
    let service = NativeSessions::new(dir.path().into(), &["claude".into()]);
    service.refresh().unwrap();
    assert_eq!(service.query(SessionQuery::default()).unwrap().total, 0);
    assert_eq!(service.sources().unwrap()[0].skipped_files, 1);
    let data = tempfile::tempdir().unwrap();
    let store = crate::Store::open(data.path()).unwrap();
    store.save_native_source("claude", true).unwrap();
    drop(store);
    let store = crate::Store::open(data.path()).unwrap();
    assert_eq!(store.native_sources().unwrap(), vec!["claude"]);
    store.save_native_source("claude", false).unwrap();
    assert!(store.native_sources().unwrap().is_empty());
    store.save_native_source("opencode", true).unwrap();
    assert_eq!(store.native_sources().unwrap(), vec!["opencode"]);
    assert!(store.save_native_source("unknown", true).is_err());
}

#[test]
fn codex_namespaced_mcp_calls_keep_server_arguments_results_and_duration() {
    let home = tempfile::tempdir().unwrap();
    let ts = now();
    write_rows(
        home.path(),
        "codex",
        "namespaced",
        &[
            json!({"type":"session_meta","timestamp":ts,"payload":{"id":"namespaced","originator":"codex_exec","cwd":home.path().join("project")}}),
            json!({"type":"turn_context","timestamp":ts,"payload":{"turn_id":"turn-one","model":"fixture-model"}}),
            json!({"type":"response_item","originator":"codex_exec","timestamp":ts,"payload":{"type":"function_call","call_id":"mcp-one","namespace":"mcp__fixture__source","name":"read","arguments":"{\"path\":\"example.py\"}"}}),
            json!({"type":"event_msg","originator":"codex_exec","timestamp":ts,"payload":{"type":"item_completed","item":{"type":"McpToolCall","id":"mcp-one","server":"fixture__source","tool":"read","status":"completed","result":{"content":[{"type":"text","text":"result evidence"}]},"duration":{"secs":0,"nanos":250000000}}}}),
            json!({"type":"response_item","originator":"codex_exec","timestamp":ts,"payload":{"type":"function_call","call_id":"ordinary","namespace":"unknown_fixture","name":"read","arguments":"{}"}}),
        ],
    );
    let sessions = NativeSessions::new(home.path().into(), &["codex".into()]);
    sessions.refresh().unwrap();
    let calls = sessions
        .calls(CallQuery {
            agent: Some("codex-cli".into()),
            kind: Some("mcp".into()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(calls.total, 1);
    assert_eq!(calls.items[0].server.as_deref(), Some("fixture__source"));
    assert_eq!(calls.items[0].tool_name.as_deref(), Some("read"));
    assert_eq!(calls.items[0].duration_ms, Some(250));
    let detail = sessions
        .conversation(&calls.items[0].session_id, None, Some(&calls.items[0].id))
        .unwrap();
    let event = detail
        .items
        .iter()
        .find(|e| e.id == calls.items[0].id)
        .unwrap();
    assert!(event.input.as_deref().unwrap().contains("example.py"));
    assert!(event.output.as_deref().unwrap().contains("result evidence"));
    assert_eq!(event.status, "returned");
    assert_eq!(
        sessions
            .calls(CallQuery {
                agent: Some("codex-cli".into()),
                kind: Some("tool".into()),
                ..Default::default()
            })
            .unwrap()
            .total,
        1
    );
}
