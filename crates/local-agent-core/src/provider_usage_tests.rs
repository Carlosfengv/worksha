use super::provider_usage::*;
use crate::{
    Provider,
    observation::ModelObservation,
    sessions::{CallSummary, Usage},
};
use chrono::{DateTime, Utc};
use serde_json::json;

fn now() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-10-19T12:00:00Z")
        .unwrap()
        .with_timezone(&Utc)
}
fn query() -> Query {
    Query {
        source: "gateway".into(),
        ..Default::default()
    }
}
fn provider(id: &str) -> Provider {
    serde_json::from_value(json!({"id":id,"name":"Same name","endpoint":"https://example.com","protocol":"responses","model":"model","models":["model"],"configured":true,"revision":1,"category":"cloud"})).unwrap()
}
fn evidence(id: &str) -> Evidence {
    Evidence {
        agent: "codex".into(),
        provider_id: id.into(),
        provider_name: "Same name".into(),
        endpoint: "https://example.com".into(),
        protocol: "responses".into(),
        revision: 12,
        native_provider: Some("local_agent_known".into()),
        model_aliases: vec![],
    }
}
fn gateway(id: &str) -> ModelObservation {
    serde_json::from_value(json!({"id":id,"agentId":"codex","provider":"Same name","endpoint":"https://example.com","bindingRevision":12,"startedAt":"2026-10-19T10:00:00Z","endedAt":"2026-10-19T10:00:01Z","status":"success","protocol":"responses","revision":1,"model":"model","usage":{"inputTokens":10,"outputTokens":0,"totalTokens":10},"durationMs":1000,"ttftMs":50})).unwrap()
}
fn row(id: &str) -> Record {
    let mut r = gateway(id);
    r.provider_id = Some("a".into());
    gateway_records(vec![r], &[]).remove(0)
}
fn snapshot(q: Query, rows: Vec<Record>) -> Snapshot {
    aggregate(
        q,
        &[provider("a"), provider("b")],
        rows,
        "source-time".into(),
        "ready".into(),
        now(),
    )
    .unwrap()
}

#[test]
fn provider_model_totals_isolate_same_models_and_cover_full_filtered_history() {
    let mut rows: Vec<_> = (0..85).map(|i| row(&format!("a-{i}"))).collect();
    let mut b = row("b");
    b.provider_id = Some("b".into());
    b.usage.total_tokens = Some(0);
    rows.push(b.clone());
    rows.push(b);
    let mut unknown = row("unknown");
    unknown.provider_id = None;
    rows.push(unknown);
    let result = snapshot(
        Query {
            provider_id: Some("a".into()),
            ..query()
        },
        rows.clone(),
    );
    assert_eq!(result.calls.len(), 40);
    assert_eq!(result.provider_models.len(), 2);
    assert_eq!(result.provider_models[0].totals.calls, 85);
    assert_eq!(
        result.provider_models[0].totals.usage.total_tokens,
        Some(850)
    );
    assert_eq!(result.provider_models[1].totals.calls, 1);
    assert_eq!(result.provider_models[1].totals.usage.total_tokens, Some(0));
    assert_eq!(result.unattributed.calls, 1);
    let next = snapshot(
        Query {
            provider_id: Some("a".into()),
            cursor: result.next_cursor,
            ..query()
        },
        rows.clone(),
    );
    assert_eq!(next.provider_models[0].totals.calls, 85);
    let all = snapshot(query(), rows.clone());
    assert_eq!(all.provider_models[0].totals.calls, 85);
    let filtered = snapshot(
        Query {
            agent: Some("opencode".into()),
            ..query()
        },
        rows.clone(),
    );
    assert!(filtered.provider_models.is_empty());
    for item in &mut rows {
        item.time = Some("2026-10-17T00:00:00Z".into());
    }
    let recent = snapshot(
        Query {
            days: Some(1),
            ..query()
        },
        rows,
    );
    assert!(recent.provider_models.is_empty());
    let json = serde_json::to_value(all).unwrap();
    assert_eq!(json["providerModels"][0]["providerId"], "a");
    assert_eq!(json["providerModels"][0]["model"], "model");
}

