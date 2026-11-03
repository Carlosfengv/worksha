//! Explicit capacity run; fixtures never point at the user's source home.
use super::*;
use serde_json::json;
use std::{io::Write, time::Instant};

#[test]
#[ignore = "generated native directory capacity benchmark; run explicitly without other builds"]
fn native_directory_first_scan_and_statistics_capacity() {
    let streaming = std::env::var("LOCAL_AGENT_NATIVE_STREAMING").as_deref() == Ok("1");
    let files: usize = std::env::var("LOCAL_AGENT_NATIVE_FILES")
        .unwrap_or("1000".into())
        .parse()
        .unwrap();
    let calls: usize = std::env::var("LOCAL_AGENT_NATIVE_CALLS")
        .unwrap_or("100".into())
        .parse()
        .unwrap();
    let rounds: usize = std::env::var("LOCAL_AGENT_NATIVE_ROUNDS")
        .unwrap_or("3".into())
        .parse()
        .unwrap();
    assert!(
        (1..=5000).contains(&files) && (1..=1000).contains(&calls) && (1..=5).contains(&rounds)
    );
    let home = tempfile::tempdir().unwrap();
    let project = home.path().join("workspace");
    fs::create_dir_all(project.join(".git")).unwrap();
    let ts = (Utc::now() - chrono::Duration::minutes(1)).to_rfc3339();
    for agent in ["claude", "codex"] {
        let root = if agent == "claude" {
            home.path().join(".claude/projects/project")
        } else {
            home.path().join(".codex/sessions/2026/10/19")
        };
        fs::create_dir_all(&root).unwrap();
        for file in 0..files {
            let id = format!("capacity-{file:05}");
            let mut out = std::io::BufWriter::new(
                fs::File::create(root.join(format!("{id}.jsonl"))).unwrap(),
            );
            if agent == "claude" {
                writeln!(out,"{}",json!({"type":"user","sessionId":id,"uuid":"user","cwd":project,"timestamp":ts,"message":{"content":"capacity fixture"}})).unwrap();
            } else {
                writeln!(out,"{}",json!({"type":"session_meta","timestamp":ts,"payload":{"id":id,"cwd":project,"originator":"codex_cli_rs"}})).unwrap();
                writeln!(out,"{}",json!({"type":"turn_context","timestamp":ts,"payload":{"model":"capacity-model","turn_id":"turn"}})).unwrap();
            }
            for call in 0..calls {
                let response = format!("response-{file}-{call}");
                let row = if agent == "claude" {
                    json!({"type":"assistant","sessionId":id,"uuid":response,"timestamp":ts,"message":{"id":response,"model":format!("capacity-{}",call%37),"content":[{"type":"text","text":"native body sentinel"}],"usage":{"input_tokens":10,"output_tokens":2}}})
                } else {
                    json!({"type":"token_usage_record","timestamp":ts,"payload":{"response_id":response,"usage":{"input_tokens":10,"output_tokens":2}}})
                };
                writeln!(out, "{row}").unwrap();
            }
        }
    }
    for round in 0..rounds {
        let native = NativeSessions::new(home.path().into(), &["claude".into(), "codex".into()]);
        let start = Instant::now();
        native.refresh().unwrap();
        let scan = start.elapsed().as_secs_f64();
        let start = Instant::now();
        let catalog = native.project_catalog().unwrap();
        let catalog_seconds = start.elapsed().as_secs_f64();
        assert_eq!(catalog.sessions.len(), files * 2);
        assert_eq!(catalog.calls.len(), files * calls * 2);
        assert_eq!(
            catalog
                .sessions
                .iter()
                .filter_map(|s| s.usage.total_tokens)
                .sum::<u64>(),
            (files * calls * 24) as u64
        );
        assert!(
            native
                .sources()
                .unwrap()
                .iter()
                .filter(|s| s.enabled)
                .all(|s| s.status == "ready")
        );
        let data = tempfile::tempdir().unwrap();
        let mut sql = crate::observation::sql::History::open(data.path()).unwrap();
        sql.refresh(&[]).unwrap();
        let start = Instant::now();
        sql.prepare_fees(&catalog).unwrap();
        let fees = start.elapsed().as_secs_f64();
        let start = Instant::now();
        if streaming {
            assert_eq!(
                sql.sync_native_provider(&catalog, &[]).unwrap(),
                files * calls * 2
            );
        } else {
            let providers = crate::provider_usage::native_records(catalog.calls.clone(), &[]);
            sql.set_native_usage(providers, vec![]).unwrap();
        }
        let provider_load = start.elapsed().as_secs_f64();
        let start = Instant::now();
        let report = sql
            .provider_usage(
                crate::provider_usage::Query {
                    source: "native".into(),
                    provider_id: Some("_unknown".into()),
                    ..Default::default()
                },
                &[],
                "fixture".into(),
                "ready".into(),
                Utc::now(),
            )
            .unwrap();
        let provider = start.elapsed().as_secs_f64();
        assert_eq!(report["summary"]["calls"], files * calls * 2);
        assert_eq!(
            report["summary"]["usage"]["totalTokens"],
            files * calls * 24
        );
        let start = Instant::now();
        if streaming {
            assert_eq!(
                sql.sync_native_models(&catalog, "claude").unwrap(),
                files * calls * 2
            );
        } else {
            let (rows, _, _) = native.usage_records("claude").unwrap();
            sql.set_native_usage(vec![], rows).unwrap();
        }
        let model_load = start.elapsed().as_secs_f64();
        let start = Instant::now();
        let report = sql
            .model_usage(
                crate::model_usage::UsageQuery {
                    agent: "claude".into(),
                    source: "native".into(),
                    days: None,
                    provider: None,
                    model: None,
                    cursor: None,
                },
                "fixture".into(),
                "ready".into(),
                Utc::now(),
            )
            .unwrap();
        let model = start.elapsed().as_secs_f64();
        assert_eq!(report["summary"]["attempts"], files * calls);
        assert_eq!(
            report["summary"]["usage"]["totalTokens"],
            files * calls * 12
        );
        let start = Instant::now();
        native.refresh().unwrap();
        let unchanged = start.elapsed().as_secs_f64();
        assert_eq!(native.revision().unwrap(), catalog.revision);
        println!(
            "NATIVE_CAPACITY {}",
            json!({"round":round,"files":files*2,"calls":files*calls*2,"scan":scan,"catalog":catalog_seconds,"fees":fees,"providerLoad":provider_load,"provider":provider,"modelLoad":model_load,"model":model,"unchangedScan":unchanged})
        );
        if streaming {
            incremental_capacity(&native, &catalog, &mut sql, home.path(), files, calls);
        }
    }
}

