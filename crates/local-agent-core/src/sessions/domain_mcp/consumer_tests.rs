use super::*;
use serde_json::{Value, json};
use std::{
    cell::{Cell, RefCell},
    fs,
    io::Write,
    path::PathBuf,
};

struct Fixture {
    _dir: tempfile::TempDir,
    native: NativeSessions,
    source: PathBuf,
    marker: PathBuf,
}
fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().canonicalize().unwrap();
    let marker = home.join("must-not-execute");
    let project = home.join("managed-project");
    fs::create_dir_all(&project).unwrap();
    fs::write(home.join(".claude.json"), serde_json::to_vec(&json!({"mcpServers":{
        "fixture":{"command":"/usr/bin/touch","args":[marker],"env":{"API_KEY":"private-mcp-fixture-secret"}},
        "other":{"command":"never-run"}
    }})).unwrap()).unwrap();
    let source = home.join(".claude/projects/fixture/history.jsonl");
    fs::create_dir_all(source.parent().unwrap()).unwrap();
    let mut out = fs::File::create(&source).unwrap();
    let base = chrono::Utc::now() - chrono::Duration::minutes(5);
    writeln!(out, "{}", json!({"type":"user","sessionId":"common-mcp","uuid":"input","cwd":project,"entrypoint":"cli","timestamp":base.to_rfc3339(),"message":{"content":"MCP fixture"}})).unwrap();
    for n in 0..70 {
        let client = ["cli", "claude-desktop", "sdk-cli"][n % 3];
        let ts = base + chrono::Duration::seconds(n as i64 * 2);
        let mut tools = vec![
            json!({"type":"tool_use","id":format!("tool-{n}"),"name":"mcp__fixture__read","input":{}}),
        ];
        let mut results = vec![
            json!({"type":"tool_result","tool_use_id":format!("tool-{n}"),"content":"fixture result"}),
        ];
        if n % 2 == 0 {
            tools.push(json!({"type":"tool_use","id":format!("other-{n}"),"name":"mcp__other__read","input":{}}));
            results.push(json!({"type":"tool_result","tool_use_id":format!("other-{n}"),"content":"fixture result"}));
        }
        writeln!(out, "{}", json!({"type":"assistant","sessionId":"common-mcp","uuid":format!("response-{n}"),"entrypoint":client,"timestamp":ts.to_rfc3339(),"message":{"id":format!("response-{n}"),"model":"fixture-model","usage":{"input_tokens":10,"output_tokens":2},"content":tools}})).unwrap();
        writeln!(out, "{}", json!({"type":"user","sessionId":"common-mcp","uuid":format!("result-{n}"),"entrypoint":client,"timestamp":(ts+chrono::Duration::seconds(1)).to_rfc3339(),"message":{"content":results}})).unwrap();
    }
    drop(out);
    let native = NativeSessions::new(home, &["claude".into()]);
    native.refresh().unwrap();
    Fixture {
        _dir: dir,
        native,
        source,
        marker,
    }
}
fn stable(value: &McpSnapshot) -> Value {
    let mut value = serde_json::to_value(value).unwrap();
    value.as_object_mut().unwrap().remove("asOf");
    value
}
fn common(native: &NativeSessions, private: &McpSnapshot) -> m::McpFacts {
    normalize::snapshot(
        private,
        &normalize::project_ids(&native.lock().unwrap(), &Default::default()),
    )
}
fn compare(
    native: &NativeSessions,
    query: McpQuery,
    hidden: &std::collections::HashSet<String>,
) -> McpSnapshot {
    let expected = native.read_native_mcp(query.clone(), hidden).unwrap();
    let actual = native
        .domain_mcp_with_hidden_projects(query, hidden)
        .unwrap();
    assert_eq!(stable(&expected), stable(&actual));
    actual
}

