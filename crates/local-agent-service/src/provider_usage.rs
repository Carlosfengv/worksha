use crate::{ApiResult, AppState, error, json_value};
use axum::extract::{Query, State};
use local_agent_core::provider_usage::Query as UsageQuery;

pub async fn query(State(state): State<AppState>, Query(query): Query<UsageQuery>) -> ApiResult {
    query
        .validate_with_directory(state.sessions.directory())
        .map_err(|e| error(e.into()))?;
    crate::blocking::query(state.query_capacity.clone(), move || {
        let providers = state
            .store
            .lock()
            .map_err(|_| error("供应商存储暂不可用".into()))?
            .providers()
            .map_err(error)?;
        let mut evidence = state.gateway.usage_evidence().map_err(error)?;
        evidence.extend(state.observation.usage_evidence().map_err(error)?);
        if query.source == "gateway" {
            let value = state
                .observation
                .sql_query_now(|history, now| {
                    history.use_directory(state.sessions.directory())?;
                    history.set_evidence(&evidence)?;
                    history.provider_usage(
                        query,
                        &providers,
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
        let sources = state.sessions.sources().map_err(crate::native_error)?;
        let source_state = if !sources.iter().any(|source| source.enabled) {
            "disabled"
        } else if sources
            .iter()
            .any(|source| source.enabled && source.status != "ready")
        {
            "partial"
        } else {
            "ready"
        };
        let value = state
            .observation
            .sql_query(|h| {
                h.sync_native_provider(&catalog, &evidence)?;
                h.provider_usage(
                    query,
                    &providers,
                    catalog.as_of.clone(),
                    source_state.to_owned(),
                    chrono::Utc::now(),
                )
            })
            .map_err(error)?;
        if authorization != crate::sessions::authorizations(&state)? {
            return Err(error("原生来源已更新，请刷新供应商统计".into()));
        }
        json_value(value)
    })
    .await
}
