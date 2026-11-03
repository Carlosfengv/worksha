use crate::{ApiResult, AppState, error, json_value};
use axum::extract::{Query, State};
use local_agent_core::model_usage::UsageQuery;

pub async fn query(State(state): State<AppState>, Query(query): Query<UsageQuery>) -> ApiResult {
    query
        .validate_with_directory(state.sessions.directory())
        .map_err(|message| error(message.into()))?;
    crate::blocking::query(state.query_capacity.clone(), move || {
        if query.source == "gateway" {
            let value = state
                .observation
                .sql_query_now(|history, now| {
                    history.use_directory(state.sessions.directory())?;
                    history.model_usage(
                        query,
                        now.to_rfc3339(),
                        if state.observation.persistence_error() {
                            "partial"
                        } else {
                            "ready"
                        }
                        .into(),
                        now,
                    )
                })
                .map_err(error)?;
            return json_value(value);
        }
        let authorization = crate::sessions::authorizations(&state)?;
        let catalog = state
            .sessions
            .project_catalog()
            .map_err(crate::native_error)?;
        let (as_of, source_state) = state
            .sessions
            .usage_status(&query.agent)
            .map_err(crate::native_error)?;
        let value = state
            .observation
            .sql_query(|h| {
                h.sync_native_models(&catalog, &query.agent)?;
                h.model_usage(query, as_of, source_state, chrono::Utc::now())
            })
            .map_err(error)?;
        if authorization != crate::sessions::authorizations(&state)? {
            return Err(error("原生来源已更新，请刷新模型用量".into()));
        }
        json_value(value)
    })
    .await
}
