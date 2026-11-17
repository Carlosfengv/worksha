use crate::{ApiResult, AppState, error, json_value, observation::request_query};
use axum::extract::{Query, State};
use local_agent_core::model_usage::UsageQuery;
use serde_json::json;

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
        if query.source == "unified" {
            let (mut value, source) = state
                .observation
                .sql_query_now(|history, now| {
                    let mut source = request_query::prepare_unified_sources(&state, history)?;
                    if source.error.is_none()
                        && source.sources.iter().any(|collector| collector.enabled)
                        && !source
                            .catalog_revision
                            .is_some_and(|revision| history.unified_native_models_match(revision))
                    {
                        let applied = (|| {
                            let catalog =
                                state.sessions.project_catalog().map_err(|e| e.message)?;
                            history.sync_unified_native_models(&catalog)
                        })();
                        if let Err(reason) = applied {
                            source = request_query::unavailable_sources(history, reason)?;
                        }
                    }
                    let native_status = if source.error.is_some() {
                        "unavailable"
                    } else if query.agent == "unknown"
                        && source.sources.iter().any(|collector| collector.enabled)
                    {
                        "partial"
                    } else {
                        let family = state.sessions.directory().family(&query.agent);
                        source
                            .sources
                            .iter()
                            .find(|collector| Some(collector.agent_id.as_str()) == family)
                            .map(|collector| collector.status.as_str())
                            .unwrap_or("disabled")
                    };
                    let state_label =
                        if state.observation.persistence_error() || source.error.is_some() {
                            "partial"
                        } else {
                            "ready"
                        };
                    let as_of = source.as_of.clone().unwrap_or_else(|| now.to_rfc3339());
                    let mut usage = history.model_usage(query, as_of, state_label.into(), now)?;
                    usage["nativeSourceState"] = json!(native_status);
                    Ok((usage, source))
                })
                .map_err(error)?;
            source.check_consent(&state.sessions).map_err(error)?;
            value["nativeSourceError"] = json!(source.error);
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
