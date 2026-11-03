use super::*;
use serde_json::{Value, json};

fn identity() -> FactIdentity {
    FactIdentity {
        provider_key: "org.example.reader:domain.project-source@1:history".into(),
        source_id: "org.example.reader:history".into(),
        implementation_identity: "digest-one".into(),
        epoch: 2u64.into(),
        grant_revision: 3u64.into(),
        observed_at: "2026-10-31T00:00:00Z".into(),
        evidence: EvidenceLevel::SourceReported,
    }
}
fn publisher() -> FactPublication {
    FactPublication::new(
        identity(),
        vec!["org.example.reader:cli".into()],
        FactKind::ALL.to_vec(),
    )
    .unwrap()
}
fn records() -> Vec<FactCandidate> {
    serde_json::from_value(json!([
        {"kind":"project","value":{"nativeId":"p:one","name":"Reported workspace","pathHint":"/private/source-reported-only"}},
        {"kind":"session","value":{"nativeId":"s:one","productIds":["org.example.reader:cli"],"unknownClient":false,"title":"Fixture","createdAt":null,"updatedAt":null,"projectRef":"p:one","usage":{"inputTokens":"15","outputTokens":"4","cachedInputTokens":"5","cacheWriteInputTokens":null,"reasoningOutputTokens":"1","totalTokens":"19"},"partial":false}},
        {"kind":"response","value":{"nativeId":"r:one","sessionRef":"s:one","productId":"org.example.reader:cli","occurredAt":null,"model":"fixture","outcome":"success","usage":{"inputTokens":"10","outputTokens":"2","cachedInputTokens":null,"cacheWriteInputTokens":null,"reasoningOutputTokens":null,"totalTokens":"12"},"reportedResponseId":"same-unverified-upstream-id","durationMs":"200"}},
        {"kind":"activity","value":{"nativeId":"a:one","sessionRef":"s:one","responseRef":"r:one","occurredAt":null,"phase":"generating","durationMs":"100"}},
        {"kind":"write","value":{"nativeId":"w:one","sessionRef":"s:one","projectRef":"p:one","responseRef":"r:one","relativePath":"src/example.txt","occurredAt":null,"outcome":"success"}},
        {"kind":"mcp-catalog","value":{"nativeId":"m:one","serverName":"fixture","toolNames":["lookup"]}},
        {"kind":"mcp-execution","value":{"nativeId":"e:one","serverRef":"m:one","sessionRef":"s:one","responseRef":"r:one","toolName":"lookup","occurredAt":null,"durationMs":null,"outcome":"unknown"}}
    ])).unwrap()
}
fn batch(records: Vec<FactCandidate>, n: u64) -> FactBatch {
    FactBatch {
        source_revision: "generation-1".into(),
        batch_id: n.into(),
        records,
    }
}
fn checkpoint(batches: u64, records: u64) -> Checkpoint {
    Checkpoint {
        source_revision: "generation-1".into(),
        batches: batches.into(),
        records: records.into(),
        complete: true,
        coverage: Coverage::Complete,
    }
}
fn finish(records: Vec<FactCandidate>) -> Result<FactSnapshot> {
    let mut p = publisher();
    let n = records.len() as u64;
    p.accept(batch(records, 0))?;
    p.finish(&checkpoint(1, n))
}
#[test]
fn missing_mcp_tool_observation_is_distinct_from_an_observed_empty_catalog() {
    for tools in [
        serde_json::Value::Null,
        serde_json::json!([]),
        serde_json::json!(["lookup"]),
    ] {
        let candidate: FactCandidate = serde_json::from_value(serde_json::json!({
            "kind":"mcp-catalog","value":{"nativeId":"m","serverName":"server","toolNames":tools}
        }))
        .unwrap();
        let snapshot = finish(vec![candidate]).unwrap();
        assert_eq!(
            serde_json::to_value(&snapshot.records[0].value).unwrap()["value"]["toolNames"],
            tools
        );
    }
    let duplicate:FactCandidate=serde_json::from_value(serde_json::json!({
        "kind":"mcp-catalog","value":{"nativeId":"m","serverName":"server","toolNames":["lookup","lookup"]}
    })).unwrap();
    assert!(finish(vec![duplicate]).is_err());
}
#[test]
fn complete_graph_gets_host_identity_and_links_without_native_access_or_gateway_claims() {
    let r = finish(records()).unwrap();
    assert_eq!(r.identity, identity());
    assert_eq!(r.records.len(), 7);
    for fact in &r.records {
        assert_eq!(fact.identity, identity());
        assert_eq!(
            fact.id,
            object_id(
                &identity().source_id,
                fact.value.kind(),
                fact.value.native_id()
            )
        );
    }
    let source = &identity().source_id;
    if let FactCandidate::Session(s) = &r.records[1].value {
        assert_eq!(
            s.project_ref.as_deref(),
            Some(object_id(source, "project", "p:one").as_str())
        );
        assert_eq!(s.usage.input_tokens.as_ref().unwrap().value(), 15);
    } else {
        panic!("session");
    }
    if let FactCandidate::Response(v) = &r.records[2].value {
        assert_eq!(v.session_ref, object_id(source, "session", "s:one"));
        assert_eq!(v.usage.input_tokens.as_ref().unwrap().value(), 10);
        assert_eq!(
            v.reported_response_id.as_deref(),
            Some("same-unverified-upstream-id")
        );
    } else {
        panic!("response");
    }
    let value = serde_json::to_value(&r).unwrap();
    assert_eq!(value["identity"]["evidence"], "source-reported");
    assert!(!value.to_string().contains("tokensPerSecond"));
    assert!(!value.to_string().contains("workspaceAuthorized"));
    // Catalog and execution remain distinct records, including unknown outcomes.
    assert_eq!(r.records[5].value.kind(), "mcp-catalog");
    assert_eq!(r.records[6].value.kind(), "mcp-execution");
    let totals = r.summary(&r.records).unwrap();
    let mut tally = FactTally::new(source, &r.available_kinds, r.coverage.clone()).unwrap();
    for record in &r.records {
        tally.observe(&record.value).unwrap();
    }
    assert_eq!(tally.finish(), totals);
    assert_eq!(totals.sessions.unit, UsageUnit::SessionCumulative);
    assert_eq!(totals.responses.unit, UsageUnit::ModelResponse);
    assert_eq!(totals.sessions.usage.input_tokens.unwrap().value(), 15);
    assert_eq!(totals.responses.usage.input_tokens.unwrap().value(), 10);
    assert_eq!(totals.mcp_catalogs.unwrap().value(), 1);
    assert_eq!(totals.mcp_executions.unwrap().value(), 1);
    assert!(totals.cross_source_overlap_unknown);
}

