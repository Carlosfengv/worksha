use super::*;
use crate::domain::{
    self as d,
    facts::{
        query::{FactQuery, FactWindow},
        *,
    },
};
use serde_json::json;

fn fixture() -> (tempfile::TempDir, NativeSessions, CallSummary, CallSummary) {
    let home = tempfile::tempdir().unwrap();
    let project = home.path().join("workspace");
    let source = home.path().join(".claude/projects/fixture/session.jsonl");
    fs::create_dir_all(source.parent().unwrap()).unwrap();
    fs::create_dir(&project).unwrap();
    let rows = [
        json!({"type":"user","sessionId":"indexed","uuid":"u","cwd":project,"timestamp":"2026-10-31T12:00:00Z","message":{"content":"fixture"}}),
        json!({"type":"assistant","sessionId":"indexed","uuid":"a","timestamp":"2026-10-31T12:00:01Z","message":{"id":"model","model":"fixture","content":[{"type":"tool_use","id":"tool","name":"mcp__fixture__lookup","input":{}}],"usage":{"input_tokens":1,"output_tokens":1}}}),
    ];
    fs::write(
        source,
        rows.iter().map(|r| format!("{r}\n")).collect::<String>(),
    )
    .unwrap();
    let native = NativeSessions::new(home.path().into(), &["claude".into()]);
    native.refresh().unwrap();
    let catalog = native.project_catalog().unwrap();
    let model = catalog
        .calls
        .iter()
        .find(|c| c.kind == "model")
        .unwrap()
        .clone();
    let tool = catalog
        .calls
        .iter()
        .find(|c| c.kind == "mcp")
        .unwrap()
        .clone();
    (home, native, model, tool)
}
fn replace(native: &NativeSessions, calls: Vec<CallSummary>) {
    let mut state = native.lock().unwrap();
    let file = state.files.values_mut().next().unwrap();
    file.summary.model_calls = calls.iter().filter(|c| c.kind == "model").count();
    file.summary.mcp_calls = calls.iter().filter(|c| c.kind == "mcp").count();
    file.fact_calls = domain_facts::CallIndex::new(&calls);
    file.calls = Arc::new(calls);
    state.revision += 1;
}
fn query(kind: FactKind) -> FactQuery {
    FactQuery {
        scope: d::QueryScope {
            kind: d::ScopeKind::Builtin,
            source_ids: vec!["claude".into()],
        },
        kind,
        project_id: None,
        session_id: None,
        response_id: None,
        snapshot_token: None,
        cursor: None,
    }
}
fn unknown() -> d::Usage {
    d::Usage {
        input_tokens: None,
        output_tokens: None,
        cached_input_tokens: None,
        cache_write_input_tokens: None,
        reasoning_output_tokens: None,
        total_tokens: None,
    }
}
fn reference(call: &CallSummary, identity: &FactIdentity) -> FactRecord {
    let value = if call.kind == "model" {
        FactCandidate::Response(ResponseCandidate {
            native_id: call.id.clone(),
            session_ref: call.session_id.clone(),
            product_id: call.agent_product_id.clone(),
            occurred_at: call.timestamp.clone(),
            model: call.model.clone(),
            outcome: Outcome::Unknown,
            usage: unknown(),
            reported_response_id: call.response_id.clone(),
            duration_ms: None,
        })
    } else {
        FactCandidate::McpExecution(McpExecutionCandidate {
            native_id: call.id.clone(),
            session_ref: Some(call.session_id.clone()),
            server_ref: None,
            response_ref: None,
            tool_name: call.tool_name.clone().unwrap(),
            occurred_at: call.timestamp.clone(),
            duration_ms: None,
            outcome: Outcome::Unknown,
        })
    };
    FactRecord {
        id: call.id.clone(),
        identity: identity.clone(),
        source_revision: "reference".into(),
        value,
    }
}
fn label(record: &FactRecord) -> &str {
    match &record.value {
        FactCandidate::Response(v) => v.model.as_deref().unwrap(),
        FactCandidate::McpExecution(v) => &v.tool_name,
        _ => panic!("unexpected kind"),
    }
}

