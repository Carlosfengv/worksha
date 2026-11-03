//! Resolve objects in the retained fact selection without reading a resource body.
use super::*;
use d::object_summary as o;

pub(in crate::domain) fn read(
    state: &AppState,
    selector: &o::ObjectSelector,
    fields: &[o::SummaryField],
) -> Result<o::ObjectSummaryRead> {
    use d::facts::{FactCandidate as F, FactKind};
    let (id, origin, kind) = match selector {
        o::ObjectSelector::Project { id, origin } => (id, origin, FactKind::Project),
        o::ObjectSelector::Session { id, origin } => (id, origin, FactKind::Session),
        _ => return Err(fail("invalid_input")),
    };
    let o::ObjectOrigin::Unified {
        scope,
        source_id,
        snapshot_token,
    } = origin
    else {
        return Err(fail("invalid_input"));
    };
    let (frozen, signature) = state.plugins.frozen_facts(snapshot_token)?;
    if frozen.scope != *scope || frozen.kind != kind {
        return Err(fail("permission_denied"));
    }
    let before = capture(state, &frozen)?;
    state
        .plugins
        .check_facts(snapshot_token, &before.signature)?;
    if signature != before.signature || kind == FactKind::Project && before.hidden.contains(id) {
        return Err(fail("stale_snapshot"));
    }
    let mut selection = frozen.clone();
    let selected = if kind == FactKind::Project {
        &mut selection.project_id
    } else {
        &mut selection.session_id
    };
    if selected.as_ref().is_some_and(|selected| selected != id) {
        return Err(fail("permission_denied"));
    }
    *selected = Some(id.clone());
    let (record, count, source) = if let Some(source) = before
        .native_status
        .iter()
        .find(|s| s.source_id == *source_id)
    {
        if source.authorization != d::Authorization::Authorized
            || source.coverage == d::Coverage::Unavailable
        {
            return Err(fail("grant_revoked"));
        }
        let (records, summary) = state
            .sessions
            .domain_facts_for_source(
                source_id,
                &selection,
                None,
                &before.hidden,
                source.coverage.clone(),
            )
            .map_err(|e| fail(e.code))?;
        let record = records
            .into_iter()
            .find(|r| r.id == *id)
            .ok_or(fail("not_found"))?;
        (record, summary.sessions.records, source.clone())
    } else {
        let view = before
            .views
            .iter()
            .find(|v| v.source().id == *source_id)
            .ok_or(fail("not_found"))?;
        let source = view.state();
        if source.authorization != d::Authorization::Authorized
            || source.coverage == d::Coverage::Unavailable
        {
            return Err(fail("grant_revoked"));
        }
        let legacy = match view {
            SourceView::Legacy(v) => legacy_snapshot(v)?,
            _ => None,
        };
        let snapshot = match view {
            SourceView::Full(v) => v.snapshot(),
            SourceView::Legacy(_) => legacy.as_ref(),
        }
        .ok_or(fail("not_found"))?;
        let graph = snapshot.filtered_graph(&selection, &before.hidden);
        let record = graph
            .iter()
            .find(|r| r.id == *id && r.value.fact_kind() == kind)
            .cloned()
            .ok_or(fail("not_found"))?;
        // Lack of session capability is unknown, not zero linked sessions.
        let count = snapshot
            .available_kinds
            .contains(&FactKind::Session)
            .then(|| {
                (graph
                    .iter()
                    .filter(|r| matches!(r.value, F::Session(_)))
                    .count() as u64)
                    .into()
            });
        (record, count, source)
    };
    let summary = match &record.value {
        F::Session(session) => super::super::object_summary::session_summary(
            &d::SessionRecord {
                id: record.id.clone(),
                native_id: session.native_id.clone(),
                product_ids: session.product_ids.clone(),
                unknown_client: session.unknown_client,
                title: session.title.clone(),
                created_at: session.created_at.clone(),
                updated_at: session.updated_at.clone(),
                project_ref: session.project_ref.clone(),
                usage: session.usage.clone(),
                partial: session.partial,
            },
            &source,
            fields,
        )?,
        F::Project(_) => o::ObjectSummary {
            api_version: o::API_VERSION.into(),
            target: o::ObjectTarget::Project,
            coverage: source.coverage,
            freshness: source.freshness,
            captured_at: record.identity.observed_at,
            metrics: fields
                .iter()
                .map(|field| o::SummaryMetric {
                    field: *field,
                    value: count.clone().map(|value| o::SummaryValue::Count { value }),
                    missing_reason: count.is_none().then_some(o::MissingReason::NotRecorded),
                })
                .collect(),
        },
        _ => return Err(fail("not_found")),
    };
    let after = capture(state, &frozen)?;
    state
        .plugins
        .check_facts(snapshot_token, &after.signature)?;
    Ok(o::ObjectSummaryRead {
        selector: selector.clone(),
        read_identity: local_agent_core::projects::hash(
            &serde_json::json!([snapshot_token, signature, selector, fields]).to_string(),
        ),
        summary,
    })
}
