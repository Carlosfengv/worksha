use super::sources::{SourceView, external_sources};
use crate::{
    AppState,
    plugin_host::external::{self, Checked, Result, fail},
};
use local_agent_core::domain as d;
use serde_json::json;

pub(super) fn capabilities(state: &AppState) -> Result<d::CapabilityCatalog> {
    let mut entries = state
        .sessions
        .domain_capabilities()
        .map_err(|_| fail("unavailable"))?;
    entries.extend(state.readonly.projects.domain_capabilities());
    entries.extend(state.readonly.accounts.domain_capabilities());
    entries.push(state.observation.domain_activity_capability());
    entries.extend(local_agent_core::requests::domain_read::builtin_capabilities());
    let (sources, external_problem) = external_sources(state);
    entries.extend(sources.iter().flat_map(SourceView::capabilities));
    let revision = state
        .plugins
        .domain_catalog_revision(&json!([entries, external_problem]))?;
    let result = d::CapabilityCatalog {
        api_version: d::API_VERSION.into(),
        revision,
        entries,
        external_problem,
    };
    result.validate().map_err(|_| fail("invalid_record"))?;
    Ok(result)
}

pub(super) fn directory(state: &AppState) -> Result<d::UnifiedCatalog> {
    let mut catalog = state
        .sessions
        .domain_catalog()
        .map_err(|_| fail("unavailable"))?;
    let (sources, problem) = external_sources(state);
    let mut consents = vec![];
    for view in sources {
        if !view.source().status.enabled {
            continue;
        }
        consents.push(d::SourceConsentStatus {
            plugin_id: view.plugin_id().into(),
            digest: view.digest().into(),
            source_id: view.source().id.clone(),
            enabled: view.source().status.authorization == d::Authorization::Authorized,
            persist_metadata: view.persist_metadata(),
        });
        catalog.providers.push(view.provider().clone());
        catalog.products.extend_from_slice(view.products());
        catalog.sources.push(view.source().clone());
    }
    catalog.revision = state.plugins.domain_catalog_revision(&json!([
        catalog.providers,
        catalog.products,
        catalog.sources,
        consents,
        problem
    ]))?;
    catalog.validate().map_err(|_| fail("invalid_record"))?;
    Ok(d::UnifiedCatalog {
        catalog,
        consents,
        external_problem: problem,
    })
}
pub(super) struct Capture {
    pub signature: String,
    /// Permission/implementation vector without native history scan progress.
    pub source_signature: String,
    pub views: Vec<SourceView>,
    pub native_status: Vec<d::SourceState>,
    pub hidden: Vec<String>,
    pub unavailable: Vec<d::SourceState>,
}
pub(super) fn capture(state: &AppState, query: &d::UnifiedSessionQuery) -> Result<Capture> {
    query.filter.validate().map_err(|_| fail("invalid_input"))?;
    query.scope.validate().map_err(|_| fail("invalid_input"))?;
    let ids = &query.scope.source_ids;
    let native_catalog = state
        .sessions
        .domain_catalog()
        .map_err(|_| fail("unavailable"))?;
    let builtin_ids: Vec<_> = native_catalog
        .sources
        .iter()
        .filter(|s| ids.contains(&s.id))
        .map(|s| s.id.clone())
        .collect();
    let external_ids: Vec<_> = ids
        .iter()
        .filter(|id| !builtin_ids.contains(id))
        .cloned()
        .collect();
    if matches!(query.scope.kind, d::ScopeKind::Builtin) && !external_ids.is_empty()
        || matches!(query.scope.kind, d::ScopeKind::External) && !builtin_ids.is_empty()
        || matches!(query.scope.kind, d::ScopeKind::Combined)
            && (builtin_ids.is_empty() || external_ids.is_empty())
    {
        return Err(fail("invalid_input"));
    }
    let mut unavailable = vec![];
    let views = if !external_ids.is_empty() {
        let (mut views, problem) = external_sources(state);
        if let Some(problem) = problem {
            unavailable = external_ids
                .iter()
                .filter(|id| !views.iter().any(|v| &v.source().id == *id))
                .map(|id| d::SourceState {
                    source_id: id.clone(),
                    authorization: d::Authorization::Required,
                    freshness: d::Freshness::Failed,
                    coverage: d::Coverage::Unavailable,
                    problem: Some(problem.clone()),
                })
                .collect();
        }
        views.retain(|v| ids.contains(&v.source().id));
        if views.len() + unavailable.len() != external_ids.len() {
            return Err(fail("not_found"));
        }
        views
    } else {
        vec![]
    };
    if let Some(product) = &query.filter.product_id {
        let found = views
            .iter()
            .any(|v| v.products().iter().any(|p| &p.id == product))
            || native_catalog
                .products
                .iter()
                .any(|p| &p.id == product && builtin_ids.contains(&p.source_id));
        if !found {
            return Err(fail("invalid_input"));
        }
    }
    let native_status: Vec<_> = native_catalog
        .sources
        .iter()
        .filter(|s| builtin_ids.contains(&s.id))
        .map(|s| d::SourceState {
            source_id: s.id.clone(),
            authorization: s.status.authorization.clone(),
            freshness: s.status.freshness.clone(),
            coverage: if s.status.authorization != d::Authorization::Authorized
                || !s.status.runtime_active
                || s.status.freshness == d::Freshness::Unread
            {
                d::Coverage::Unavailable
            } else if s.status.freshness == d::Freshness::Current {
                d::Coverage::Complete
            } else {
                d::Coverage::Partial
            },
            problem: None,
        })
        .collect();
    let mut hidden = crate::projects::hidden(state)
        .map_err(|_| fail("unavailable"))?
        .into_iter()
        .map(|p| p.id)
        .collect::<Vec<_>>();
    for view in &views {
        if let SourceView::Full(full) = view {
            hidden.extend(full.hidden_projects.iter().cloned());
        }
    }
    hidden.sort();
    hidden.dedup();
    let revision = if !builtin_ids.is_empty() {
        Some(
            state
                .sessions
                .domain_revision()
                .map_err(|_| fail("unavailable"))?,
        )
    } else {
        None
    };
    let signature = json!([
        revision,
        native_status,
        views.iter().map(SourceView::vector).collect::<Vec<_>>(),
        hidden,
        unavailable
    ])
    .to_string();
    let source_signature = json!([
        native_catalog
            .sources
            .iter()
            .filter(|s| builtin_ids.contains(&s.id))
            .map(|s| (
                &s.id,
                &s.provider_key,
                &s.status.authorization,
                &s.status.grant_revision,
                s.status.runtime_active
            ))
            .collect::<Vec<_>>(),
        native_catalog
            .providers
            .iter()
            .filter(|p| native_catalog
                .sources
                .iter()
                .any(|s| builtin_ids.contains(&s.id) && s.provider_key == p.provider_key))
            .collect::<Vec<_>>(),
        views.iter().map(SourceView::vector).collect::<Vec<_>>(),
        hidden,
        unavailable
    ])
    .to_string();
    Ok(Capture {
        signature,
        source_signature,
        views,
        native_status,
        hidden,
        unavailable,
    })
}
pub(super) fn sessions(
    state: &AppState,
    query: d::UnifiedSessionQuery,
) -> Result<d::UnifiedSessionPage> {
    let before = capture(state, &query)?;
    let (token, frozen, after) = state.plugins.domain_freeze(&query, &before.signature)?;
    let mut rows = vec![];
    let mut summaries = vec![];
    let source_count = query.scope.source_ids.len();
    let mut states = before.unavailable;
    for mut source in before.native_status {
        if source.coverage != d::Coverage::Unavailable {
            match state.sessions.domain_sessions_for_source(
                &source.source_id,
                &query.filter,
                after.clone(),
                frozen.cutoff,
                &before.hidden,
            ) {
                Ok((items, summary)) => {
                    rows.extend(items);
                    summaries.push(summary);
                }
                Err(error) => {
                    source.coverage = d::Coverage::Unavailable;
                    source.problem = Some(error.code.into());
                }
            }
        }
        states.push(source);
    }
    for view in &before.views {
        let mut source_state = view.state();
        if source_state.coverage == d::Coverage::Unavailable {
            states.push(source_state);
            continue;
        }
        match view.sessions(&query.filter, after.clone(), frozen.cutoff, &before.hidden) {
            Ok((items, summary)) => {
                rows.extend(items);
                summaries.push(summary);
            }
            Err(e) => {
                source_state.coverage = d::Coverage::Unavailable;
                source_state.problem = Some(e.code.into());
            }
        }
        states.push(source_state);
    }
    rows.sort_by_key(|item| d::SessionOrder::of(&item.record));
    let more = rows.len() > d::PAGE_SIZE;
    rows.truncate(d::PAGE_SIZE);
    let cursor = if more {
        rows.last()
            .map(|r| {
                serde_json::to_vec(&d::SessionOrder::of(&r.record))
                    .checked("invalid_record")
                    .map(|bytes| {
                        format!(
                            "{token}.{}",
                            bytes.iter().map(|b| format!("{b:02x}")).collect::<String>()
                        )
                    })
            })
            .transpose()?
    } else {
        None
    };
    let after = capture(state, &query)?;
    state
        .plugins
        .domain_check_frozen(&token, &after.signature)?;
    let coverage = if states
        .iter()
        .all(|s| s.coverage == d::Coverage::Unavailable)
    {
        d::Coverage::Unavailable
    } else if states.iter().all(|s| s.coverage == d::Coverage::Complete) {
        d::Coverage::Complete
    } else {
        d::Coverage::Partial
    };
    Ok(d::UnifiedSessionPage {
        items: rows,
        snapshot_token: token.clone(),
        next_cursor: cursor,
        as_of: frozen.as_of,
        coverage,
        source_states: states,
        applied_filter: query,
        summary_ref: token,
        source_summaries: summaries,
        overlap_unknown: source_count > 1,
    })
}
pub(super) async fn conversation(
    state: AppState,
    request: d::UnifiedConversationRequest,
) -> Result<d::UnifiedConversation> {
    if request.snapshot_token.len() > 100
        || request.session_id.len() > 512
        || request.cursor.as_ref().is_some_and(|s| s.len() > 4096)
    {
        return Err(fail("invalid_input"));
    }
    let plugins = state.plugins.clone();
    let token = request.snapshot_token.clone();
    let frozen = external::run_sync(move || plugins.domain_frozen(&token)).await?;
    let copy = state.clone();
    let query = frozen.query.clone();
    let before = external::run_sync(move || capture(&copy, &query)).await?;
    if before.signature != frozen.signature {
        return Err(fail("stale_snapshot"));
    }
    let native_source = before
        .native_status
        .iter()
        .find(|s| {
            s.authorization == d::Authorization::Authorized
                && request.session_id.starts_with(&format!("{}:", s.source_id))
        })
        .map(|s| s.source_id.clone());
    let (source_id, page) = if let Some(native_source) = native_source {
        let copy = state.clone();
        let request = request.clone();
        let filter = frozen.query.filter.clone();
        let page = crate::blocking::run_with(state.query_capacity.clone(), move || {
            if !copy
                .sessions
                .domain_session_matches(&request.session_id, &filter, frozen.cutoff)
                .map_err(|_| fail("unavailable"))?
            {
                return Err(fail("not_found"));
            }
            copy.sessions
                .domain_conversation(
                    &request.session_id,
                    request.cursor.as_deref(),
                    request.anchor.as_deref(),
                )
                .map_err(|_| fail("stale_snapshot"))
        })
        .await
        .map_err(|_| fail("busy"))??;
        (native_source, page)
    } else {
        let view = before
            .views
            .iter()
            .find(|v| v.matches_session(&request.session_id, &frozen.query.filter, frozen.cutoff))
            .ok_or(fail("permission_denied"))?;
        (
            view.source().id.clone(),
            view.read_conversation(&state, &request).await?,
        )
    };
    let copy = state.clone();
    let query = frozen.query;
    let after = external::run_sync(move || capture(&copy, &query)).await?;
    state
        .plugins
        .domain_check_frozen(&request.snapshot_token, &after.signature)?;
    Ok(d::UnifiedConversation {
        source_id,
        snapshot_token: request.snapshot_token,
        conversation: page,
    })
}