#[test]
fn undeclared_fact_units_are_unknown_and_cannot_be_injected_by_a_guest() {
    let mut p = FactPublication::new(
        identity(),
        vec!["org.example.reader:cli".into()],
        vec![FactKind::Project],
    )
    .unwrap();
    p.accept(batch(vec![records()[0].clone()], 0)).unwrap();
    let complete = p.finish(&checkpoint(1, 1)).unwrap();
    let summary = complete.summary(&complete.records).unwrap();
    assert_eq!(summary.projects.unwrap().value(), 1);
    assert!(summary.mcp_executions.is_none());
    assert!(summary.sessions.records.is_none());
    assert!(summary.responses.usage.total_tokens.is_none());
    assert_eq!(summary.responses.coverage, Coverage::Unavailable);
    let mut p = FactPublication::new(
        identity(),
        vec!["org.example.reader:cli".into()],
        vec![FactKind::Project],
    )
    .unwrap();
    assert!(p.accept(batch(records(), 0)).is_err());
    assert!(p.finish(&checkpoint(0, 0)).is_err());
}

#[test]
fn graph_filters_keep_summary_units_and_hide_only_project_evidence() {
    let snapshot = finish(records()).unwrap();
    let source = &identity().source_id;
    let mut query = super::super::query::FactQuery {
        scope: crate::domain::QueryScope {
            kind: crate::domain::ScopeKind::External,
            source_ids: vec![source.clone()],
        },
        kind: FactKind::Project,
        project_id: Some(object_id(source, "project", "p:one")),
        session_id: None,
        response_id: None,
        snapshot_token: None,
        cursor: None,
    };
    let graph = snapshot.filtered_graph(&query, &[]);
    assert_eq!(graph.len(), 7);
    let summary = snapshot.summary(&graph).unwrap();
    assert_eq!(summary.sessions.usage.input_tokens.unwrap().value(), 15);
    assert_eq!(summary.responses.usage.input_tokens.unwrap().value(), 10);
    let hidden = vec![query.project_id.clone().unwrap()];
    assert!(snapshot.filtered_graph(&query, &hidden).is_empty());
    query.project_id = None;
    query.response_id = Some(object_id(source, "response", "r:one"));
    assert_eq!(snapshot.filtered_graph(&query, &[]).len(), 7);
    query.response_id = Some("unverified-upstream-id".into());
    assert!(snapshot.filtered_graph(&query, &[]).is_empty());
    query.response_id = None;
    let redacted = snapshot.filtered_graph(&query, &hidden);
    assert_eq!(redacted.len(), 5);
    assert!(redacted.iter().all(|f| match &f.value {
        FactCandidate::Project(_) | FactCandidate::Write(_) => false,
        FactCandidate::Session(s) => s.project_ref.is_none(),
        _ => true,
    }));
    let summary = snapshot.summary(&redacted).unwrap();
    assert_eq!(summary.sessions.records.unwrap().value(), 1);
    assert_eq!(summary.responses.records.unwrap().value(), 1);
    query.session_id = Some("other-source-session".into());
    assert!(snapshot.filtered_graph(&query, &[]).is_empty());
}