#[test]
fn common_mcp_consumer_preserves_all_filters_full_range_statistics_and_cursor_pages() {
    let f = fixture();
    let original = fs::read(&f.source).unwrap();
    let hidden = Default::default();
    let all = compare(&f.native, McpQuery::default(), &hidden);
    assert_eq!(all.total_calls, 105);
    assert_eq!(all.usage_summary.related_tokens, Some(840));
    assert_eq!(
        all.groups
            .iter()
            .filter_map(|g| g.related_tokens)
            .sum::<u64>(),
        1260
    );
    for agent in [
        "claude",
        "claude-code-cli",
        "claude-code-desktop",
        "codex-cli",
        "codex-desktop",
        "opencode",
        "antigravity",
        "unknown",
    ] {
        compare(
            &f.native,
            McpQuery {
                agent: Some(agent.into()),
                ..Default::default()
            },
            &hidden,
        );
    }
    for days in [7, 30] {
        compare(
            &f.native,
            McpQuery {
                days: Some(days),
                ..Default::default()
            },
            &hidden,
        );
    }
    for service in all
        .services
        .iter()
        .map(|s| &s.id)
        .chain(all.groups.iter().map(|g| &g.id))
    {
        compare(
            &f.native,
            McpQuery {
                service: Some(service.clone()),
                ..Default::default()
            },
            &hidden,
        );
    }
    compare(
        &f.native,
        McpQuery {
            model: Some(all.facets.models[0].id.clone()),
            project: Some(all.facets.projects[0].id.clone()),
            ..Default::default()
        },
        &hidden,
    );
    compare(
        &f.native,
        McpQuery {
            call: Some(all.calls[0].call.id.clone()),
            ..Default::default()
        },
        &hidden,
    );
    let mut cursor = None;
    let mut sizes = vec![];
    loop {
        let page = compare(
            &f.native,
            McpQuery {
                cursor,
                ..Default::default()
            },
            &hidden,
        );
        assert_eq!(page.usage_summary.related_tokens, Some(840));
        assert_eq!(page.total_calls, 105);
        sizes.push(page.calls.len());
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(sizes, [40, 40, 25]);
    let raw = f
        .native
        .domain_mcp_facts(Default::default(), &hidden)
        .unwrap();
    let session = f.native.query(Default::default()).unwrap().items.remove(0);
    assert_eq!(raw.executions[0].project_ref, session.project_id);
    let encoded = serde_json::to_string(&raw).unwrap();
    assert!(!encoded.contains(f._dir.path().to_str().unwrap()));
    assert!(!encoded.contains("private-mcp-fixture-secret"));
    assert_eq!(fs::read(&f.source).unwrap(), original);
    assert!(!f.marker.exists());
}

#[test]
fn compatibility_response_really_consumes_common_catalog_group_usage_and_facet_results() {
    let f = fixture();
    let private = f.native.mcp(Default::default()).unwrap();
    let mut common = common(&f.native, &private);
    common.catalogs[0].name = "统一目录结果".into();
    common.groups[0].name = "统一分组结果".into();
    common.usage_summary.related_tokens = Some(9007199254740993u64.into());
    common.model_facets[0].label = "统一模型结果".into();
    let result = consume::snapshot(common, private).unwrap();
    assert_eq!(result.services[0].name, "统一目录结果");
    assert_eq!(result.groups[0].name, "统一分组结果");
    assert_eq!(result.usage_summary.related_tokens, Some(9007199254740993));
    assert_eq!(result.facets.models[0].label, "统一模型结果");
}

#[test]
fn common_result_cannot_bind_unknown_native_paths_or_invent_connection_proof() {
    let f = fixture();
    for corrupt in 0..6 {
        let private = f.native.mcp(Default::default()).unwrap();
        let mut common = common(&f.native, &private);
        match corrupt {
            0 => common.project_facets = vec!["/not-an-authorized-path".into()],
            1 => common.catalogs[0].connection_status = "connected".into(),
            2 => common.executions[0].service_id = "unrelated-catalog".into(),
            3 => common.groups[0].error_rate = Some(f64::NAN),
            4 => common.executions[0].response_id = Some("foreign-response".into()),
            _ => common.catalogs[0].observation_status = "not_enabled".into(),
        }
        assert!(consume::snapshot(common, private).is_err());
    }
    assert!(!f.marker.exists());
}

#[test]
fn common_project_objects_use_native_identity_and_hidden_or_conflicting_evidence_does_not_link() {
    let f = fixture();
    let snapshot = f.native.mcp(Default::default()).unwrap();
    let project = snapshot.calls[0].project_path.clone().unwrap();
    let hidden = std::collections::HashSet::from([project.clone()]);
    let facts = f
        .native
        .domain_mcp_facts(Default::default(), &hidden)
        .unwrap();
    assert!(facts.executions.iter().all(|e| e.project_ref.is_none()));
    let state = f.native.lock().unwrap();
    let bindings = normalize::project_ids(&state, &hidden);
    assert!(!bindings.contains_key(&project));
    drop(state);
    let mut private = f.native.mcp(Default::default()).unwrap();
    private.services[0].project_path = Some("/configuration-without-session-evidence".into());
    let facts = common(&f.native, &private);
    assert_eq!(facts.catalogs[0].project_ref, None);
    assert!(facts.catalogs[0].project_facet_ref.is_some());
    let mut state = f.native.lock().unwrap();
    let mut conflict = state.files.values().next().unwrap().clone();
    conflict.summary.native_id = "conflicting-session".into();
    conflict.summary.id = "claude:conflicting-session".into();
    conflict.summary.project_id = Some("different-project-evidence".into());
    conflict.path = f.source.with_file_name("metadata-only-conflict.jsonl");
    state.files.insert(conflict.path.clone(), conflict);
    assert!(!normalize::project_ids(&state, &Default::default()).contains_key(&project));
}

#[test]
fn forged_mcp_invocation_and_late_plugin_stop_never_publish_a_private_frame() {
    let f = fixture();
    let calls = Cell::new(0);
    let pending = RefCell::new(Some(f.native.mcp(Default::default()).unwrap()));
    let lease = f.native.source_plugins.lease("claude").unwrap();
    let expected = m::McpRequest {
        invocation: d::Invocation {
            invocation_id: "fixture".into(),
            provider_key: "native-mcp".into(),
            implementation_identity: "fixture".into(),
            epoch: 1u64.into(),
            grant_revision: 1u64.into(),
            catalog_revision: 1u64.into(),
            resource_ref: "opaque".into(),
            deadline_ms: None,
            budget: d::Budget {
                batch_items: 40,
                max_records: u64::MAX.into(),
                max_document_bytes: u64::MAX.into(),
            },
        },
        filter: (&McpQuery::default()).into(),
    };
    let broker = || {
        calls.set(calls.get() + 1);
        f.native.stop_source_plugin("claude")?;
        Ok(pending.borrow_mut().take().unwrap())
    };
    let check = || {
        lease
            .ensure_active()
            .map_err(|e| NativeError::new(e.code, &e.message))
    };
    let mut reader = NativeRead {
        broker: &broker,
        check: &check,
        expected: expected.clone(),
        legacy: None,
        error: None,
        project_ids: normalize::project_ids(&f.native.lock().unwrap(), &Default::default()),
    };
    let mut forged = expected.clone();
    forged.invocation.resource_ref = "other-resource".into();
    assert_eq!(
        reader.read_mcp(&forged).unwrap_err().code,
        d::ErrorCode::StaleSnapshot
    );
    assert_eq!(calls.get(), 0);
    assert_eq!(
        reader.read_mcp(&expected).unwrap_err().code,
        d::ErrorCode::PluginDisabled
    );
    assert!(reader.legacy.is_none());
}