#[test]
fn gateway_identity_and_full_route_evidence_never_use_name_or_endpoint_alone() {
    let evidence = [evidence("a")];
    let old = gateway_records(vec![gateway("old")], &evidence).remove(0);
    assert_eq!(old.provider_id.as_deref(), Some("a"));
    assert_eq!(old.attribution, "route_snapshot");
    let mut changed = gateway("changed");
    changed.binding_revision = 13;
    assert!(
        gateway_records(vec![changed], &evidence)[0]
            .provider_id
            .is_none()
    );
    let conflict = [
        super::provider_usage_tests::evidence("a"),
        super::provider_usage_tests::evidence("b"),
    ];
    assert_eq!(
        gateway_records(vec![gateway("conflict")], &conflict)[0].attribution,
        "conflicting_evidence"
    );
    let mut explicit = gateway("new");
    explicit.provider_id = Some("b".into());
    explicit.provider = "renamed".into();
    explicit.endpoint = "https://old.example.com".into();
    assert_eq!(
        gateway_records(vec![explicit], &conflict)[0]
            .provider_id
            .as_deref(),
        Some("b")
    );
    let mut removed = row("removed");
    removed.provider_id = Some("removed".into());
    let data = snapshot(
        Query {
            provider_id: Some("_unknown".into()),
            ..query()
        },
        vec![removed],
    );
    assert_eq!(data.unattributed.calls, 1);
    assert_eq!(data.calls[0].attribution, "removed_provider");
    assert!(data.providers.iter().all(|p| p.totals.calls == 0));
}

#[test]
fn full_totals_models_zero_partial_and_cursor_are_independent_of_current_page() {
    let mut rows: Vec<_> = (0..85)
        .map(|n| {
            let mut r = row(&format!("r{n:03}"));
            r.model = Some(if n % 2 == 0 { "retired-model" } else { "model" }.into());
            r
        })
        .collect();
    rows.push(rows[0].clone());
    rows[2].status = "error".into();
    rows[3].usage = Usage::default();
    rows[4].status = "in_progress".into();
    let q = Query {
        provider_id: Some("a".into()),
        ..query()
    };
    let first = snapshot(q.clone(), rows.clone());
    assert_eq!(first.summary.calls, 85);
    assert_eq!(first.selected.as_ref().unwrap().calls, 85);
    assert_eq!(first.calls.len(), 40);
    assert_eq!(first.total_calls, 85);
    assert_eq!(first.models.len(), 2);
    assert_eq!(first.summary.usage.total_tokens, Some(840));
    assert_eq!(first.summary.usage.output_tokens, Some(0));
    assert_eq!(first.summary.reported["totalTokens"], 84);
    assert_eq!(first.summary.errors, 1);
    assert_eq!(first.summary.in_progress, 1);
    assert_eq!(first.summary.latency_samples, 84);
    let next = snapshot(
        Query {
            cursor: first.next_cursor.clone(),
            ..q.clone()
        },
        rows.clone(),
    );
    assert_eq!(next.summary.calls, 85);
    assert_eq!(next.calls.len(), 40);
    assert_ne!(first.calls[0].id, next.calls[0].id);
    let filtered = snapshot(
        Query {
            model: Some("retired-model".into()),
            ..q.clone()
        },
        rows.clone(),
    );
    assert_eq!(filtered.selected.as_ref().unwrap().calls, 43);
    assert_eq!(filtered.summary.calls, 85);
    assert_eq!(filtered.model_options.len(), 2);
    rows.push(row("later"));
    assert!(
        aggregate(
            Query {
                cursor: first.next_cursor,
                ..q
            },
            &[provider("a")],
            rows,
            "t".into(),
            "ready".into(),
            now()
        )
        .is_err()
    );
}

