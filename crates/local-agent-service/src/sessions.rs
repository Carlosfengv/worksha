use crate::{ApiResult, AppState, error, json_value, native_error};
use axum::{
    Json,
    extract::{Path, Query, State},
};
use local_agent_core::sessions::{CallQuery, SessionQuery};
use serde::Deserialize;
pub(crate) mod work;

// Data generations can advance while a coherent read is in progress. Consent
// revisions remain a strict response boundary, independent of scan publication.
pub(crate) type Authorizations = Vec<(String, bool, u64)>;
pub(crate) fn authorizations(
    state: &AppState,
) -> Result<Authorizations, (axum::http::StatusCode, Json<serde_json::Value>)> {
    Ok(state
        .sessions
        .sources()
        .map_err(native_error)?
        .into_iter()
        .map(|s| (s.agent_id, s.enabled, s.authorization_revision))
        .collect())
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SourceChange {
    agent_id: String,
    enabled: bool,
}
pub(crate) async fn save_source(
    State(state): State<AppState>,
    Json(change): Json<SourceChange>,
) -> ApiResult {
    crate::blocking::management(move || {
        #[cfg(test)]
        state.session_work.source_started.notify_one();
        state.session_work.ensure_open()?;
        state.store.lock().map_err(|_| error("数据服务暂不可用".into()))?
            .save_native_source(&change.agent_id, change.enabled).map_err(error)?;
        state.sessions.set_enabled(&change.agent_id, change.enabled).map_err(native_error)?;
        // This belongs to the actual committed worker, not the cancelled waiter.
        work::after_source_change(&state, (!change.enabled).then_some(change.agent_id.clone()))?;
        Ok(Json(serde_json::json!({"status":if change.enabled {"pending"} else {"disabled"}, "affectedScope":change.agent_id})))
    }).await
}
pub(crate) async fn refresh_sessions(State(state): State<AppState>) -> ApiResult {
    work::refresh(&state)?;
    Ok(Json(serde_json::json!({"status":"scanning"})))
}
pub(crate) async fn query_sessions(
    State(state): State<AppState>,
    Query(query): Query<SessionQuery>,
) -> ApiResult {
    crate::blocking::query(state.query_capacity.clone(), move || {
        let authorization = authorizations(&state)?;
        if query.include_gateway == Some(true) {
            let (gateway, partial) = state
                .observation
                .sql_query(|history| history.gateway_sessions())
                .unwrap_or_else(|_| (Vec::new(), true));
            let mut page = state
                .sessions
                .query_with_gateway(
                    query,
                    gateway,
                    partial || state.observation.history_query_partial(),
                )
                .map_err(native_error)?;
            let hidden = crate::projects::hidden(&state)?;
            for session in &mut page.items {
                local_agent_core::projects::visibility::mark_session(session, &hidden);
            }
            if authorization != authorizations(&state)?
                || hidden != crate::projects::hidden(&state)?
            {
                return Err(native_error(local_agent_core::sessions::NativeError {
                    code: "stale_snapshot",
                    message: "会话来源已更新，请刷新后重试".into(),
                }));
            }
            return json_value(page);
        }
        let (mut page, catalog) = state
            .sessions
            .query_with_catalog(query)
            .map_err(native_error)?;
        let hidden = crate::projects::hidden(&state)?;
        for session in &mut page.items {
            local_agent_core::projects::visibility::mark_session(session, &hidden);
        }
        crate::associated_costs::with_index(&state, &catalog, |index| {
            let costs = index
                .summary(
                    &page.selected_session_ids,
                    page.applied_filter.agent.as_deref(),
                )
                .map_err(error)?;
            let mut value = json_value(page)?;
            value.0["associatedCosts"] = costs;
            if authorization != authorizations(&state)? {
                return Err(native_error(local_agent_core::sessions::NativeError {
                    code: "stale_snapshot",
                    message: "会话来源已更新，请刷新后重试".into(),
                }));
            }
            Ok(value)
        })
    })
    .await
}
pub(crate) async fn query_calls(
    State(state): State<AppState>,
    Query(query): Query<CallQuery>,
) -> ApiResult {
    crate::blocking::query(state.query_capacity.clone(), move || {
        json_value(state.sessions.calls(query).map_err(native_error)?)
    })
    .await
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct MemoryQuery {
    file_id: Option<String>,
}
pub(crate) async fn memory(
    State(state): State<AppState>,
    Path(agent): Path<String>,
    Query(query): Query<MemoryQuery>,
) -> ApiResult {
    crate::blocking::query(state.query_capacity.clone(), move || {
        let hidden = crate::projects::hidden(&state)?;
        let paths = local_agent_core::projects::visibility::hidden_paths(
            &state.sessions.project_catalog().map_err(native_error)?,
            &hidden,
        );
        let snapshot = state
            .sessions
            .memory(&agent, query.file_id.as_deref(), &paths)
            .map_err(native_error)?;
        if hidden != crate::projects::hidden(&state)? {
            return Err(native_error(local_agent_core::sessions::NativeError {
                code: "stale_snapshot",
                message: "项目显示设置已变化，请刷新记忆列表".into(),
            }));
        }
        json_value(snapshot)
    })
    .await
}
pub(crate) async fn query_mcp(
    State(state): State<AppState>,
    Query(query): Query<local_agent_core::sessions::McpQuery>,
) -> ApiResult {
    crate::blocking::query(state.query_capacity.clone(), move || {
        let hidden = crate::projects::hidden(&state)?;
        let paths = local_agent_core::projects::visibility::hidden_paths(
            &state.sessions.project_catalog().map_err(native_error)?,
            &hidden,
        );
        let result = state
            .sessions
            .domain_mcp_with_hidden_projects(query, &paths)
            .map_err(native_error)?;
        if hidden != crate::projects::hidden(&state)? {
            return Err(native_error(local_agent_core::sessions::NativeError {
                code: "stale_snapshot",
                message: "项目显示设置已更新，请刷新后继续".into(),
            }));
        }
        json_value(result)
    })
    .await
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ConversationQuery {
    cursor: Option<String>,
    anchor: Option<String>,
}
pub(crate) async fn conversation(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<ConversationQuery>,
) -> ApiResult {
    crate::blocking::query(state.query_capacity.clone(), move || {
        let revision = state.sessions.revision().map_err(native_error)?;
        let mut conversation = state
            .sessions
            .conversation(&id, query.cursor.as_deref(), query.anchor.as_deref())
            .map_err(native_error)?;
        local_agent_core::projects::visibility::mark_session(
            &mut conversation.session,
            &crate::projects::hidden(&state)?,
        );
        let catalog = state.sessions.project_catalog().map_err(native_error)?;
        crate::associated_costs::with_index(&state, &catalog, |index| {
            let costs = index
                .summary(std::slice::from_ref(&conversation.session.id), None)
                .map_err(error)?;
            let mut value = json_value(conversation)?;
            value.0["associatedCosts"] = costs;
            if revision != state.sessions.revision().map_err(native_error)? {
                return Err(native_error(local_agent_core::sessions::NativeError {
                    code: "stale_snapshot",
                    message: "会话来源已更新，请刷新后重试".into(),
                }));
            }
            Ok(value)
        })
    })
    .await
}
