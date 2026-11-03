use crate::{
    associated_cost::Index, observation::ModelObservation, projects::ProjectCatalog,
    sessions::NativeSessions,
};
use serde_json::json;
use std::fs;
pub(crate) fn catalog() -> ProjectCatalog {
    let tmp = tempfile::tempdir().unwrap();
    let source = tmp.path().join(".claude/projects/p");
    fs::create_dir_all(&source).unwrap();
    for id in ["one", "two"] {
        let user = json!({"type":"user","sessionId":id,"uuid":format!("u-{id}"),"cwd":tmp.path(),"timestamp":"2026-10-20T01:00:00Z","message":{"content":"cost fixture"}});
        let response = json!({"type":"assistant","sessionId":id,"uuid":format!("a-{id}"),"timestamp":"2026-10-20T01:00:01Z","message":{"id":format!("response-{id}"),"model":"deepseek-flash","content":[{"type":"text","text":"done"}],"usage":{"input_tokens":10,"output_tokens":5}}});
        fs::write(
            source.join(format!("{id}.jsonl")),
            format!("{user}\n{response}\n"),
        )
        .unwrap();
    }
    let native = NativeSessions::new(tmp.path().to_path_buf(), &["claude".into()]);
    native.refresh().unwrap();
    let mut c = native.project_catalog().unwrap();
    for s in &mut c.sessions {
        s.agent_product_id = Some("claude-code-cli".into());
        s.agent_products = vec!["claude-code-cli".into(), "claude-code-desktop".into()];
    }
    for call in &mut c.calls {
        call.agent_product_id = Some("claude-code-cli".into());
    }
    c
}
fn row(id: &str, session: Option<&str>, response: Option<&str>, actual: bool) -> ModelObservation {
    serde_json::from_value(json!({"id":id,"agentId":"claude","agentProductId":"claude-code-cli","provider":"saved","providerId":"saved","endpoint":if actual {"https://openrouter.ai/api/v1"}else{"https://api.deepseek.com"},"model":"deepseek-flash","bindingRevision":1,"startedAt":"2026-10-20T01:00:00Z","endedAt":"2026-10-20T01:00:01Z","status":"success","revision":1,"protocol":"messages","sessionId":session,"responseId":response,"usage":{"inputTokens":10,"outputTokens":5,"totalTokens":15,"cachedInputTokens":0},"reportedCost":if actual {json!({"amount":0,"currency":"USD","source":"openrouter.usage.cost"})}else{serde_json::Value::Null}})).unwrap()
}
#[test]
fn session_scope_deduplicates_money_and_preserves_native_coverage() {
    let c = catalog();
    let exact = row("a", Some("claude:one"), Some("response-one"), true);
    let session = row("b", Some("claude:one"), None, false);
    let mut unknown = row("c", Some("claude:one"), None, false);
    unknown.usage = Default::default();
    let index = Index::new(
        &c,
        vec![
            exact.clone(),
            exact,
            session,
            unknown,
            row("other", Some("claude:two"), Some("response-two"), true),
        ],
        false,
    );
    let s = index.summary(&["claude:one".into(), "claude:one".into()], None);
    assert_eq!(s.gateway_attempts, 3);
    assert_eq!(s.response_linked_attempts, 1);
    assert_eq!(s.session_linked_attempts, 2);
    assert_eq!(s.native_responses, 1);
    assert_eq!(s.matched_native_responses, 1);
    assert_eq!(s.cost.actual[0].amount, 0.0);
    assert_eq!(s.cost.actual[0].calls, 1);
    assert_eq!(s.cost.estimated[0].amount, 0.000009);
    assert_eq!(s.cost.unknown_calls, 1);
    assert_eq!(
        index
            .summary(&["claude:one".into()], Some("claude-code-desktop"))
            .gateway_attempts,
        0
    );
    let call = c
        .calls
        .iter()
        .find(|x| x.session_id == "claude:one")
        .unwrap();
    assert_eq!(index.response_cost(&call.id).kind, "actual");
}
#[test]
fn exact_response_can_supply_session_but_creator_cannot_supply_client() {
    let c = catalog();
    let index = Index::new(&c, vec![row("a", None, Some("response-one"), false)], false);
    assert_eq!(
        index.summary(&["claude:one".into()], None).gateway_attempts,
        1
    );
    let mut r = row("unknown", Some("claude:one"), None, false);
    r.agent_product_id = None;
    let index = Index::new(&c, vec![r], false);
    assert_eq!(
        index
            .summary(&["claude:one".into()], Some("claude-code-cli"))
            .gateway_attempts,
        0
    );
    let unknown = index.summary(&["claude:one".into()], Some("claude-unknown"));
    assert_eq!(unknown.gateway_attempts, 1);
    let all = index.summary(&["claude:one".into()], None);
    assert_eq!(all.unlinked_native_responses, 1);
    assert_eq!(all.cost.unknown_calls, 0);
}
#[test]
fn conflicting_session_or_client_never_moves_a_charge() {
    let c = catalog();
    for mut r in [
        row("a", Some("claude:two"), Some("response-one"), false),
        row("b", Some("claude:one"), Some("response-one"), false),
    ] {
        if r.id == "b" {
            r.agent_product_id = Some("claude-code-desktop".into());
        }
        let index = Index::new(&c, vec![r], false);
        assert_eq!(
            index
                .summary(&["claude:one".into(), "claude:two".into()], None)
                .gateway_attempts,
            0
        );
        assert_eq!(
            index
                .summary(&["claude:one".into()], None)
                .conflicting_native_responses,
            1
        );
    }
}
#[test]
fn ambiguous_responses_keep_session_scope_without_fabricating_response_parity() {
    let c = catalog();
    let index = Index::new(
        &c,
        vec![
            row("a", Some("claude:one"), Some("response-one"), false),
            row("b", Some("claude:one"), Some("response-one"), false),
        ],
        true,
    );
    let s = index.summary(&["claude:one".into()], None);
    assert_eq!(s.gateway_attempts, 2);
    assert_eq!(s.session_linked_attempts, 2);
    assert_eq!(s.matched_native_responses, 0);
    assert_eq!(s.conflicting_native_responses, 1);
    assert_eq!(s.source_state, "partial");
    let call = c
        .calls
        .iter()
        .find(|x| x.session_id == "claude:one")
        .unwrap();
    assert_eq!(index.response_cost(&call.id).kind, "unknown");
}
#[test]
fn unindexed_or_revoked_sessions_cannot_expose_associated_fees() {
    let mut c = catalog();
    c.sessions.clear();
    c.calls.clear();
    let index = Index::new(
        &c,
        vec![row("a", Some("claude:one"), Some("response-one"), false)],
        false,
    );
    let s = index.summary(&["claude:one".into()], None);
    assert_eq!(s.gateway_attempts, 0);
    assert!(s.cost.actual.is_empty() && s.cost.estimated.is_empty());
}