#[test]
fn fact_pages_seek_without_skips_duplicates_or_full_page_summaries() {
    use super::super::query::FactWindow;
    let snapshot = finish(records()).unwrap();
    let original = snapshot.records[1].clone();
    let rows: Vec<_> = (0..123)
        .map(|n| {
            let mut r = original.clone();
            r.id = format!("session-{n:04}");
            r
        })
        .collect();
    let mut after = None;
    let mut seen = Vec::new();
    let mut sizes = Vec::new();
    loop {
        let mut window = FactWindow::new(after);
        for row in rows.iter().rev() {
            window.observe(row.clone());
        }
        let (page, next) = window.finish();
        sizes.push(page.len());
        seen.extend(page.into_iter().map(|f| f.id));
        if next.is_none() {
            break;
        }
        after = next;
    }
    assert_eq!(sizes, [40, 40, 40, 3]);
    assert_eq!(seen, rows.iter().map(|r| r.id.clone()).collect::<Vec<_>>());
}

#[test]
fn streaming_fact_tally_matches_filtered_snapshot_totals_for_mixed_unknown_usage() {
    let mut snapshot = finish(records()).unwrap();
    let mut extra = snapshot.records[2].clone();
    extra.id = "second-response".into();
    if let FactCandidate::Response(r) = &mut snapshot.records[2].value {
        r.usage.input_tokens = None;
    }
    if let FactCandidate::Response(r) = &mut extra.value {
        r.usage.output_tokens = None;
    }
    snapshot.records.push(extra);
    for coverage in [Coverage::Complete, Coverage::Partial] {
        snapshot.coverage = coverage.clone();
        let expected = snapshot.summary(&snapshot.records).unwrap();
        let mut tally = FactTally::new(
            &snapshot.identity.source_id,
            &snapshot.available_kinds,
            coverage,
        )
        .unwrap();
        for record in &snapshot.records {
            tally.observe(&record.value).unwrap();
        }
        assert_eq!(tally.finish(), expected);
    }
    let unavailable = FactTally::new(
        &snapshot.identity.source_id,
        &FactKind::ALL,
        Coverage::Unavailable,
    )
    .unwrap()
    .finish();
    assert!(unavailable.projects.is_none() && unavailable.sessions.records.is_none());
}
#[test]
fn source_identity_is_structured_stable_across_digest_and_independent_across_sources() {
    assert_ne!(object_id("a:b", "c", "d"), object_id("a", "b:c", "d"));
    assert_ne!(
        object_id("a", "session", "id"),
        object_id("b", "session", "id")
    );
    let old = finish(records()).unwrap();
    let mut newer = identity();
    newer.implementation_identity = "digest-two".into();
    newer.epoch = 4u64.into();
    let mut p = FactPublication::new(
        newer.clone(),
        vec!["org.example.reader:cli".into()],
        FactKind::ALL.to_vec(),
    )
    .unwrap();
    p.accept(batch(records(), 0)).unwrap();
    let next = p.finish(&checkpoint(1, 7)).unwrap();
    assert_eq!(
        old.records.iter().map(|r| &r.id).collect::<Vec<_>>(),
        next.records.iter().map(|r| &r.id).collect::<Vec<_>>()
    );
    assert_eq!(next.identity, newer);
    assert_ne!(old.identity, next.identity);
}
#[test]
fn candidates_cannot_forge_host_authority_or_add_body_fields() {
    let base = serde_json::to_value(&records()[0]).unwrap();
    for key in [
        "observedAt",
        "grantRevision",
        "evidence",
        "upstreamSent",
        "billable",
        "workspaceAuthorized",
        "body",
    ] {
        let mut v = base.clone();
        v["value"][key] = json!(true);
        assert!(serde_json::from_value::<FactCandidate>(v).is_err(), "{key}");
    }
    let mut v = base;
    v["identity"] = serde_json::to_value(identity()).unwrap();
    assert!(serde_json::from_value::<FactCandidate>(v).is_err());
}
#[test]
fn dangling_cross_session_product_and_project_links_reject_the_whole_generation() {
    for (index, key, wrong) in [
        (1, "projectRef", "missing-project"),
        (2, "sessionRef", "missing-session"),
        (2, "productId", "claude-code-cli"),
        (3, "responseRef", "missing-response"),
        (4, "projectRef", "missing-project"),
        (6, "serverRef", "missing-server"),
    ] {
        let mut v = serde_json::to_value(records()).unwrap();
        v[index]["value"][key] = json!(wrong);
        let values = serde_json::from_value(v).unwrap();
        assert!(finish(values).is_err(), "{index}:{key}");
    }
    let mut v = serde_json::to_value(records()).unwrap();
    let mut another = v[1].clone();
    another["value"]["nativeId"] = json!("another-session");
    v.as_array_mut().unwrap().push(another);
    v[3]["value"]["sessionRef"] = json!("another-session");
    assert!(finish(serde_json::from_value(v).unwrap()).is_err());
}
#[test]
fn invalid_input_poisons_publication_and_no_incomplete_generation_escapes() {
    let mut p = publisher();
    p.accept(batch(records(), 0)).unwrap();
    assert!(p.accept(batch(records(), 1)).is_err());
    assert!(p.accept(batch(vec![records()[0].clone()], 1)).is_err());
    assert!(p.finish(&checkpoint(1, 7)).is_err());
    for change in ["complete", "batches", "records", "revision"] {
        let mut p = publisher();
        p.accept(batch(records(), 0)).unwrap();
        let mut c = checkpoint(1, 7);
        match change {
            "complete" => c.complete = false,
            "batches" => c.batches = 2u64.into(),
            "records" => c.records = 8u64.into(),
            _ => c.source_revision = "different".into(),
        }
        assert!(p.finish(&c).is_err());
    }
    let empty = publisher().finish(&checkpoint(0, 0)).unwrap();
    assert!(empty.records.is_empty());
}
#[test]
fn counts_are_decimal_and_unknown_is_preserved_without_cross_unit_addition() {
    let mut v = serde_json::to_value(records()).unwrap();
    for key in [
        "inputTokens",
        "outputTokens",
        "cachedInputTokens",
        "cacheWriteInputTokens",
        "reasoningOutputTokens",
        "totalTokens",
    ] {
        v[1]["value"]["usage"][key] = Value::Null;
    }
    v[1]["value"]["productIds"] = json!([]);
    v[1]["value"]["unknownClient"] = json!(true);
    v[2]["value"]["productId"] = Value::Null;
    let result = finish(serde_json::from_value(v.clone()).unwrap()).unwrap();
    if let FactCandidate::Session(s) = &result.records[1].value {
        assert!(s.usage.total_tokens.is_none());
    }
    v[2]["value"]["usage"]["inputTokens"] = json!("18446744073709551615");
    assert!(finish(serde_json::from_value(v.clone()).unwrap()).is_ok());
    v[2]["value"]["usage"]["inputTokens"] = json!(18446744073709551615u64);
    assert!(serde_json::from_value::<Vec<FactCandidate>>(v).is_err());
    let mut invalid = records();
    if let FactCandidate::Response(v) = &mut invalid[2] {
        v.usage.cached_input_tokens = Some(11u64.into());
    }
    assert!(finish(invalid).is_err());
}
#[test]
fn record_and_batch_limits_fail_closed_without_silent_truncation() {
    let mut p = publisher();
    for batch_id in 0..(MAX_FACTS / BATCH_FACTS) {
        let rows = (0..BATCH_FACTS)
            .map(|n| {
                FactCandidate::Project(ProjectCandidate {
                    native_id: format!("{batch_id}:{n}"),
                    name: "fixture".into(),
                    path_hint: None,
                })
            })
            .collect();
        p.accept(batch(rows, batch_id as u64)).unwrap();
    }
    assert_eq!(
        p.accept(batch(vec![records()[0].clone()], 16))
            .unwrap_err()
            .code,
        ErrorCode::ResourceLimit
    );
    assert!(p.finish(&checkpoint(16, MAX_FACTS as u64)).is_err());
    let mut p = publisher();
    assert!(
        p.accept(batch(vec![records()[0].clone(); BATCH_FACTS + 1], 0))
            .is_err()
    );
}

#[test]
fn schemas_are_generated_from_the_same_strict_facts_types() {
    let root =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../contracts/domain/facts");
    for (name, expected) in super::super::schemas().as_object().unwrap() {
        let actual: Value = serde_json::from_slice(
            &std::fs::read(root.join(format!("{name}.schema.json"))).unwrap(),
        )
        .unwrap();
        assert_eq!(&actual, expected, "facts schema drift: {name}");
    }
}