#[test]
fn indexed_fact_pages_match_full_scan_for_offsets_unknown_times_and_duplicate_keys() {
    let (_home, native, model, mcp) = fixture();
    let mut calls = vec![];
    for template in [&model, &mcp] {
        for i in 0..137 {
            let mut c = template.clone();
            c.id = format!("claude:indexed:{}:{i:03}", c.kind);
            c.timestamp = match i % 4 {
                0 => None,
                1 => Some("2026-10-31T12:00:00.0008Z".into()),
                2 => Some("2026-10-31T20:00:00+08:00".into()),
                _ => Some("2026-10-31T11:59:59Z".into()),
            };
            c.model = Some(format!("model-{i}"));
            c.tool_name = Some(format!("tool-{i}"));
            calls.push(c);
        }
        // More than a page of equal keys must not hide later distinct rows;
        // the latest original occurrence wins, as in the original full scan.
        let mut duplicate = calls[calls.len() - 137].clone();
        for i in 0..60 {
            duplicate.model = Some(format!("last-{i}"));
            duplicate.tool_name = Some(format!("last-{i}"));
            calls.push(duplicate.clone());
        }
    }
    replace(&native, calls.clone());
    for kind in [FactKind::Response, FactKind::McpExecution] {
        let q = query(kind);
        let mut after = None;
        let mut delivered = vec![];
        loop {
            let (actual, summary) = native
                .domain_facts_for_source("claude", &q, after.clone(), &[], d::Coverage::Complete)
                .unwrap();
            assert!(actual.len() <= d::PAGE_SIZE + 1);
            assert_eq!(summary.responses.records.unwrap().value(), 197);
            assert_eq!(summary.mcp_executions.unwrap().value(), 197);
            let identity = actual[0].identity.clone();
            let mut oracle = FactWindow::new(after.clone());
            for call in calls
                .iter()
                .filter(|c| (kind == FactKind::Response) == (c.kind == "model"))
            {
                oracle.observe(reference(call, &identity));
            }
            let (expected, next) = oracle.finish();
            assert_eq!(
                actual
                    .iter()
                    .take(d::PAGE_SIZE)
                    .map(|r| (&r.id, label(r)))
                    .collect::<Vec<_>>(),
                expected
                    .iter()
                    .map(|r| (&r.id, label(r)))
                    .collect::<Vec<_>>()
            );
            delivered.extend(expected.iter().map(|r| r.id.clone()));
            after = next;
            if after.is_none() {
                break;
            }
        }
        assert_eq!(delivered.len(), 137);
        assert_eq!(
            delivered
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len(),
            137
        );
    }
}

#[test]
fn response_overflow_cache_does_not_poison_legacy_or_exact_response_reads() {
    let (_home, native, model, _) = fixture();
    let mut max = model.clone();
    max.id = "claude:indexed:model:max".into();
    max.usage.as_mut().unwrap().input_tokens = Some(u64::MAX);
    let mut one = model;
    one.id = "claude:indexed:model:one".into();
    one.usage.as_mut().unwrap().input_tokens = Some(1);
    replace(&native, vec![max.clone(), one]);
    assert_eq!(native.calls(CallQuery::default()).unwrap().total, 2);
    let mut q = query(FactKind::Response);
    assert_eq!(
        native
            .domain_facts_for_source("claude", &q, None, &[], d::Coverage::Complete)
            .unwrap_err()
            .code,
        "resource_limit"
    );
    q.response_id = Some(max.id);
    let (_, summary) = native
        .domain_facts_for_source("claude", &q, None, &[], d::Coverage::Complete)
        .unwrap();
    assert_eq!(summary.responses.records.unwrap().value(), 1);
    assert_eq!(
        summary.responses.usage.input_tokens.unwrap().value(),
        u64::MAX
    );
}