fn request_contexts(
    catalog: &crate::projects::ProjectCatalog,
) -> HashMap<String, crate::requests::Context> {
    catalog
        .sessions
        .iter()
        .flat_map(|s| {
            let context = crate::requests::Context {
                session_id: s.id.clone(),
                title: s.title.clone(),
                project_id: s.project_id.clone(),
                project_name: s.project_name.clone(),
            };
            [
                (s.id.clone(), context.clone()),
                (format!("{}:{}", s.agent_id, s.native_id), context),
            ]
        })
        .collect()
}

fn incremental_capacity(
    native: &NativeSessions,
    catalog: &crate::projects::ProjectCatalog,
    sql: &mut crate::observation::sql::History,
    home: &Path,
    files: usize,
    calls: usize,
) {
    let provider = crate::provider_usage::Query {
        source: "native".into(),
        provider_id: Some("_unknown".into()),
        ..Default::default()
    };
    let model = crate::model_usage::UsageQuery {
        agent: "claude".into(),
        source: "native".into(),
        days: None,
        provider: None,
        model: None,
        cursor: None,
    };
    let start = Instant::now();
    assert_eq!(sql.sync_native_provider(catalog, &[]).unwrap(), 0);
    assert_eq!(sql.sync_native_models(catalog, "claude").unwrap(), 0);
    sql.provider_usage(
        provider.clone(),
        &[],
        "fixture".into(),
        "ready".into(),
        Utc::now(),
    )
    .unwrap();
    sql.model_usage(model.clone(), "fixture".into(), "ready".into(), Utc::now())
        .unwrap();
    let unchanged_statistics = start.elapsed().as_secs_f64();
    let start = Instant::now();
    sql.sync_native_analysis(catalog).unwrap();
    let overview_load = start.elapsed().as_secs_f64();
    let start = Instant::now();
    sql.sync_native_catalog(
        catalog.revision.to_string(),
        &request_contexts(catalog),
        &catalog.calls,
    )
    .unwrap();
    let requests_load = start.elapsed().as_secs_f64();
    println!(
        "NATIVE_REQUEST_STAGE {}",
        json!({"stage":"cold-index","seconds":requests_load})
    );
    let request_query = crate::requests::Query {
        source: Some("native".into()),
        ..Default::default()
    };
    let start = Instant::now();
    let source = sql
        .source_report(&request_query, Utc::now(), vec![], None)
        .unwrap();
    let requests_report = start.elapsed().as_secs_f64();
    println!(
        "NATIVE_REQUEST_STAGE {}",
        json!({"stage":"cold-report","seconds":requests_report})
    );
    assert_eq!(source["native"]["requests"], files * calls * 2);
    assert_eq!(source["native"]["usage"]["totalTokens"], files * calls * 24);
    let start = Instant::now();
    let page = sql
        .request_page(&request_query, Utc::now(), source.clone(), None)
        .unwrap();
    let requests_statistics = start.elapsed().as_secs_f64();
    println!(
        "NATIVE_REQUEST_STAGE {}",
        json!({"stage":"cold-page","seconds":requests_statistics})
    );
    assert_eq!(page["summary"], source["native"]);
    assert_eq!(page["total"], files * calls * 2);
    let path = home
        .join(".claude/projects/project")
        .join(format!("capacity-{:05}.jsonl", files - 1));
    let id = format!("capacity-{:05}", files - 1);
    let row = json!({"type":"assistant","sessionId":id,"uuid":"capacity-appended","timestamp":(Utc::now()-chrono::Duration::seconds(30)).to_rfc3339(),"message":{"id":"capacity-appended","model":"capacity-0","content":[{"type":"text","text":"appended body sentinel"}],"usage":{"input_tokens":10,"output_tokens":2}}});
    writeln!(
        fs::OpenOptions::new().append(true).open(&path).unwrap(),
        "{row}"
    )
    .unwrap();
    let start = Instant::now();
    native.refresh().unwrap();
    let scan = start.elapsed().as_secs_f64();
    let next = native.project_catalog().unwrap();
    let (removed, added) = next.calls.delta(&catalog.calls);
    assert_eq!((removed.len(), added.len()), (1, 1));
    assert_eq!(next.calls.len(), files * calls * 2 + 1);
    let start = Instant::now();
    sql.prepare_fees(&next).unwrap();
    let fees = start.elapsed().as_secs_f64();
    let start = Instant::now();
    let loaded_provider = sql.sync_native_provider(&next, &[]).unwrap();
    let provider_load = start.elapsed().as_secs_f64();
    assert_eq!(loaded_provider, 1);
    let start = Instant::now();
    let p = sql
        .provider_usage(provider, &[], "fixture".into(), "ready".into(), Utc::now())
        .unwrap();
    let provider_seconds = start.elapsed().as_secs_f64();
    assert_eq!(p["summary"]["calls"], files * calls * 2 + 1);
    assert_eq!(
        p["summary"]["usage"]["totalTokens"],
        files * calls * 24 + 12
    );
    let start = Instant::now();
    let loaded_model = sql.sync_native_models(&next, "claude").unwrap();
    let model_load = start.elapsed().as_secs_f64();
    assert_eq!(loaded_model, 1);
    let start = Instant::now();
    let m = sql
        .model_usage(model, "fixture".into(), "ready".into(), Utc::now())
        .unwrap();
    let model_seconds = start.elapsed().as_secs_f64();
    assert_eq!(m["summary"]["attempts"], files * calls + 1);
    assert_eq!(
        m["summary"]["usage"]["totalTokens"],
        files * calls * 12 + 12
    );
    let start = Instant::now();
    assert_eq!(sql.sync_native_analysis(&next).unwrap(), 1);
    let overview = start.elapsed().as_secs_f64();
    let start = Instant::now();
    let analysis = sql
        .analysis(
            &crate::observation::OverviewQuery {
                agent: None,
                range: 168,
                metric: "tokens".into(),
                dimension: "agent".into(),
                source: "native".into(),
            },
            Utc::now(),
        )
        .unwrap();
    let overview_statistics = start.elapsed().as_secs_f64();
    assert_eq!(analysis["samples"], files * calls * 2 + 1);
    assert_eq!(
        analysis["total"].as_f64(),
        Some((files * calls * 24 + 12) as f64)
    );
    let start = Instant::now();
    sql.sync_native_catalog(
        next.revision.to_string(),
        &request_contexts(&next),
        &next.calls,
    )
    .unwrap();
    let requests = start.elapsed().as_secs_f64();
    let source = sql
        .source_report(&crate::requests::Query::default(), Utc::now(), vec![], None)
        .unwrap();
    assert_eq!(source["native"]["requests"], files * calls * 2 + 1);
    assert_eq!(
        source["native"]["usage"]["totalTokens"],
        files * calls * 24 + 12
    );
    let start = Instant::now();
    let fee_ids: Vec<_> = next.sessions.iter().map(|s| s.id.clone()).collect();
    assert_eq!(
        sql.fee_summary(&fee_ids, None, false).unwrap()["nativeResponses"],
        files * calls * 2 + 1
    );
    let fee_statistics = start.elapsed().as_secs_f64();
    let start = Instant::now();
    let project = crate::projects::query(
        &next,
        crate::projects::ProjectQuery {
            scope: Some("all".into()),
            ..Default::default()
        },
        &Default::default(),
    )
    .unwrap();
    let projects_statistics = start.elapsed().as_secs_f64();
    assert_eq!(
        project.summary.usage.total_tokens,
        Some((files * calls * 24 + 12) as u64)
    );
    assert_eq!(
        next.sessions
            .iter()
            .map(|s| s.usage.total_tokens.unwrap_or(0))
            .sum::<u64>(),
        (files * calls * 24 + 12) as u64
    );
    println!(
        "NATIVE_INCREMENTAL {}",
        json!({"calls":files*calls*2,"changedFiles":added.len(),"loadedProvider":loaded_provider,"loadedModel":loaded_model,"scan":scan,"fees":fees,"feeStatistics":fee_statistics,"providerLoad":provider_load,"provider":provider_seconds,"modelLoad":model_load,"model":model_seconds,"overview":overview,"overviewStatistics":overview_statistics,"requests":requests,"firstOverviewLoad":overview_load,"firstRequestsLoad":requests_load,"firstRequestsReport":requests_report,"firstRequestsStatistics":requests_statistics,"projectsStatistics":projects_statistics,"unchangedStatistics":unchanged_statistics})
    );
    if std::env::var("LOCAL_AGENT_NATIVE_AGGREGATE_CHANGES").as_deref() == Ok("1") {
        refresh_capacity(native, sql, &path, files, calls);
    }
}