fn native(id: &str, provider: Option<&str>, model: &str) -> CallSummary {
    CallSummary {
        response_id: None,
        native_agent_product_id: None,
        agent_attribution: None,
        agent_product_id: None,
        id: id.into(),
        session_id: "session".into(),
        session_title: "session".into(),
        agent_id: "codex".into(),
        turn_id: "turn".into(),
        kind: "model".into(),
        name: "response".into(),
        status: "returned".into(),
        timestamp: Some("2026-10-19T10:00:00Z".into()),
        model: Some(model.into()),
        provider: provider.map(str::to_owned),
        model_response_id: None,
        usage: Some(Usage {
            total_tokens: Some(10),
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
fn native_ids_aliases_and_shared_calls_preserve_unknown_without_guessing() {
    let mut e = evidence("a");
    e.model_aliases = vec![("unique-alias".into(), "real-model".into())];
    let records = native_records(
        vec![
            native("one", Some("local_agent_known"), "model"),
            native("two", None, "unique-alias"),
            native("three", Some("Same name"), "model"),
            native("one", Some("local_agent_known"), "model"),
        ],
        &[e],
    );
    assert_eq!(records[0].attribution, "native_provider");
    assert_eq!(records[1].attribution, "native_model");
    assert_eq!(records[1].model.as_deref(), Some("real-model"));
    assert!(records[2].provider_id.is_none());
    let data = snapshot(
        Query {
            source: "native".into(),
            ..query()
        },
        records.clone(),
    );
    assert_eq!(data.summary.calls, 3);
    assert_eq!(data.unattributed.calls, 1);
    let cli = snapshot(
        Query {
            source: "native".into(),
            agent: Some("codex-cli".into()),
            ..query()
        },
        records,
    );
    assert_eq!(
        cli.summary.calls, 0,
        "a shared configuration does not prove CLI use"
    );
    let ambiguous = native_records(
        vec![native("ambiguous", Some("local_agent_known"), "model")],
        &[evidence("a"), evidence("b")],
    );
    assert_eq!(ambiguous[0].attribution, "conflicting_evidence");
}
#[test]
fn repeated_provider_evidence_preserves_identity_and_ambiguous_aliases_remain_unchanged() {
    let mut first = evidence("a");
    first.model_aliases = vec![("alias".into(), "first-model".into())];
    let mut repeated = evidence("a");
    repeated.model_aliases = first.model_aliases.clone();
    let mut changed_alias = evidence("a");
    changed_alias.model_aliases = vec![("alias".into(), "second-model".into())];
    let retained = [first, repeated, changed_alias];

    let gateway = gateway_records(vec![gateway("repeated")], &retained);
    assert_eq!(gateway[0].provider_id.as_deref(), Some("a"));
    assert_eq!(gateway[0].attribution, "route_snapshot");
    let native = native_records(
        vec![
            native("declared", Some("local_agent_known"), "alias"),
            native("alias-only", None, "alias"),
        ],
        &retained,
    );
    for row in &native {
        assert_eq!(row.provider_id.as_deref(), Some("a"));
        assert_eq!(row.model.as_deref(), Some("alias"));
    }
    assert_eq!(native[0].attribution, "native_provider");
    assert_eq!(native[1].attribution, "native_model");
}

#[test]
fn time_ranges_preserve_unknown_and_bad_filters_are_rejected() {
    let mut unknown = row("no-time");
    unknown.time = None;
    let mut future = row("future");
    future.time = Some("2026-10-20T10:00:00Z".into());
    let mut old = row("old");
    old.time = Some("2026-01-01T10:00:00Z".into());
    assert_eq!(
        snapshot(query(), vec![unknown.clone(), future.clone(), old.clone()])
            .summary
            .calls,
        2
    );
    let day = snapshot(
        Query {
            days: Some(1),
            ..query()
        },
        vec![unknown, future, old],
    );
    assert_eq!(day.summary.calls, 0);
    assert_eq!(day.missing_time, 1);
    assert!(
        Query {
            days: Some(2),
            ..query()
        }
        .validate()
        .is_err()
    );
    assert!(
        Query {
            source: "all".into(),
            ..query()
        }
        .validate()
        .is_err(),
        "overlapping sources cannot be added"
    );
    assert!(
        Query {
            agent: Some("made-up".into()),
            ..query()
        }
        .validate()
        .is_err()
    );
    let saved = gateway("legacy");
    let mut encoded = serde_json::to_value(&saved).unwrap();
    encoded.as_object_mut().unwrap().remove("providerId");
    assert!(
        serde_json::from_value::<ModelObservation>(encoded)
            .unwrap()
            .provider_id
            .is_none()
    );
}