/// A detail summary belongs to the original frozen selection, including filters.
/// Resolving metadata never opens a conversation or creates a replacement query.
pub(super) fn object_session_summary(
    state: &AppState,
    selector: &d::object_summary::ObjectSelector,
    fields: &[d::object_summary::SummaryField],
) -> Result<d::object_summary::ObjectSummaryRead> {
    use d::object_summary as o;
    let o::ObjectSelector::Session {
        id,
        origin:
            o::ObjectOrigin::Unified {
                scope,
                source_id,
                snapshot_token,
            },
    } = selector
    else {
        return Err(fail("invalid_input"));
    };
    let frozen = state.plugins.domain_frozen(snapshot_token)?;
    if frozen.query.scope != *scope {
        return Err(fail("permission_denied"));
    }
    let before = capture(state, &frozen.query)?;
    state
        .plugins
        .domain_check_frozen(snapshot_token, &before.signature)?;
    let summary = if let Some(source) = before
        .native_status
        .iter()
        .find(|s| s.source_id == *source_id)
    {
        if source.authorization != d::Authorization::Authorized
            || source.coverage == d::Coverage::Unavailable
        {
            return Err(fail("grant_revoked"));
        }
        if !id.starts_with(&format!("{source_id}:"))
            || !state
                .sessions
                .domain_session_matches(id, &frozen.query.filter, frozen.cutoff)
                .map_err(|e| fail(e.code))?
        {
            return Err(fail("not_found"));
        }
        state
            .sessions
            .object_summary(
                &o::ObjectSelector::Session {
                    id: id.clone(),
                    origin: o::ObjectOrigin::Builtin {},
                },
                fields,
                &before.hidden,
            )
            .map_err(|_| fail("stale_snapshot"))?
            .summary
    } else {
        let view = before
            .views
            .iter()
            .find(|view| view.source().id == *source_id)
            .ok_or(fail("not_found"))?;
        let record = view
            .session_metadata(id)?
            .filter(|record| frozen.query.filter.matches(record, frozen.cutoff))
            .ok_or(fail("not_found"))?;
        let source = view.state();
        if source.authorization != d::Authorization::Authorized
            || source.coverage == d::Coverage::Unavailable
        {
            return Err(fail("grant_revoked"));
        }
        super::object_summary::session_summary(&record, &source, fields)?
    };
    let after = capture(state, &frozen.query)?;
    state
        .plugins
        .domain_check_frozen(snapshot_token, &after.signature)?;
    Ok(o::ObjectSummaryRead {
        selector: selector.clone(),
        read_identity: local_agent_core::projects::hash(
            &json!([snapshot_token, before.signature, selector, fields]).to_string(),
        ),
        summary,
    })
}
