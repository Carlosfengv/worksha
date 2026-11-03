//! Unified queries consume fixed publications only. External IDs and durations
//! remain source claims; no guest runs, refreshes or body reads occur here.
use super::{query, sources::SourceView};
use crate::{
    AppState,
    plugin_host::external::{Result, fail},
};
use local_agent_core::{
    domain::{
        self as d,
        facts::{FactCandidate, FactKind, FactTally},
        requests::query as q,
    },
    observation::sql::unified_requests::Publication,
};
use std::collections::HashMap;
pub(crate) mod export;
#[cfg(test)]
mod tests;

fn failure(e: d::Failure) -> crate::plugin_host::external::Error {
    fail(match e.code {
        d::ErrorCode::InvalidInput => "invalid_input",
        d::ErrorCode::StaleSnapshot | d::ErrorCode::GrantRevoked => "stale_snapshot",
        d::ErrorCode::ResourceLimit => "resource_limit",
        _ => "unavailable",
    })
}
pub(super) fn query(
    state: &AppState,
    input: q::UnifiedRequestQuery,
) -> Result<q::UnifiedRequestPage> {
    query_with(state, input, &|| {})
}
fn capture(state: &AppState, input: &q::UnifiedRequestQuery) -> Result<query::Capture> {
    query::capture(
        state,
        &d::UnifiedSessionQuery {
            scope: input.scope.clone(),
            filter: d::SessionFilter {
                product_id: input.filter.product_id.clone(),
                ..Default::default()
            },
            snapshot_token: None,
            cursor: None,
        },
    )
}
fn query_with(
    state: &AppState,
    input: q::UnifiedRequestQuery,
    before_finish: &dyn Fn(),
) -> Result<q::UnifiedRequestPage> {
    input.validate().map_err(failure)?;
    let before = capture(state, &input)?;
    let signature = before.source_signature.clone();
    let frozen = input.snapshot_token.is_some();
    let run = |history: &mut local_agent_core::observation::sql::History| {
        let result = (|| {
            let p = if let Some(token) = &input.snapshot_token {
                history
                    .check_unified_requests(token, &signature, &state.sessions)
                    .map_err(failure)?;
                None
            } else {
                if !before.native_status.is_empty() {
                    crate::observation::request_query::prepare_sources(state, history)
                        .map_err(|_| fail("unavailable"))?;
                }
                let mut p = publication(&before, &input, chrono::Utc::now())?;
                p.history_partial = state.observation.history_query_partial();
                // Gateway history is independently readable, but any native
                // enrichment captured with it belongs to that reader's epoch.
                if before
                    .native_status
                    .iter()
                    .any(|s| s.coverage != d::Coverage::Unavailable)
                {
                    p.authorizations = state
                        .sessions
                        .domain_activity_authorizations()
                        .map_err(|_| fail("unavailable"))?
                        .into_iter()
                        .filter(|a| {
                            before.native_status.iter().any(|s| {
                                s.source_id == a.source_id()
                                    && s.coverage != d::Coverage::Unavailable
                            })
                        })
                        .collect();
                    if p.authorizations.len()
                        != before
                            .native_status
                            .iter()
                            .filter(|s| s.coverage != d::Coverage::Unavailable)
                            .count()
                    {
                        return Err(fail("stale_snapshot"));
                    }
                    for a in &p.authorizations {
                        a.ensure_current(&state.sessions)
                            .map_err(|_| fail("stale_snapshot"))?;
                    }
                }
                Some(p)
            };
            let page = history
                .unified_request_page(&input, chrono::Utc::now(), &signature, p)
                .map_err(failure)?;
            before_finish();
            let after = capture(state, &input)?;
            history
                .check_unified_requests(
                    &page.snapshot_token,
                    &after.source_signature,
                    &state.sessions,
                )
                .map_err(failure)?;
            Ok(page)
        })();
        Ok(result)
    };
    if frozen {
        state.observation.cached_sql_query(run)
    } else {
        state.observation.sql_query(run)
    }
    .map_err(|_| fail("unavailable"))?
}
pub(super) fn object_summary(
    state: &AppState,
    selector: &d::object_summary::ObjectSelector,
    fields: &[d::object_summary::SummaryField],
) -> Result<d::object_summary::ObjectSummaryRead> {
    use d::object_summary as o;
    let o::ObjectSelector::Request {
        id,
        unit,
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
    let input = state
        .observation
        .cached_sql_query(|history| Ok(history.object_request_query(snapshot_token)))
        .map_err(|_| fail("stale_snapshot"))?
        .map_err(failure)?;
    if input.scope != *scope || input.unit != *unit {
        return Err(fail("permission_denied"));
    }
    let before = capture(state, &input)?;
    let record = state
        .observation
        .cached_sql_query(|history| {
            Ok(history.object_request_record(
                snapshot_token,
                &before.source_signature,
                source_id,
                id,
                &state.sessions,
            ))
        })
        .map_err(|_| fail("stale_snapshot"))?
        .map_err(failure)?;
    let source = before
        .native_status
        .iter()
        .chain(&before.unavailable)
        .find(|s| s.source_id == *source_id)
        .cloned()
        .or_else(|| {
            before
                .views
                .iter()
                .find(|v| v.source().id == *source_id)
                .map(SourceView::state)
        })
        .ok_or(fail("not_found"))?;
    let summary = super::object_summary::request_summary(&record, &source, fields)?;
    let after = capture(state, &input)?;
    state
        .observation
        .cached_sql_query(|history| {
            Ok(history.check_unified_requests(
                snapshot_token,
                &after.source_signature,
                &state.sessions,
            ))
        })
        .map_err(|_| fail("stale_snapshot"))?
        .map_err(failure)?;
    Ok(o::ObjectSummaryRead {
        selector: selector.clone(),
        read_identity: local_agent_core::projects::hash(
            &serde_json::json!([
                snapshot_token,
                before.source_signature,
                selector,
                fields,
                record
            ])
            .to_string(),
        ),
        summary,
    })
}
fn publication(
    before: &query::Capture,
    input: &q::UnifiedRequestQuery,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<Publication> {
    let mut rows = vec![];
    let mut states = before.unavailable.clone();
    let mut summaries = vec![];
    let mut bytes = 0usize;
    for source in &before.unavailable {
        summaries.push(unknown(source, input.unit));
    }
    for view in &before.views {
        let mut status = view.state();
        let snapshot = match view {
            SourceView::Full(v) if input.unit == d::requests::RequestUnit::ModelResponse => v
                .snapshot()
                .filter(|s| s.available_kinds.contains(&FactKind::Response)),
            _ => None,
        };
        if let Some(snapshot) = snapshot {
            let selection = d::facts::query::FactQuery {
                scope: input.scope.clone(),
                kind: FactKind::Response,
                session_id: input.filter.session_id.clone(),
                project_id: input.filter.project_id.clone(),
                response_id: None,
                snapshot_token: None,
                cursor: None,
            };
            let graph = snapshot.filtered_graph(&selection, &before.hidden);
            let sessions: HashMap<_, _> = graph
                .iter()
                .filter_map(|f| match &f.value {
                    FactCandidate::Session(s) => Some((f.id.as_str(), s)),
                    _ => None,
                })
                .collect();
            let projects: HashMap<_, _> = graph
                .iter()
                .filter_map(|f| match &f.value {
                    FactCandidate::Project(p) => Some((f.id.as_str(), p)),
                    _ => None,
                })
                .collect();
            let mut tally = FactTally::new(
                &status.source_id,
                &[FactKind::Response],
                status.coverage.clone(),
            )
            .map_err(failure)?;
            let mut missing = 0u64;
            let mut unknown_time = 0u64;
            for fact in &graph {
                let FactCandidate::Response(response) = &fact.value else {
                    continue;
                };
                let session = sessions.get(response.session_ref.as_str());
                let project = session.and_then(|s| s.project_ref.as_ref());
                let row = q::UnifiedRequestItem {
                    evidence_detail: None,
                    record: q::RequestQueryRecord {
                        id: fact.id.clone(),
                        source_id: status.source_id.clone(),
                        unit: input.unit,
                        product_id: response.product_id.clone(),
                        session_ref: Some(response.session_ref.clone()),
                        session_title: session.map(|s| s.title.clone()),
                        project_ref: project.cloned(),
                        project_name: project
                            .and_then(|id| projects.get(id.as_str()))
                            .map(|p| p.name.clone()),
                        occurred_at: response.occurred_at.clone(),
                        ended_at: None,
                        model: response.model.clone(),
                        provider_key: view.provider().provider_key.clone(),
                        provider_name: view.provider().plugin_id.clone(),
                        outcome: match response.outcome {
                            d::facts::Outcome::Success => "success",
                            d::facts::Outcome::Failure => "failed",
                            d::facts::Outcome::Unknown => "unknown",
                        }
                        .into(),
                        usage: response.usage.clone(),
                        usage_missing: u64::from(response.usage.total_tokens.is_none()).into(),
                        attempts: None,
                        duration_ms: None,
                        ttft_ms: None,
                        throughput: None,
                    },
                    evidence: q::RequestEvidence::SourceReported,
                    host_request_id: None,
                    reported_response_id: response.reported_response_id.clone(),
                    reported_duration_ms: response.duration_ms.clone(),
                };
                if matches(input, &row, now, false) && row.record.occurred_at.is_none() {
                    unknown_time += 1;
                }
                if matches(input, &row, now, true) {
                    tally.observe(&fact.value).map_err(failure)?;
                    missing += u64::from(response.usage.total_tokens.is_none());
                    bytes = bytes
                        .checked_add(
                            serde_json::to_vec(&row)
                                .map_err(|_| fail("invalid_record"))?
                                .len(),
                        )
                        .ok_or(fail("resource_limit"))?;
                    if rows.len() >= 8192 || bytes > 16 * 1024 * 1024 {
                        return Err(fail("resource_limit"));
                    }
                    rows.push(row);
                }
            }
            let totals = tally.finish();
            summaries.push(q::RequestSourceSummary {
                source_id: status.source_id.clone(),
                unit: input.unit,
                evidence: q::RequestEvidence::SourceReported,
                coverage: status.coverage.clone(),
                records: totals.responses.records,
                usage: totals.responses.usage,
                usage_missing: Some(missing.into()),
                attempts: None,
                unknown_time: Some(unknown_time.into()),
                evidence_summary: None,
            });
        } else {
            status.coverage = d::Coverage::Unavailable;
            if status.problem.is_none() {
                status.problem = Some("unsupported".into());
            }
            summaries.push(unknown(&status, input.unit));
        }
        states.push(status);
    }
    states.extend(before.native_status.iter().cloned().map(|mut s| {
        if input.unit == d::requests::RequestUnit::LogicalRequest {
            s.authorization = d::Authorization::Authorized;
            s.coverage = d::Coverage::Complete;
            s.freshness = d::Freshness::Current;
            s.problem = None;
        }
        s
    }));
    Ok(Publication {
        signature: before.source_signature.clone(),
        states,
        native: before.native_status.clone(),
        authorizations: vec![],
        hidden: before.hidden.clone(),
        external: rows,
        summaries,
        history_partial: false,
    })
}
fn unknown(s: &d::SourceState, unit: d::requests::RequestUnit) -> q::RequestSourceSummary {
    q::RequestSourceSummary {
        source_id: s.source_id.clone(),
        unit,
        evidence: q::RequestEvidence::SourceReported,
        coverage: d::Coverage::Unavailable,
        records: None,
        usage: (&local_agent_core::sessions::Usage::default()).into(),
        usage_missing: None,
        attempts: None,
        unknown_time: None,
        evidence_summary: None,
    }
}
fn matches(
    input: &q::UnifiedRequestQuery,
    row: &q::UnifiedRequestItem,
    now: chrono::DateTime<chrono::Utc>,
    dates: bool,
) -> bool {
    let f = &input.filter;
    let r = &row.record;
    for (filter, value) in [
        (&f.product_id, r.product_id.as_ref()),
        (&f.session_id, r.session_ref.as_ref()),
        (&f.project_id, r.project_ref.as_ref()),
        (&f.provider_key, Some(&r.provider_key)),
        (&f.model, r.model.as_ref()),
        (&f.outcome, Some(&r.outcome)),
    ] {
        if filter
            .as_ref()
            .is_some_and(|expected| Some(expected) != value)
        {
            return false;
        }
    }
    if let Some(search) = &f.search {
        let text = [
            &r.id,
            &r.source_id,
            r.session_title.as_deref().unwrap_or(""),
            r.project_name.as_deref().unwrap_or(""),
            r.model.as_deref().unwrap_or(""),
            &r.provider_name,
        ]
        .join(" ")
        .to_lowercase();
        if !text.contains(&search.to_lowercase()) {
            return false;
        }
    }
    let at = r
        .occurred_at
        .as_ref()
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok());
    if at.is_some_and(|at| at >= now) {
        return false;
    }
    if dates {
        if f.start.as_ref().is_some_and(|s| {
            at.is_none_or(|at| at < chrono::DateTime::parse_from_rfc3339(s).unwrap())
        }) {
            return false;
        }
        if f.end.as_ref().is_some_and(|s| {
            at.is_none_or(|at| at >= chrono::DateTime::parse_from_rfc3339(s).unwrap())
        }) {
            return false;
        }
    }
    true
}
