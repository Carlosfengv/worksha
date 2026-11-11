use crate::{ApiResult, AppState, error, json_value};
use axum::{
    Json,
    extract::{Query, State},
    http::StatusCode,
};
use local_agent_core::{
    observation::sql::History,
    requests::{self, Context},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::HashMap;

pub(crate) struct Sources {
    pub sources: Vec<local_agent_core::sessions::SourceStatus>,
    pub as_of: Option<String>,
    pub error: Option<String>,
}
fn effective_sources(
    sessions: &local_agent_core::sessions::NativeSessions,
) -> Result<Vec<local_agent_core::sessions::SourceStatus>, String> {
    let mut sources = sessions.sources().map_err(|e| e.message)?;
    for source in &mut sources {
        source.enabled &= sessions.source_active(&source.agent_id);
    }
    Ok(sources)
}
/// The current metadata key only; unlike prepare_sources this never syncs history.
pub(crate) fn native_metadata_key(
    sessions: &local_agent_core::sessions::NativeSessions,
) -> Result<String, String> {
    let sources = effective_sources(sessions)?;
    let (revision, _) = sessions.catalog_status().map_err(|e| e.message)?;
    serde_json::to_string(&(
        revision,
        sources
            .iter()
            .map(|s| (&s.agent_id, s.enabled, s.authorization_revision))
            .collect::<Vec<_>>(),
    ))
    .map_err(|_| "来源版本不可用".into())
}
pub(crate) fn prepare_sources(state: &AppState, history: &mut History) -> Result<Sources, String> {
    history.use_directory(state.sessions.directory())?;
    let sources = effective_sources(&state.sessions)?;
    let (revision, as_of) = state.sessions.catalog_status().map_err(|e| e.message)?;
    let key = serde_json::to_string(&(
        revision,
        sources
            .iter()
            .map(|s| (&s.agent_id, s.enabled, s.authorization_revision))
            .collect::<Vec<_>>(),
    ))
    .map_err(|_| "来源版本不可用")?;
    if !history.native_matches(&key) {
        let mut catalog = state.sessions.project_catalog().map_err(|e| e.message)?;
        let mut contexts = HashMap::new();
        for s in catalog.sessions {
            if !sources
                .iter()
                .any(|source| source.enabled && source.agent_id == s.agent_id)
            {
                continue;
            }
            let context = Context {
                session_id: s.id.clone(),
                title: s.title,
                project_id: s.project_id,
                project_name: s.project_name,
            };
            contexts.insert(s.id, context.clone());
            contexts.insert(format!("{}:{}", s.agent_id, s.native_id), context);
        }
        catalog.calls.retain(|c| {
            sources
                .iter()
                .any(|s| s.enabled && s.agent_id == c.agent_id)
        });
        history.sync_native_catalog(key, &contexts, &catalog.calls)?;
    }
    history.refresh_native()?;
    Ok(Sources {
        sources,
        as_of: Some(as_of),
        error: None,
    })
}
pub(super) fn check_consent(state: &AppState, report: &Value) -> Result<(), String> {
    check_source_consent(&state.sessions, report)
}
fn check_source_consent(
    sessions: &local_agent_core::sessions::NativeSessions,
    report: &Value,
) -> Result<(), String> {
    let current = effective_sources(sessions).map_err(|_| "原生授权状态读取失败")?;
    if report["collectors"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|old| {
            old["enabled"] == true
                && !current.iter().any(|now| {
                    Some(now.agent_id.as_str()) == old["agentId"].as_str()
                        && now.enabled
                        && Some(now.authorization_revision) == old["authorizationRevision"].as_u64()
                })
        })
    {
        return Err("原生来源授权已变化，请刷新；旧快照已停止提供".into());
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    use local_agent_core::sessions::NativeSessions;
    #[test]
    fn stopping_an_enabled_source_invalidates_its_frozen_request_report() {
        let dir = tempfile::tempdir().unwrap();
        let sessions = NativeSessions::new(dir.path().join("home"), &["claude".into()]);
        let source = sessions
            .sources()
            .unwrap()
            .into_iter()
            .find(|s| s.agent_id == "claude")
            .unwrap();
        assert!(source.enabled && sessions.source_active("claude"));
        let report = json!({"collectors":[{"agentId":source.agent_id,"enabled":source.enabled,"authorizationRevision":source.authorization_revision}]});
        check_source_consent(&sessions, &report).unwrap();
        sessions.stop_source_plugin("claude").unwrap();
        assert!(
            sessions
                .sources()
                .unwrap()
                .into_iter()
                .find(|s| s.agent_id == "claude")
                .unwrap()
                .enabled,
            "saved enable intent is unchanged"
        );
        assert!(check_source_consent(&sessions, &report).is_err());
    }
    #[test]
    fn a_fresh_report_excludes_stopped_readers_but_preserves_healthy_readers_and_enable_intent() {
        let dir = tempfile::tempdir().unwrap();
        let sessions =
            NativeSessions::new(dir.path().join("home"), &["claude".into(), "codex".into()]);
        sessions.stop_source_plugin("claude").unwrap();
        let effective = effective_sources(&sessions).unwrap();
        assert!(
            !effective
                .iter()
                .find(|s| s.agent_id == "claude")
                .unwrap()
                .enabled
        );
        assert!(
            effective
                .iter()
                .find(|s| s.agent_id == "codex")
                .unwrap()
                .enabled
        );
        assert!(
            sessions
                .sources()
                .unwrap()
                .iter()
                .find(|s| s.agent_id == "claude")
                .unwrap()
                .enabled
        );
        let report = json!({"collectors":effective.iter().map(|s|json!({"agentId":s.agent_id,"enabled":s.enabled,"authorizationRevision":s.authorization_revision})).collect::<Vec<_>>()});
        check_source_consent(&sessions, &report).unwrap();
    }
}
pub(super) fn stale(message: String) -> (StatusCode, Json<Value>) {
    (
        StatusCode::CONFLICT,
        Json(json!({"code":"stale_snapshot","message":message})),
    )
}
pub async fn dashboard(
    State(state): State<AppState>,
    Query(query): Query<requests::Query>,
) -> ApiResult {
    query
        .validate_with_directory(state.sessions.directory())
        .map_err(error)?;
    crate::blocking::query(state.query_capacity.clone(), move || {
        if let Some(id) = &query.snapshot {
            return state
                .observation
                .hub
                .cached_query(|history| {
                    let now = chrono::Utc::now();
                    let report = history.frozen_report(id)?;
                    check_consent(&state, &report)?;
                    let page = history.request_page(&query, now, Value::Null, None)?;
                    check_consent(&state, &report)?;
                    Ok(page)
                })
                .map(Json)
                .map_err(stale);
        }
        let result = state
            .observation
            .hub
            .query(|history| {
                let source = prepare_sources(&state, history)?;
                let now = chrono::Utc::now();
                let report = history.source_report(&query, now, source.sources, source.as_of)?;
                check_consent(&state, &report)?;
                let message = source.error.or(state.observation.hub.error());
                let page = history.request_page(&query, now, report.clone(), message)?;
                check_consent(&state, &report)?;
                Ok(page)
            })
            .map_err(error)?;
        json_value(result)
    })
    .await
}
#[derive(Deserialize)]
pub struct DetailQuery {
    id: String,
}
pub async fn detail(State(state): State<AppState>, Query(query): Query<DetailQuery>) -> ApiResult {
    if query.id.len() > 1024 {
        return Err(error("请求 ID 无效".into()));
    }
    crate::blocking::query(state.query_capacity.clone(), move ||{
        let result=state.observation.hub.query(|history|{
            let source=prepare_sources(&state,history)?;
            let request=history.request_detail(&query.id)?;
            let route_trace = request.as_ref()
                .filter(|row| row["source"] == "gateway")
                .and_then(|row| row["id"].as_str())
                .map(|id| history.request_route_trace(id))
                .transpose()?
                .flatten();
            let current=effective_sources(&state.sessions)?;
            if source.sources.iter().any(|old|old.enabled&&!current.iter().any(|now|now.enabled&&old.agent_id==now.agent_id&&old.authorization_revision==now.authorization_revision)){
                return Err("原生来源授权已变化，请刷新".into());
            }
            let mut detail = json!({"request":request,"asOf":chrono::Utc::now().to_rfc3339(),"sourceError":source.error.or(state.observation.hub.error())});
            if let Some(trace) = route_trace {
                detail["routeTrace"] = trace;
            }
            Ok(detail)
        }).map_err(error)?;
        json_value(result)
    }).await
}