#[test]
fn details_groups_full_scope_and_pages_each_attempt_once() {
    use crate::associated_cost::detail::Query;
    let c = catalog();
    let mut rows = vec![];
    for n in 0..85 {
        let mut r = row(&format!("a{n:03}"), Some("claude:one"), None, n < 40);
        r.provider_id = Some(if n < 40 { "actual" } else { "estimate" }.into());
        r.provider = "same display name".into();
        if n >= 80 {
            r.usage = Default::default();
        }
        rows.push(r);
    }
    rows.push(rows[0].clone());
    let index = Index::new(&c, rows.clone(), false);
    let ids: Vec<String> = vec!["claude:one".into(), "claude:one".into()];
    let q = Query {
        scope: "session".into(),
        id: ids[0].clone(),
        ..Default::default()
    };
    let p = index.details(&ids, &q).unwrap();
    assert_eq!(p.total, 85);
    assert_eq!(p.providers.len(), 2); // Display names are not identity.
    assert_eq!(p.models.len(), 2);
    assert_eq!(p.filtered_cost.actual[0].calls, 40);
    assert_eq!(p.filtered_cost.actual[0].amount, 0.0);
    assert_eq!(p.filtered_cost.estimated[0].calls, 40);
    assert_eq!(p.filtered_cost.unknown_calls, 5);
    assert_eq!(p.calls.len(), 40);
    let mut seen: std::collections::HashSet<_> = p.calls.iter().map(|a| a.id.clone()).collect();
    let mut cursor = p.next_cursor;
    while cursor.is_some() {
        let next = index
            .details(
                &ids,
                &Query {
                    cursor: cursor.clone(),
                    ..q.clone()
                },
            )
            .unwrap();
        assert_eq!(next.total, 85);
        for a in &next.calls {
            assert!(seen.insert(a.id.clone()));
        }
        cursor = next.next_cursor;
    }
    assert_eq!(seen.len(), 85);
    let estimate = p.providers.iter().find(|g| g.attempts == 45).unwrap();
    let filtered = index
        .details(
            &ids,
            &Query {
                provider: Some(estimate.provider_key.clone()),
                ..q.clone()
            },
        )
        .unwrap();
    assert_eq!(filtered.total, 45);
    assert!(filtered.filtered_cost.actual.is_empty());
    assert_eq!(filtered.filtered_providers.len(), 1);
    assert_eq!(filtered.filtered_models[0].attempts, 45);
    assert!(
        index
            .details(
                &ids,
                &Query {
                    cursor: filtered.next_cursor.clone(),
                    ..q.clone()
                }
            )
            .is_err()
    );
    // A new global index revision alone must not interrupt an unchanged fee scope.
    let mut refreshed = c.clone();
    refreshed.revision += 1;
    let stable = Index::new(&refreshed, rows.clone(), false);
    assert!(
        stable
            .details(
                &ids,
                &Query {
                    cursor: filtered.next_cursor.clone(),
                    provider: Some(estimate.provider_key.clone()),
                    ..q.clone()
                }
            )
            .is_ok()
    );
    rows[0].revision += 1;
    rows[0].reported_cost = None;
    let changed = Index::new(&c, rows, false);
    assert!(
        changed
            .details(
                &ids,
                &Query {
                    cursor: filtered.next_cursor,
                    provider: Some(estimate.provider_key.clone()),
                    ..q
                }
            )
            .is_err()
    );
}

#[test]
fn detail_keeps_unknown_model_and_exact_fallback_and_rejects_revoked_scope() {
    use crate::associated_cost::detail::Query;
    let c = catalog();
    let mut r = row("a", None, Some("response-one"), false);
    r.model = None;
    let index = Index::new(&c, vec![r], false);
    let q = Query {
        scope: "session".into(),
        id: "claude:one".into(),
        model: Some("_unknown".into()),
        ..Default::default()
    };
    let p = index.details(std::slice::from_ref(&q.id), &q).unwrap();
    assert_eq!(p.total, 1);
    assert_eq!(p.calls[0].association, "response_id");
    assert_eq!(p.calls[0].session_id.as_deref(), Some("claude:one"));
    assert_eq!(p.filtered_cost.unknown_calls, 1);
    assert!(index.details(&[], &q).unwrap().calls.is_empty());
    assert!(
        index
            .details(
                &[],
                &Query {
                    agent: Some("claude-code-cli".into()),
                    ..q
                }
            )
            .is_err()
    );
}
