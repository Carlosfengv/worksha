//! Bounded views over published native, gateway and source-reported activity.
//! No source refresh, guest execution, body read or implicit native consent.
use super::{query, sources::SourceView};
use crate::{
    AppState,
    plugin_host::external::{Result, fail},
};
use local_agent_core::domain::{
    self as d,
    activity::query as a,
    facts::{FactCandidate, FactKind},
};
use std::collections::HashMap;
mod metrics;
#[cfg(test)]
mod tests;

pub(super) fn query(
    state: &AppState,
    input: a::UnifiedActivityQuery,
) -> Result<a::UnifiedActivityPage> {
    query_with(state, input, &|| {})
}
fn query_with(
    state: &AppState,
    input: a::UnifiedActivityQuery,
    before_finish: &dyn Fn(),
) -> Result<a::UnifiedActivityPage> {
    input.validate().map_err(|_| fail("invalid_input"))?;
    let common = d::UnifiedSessionQuery {
        scope: input.scope.clone(),
        filter: d::SessionFilter {
            product_id: input.product_id.clone(),
            ..Default::default()
        },
        snapshot_token: None,
        cursor: None,
    };
    let before = query::capture(state, &common)?;
    let now = chrono::Utc::now();
    let now_ms = now.timestamp_millis();
    let collecting = state.observation.native_activity_collecting();
    let native = state.observation.published_native_activity();
    let mut window = a::ActivityWindow::default();
    let mut states = Vec::new();
    let mut used_native = Vec::new();
    let native_catalog = state
        .sessions
        .domain_catalog()
        .map_err(|_| fail("unavailable"))?;
    for source in &before.native_status {
        let mut status = source.clone();
        let publication = native.as_ref().ok();
        let authority = publication.and_then(|p| {
            p.authorizations
                .iter()
                .find(|s| s.source_id() == source.source_id)
        });
        let usable = collecting
            && source.authorization == d::Authorization::Authorized
            && authority.is_some_and(|s| s.ensure_current(&state.sessions).is_ok());
        let observed = publication
            .and_then(|p| p.checked_at.as_ref())
            .filter(|t| d::timestamp(t).is_some());
        let fresh = observed
            .and_then(|t| d::timestamp(t))
            .is_some_and(|at| (0..=15000).contains(&now_ms.saturating_sub(at)));
        if !usable || observed.is_none() {
            status.coverage = d::Coverage::Unavailable;
            status.freshness = d::Freshness::Unread;
            status.problem = Some(
                if source.authorization != d::Authorization::Authorized {
                    "permission_denied"
                } else if !collecting {
                    "collector_stopped"
                } else {
                    "needs_refresh"
                }
                .into(),
            );
        } else if let (Some(p), Some(authority), Some(observed)) =
            (publication, authority, observed)
        {
            // This is a bounded sampler publication, not complete historical coverage.
            status.coverage = d::Coverage::Partial;
            status.freshness = if fresh {
                d::Freshness::Current
            } else {
                d::Freshness::Stale
            };
            status.problem = Some(
                if publication.is_some_and(|p| p.error.is_some()) {
                    "partial_source"
                } else if !fresh {
                    "sample_expired"
                } else {
                    "bounded_observation"
                }
                .into(),
            );
            used_native.push(authority);
            for row in p.rows.iter().filter(|r| r.agent_id == source.source_id) {
                let running = [
                    "processing",
                    "thinking",
                    "tool_requested",
                    "tool_returned",
                    "streaming",
                ]
                .contains(&row.phase.as_str());
                let current = fresh
                    && !row.stale
                    && running
                    && d::timestamp(&row.event_at)
                        .is_some_and(|at| (0..=30000).contains(&now_ms.saturating_sub(at)));
                let record = a::ActivityRecord {
                    id: row.id.clone(),
                    kind: a::ActivityKind::NativeMessage,
                    source_id: row.agent_id.clone(),
                    provider_key: format!("builtin:domain.activity-source@1:{}", row.agent_id),
                    source_revision: p.revision.to_string(),
                    observed_at: observed.clone(),
                    occurred_at: Some(row.event_at.clone()),
                    session_id: Some(row.session_id.clone()),
                    message_id: row.message_id().map(Into::into),
                    product_ids: row.agent_product_id.iter().cloned().collect(),
                    unknown_client: row.agent_product_id.is_none(),
                    title: row.title.clone(),
                    model: row.model.clone(),
                    phase: row.phase.clone(),
                    state: if current {
                        a::ActivityState::Current
                    } else if running {
                        a::ActivityState::Expired
                    } else {
                        a::ActivityState::Historical
                    },
                    tool_names: row.tool_names.clone(),
                    logical_request_id: None,
                    historical_duration_ms: None,
                    measurements: Some(metrics::native(row, now_ms, current)),
                };
                if input.matches(&record, now_ms) {
                    window.observe(record);
                }
            }
        }
        states.push(a::ActivitySourceState {
            kind: a::ActivityKind::NativeMessage,
            state: status,
        });
    }
    if input.include_gateway {
        let gateway = state.observation.published_gateway_activity();
        for source in &before.native_status {
            states.push(a::ActivitySourceState {
                kind: a::ActivityKind::GatewayAttempt,
                state: d::SourceState {
                    source_id: source.source_id.clone(),
                    authorization: d::Authorization::Authorized,
                    freshness: if gateway.is_ok() {
                        d::Freshness::Current
                    } else {
                        d::Freshness::Failed
                    },
                    coverage: if gateway.is_ok() {
                        d::Coverage::Partial
                    } else {
                        d::Coverage::Unavailable
                    },
                    problem: Some(
                        if gateway.is_ok() {
                            "bounded_observation"
                        } else {
                            "unavailable"
                        }
                        .into(),
                    ),
                },
            });
        }
        for row in gateway.as_ref().into_iter().flatten() {
            let Some(source) = native_catalog
                .sources
                .iter()
                .find(|s| s.id == row.request.agent_id && input.scope.source_ids.contains(&s.id))
            else {
                continue;
            };
            let current = row.request.status == "in_progress" && !row.stale;
            let record = a::ActivityRecord {
                id: row.request.id.clone(),
                kind: a::ActivityKind::GatewayAttempt,
                source_id: source.id.clone(),
                provider_key: "builtin:domain.activity-source@1:gateway".into(),
                source_revision: row.request.revision.to_string(),
                observed_at: now.to_rfc3339(),
                occurred_at: Some(row.request.started_at.clone()),
                session_id: row.request.session_id.clone(),
                message_id: row.request.activity_message.as_ref().map(|m| m.id.clone()),
                product_ids: row.request.agent_product_id.iter().cloned().collect(),
                unknown_client: row.request.agent_product_id.is_none(),
                title: "网关请求".into(),
                model: row.request.model.clone(),
                phase: row.phase.clone(),
                state: if current {
                    a::ActivityState::Current
                } else if row.request.status == "in_progress" {
                    a::ActivityState::Expired
                } else {
                    a::ActivityState::Historical
                },
                tool_names: row.tool_names.clone(),
                logical_request_id: Some(local_agent_core::requests::logical_request_key(
                    &row.request,
                )),
                historical_duration_ms: None,
                measurements: Some(metrics::gateway(row, now_ms, current)),
            };
            if input.matches(&record, now_ms) {
                window.observe(record);
            }
        }
    }
    for view in &before.views {
        let mut status = view.state();
        let snapshot = match view {
            SourceView::Full(v) if input.include_historical => v.snapshot(),
            _ => None,
        };
        if let Some(snapshot) = snapshot.filter(|s| s.available_kinds.contains(&FactKind::Activity))
        {
            let selection = d::facts::query::FactQuery {
                scope: input.scope.clone(),
                kind: FactKind::Activity,
                session_id: input.session_id.clone(),
                project_id: None,
                response_id: None,
                snapshot_token: None,
                cursor: None,
            };
            let graph = snapshot.filtered_graph(&selection, &before.hidden);
            let sessions: HashMap<_, _> = graph
                .iter()
                .filter_map(|r| match &r.value {
                    FactCandidate::Session(s) => Some((r.id.as_str(), s)),
                    _ => None,
                })
                .collect();
            for fact in &graph {
                let FactCandidate::Activity(value) = &fact.value else {
                    continue;
                };
                let session = sessions.get(value.session_ref.as_str());
                let record = a::ActivityRecord {
                    id: fact.id.clone(),
                    kind: a::ActivityKind::SourceEvent,
                    source_id: fact.identity.source_id.clone(),
                    provider_key: fact.identity.provider_key.clone(),
                    source_revision: fact.source_revision.clone(),
                    observed_at: fact.identity.observed_at.clone(),
                    occurred_at: value.occurred_at.clone(),
                    session_id: Some(value.session_ref.clone()),
                    message_id: None,
                    product_ids: session.map_or_else(Vec::new, |s| s.product_ids.clone()),
                    unknown_client: session.is_none_or(|s| s.unknown_client),
                    title: session.map_or_else(|| "来源历史活动".into(), |s| s.title.clone()),
                    model: None,
                    phase: serde_json::to_value(&value.phase)
                        .ok()
                        .and_then(|v| v.as_str().map(Into::into))
                        .unwrap_or_else(|| "unknown".into()),
                    state: a::ActivityState::Historical,
                    tool_names: vec![],
                    logical_request_id: None,
                    historical_duration_ms: value.duration_ms.clone(),
                    measurements: None,
                };
                if input.matches(&record, now_ms) {
                    if record.occurred_at.is_none() {
                        status.coverage = d::Coverage::Partial;
                        status
                            .problem
                            .get_or_insert_with(|| "unknown_event_time".into());
                    }
                    window.observe(record);
                }
            }
        } else {
            status.coverage = d::Coverage::Unavailable;
            status.problem = Some(
                if !input.include_historical {
                    "history_not_selected"
                } else if matches!(view, SourceView::Legacy(_)) {
                    "unsupported"
                } else {
                    status.problem.as_deref().unwrap_or("unsupported")
                }
                .into(),
            );
        }
        states.push(a::ActivitySourceState {
            kind: a::ActivityKind::SourceEvent,
            state: status,
        });
    }
    states.extend(
        before
            .unavailable
            .iter()
            .cloned()
            .map(|state| a::ActivitySourceState {
                kind: a::ActivityKind::SourceEvent,
                state,
            }),
    );
    before_finish();
    let after = query::capture(state, &common)?;
    if before.source_signature != after.source_signature
        || collecting != state.observation.native_activity_collecting()
    {
        return Err(fail("stale_snapshot"));
    }
    for scope in used_native {
        if scope.ensure_current(&state.sessions).is_err() {
            return Err(fail("stale_snapshot"));
        }
    }
    let (items, summaries, limited) = window.finish(&states);
    Ok(a::UnifiedActivityPage {
        items,
        as_of: now.to_rfc3339(),
        source_states: states,
        source_summaries: summaries,
        applied_query: input,
        limited,
        overlap_unknown: true,
    })
}