#[test]
#[ignore = "single-Agent native usage capacity; generated catalog metadata only"]
fn native_single_agent_usage_statistics_capacity() {
    let files: usize = std::env::var("LOCAL_AGENT_NATIVE_FILES")
        .unwrap_or("1000".into())
        .parse()
        .unwrap();
    let calls: usize = std::env::var("LOCAL_AGENT_NATIVE_CALLS")
        .unwrap_or("1000".into())
        .parse()
        .unwrap();
    assert!((1..=5000).contains(&files) && (1..=1000).contains(&calls));
    let mut c = crate::associated_cost_tests::catalog();
    let call = c.calls[0].clone();
    let session = c.sessions[0].clone();
    c.sessions.clear();
    c.calls.clear();
    c.revision = 1;
    for file in 0..files {
        let mut s = session.clone();
        s.id = format!("claude:capacity-{file:05}");
        s.native_id = format!("capacity-{file:05}");
        s.model_calls = calls;
        s.usage = Usage {
            input_tokens: Some(calls as u64 * 10),
            output_tokens: Some(calls as u64 * 2),
            total_tokens: Some(calls as u64 * 12),
            ..Default::default()
        };
        let batch = (0..calls)
            .map(|i| {
                let mut r = call.clone();
                r.id = format!("single-{file:05}-{i:05}");
                r.session_id = s.id.clone();
                r.model = Some(format!("capacity-{}", i % 37));
                r.response_id = None;
                r.model_response_id = None;
                r.usage = Some(Usage {
                    input_tokens: Some(10),
                    output_tokens: Some(2),
                    total_tokens: Some(12),
                    ..Default::default()
                });
                r
            })
            .collect();
        c.calls.chunks.push(std::sync::Arc::new(batch));
        c.sessions.push(s);
    }
    // Isolate the in-memory project pass before the full disk-backed matrix.
    // This diagnostic never substitutes for the ordinary-disk capacity rounds.
    if std::env::var("LOCAL_AGENT_NATIVE_PROJECT_ONLY").as_deref() == Ok("1") {
        let q = crate::projects::ProjectQuery {
            scope: Some("all".into()),
            ..Default::default()
        };
        let start = Instant::now();
        let first = crate::projects::query(&c, q.clone(), &Default::default()).unwrap();
        let cold = start.elapsed().as_secs_f64();
        assert_eq!(first.summary.model_calls, files * calls);
        assert_eq!(
            first.summary.usage.total_tokens,
            Some((files * calls * 12) as u64)
        );
        std::sync::Arc::make_mut(&mut c.calls.chunks[0])[0].usage = Some(Usage {
            input_tokens: Some(20),
            output_tokens: Some(3),
            total_tokens: Some(23),
            ..Default::default()
        });
        c.revision += 1;
        let start = Instant::now();
        let next = crate::projects::query(&c, q, &Default::default()).unwrap();
        let changed = start.elapsed().as_secs_f64();
        assert_eq!(next.summary.model_calls, files * calls);
        assert_eq!(
            next.summary.usage.total_tokens,
            Some((files * calls * 12 + 11) as u64)
        );
        println!(
            "PROJECT_CAPACITY {}",
            json!({"calls":files*calls,"cold":cold,"changed":changed,"allPageStatisticsMatch":true,"profile":"generated catalog diagnostic, not disk acceptance"})
        );
        return;
    }
    if std::env::var("LOCAL_AGENT_NATIVE_COVERAGE_ONLY").as_deref() == Ok("1") {
        let dir = tempfile::tempdir().unwrap();
        let mut h = crate::observation::sql::History::open(dir.path()).unwrap();
        h.refresh(&[]).unwrap();
        let ids: Vec<_> = c.sessions.iter().map(|s| s.id.clone()).collect();
        let start = Instant::now();
        h.prepare_fees(&c).unwrap();
        let prepare = start.elapsed().as_secs_f64();
        let scopes = [
            None,
            Some("claude-code-cli"),
            Some("claude-code-desktop"),
            Some("claude-unknown"),
        ];
        let start = Instant::now();
        for scope in scopes {
            assert_eq!(
                h.fee_summary(&ids, scope, false).unwrap()["nativeResponses"],
                if scope.is_none() || scope == Some("claude-code-cli") {
                    files * calls
                } else {
                    0
                }
            );
        }
        let cold = start.elapsed().as_secs_f64();
        let mut extra = c.calls[0].clone();
        extra.id = "coverage-capacity-appended".into();
        extra.response_id = None;
        std::sync::Arc::make_mut(&mut c.calls.chunks[0]).push(extra);
        c.revision += 1;
        let start = Instant::now();
        h.prepare_fees(&c).unwrap();
        let incremental_prepare = start.elapsed().as_secs_f64();
        let start = Instant::now();
        for scope in scopes {
            assert_eq!(
                h.fee_summary(&ids, scope, false).unwrap()["nativeResponses"],
                if scope.is_none() || scope == Some("claude-code-cli") {
                    files * calls + 1
                } else {
                    0
                }
            );
        }
        let changed = start.elapsed().as_secs_f64();
        println!(
            "FEE_COVERAGE_CAPACITY {}",
            json!({"calls":files*calls,"prepare":prepare,"coldFourScopes":cold,"incrementalPrepare":incremental_prepare,"changedFourScopes":changed,"countsMatch":true,"profile":"generated catalog diagnostic, not disk acceptance"})
        );
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let mut sql = crate::observation::sql::History::open(dir.path()).unwrap();
    sql.refresh(&[]).unwrap();
    let p = crate::provider_usage::Query {
        source: "native".into(),
        provider_id: Some("_unknown".into()),
        ..Default::default()
    };
    let m = crate::model_usage::UsageQuery {
        source: "native".into(),
        agent: "claude".into(),
        days: None,
        provider: None,
        model: None,
        cursor: None,
    };
    let start = Instant::now();
    sql.sync_native_provider(&c, &[]).unwrap();
    let provider_load = start.elapsed().as_secs_f64();
    let start = Instant::now();
    sql.provider_usage(p.clone(), &[], "fixture".into(), "ready".into(), Utc::now())
        .unwrap();
    let provider_cold = start.elapsed().as_secs_f64();
    let start = Instant::now();
    sql.sync_native_models(&c, "claude").unwrap();
    let model_load = start.elapsed().as_secs_f64();
    let start = Instant::now();
    let first = sql
        .model_usage(m.clone(), "fixture".into(), "ready".into(), Utc::now())
        .unwrap();
    let model_cold = start.elapsed().as_secs_f64();
    assert_eq!(first["summary"]["attempts"], files * calls);
    let mut added = call;
    added.id = "single-appended".into();
    added.session_id = c.sessions.last().unwrap().id.clone();
    added.response_id = None;
    added.model_response_id = None;
    added.model = Some("capacity-0".into());
    added.usage = Some(Usage {
        input_tokens: Some(10),
        output_tokens: Some(2),
        total_tokens: Some(12),
        ..Default::default()
    });
    std::sync::Arc::make_mut(c.calls.chunks.last_mut().unwrap()).push(added);
    let last = c.sessions.last_mut().unwrap();
    last.model_calls += 1;
    last.usage.add(&Usage {
        input_tokens: Some(10),
        output_tokens: Some(2),
        total_tokens: Some(12),
        ..Default::default()
    });
    c.revision += 1;
    let start = Instant::now();
    assert_eq!(sql.sync_native_provider(&c, &[]).unwrap(), 1);
    let provider_delta_load = start.elapsed().as_secs_f64();
    let start = Instant::now();
    let providers = sql
        .provider_usage(p, &[], "fixture".into(), "ready".into(), Utc::now())
        .unwrap();
    let provider_refresh = start.elapsed().as_secs_f64();
    let start = Instant::now();
    assert_eq!(sql.sync_native_models(&c, "claude").unwrap(), 1);
    let model_delta_load = start.elapsed().as_secs_f64();
    let start = Instant::now();
    let models = sql
        .model_usage(m.clone(), "fixture".into(), "ready".into(), Utc::now())
        .unwrap();
    let model_refresh = start.elapsed().as_secs_f64();
    let expected = files * calls + 1;
    assert_eq!(providers["summary"]["calls"], expected);
    assert_eq!(providers["summary"]["usage"]["totalTokens"], expected * 12);
    assert_eq!(models["summary"]["attempts"], expected);
    assert_eq!(models["summary"]["usage"]["totalTokens"], expected * 12);
    assert_eq!(
        providers["models"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["totals"]["calls"].as_u64().unwrap())
            .sum::<u64>(),
        expected as u64
    );
    let mut groups = models.clone();
    let mut attempts = 0;
    let mut tokens = 0;
    loop {
        for item in groups["items"].as_array().unwrap() {
            attempts += item["totals"]["attempts"].as_u64().unwrap();
            tokens += item["totals"]["usage"]["totalTokens"].as_u64().unwrap();
        }
        let Some(cursor) = groups["nextCursor"].as_str() else {
            break;
        };
        let mut next = m.clone();
        next.cursor = Some(cursor.into());
        groups = sql
            .model_usage(next, "fixture".into(), "ready".into(), Utc::now())
            .unwrap();
    }
    assert_eq!(attempts, expected as u64);
    assert_eq!(tokens, (expected * 12) as u64);
    println!(
        "NATIVE_SINGLE_AGENT {}",
        json!({"calls":files*calls,"changedFileRows":calls+1,"providerLoad":provider_load,"providerCold":provider_cold,"modelLoadIncludingFees":model_load,"modelCold":model_cold,"providerDeltaLoad":provider_delta_load,"providerRefresh":provider_refresh,"modelDeltaLoad":model_delta_load,"modelRefresh":model_refresh,"allModelGroupsMatch":true,"catalogMetadataOnly":true})
    );
}

fn refresh_capacity(
    native: &NativeSessions,
    sql: &mut crate::observation::sql::History,
    path: &Path,
    files: usize,
    calls: usize,
) {
    let original = fs::read_to_string(path).unwrap();
    let mut lines: Vec<serde_json::Value> = original
        .lines()
        .map(|s| serde_json::from_str(s).unwrap())
        .collect();
    lines[1]["message"]["model"] = json!("capacity-rewritten");
    lines[1]["message"]["usage"] = json!({"input_tokens":20,"output_tokens":4});
    let rewritten = lines.iter().map(|s| format!("{s}\n")).collect::<String>();
    for phase in ["rewrite", "delete", "restore"] {
        match phase {
            "rewrite" => fs::write(path, &rewritten).unwrap(),
            "delete" => fs::remove_file(path).unwrap(),
            _ => fs::write(path, &original).unwrap(),
        }
        let expected = files * calls * 2 + if phase == "delete" { 0 } else { 1 }
            - if phase == "delete" { calls } else { 0 };
        let tokens = expected * 12 + if phase == "rewrite" { 12 } else { 0 };
        let start = Instant::now();
        native.refresh().unwrap();
        let catalog = native.project_catalog().unwrap();
        let scan = start.elapsed().as_secs_f64();
        assert_eq!(catalog.calls.len(), expected);
        let start = Instant::now();
        sql.prepare_fees(&catalog).unwrap();
        let fees = start.elapsed().as_secs_f64();
        let start = Instant::now();
        sql.sync_native_provider(&catalog, &[]).unwrap();
        let provider_load = start.elapsed().as_secs_f64();
        let start = Instant::now();
        let p = sql
            .provider_usage(
                crate::provider_usage::Query {
                    source: "native".into(),
                    provider_id: Some("_unknown".into()),
                    ..Default::default()
                },
                &[],
                "fixture".into(),
                "ready".into(),
                Utc::now(),
            )
            .unwrap();
        let provider = start.elapsed().as_secs_f64();
        let start = Instant::now();
        sql.sync_native_models(&catalog, "claude").unwrap();
        let model_load = start.elapsed().as_secs_f64();
        let start = Instant::now();
        let m = sql
            .model_usage(
                crate::model_usage::UsageQuery {
                    source: "native".into(),
                    agent: "claude".into(),
                    days: None,
                    provider: None,
                    model: None,
                    cursor: None,
                },
                "fixture".into(),
                "ready".into(),
                Utc::now(),
            )
            .unwrap();
        let model = start.elapsed().as_secs_f64();
        assert_eq!(p["summary"]["calls"], expected);
        assert_eq!(p["summary"]["usage"]["totalTokens"], tokens);
        assert_eq!(m["summary"]["attempts"], expected - files * calls);
        assert_eq!(
            m["summary"]["usage"]["totalTokens"],
            tokens - files * calls * 12
        );
        assert_eq!(
            catalog
                .sessions
                .iter()
                .filter_map(|s| s.usage.total_tokens)
                .sum::<u64>(),
            tokens as u64
        );
        let start = Instant::now();
        let fee_ids: Vec<_> = catalog.sessions.iter().map(|s| s.id.clone()).collect();
        assert_eq!(
            sql.fee_summary(&fee_ids, None, false).unwrap()["nativeResponses"],
            expected
        );
        let fee_statistics = start.elapsed().as_secs_f64();
        let start = Instant::now();
        let projects = crate::projects::query(
            &catalog,
            crate::projects::ProjectQuery {
                scope: Some("all".into()),
                ..Default::default()
            },
            &Default::default(),
        )
        .unwrap();
        let projects_statistics = start.elapsed().as_secs_f64();
        assert_eq!(projects.summary.usage.total_tokens, Some(tokens as u64));
        sql.sync_native_analysis(&catalog).unwrap();
        let overview = sql
            .analysis(
                &crate::observation::OverviewQuery {
                    source: "native".into(),
                    agent: None,
                    range: 168,
                    metric: "tokens".into(),
                    dimension: "agent".into(),
                },
                Utc::now(),
            )
            .unwrap();
        assert_eq!(overview["samples"], expected);
        assert_eq!(overview["total"].as_f64(), Some(tokens as f64));
        sql.sync_native_catalog(
            catalog.revision.to_string(),
            &request_contexts(&catalog),
            &catalog.calls,
        )
        .unwrap();
        let source = sql
            .source_report(
                &crate::requests::Query {
                    source: Some("native".into()),
                    ..Default::default()
                },
                Utc::now(),
                vec![],
                None,
            )
            .unwrap();
        assert_eq!(source["native"]["requests"], expected);
        assert_eq!(source["native"]["usage"]["totalTokens"], tokens);
        let start = Instant::now();
        let page = sql
            .request_page(
                &crate::requests::Query {
                    source: Some("native".into()),
                    ..Default::default()
                },
                Utc::now(),
                source.clone(),
                None,
            )
            .unwrap();
        let requests_statistics = start.elapsed().as_secs_f64();
        assert_eq!(page["summary"], source["native"]);
        assert_eq!(page["total"], expected);
        println!(
            "NATIVE_REFRESH {}",
            json!({"phase":phase,"calls":expected,"tokens":tokens,"scan":scan,"fees":fees,"feeStatistics":fee_statistics,"providerLoad":provider_load,"provider":provider,"modelLoad":model_load,"model":model,"requestsStatistics":requests_statistics,"projectsStatistics":projects_statistics,"allPageStatisticsMatch":true})
        );
    }
}
