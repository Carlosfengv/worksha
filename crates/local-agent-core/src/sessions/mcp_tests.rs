use super::*;
use serde_json::{Value, json};
use std::collections::BTreeSet;

fn write(path: &Path, content: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        path,
        if path.extension().is_some_and(|s| s == "jsonl") {
            format!("{content}\n")
        } else {
            content.into()
        },
    )
    .unwrap();
}

#[test]
fn independent_clients_share_configs_but_not_call_or_response_ownership() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path();
    let project = home.join("repo");
    fs::create_dir_all(&project).unwrap();
    write(
        &home.join(".claude.json"),
        r#"{"mcpServers":{"shared":{"command":"never-run"},"other":{"command":"never-run"}}}"#,
    );
    write(
        &home.join(".codex/config.toml"),
        "[mcp_servers.shared]\ncommand='never-run'\n",
    );
    let ts = now();
    let mut rows = vec![
        json!({"type":"user","sessionId":"clients","uuid":"u","cwd":project,"timestamp":ts,"message":{"content":"shared"}}),
    ];
    for (id, client, tokens) in [
        ("cli", "cli", 110),
        ("desktop", "claude-desktop", 20),
        ("unknown", "sdk-cli", 30),
        ("cross", "cli", 40),
    ] {
        let mut tools =
            vec![json!({"type":"tool_use","id":id,"name":"mcp__shared__read","input":{}})];
        if id == "cli" {
            tools
                .push(json!({"type":"tool_use","id":"other","name":"mcp__other__read","input":{}}));
        }
        rows.push(json!({"type":"assistant","uuid":id,"entrypoint":client,"timestamp":ts,"message":{"id":id,"model":"fixture-model","usage":{"input_tokens":tokens,"output_tokens":0},"content":tools}}));
        if id == "cross" {
            rows.push(json!({"type":"user","uuid":"result","entrypoint":"claude-desktop","timestamp":ts,"message":{"content":[{"type":"tool_result","tool_use_id":"cross","content":"ok"}]}}));
        }
    }
    write(
        &home.join(".claude/projects/p/clients.jsonl"),
        &rows
            .iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join("\n"),
    );
    let rows = [
        json!({"type":"session_meta","timestamp":ts,"payload":{"id":"cx","cwd":project,"originator":"codex_work_desktop"}}),
        json!({"type":"response_item","originator":"codex_cli_rs","timestamp":ts,"payload":{"type":"function_call","call_id":"cxcli","name":"mcp__shared__read","arguments":"{}"}}),
        json!({"type":"response_item","originator":"codex_work_desktop","timestamp":ts,"payload":{"type":"function_call","call_id":"cxdesktop","name":"mcp__shared__read","arguments":"{}"}}),
        json!({"type":"response_item","timestamp":ts,"payload":{"type":"function_call","call_id":"cxunknown","name":"mcp__shared__read","arguments":"{}"}}),
    ];
    write(
        &home.join(".codex/sessions/cx.jsonl"),
        &rows
            .iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join("\n"),
    );
    let native = NativeSessions::new(home.into(), &["claude".into(), "codex".into()]);
    native.refresh().unwrap();
    let all = native.mcp(Default::default()).unwrap();
    assert_eq!(all.total_calls, 8);
    assert_eq!(all.usage_summary.related_tokens, Some(200));
    assert_eq!(all.usage_summary.related_responses, 4);
    assert_eq!(
        all.groups
            .iter()
            .filter_map(|g| g.related_tokens)
            .sum::<u64>(),
        310
    );
    let cases = [
        ("claude-code-cli", 2, Some(110)),
        ("claude-code-desktop", 2, Some(20)),
        ("codex-cli", 1, None),
        ("codex-desktop", 1, None),
        ("opencode", 0, None),
        ("unknown", 2, Some(30)),
    ];
    for (agent, count, tokens) in cases {
        let result = native
            .mcp(mcp::McpQuery {
                agent: Some(agent.into()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(result.total_calls, count, "{agent}");
        assert_eq!(result.usage_summary.related_tokens, tokens, "{agent}");
        assert!(result.calls.iter().all(|r| crate::agents::matches(
            Some(agent),
            &r.call.agent_id,
            r.call.agent_product_id.as_deref()
        )));
        assert!(
            result
                .services
                .iter()
                .all(|s| s.applicable_agent_ids.len() <= 2)
        );
        if agent == "claude-code-desktop" {
            let cross = result
                .calls
                .iter()
                .find(|r| {
                    r.call.tool_name.as_deref() == Some("read")
                        && r.related_usage_state == "outside_agent_scope"
                })
                .unwrap();
            assert_eq!(
                cross.related_agent_product_id.as_deref(),
                Some("claude-code-cli")
            );
            assert!(cross.related_usage.is_none());
            assert_eq!(
                result
                    .services
                    .iter()
                    .filter(|s| s.name == "shared")
                    .count(),
                1
            );
        }
    }
    let catalog = native.project_catalog().unwrap();
    let projects =
        crate::projects::query(&catalog, Default::default(), &Default::default()).unwrap();
    let project = &projects.items[0].identity.id;
    let models = native
        .calls(CallQuery {
            project: Some(project.clone()),
            kind: Some("model".into()),
            agent: Some("claude-code-cli".into()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(models.total, 2);
    assert_eq!(
        models
            .items
            .iter()
            .filter_map(|c| c.usage.as_ref().and_then(|u| u.total_tokens))
            .sum::<u64>(),
        150
    );
    assert_eq!(
        native
            .calls(CallQuery {
                project: Some(format!("project:{}", "0".repeat(64))),
                ..Default::default()
            })
            .unwrap()
            .total,
        0
    );
    assert_eq!(
        native
            .calls(CallQuery {
                project: Some("not-a-project".into()),
                ..Default::default()
            })
            .err()
            .unwrap()
            .code,
        "invalid_input"
    );
}

#[test]
fn mcp_name_rollup_filters_and_unique_related_response_tokens() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path();
    let a = home.join("repo-a");
    let b = home.join("repo-b");
    fs::create_dir_all(a.join(".git")).unwrap();
    fs::create_dir_all(b.join(".git")).unwrap();
    write(&home.join(".claude.json"), &json!({"projects":{a.to_str().unwrap():{"mcpServers":{"shared":{"command":"private-a"},"other":{"command":"private-other"}}}}}).to_string());
    write(
        &home.join(".codex/config.toml"),
        "[mcp_servers.shared]\ncommand='private-codex'\n",
    );
    let ts = now();
    let cl = [
        json!({"type":"user","uuid":"u","sessionId":"cl","cwd":a,"timestamp":ts,"message":{"content":"rollup test"}}),
        json!({"type":"assistant","uuid":"a1","sessionId":"cl","timestamp":ts,"message":{"id":"r1","model":"alpha","usage":{"input_tokens":100,"output_tokens":10},"content":[{"type":"tool_use","id":"c1","name":"mcp__shared__read","input":{}},{"type":"tool_use","id":"c2","name":"mcp__shared__write","input":{}},{"type":"tool_use","id":"c3","name":"mcp__other__read","input":{}}]}}),
        json!({"type":"assistant","uuid":"a2","sessionId":"cl","timestamp":ts,"message":{"id":"r2","model":"beta","usage":{"input_tokens":10,"output_tokens":5},"content":[{"type":"tool_use","id":"c4","name":"mcp__shared__read","input":{}}]}}),
    ];
    write(
        &home.join(".claude/projects/a/cl.jsonl"),
        &cl.iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join("\n"),
    );
    let cx = [
        json!({"type":"session_meta","timestamp":ts,"payload":{"id":"cx","cwd":b}}),
        json!({"type":"turn_context","timestamp":ts,"payload":{"turn_id":"t","model":"gamma"}}),
        json!({"type":"event_msg","timestamp":ts,"payload":{"type":"item_completed","item":{"type":"McpToolCall","id":"cx1","server":"shared","tool":"read","status":"completed","duration":{"secs":1,"nanos":0}}}}),
    ];
    write(
        &home.join(".codex/sessions/cx.jsonl"),
        &cx.iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join("\n"),
    );
    let sessions = NativeSessions::new(home.into(), &["claude".into(), "codex".into()]);
    sessions.refresh().unwrap();
    let all = sessions.mcp(Default::default()).unwrap();
    let shared = all.groups.iter().find(|g| g.name == "shared").unwrap();
    assert_eq!(shared.call_count, Some(4));
    assert_eq!(shared.agent_ids, vec!["claude-unknown", "codex-unknown"]);
    assert_eq!(
        shared.configured_agent_ids,
        vec![
            "claude-code-cli",
            "claude-code-desktop",
            "codex-cli",
            "codex-desktop"
        ]
    );
    assert_eq!(all.usage_summary.related_tokens, Some(125));
    assert_eq!(all.usage_summary.related_responses, 2);
    assert_eq!(shared.configuration_count, 2);
    assert_eq!(
        shared.last_call_at.as_deref(),
        all.calls
            .iter()
            .find(|r| r.call.agent_id == "claude")
            .unwrap()
            .call
            .timestamp
            .as_deref()
    );
    assert_eq!(shared.related_tokens, Some(125));
    assert_eq!(shared.related_responses, 2);
    assert_eq!(shared.token_covered_calls, 3);
    assert_eq!(shared.token_missing_calls, 1);
    assert_eq!(shared.p95_ms, Some(1000));
    assert_eq!(
        all.groups
            .iter()
            .find(|g| g.name == "other")
            .unwrap()
            .related_tokens,
        Some(110)
    );
    let scope = mcp::McpQuery {
        service: Some(shared.id.clone()),
        ..Default::default()
    };
    let detail = sessions.mcp(scope.clone()).unwrap();
    assert_eq!(detail.total_calls, 4);
    assert_eq!(
        detail.selected_group_id.as_deref(),
        Some(shared.id.as_str())
    );
    assert_eq!(detail.facets.models.len(), 3);
    assert_eq!(detail.facets.projects.len(), 2);
    let alpha = detail
        .facets
        .models
        .iter()
        .find(|f| f.label == "alpha")
        .unwrap()
        .id
        .clone();
    let project = detail
        .facets
        .projects
        .iter()
        .find(|f| f.label == a.to_str().unwrap())
        .unwrap()
        .id
        .clone();
    let hidden_paths = std::collections::HashSet::from([a.to_str().unwrap().to_string()]);
    let hidden = sessions
        .mcp_with_hidden_projects(scope.clone(), &hidden_paths)
        .unwrap();
    assert_eq!(hidden.total_calls, detail.total_calls);
    assert_eq!(
        hidden.usage_summary.related_tokens,
        detail.usage_summary.related_tokens
    );
    assert!(!hidden.facets.projects.iter().any(|p| p.id == project));
    assert!(
        hidden
            .calls
            .iter()
            .any(|c| c.project_path.as_deref() == a.to_str())
    );
    assert_eq!(
        sessions
            .mcp_with_hidden_projects(
                mcp::McpQuery {
                    project: Some(project.clone()),
                    ..scope.clone()
                },
                &hidden_paths
            )
            .unwrap_err()
            .code,
        "project_hidden"
    );
    let filtered = sessions
        .mcp(mcp::McpQuery {
            agent: Some("claude".into()),
            model: Some(alpha),
            project: Some(project),
            ..scope.clone()
        })
        .unwrap();
    assert_eq!(filtered.total_calls, 2);
    assert!(
        filtered
            .calls
            .iter()
            .all(|r| r.call.model.as_deref() == Some("alpha")
                && r.call.agent_id == "claude"
                && r.project_path.as_deref() == a.to_str())
    );
    let stats = filtered.groups.iter().find(|g| g.id == shared.id).unwrap();
    assert_eq!(stats.related_tokens, Some(110));
    assert_eq!(stats.related_responses, 1);
    assert_eq!(stats.duration_samples, 0);
    assert_eq!(stats.p95_ms, None);
    let codex = sessions
        .mcp(mcp::McpQuery {
            agent: Some("codex".into()),
            ..scope.clone()
        })
        .unwrap();
    assert_eq!(codex.total_calls, 1);
    assert_eq!(
        codex
            .groups
            .iter()
            .find(|g| g.id == shared.id)
            .unwrap()
            .related_tokens,
        None
    );
    let unknown = sessions
        .mcp(mcp::McpQuery {
            model: Some("unknown".into()),
            ..scope
        })
        .unwrap();
    assert_eq!(unknown.total_calls, 0);
    assert_eq!(
        sessions
            .mcp(mcp::McpQuery {
                model: Some("raw model".into()),
                ..Default::default()
            })
            .unwrap_err()
            .code,
        "invalid_input"
    );
    let revoked = shared.id.clone();
    sessions.set_enabled("claude", false).unwrap();
    let after = sessions
        .mcp(mcp::McpQuery {
            service: Some(revoked),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(after.total_calls, 1);
    assert!(after.calls.iter().all(|r| r.call.agent_id == "codex"));
}

#[test]
fn mcp_scopes_statistics_paging_session_links_and_source_revocation() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    let repo = home.join("repo");
    fs::create_dir_all(repo.join(".git")).unwrap();
    let native_config = home.join(".claude.json");
    let config = json!({"mcpServers":{"shared":{"command":"global-server"}},"projects":{repo.to_str().unwrap():{"mcpServers":{"shared":{"command":"private-server"}}}}});
    write(&native_config, &config.to_string());
    write(
        &repo.join(".mcp.json"),
        r#"{"mcpServers":{"shared":{"command":"project-server"},"no_calls":{"type":"http","url":"https://private"}}}"#,
    );
    write(
        &home.join(".codex/config.toml"),
        "[mcp_servers.shared]\ncommand='global-server'\n",
    );
    write(
        &repo.join(".codex/config.toml"),
        "[mcp_servers.shared]\nurl='https://private'\nenabled=false\n",
    );
    let ts = now();
    let mut rows = vec![
        json!({"type":"user","sessionId":"cl","uuid":"user0","cwd":repo,"timestamp":ts,"message":{"content":"MCP fixture session"}}),
    ];
    for n in 0..45 {
        rows.push(json!({"type":"assistant","sessionId":"cl","uuid":format!("a{n}"),"timestamp":ts,"message":{"id":format!("msg{n}"),"content":[{"type":"tool_use","id":format!("call{n}"),"name":"mcp__shared__read","input":{"query":"query-sentinel","secret":"body-sentinel"}}]}}));
        if n < 44 {
            rows.push(json!({"type":"user","sessionId":"cl","uuid":format!("r{n}"),"timestamp":ts,"message":{"content":[{"type":"tool_result","tool_use_id":format!("call{n}"),"is_error":n<5,"content":"result-sentinel"}]}}));
        }
    }
    write(
        &home.join(".claude/projects/repo/cl.jsonl"),
        &rows
            .iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join("\n"),
    );
    let codex = [
        json!({"type":"session_meta","timestamp":ts,"payload":{"id":"cx","cwd":repo}}),
        json!({"type":"event_msg","timestamp":ts,"payload":{"type":"item_completed","item":{"type":"McpToolCall","id":"codex1","server":"shared","tool":"read","status":"completed","result":{"isError":false},"duration":{"secs":1,"nanos":500000000}}}}),
    ];
    write(
        &home.join(".codex/sessions/cx.jsonl"),
        &codex
            .iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join("\n"),
    );
    let service = NativeSessions::new(home.into(), &["claude".into(), "codex".into()]);
    service.refresh().unwrap();
    let result = service.mcp(Default::default()).unwrap();
    assert_eq!(result.total_calls, 46);
    let private = result
        .services
        .iter()
        .find(|s| s.agent_id == "claude" && s.sources.iter().any(|p| p.scope == "project_local"))
        .unwrap();
    assert_eq!(private.call_count, Some(45));
    assert_eq!(private.error_count, 5);
    assert_eq!(private.returned_count, 39);
    assert_eq!(private.unknown_count, 1);
    assert_eq!(private.p95_ms, None);
    assert_eq!(private.duration_samples, 0);
    assert_eq!(private.session_count, 1);
    assert_eq!(private.recorded_tool_count, 1);
    assert_eq!(private.error_rate, Some(5.0 / 44.0));
    assert!(
        result
            .services
            .iter()
            .filter(|s| s.agent_id == "claude" && s.id != private.id)
            .all(|s| s.call_count == Some(0))
    );
    let codex_unknown = result
        .services
        .iter()
        .find(|s| s.agent_id == "codex" && s.configuration_status == "unknown")
        .unwrap();
    assert_eq!(codex_unknown.call_count, Some(1));
    assert_eq!(codex_unknown.p95_ms, Some(1500));
    assert_eq!(codex_unknown.error_rate, Some(0.0));
    assert!(
        result
            .services
            .iter()
            .filter(|s| s.agent_id == "codex" && s.configuration_status == "present")
            .all(|s| s.call_count == Some(0))
    );
    let selected = service
        .mcp(mcp::McpQuery {
            agent: Some("claude".into()),
            service: Some(private.id.clone()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(selected.total_calls, 45);
    assert_eq!(selected.calls.len(), 40);
    assert!(
        selected
            .calls
            .iter()
            .all(|c| c.service_id == private.id && c.association == "current_configuration")
    );
    let cursor = selected.next_cursor.clone().unwrap();
    let last = service
        .mcp(mcp::McpQuery {
            agent: Some("claude".into()),
            service: Some(private.id.clone()),
            cursor: Some(cursor.clone()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(last.calls.len(), 5);
    let call = &selected.calls[0].call;
    assert!(
        service
            .conversation(&call.session_id, None, Some(&call.id))
            .unwrap()
            .items
            .iter()
            .any(|e| e.id == call.id)
    );
    // A lookup outside page one still returns the full service sample count.
    let detail = service
        .mcp(mcp::McpQuery {
            call: Some(last.calls[0].call.id.clone()),
            agent: Some("claude".into()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(detail.total_calls, 1);
    assert_eq!(
        detail.selected_service_id.as_deref(),
        Some(private.id.as_str())
    );
    assert_eq!(
        detail
            .services
            .iter()
            .find(|s| s.id == private.id)
            .unwrap()
            .call_count,
        Some(45)
    );
    let body = service
        .conversation(&call.session_id, None, Some(&call.id))
        .unwrap();
    let event = body.items.iter().find(|e| e.id == call.id).unwrap();
    assert!(event.input.as_ref().unwrap().contains("query-sentinel"));
    assert!(!event.input.as_ref().unwrap().contains("body-sentinel"));
    if event.status != "missing_result" {
        assert_eq!(event.output.as_deref(), Some("result-sentinel"));
    }
    assert_eq!(
        service
            .mcp(mcp::McpQuery {
                call: Some(call.id.clone()),
                agent: Some("opencode".into()),
                ..Default::default()
            })
            .unwrap_err()
            .code,
        "not_found"
    );
    let text = serde_json::to_string(&selected).unwrap();
    for value in [
        "body-sentinel",
        "result-sentinel",
        "global-server",
        "private-server",
        "project-server",
        "https://private",
    ] {
        assert!(!text.contains(value));
    }
    let mut changed = config.clone();
    changed["projects"][repo.to_str().unwrap()]["mcpServers"]["shared"]["enabled"] = false.into();
    write(&native_config, &changed.to_string());
    assert_eq!(
        service
            .mcp(mcp::McpQuery {
                agent: Some("claude".into()),
                service: Some(private.id.clone()),
                cursor: Some(cursor),
                ..Default::default()
            })
            .unwrap_err()
            .code,
        "stale_snapshot"
    );
    service.set_enabled("claude", false).unwrap();
    let revoked = service
        .mcp(mcp::McpQuery {
            agent: Some("claude".into()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(revoked.total_calls, 0);
    assert!(revoked.services.iter().all(|s| s.call_count.is_none()));
}

#[test]
fn mcp_damaged_configs_and_unknown_session_date_are_explicit() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    write(&home.join(".claude.json"), "{secret-broken");
    write(&home.join(".codex/config.toml"), "invalid=[secret-broken");
    let rows = [
        json!({"type":"session_meta","payload":{"id":"old","cwd":home}}),
        json!({"type":"response_item","payload":{"type":"function_call","call_id":"oldcall","name":"mcp__old__tool","arguments":"{}"}}),
    ];
    write(
        &home.join(".codex/sessions/old.jsonl"),
        &rows
            .iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join("\n"),
    );
    let service = NativeSessions::new(home.into(), &["codex".into()]);
    service.refresh().unwrap();
    let all = service.mcp(Default::default()).unwrap();
    assert_eq!(all.total_calls, 1);
    assert!(all.warnings.iter().any(|s| s.contains("claude 配置")));
    assert!(all.warnings.iter().any(|s| s.contains("codex 配置")));
    assert!(
        !serde_json::to_string(&all)
            .unwrap()
            .contains("secret-broken")
    );
    let recent = service
        .mcp(mcp::McpQuery {
            days: Some(7),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(recent.total_calls, 0);
    assert_eq!(
        service
            .mcp(mcp::McpQuery {
                days: Some(2),
                ..Default::default()
            })
            .unwrap_err()
            .code,
        "invalid_input"
    );
    #[cfg(unix)]
    {
        fs::remove_file(home.join(".claude.json")).unwrap();
        std::os::unix::fs::symlink(home.join(".codex/config.toml"), home.join(".claude.json"))
            .unwrap();
        assert!(
            service
                .mcp(Default::default())
                .unwrap()
                .warnings
                .iter()
                .any(|s| s.contains("claude 配置"))
        );
    }
}

#[test]
fn mcp_inventory_config_only_keeps_counts_and_connections_unknown() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    fs::create_dir_all(home.join(".codex")).unwrap();
    fs::create_dir_all(home.join(".config/opencode")).unwrap();
    fs::write(home.join(".claude.json"),json!({"mcpServers":{"shared":{"type":"http","url":"https://private/?token=secret-url","headers":{"Authorization":"secret-header"}}},"projects":{home.join("project").to_str().unwrap():{"mcpServers":{"private":{"command":"never-execute","env":{"KEY":"secret-env"}}}}}}).to_string()).unwrap();
    fs::write(home.join(".codex/config.toml"),"[mcp_servers.shared]\nurl='https://private/'\nenabled=false\nhttp_headers={ Authorization='secret-header' }\n").unwrap();
    fs::write(
        home.join(".config/opencode/opencode.jsonc"),
        r#"{// comment
      "mcp":{"shared":{"type":"local","command":["never-execute"],"enabled":true}},}"#,
    )
    .unwrap();
    let service = NativeSessions::new(home.into(), &[]);
    let result = service.mcp(Default::default()).unwrap();
    assert_eq!(result.services.len(), 4);
    assert!(result.calls.is_empty());
    assert!(result.services.iter().all(|s| s.call_count.is_none()
        && s.p95_ms.is_none()
        && s.error_rate.is_none()
        && s.connection_status == "unknown"
        && s.tool_catalog_count.is_none()));
    assert_eq!(
        result
            .services
            .iter()
            .find(|s| s.agent_id == "codex")
            .unwrap()
            .configured_enabled,
        Some(false)
    );
    let text = serde_json::to_string(&result).unwrap();
    for secret in [
        "secret-url",
        "secret-header",
        "secret-env",
        "never-execute",
        "https://private",
    ] {
        assert!(!text.contains(secret));
    }
    assert_eq!(
        service
            .mcp(mcp::McpQuery {
                agent: Some("invalid".into()),
                ..Default::default()
            })
            .unwrap_err()
            .code,
        "invalid_input"
    );
}

#[test]
fn mcp_cancel_timeout_refusal_and_unfinished_calls_do_not_improve_success_metrics() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    write(
        &home.join(".codex/config.toml"),
        "[mcp_servers.svc]\ncommand='not-executed'\n",
    );
    let ts = now();
    let mut rows =
        vec![json!({"type":"session_meta","timestamp":ts,"payload":{"id":"statuses","cwd":home}})];
    for (n, status) in [
        "completed",
        "failed",
        "cancelled",
        "timed_out",
        "declined",
        "running",
    ]
    .iter()
    .enumerate()
    {
        rows.push(json!({"type":"event_msg","timestamp":ts,"payload":{"type":"item_completed","item":{"type":"McpToolCall","id":format!("call{n}"),"server":"svc","tool":"read","status":status,"duration":{"secs":n+1,"nanos":0},"error":if *status=="failed" {json!({"message":"native-rpc-error-sentinel","api_key":"secret-rpc-key"})} else {Value::Null}}}}));
    }
    write(
        &home.join(".codex/sessions/statuses.jsonl"),
        &rows
            .iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join("\n"),
    );
    let service = NativeSessions::new(home.into(), &["codex".into()]);
    service.refresh().unwrap();
    let result = service.mcp(Default::default()).unwrap();
    let stats = &result.services[0];
    assert_eq!(stats.call_count, Some(6));
    assert_eq!(stats.returned_count, 1);
    assert_eq!(stats.error_count, 1);
    assert_eq!(stats.unknown_count, 4);
    assert_eq!(stats.duration_samples, 2);
    assert_eq!(stats.p95_ms, Some(2000));
    assert_eq!(stats.error_rate, Some(0.5));
    let failed = result
        .calls
        .iter()
        .find(|c| c.call.status == "error")
        .unwrap();
    let body = service
        .conversation(&failed.call.session_id, None, Some(&failed.call.id))
        .unwrap();
    let event = body.items.iter().find(|e| e.id == failed.call.id).unwrap();
    assert_eq!(event.duration_ms, Some(2000));
    assert!(event.input.is_none());
    assert!(
        event
            .output
            .as_ref()
            .unwrap()
            .contains("native-rpc-error-sentinel")
    );
    assert!(!event.output.as_ref().unwrap().contains("secret-rpc-key"));
    let statuses = result
        .calls
        .iter()
        .map(|c| c.call.status.as_str())
        .collect::<BTreeSet<_>>();
    assert!(
        statuses.contains("cancelled")
            && statuses.contains("timeout")
            && statuses.contains("declined")
            && statuses.contains("missing_result")
    );
}
