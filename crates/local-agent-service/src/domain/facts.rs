use super::{query, sources::SourceView};
pub(super) mod object_summary;
use crate::{
    AppState,
    plugin_host::external::{self, Checked, Result, fail},
};
use local_agent_core::domain::{
    self as d,
    facts::{
        FactSnapshot,
        query::{FactPage, FactQuery, FactWindow},
    },
};

fn legacy_snapshot(
    view: &crate::plugin_host::external::domain::SourceView,
) -> Result<Option<FactSnapshot>> {
    use d::facts::{
        EvidenceLevel, FactCandidate, FactIdentity, FactKind, FactRecord, SessionCandidate,
    };
    if view.source.status.authorization != d::Authorization::Authorized {
        return Ok(None);
    }
    let Some(cache) = &view.cache else {
        return Ok(None);
    };
    let identity = FactIdentity {
        provider_key: view.provider.provider_key.clone(),
        source_id: view.source.id.clone(),
        implementation_identity: view.digest.clone(),
        epoch: serde_json::from_value(serde_json::json!(cache.epoch)).checked("invalid_record")?,
        grant_revision: serde_json::from_value(serde_json::json!(cache.revision))
            .checked("invalid_record")?,
        observed_at: cache.observed_at.clone(),
        evidence: EvidenceLevel::SourceReported,
    };
    let records = cache
        .sessions
        .iter()
        .map(|s| {
            let s = crate::plugin_host::external::domain::legacy_record(
                &view.plugin_id,
                &view.product.id,
                s,
            )?;
            Ok(FactRecord {
                id: s.id,
                identity: identity.clone(),
                source_revision: cache.snapshot.clone(),
                value: FactCandidate::Session(SessionCandidate {
                    native_id: s.native_id,
                    product_ids: s.product_ids,
                    unknown_client: s.unknown_client,
                    title: s.title,
                    created_at: s.created_at,
                    updated_at: s.updated_at,
                    project_ref: s.project_ref,
                    usage: s.usage,
                    partial: s.partial,
                }),
            })
        })
        .collect::<Result<_>>()?;
    Ok(Some(FactSnapshot {
        identity,
        available_kinds: vec![FactKind::Session],
        source_revision: cache.snapshot.clone(),
        coverage: view.state().coverage,
        records,
    }))
}

pub(super) async fn conversation(
    state: AppState,
    request: d::UnifiedConversationRequest,
) -> Result<d::UnifiedConversation> {
    if request.session_id.len() > 512
        || request.snapshot_token.len() > 80
        || request.cursor.as_ref().is_some_and(|c| c.len() > 4096)
        || request.anchor.as_ref().is_some_and(|a| a.len() > 512)
    {
        return Err(fail("invalid_input"));
    }
    let copy = state.clone();
    let token = request.snapshot_token.clone();
    let (frozen, expected, before) = external::run_sync(move || {
        let (query, signature) = copy.plugins.frozen_facts(&token)?;
        let capture = capture(&copy, &query)?;
        Ok((query, signature, capture))
    })
    .await?;
    if before.signature != expected {
        return Err(fail("stale_snapshot"));
    }
    if frozen
        .session_id
        .as_ref()
        .is_some_and(|id| id != &request.session_id)
    {
        return Err(fail("permission_denied"));
    }
    let native = before
        .native_status
        .iter()
        .find(|s| {
            s.authorization == d::Authorization::Authorized
                && request.session_id.starts_with(&format!("{}:", s.source_id))
        })
        .cloned();
    let (source_id, page) = if let Some(source) = native {
        let source_id = source.source_id.clone();
        let copy = state.clone();
        let input = request.clone();
        let mut selection = frozen.clone();
        selection.session_id = Some(input.session_id.clone());
        selection.kind = d::facts::FactKind::Session;
        let hidden = before.hidden;
        let page = crate::blocking::run_with(state.query_capacity.clone(), move || {
            let (records, _) = copy
                .sessions
                .domain_facts_for_source(
                    &source.source_id,
                    &selection,
                    None,
                    &hidden,
                    source.coverage,
                )
                .map_err(|e| fail(e.code))?;
            if !records.iter().any(|r| r.id == input.session_id) {
                return Err(fail("permission_denied"));
            }
            copy.sessions
                .domain_conversation(
                    &input.session_id,
                    input.cursor.as_deref(),
                    input.anchor.as_deref(),
                )
                .map_err(|_| fail("stale_snapshot"))
        })
        .await
        .map_err(|_| fail("busy"))??;
        (source_id, page)
    } else {
        let selection = frozen.clone();
        let session_id = request.session_id.clone();
        let selected = crate::blocking::run_with(state.query_capacity.clone(), move || {
            for view in before.views {
                let legacy = match view {
                    SourceView::Legacy(ref v) => legacy_snapshot(v)?,
                    _ => None,
                };
                let snapshot = match &view {
                    SourceView::Full(v) => v.snapshot(),
                    _ => legacy.as_ref(),
                };
                if snapshot.is_some_and(|s| {
                    s.filtered_graph(&selection, &before.hidden)
                        .iter()
                        .any(|r| {
                            r.id == session_id && r.value.fact_kind() == d::facts::FactKind::Session
                        })
                }) {
                    return Ok(view);
                }
            }
            Err(fail("permission_denied"))
        })
        .await
        .map_err(|_| fail("busy"))??;
        (
            selected.source().id.clone(),
            selected.read_conversation(&state, &request).await?,
        )
    };
    let copy = state.clone();
    let token = request.snapshot_token.clone();
    external::run_sync(move || {
        let after = capture(&copy, &frozen)?;
        copy.plugins.check_facts(&token, &after.signature)
    })
    .await?;
    Ok(d::UnifiedConversation {
        source_id,
        snapshot_token: request.snapshot_token,
        conversation: page,
    })
}

fn capture(state: &AppState, query: &FactQuery) -> Result<query::Capture> {
    query.validate().map_err(|_| fail("invalid_input"))?;
    let mut capture = query::capture(
        state,
        &d::UnifiedSessionQuery {
            scope: query.scope.clone(),
            filter: d::SessionFilter::default(),
            snapshot_token: None,
            cursor: None,
        },
    )?;
    if !capture.native_status.is_empty() {
        capture.signature = serde_json::json!([
            capture.signature,
            state
                .sessions
                .domain_fact_catalog_revision()
                .map_err(|_| fail("unavailable"))?,
        ])
        .to_string();
    }
    Ok(capture)
}
pub(super) fn facts(state: &AppState, query: FactQuery) -> Result<FactPage> {
    let before = capture(state, &query)?;
    let (token, as_of, after) = state.plugins.freeze_facts(&query, &before.signature)?;
    let mut window = FactWindow::new(after.clone());
    let mut summaries = Vec::new();
    let mut hidden_projects = Vec::new();
    let mut states = before.unavailable;
    for mut native in before.native_status {
        if native.coverage != d::Coverage::Unavailable {
            let (rows, summary) = state
                .sessions
                .domain_facts_for_source(
                    &native.source_id,
                    &query,
                    after.clone(),
                    &before.hidden,
                    native.coverage.clone(),
                )
                .map_err(|e| fail(e.code))?;
            native.coverage = summary.coverage.clone();
            if query.kind == d::facts::FactKind::McpCatalog && summary.mcp_catalogs.is_none() {
                native.coverage = d::Coverage::Unavailable;
                native.problem = Some("unavailable".into());
            }
            if query.kind == d::facts::FactKind::Activity {
                if summary.activities.is_none() {
                    native.coverage = d::Coverage::Unavailable;
                    native.problem = Some("unsupported".into());
                } else if native.problem.is_none() {
                    native.problem = Some("bounded_observation".into());
                }
            }
            summaries.push(summary);
            for row in rows {
                window.observe(row);
            }
            if !local_agent_core::sessions::NativeSessions::supports_domain_fact(query.kind) {
                native.coverage = d::Coverage::Unavailable;
                native.problem = Some("unsupported".into());
            }
        }
        states.push(native);
    }
    for view in &before.views {
        let mut status = view.state();
        let legacy = match view {
            SourceView::Legacy(v) => legacy_snapshot(v)?,
            _ => None,
        };
        let snapshot = match view {
            SourceView::Full(full) => full.snapshot(),
            SourceView::Legacy(_) => legacy.as_ref(),
        };
        if let Some(snapshot) = snapshot {
            let metadata = FactSnapshot {
                identity: snapshot.identity.clone(),
                source_revision: snapshot.source_revision.clone(),
                available_kinds: snapshot.available_kinds.clone(),
                coverage: status.coverage.clone(),
                records: vec![],
            };
            for fact in &snapshot.records {
                if let d::facts::FactCandidate::Project(project) = &fact.value
                    && before.hidden.contains(&fact.id)
                {
                    hidden_projects.push(d::facts::query::HiddenProject {
                        source_id: snapshot.identity.source_id.clone(),
                        id: fact.id.clone(),
                        name: project.name.clone(),
                    });
                }
            }
            let graph = snapshot.filtered_graph(&query, &before.hidden);
            summaries.push(
                metadata
                    .summary(&graph)
                    .map_err(|_| fail("resource_limit"))?,
            );
            for fact in graph
                .into_iter()
                .filter(|r| r.value.fact_kind() == query.kind)
            {
                window.observe(fact);
            }
            if !snapshot.available_kinds.contains(&query.kind) {
                status.coverage = d::Coverage::Unavailable;
                status.problem = Some("unsupported".into());
            }
        } else if status.coverage != d::Coverage::Unavailable {
            status.coverage = d::Coverage::Unavailable;
            status.problem = Some("unsupported".into());
        }
        states.push(status);
    }
    let (items, next) = window.finish();
    let next = next
        .map(|order| {
            let bytes = serde_json::to_vec(&order).checked("invalid_record")?;
            Ok(format!(
                "{token}.{}",
                bytes.iter().map(|b| format!("{b:02x}")).collect::<String>()
            ))
        })
        .transpose()?;
    let after = capture(state, &query)?;
    state.plugins.check_facts(&token, &after.signature)?;
    if before.signature != after.signature {
        return Err(fail("stale_snapshot"));
    }
    Ok(FactPage {
        items,
        snapshot_token: token,
        next_cursor: next,
        as_of,
        source_states: states,
        source_summaries: summaries,
        applied_query: query,
        overlap_unknown: true,
        hidden_projects,
    })
}
