//! Internal D1 metadata endpoint. Existing IPC and source authorization are unchanged.
use crate::plugin_host::external::{self, Checked};
use crate::{ApiResult, AppState, json_value, native_error};
use axum::Json;
use axum::extract::State;
use local_agent_core::domain as d;
mod activity;
mod facts;
pub(crate) mod object_summary;
mod requests;
pub(crate) use requests::export as request_exports;

pub(crate) async fn requests(
    State(state): State<AppState>,
    Json(input): Json<d::requests::query::UnifiedRequestQuery>,
) -> ApiResult {
    crate::blocking::query(state.query_capacity.clone(), move || {
        external::api::response(
            requests::query(&state, input)
                .and_then(|v| serde_json::to_value(v).checked("invalid_record")),
        )
    })
    .await
}

pub(crate) async fn activity(
    State(state): State<AppState>,
    Json(input): Json<d::activity::query::UnifiedActivityQuery>,
) -> ApiResult {
    crate::blocking::query(state.query_capacity.clone(), move || {
        external::api::response(
            activity::query(&state, input)
                .and_then(|v| serde_json::to_value(v).checked("invalid_record")),
        )
    })
    .await
}

pub(crate) async fn project_visibility(
    State(state): State<AppState>,
    Json(input): Json<d::facts::query::ProjectVisibility>,
) -> ApiResult {
    external::api::response(
        external::run_sync(move || {
            state.plugins.full_project_visibility(&input)?;
            Ok(serde_json::json!({"updated":true}))
        })
        .await,
    )
}
mod query;
mod sources;

pub(crate) async fn fact_conversation(
    State(state): State<AppState>,
    Json(input): Json<d::UnifiedConversationRequest>,
) -> ApiResult {
    external::api::response(
        facts::conversation(state, input)
            .await
            .and_then(|v| serde_json::to_value(v).checked("invalid_record")),
    )
}

pub(crate) async fn facts(
    State(state): State<AppState>,
    Json(input): Json<d::facts::query::FactQuery>,
) -> ApiResult {
    crate::blocking::query(state.query_capacity.clone(), move || {
        external::api::response(
            facts::facts(&state, input)
                .and_then(|v| serde_json::to_value(v).checked("invalid_record")),
        )
    })
    .await
}

pub(crate) async fn authorize_source(
    State(state): State<AppState>,
    Json(input): Json<external::api::SourcePath>,
) -> ApiResult {
    external::api::response(
        external::run_sync(move || {
            state.plugins.domain_grant_source(
                &d::SourceConsentQuery {
                    plugin_id: input.plugin_id,
                    digest: input.digest,
                },
                std::path::Path::new(&input.path),
            )?;
            Ok(serde_json::json!({"updated":true}))
        })
        .await,
    )
}
pub(crate) async fn refresh_source(
    State(state): State<AppState>,
    Json(input): Json<d::SourceConsentQuery>,
) -> ApiResult {
    external::api::response(
        state
            .plugins
            .domain_refresh_source(input)
            .await
            .map(|_| serde_json::json!({"updated":true})),
    )
}

pub(crate) async fn capabilities(State(state): State<AppState>) -> ApiResult {
    crate::blocking::query(state.query_capacity.clone(), move || {
        external::api::response(
            query::capabilities(&state)
                .and_then(|v| serde_json::to_value(v).checked("invalid_record")),
        )
    })
    .await
}

pub(crate) async fn catalog(State(state): State<AppState>) -> ApiResult {
    crate::blocking::query(state.query_capacity.clone(), move || {
        json_value(state.sessions.domain_catalog().map_err(native_error)?)
    })
    .await
}
pub(crate) async fn directory(State(state): State<AppState>) -> ApiResult {
    crate::blocking::query(state.query_capacity.clone(), move || {
        external::api::response(
            query::directory(&state)
                .and_then(|v| serde_json::to_value(v).checked("invalid_record")),
        )
    })
    .await
}
pub(crate) async fn sessions(
    State(state): State<AppState>,
    Json(input): Json<d::UnifiedSessionQuery>,
) -> ApiResult {
    crate::blocking::query(state.query_capacity.clone(), move || {
        external::api::response(
            query::sessions(&state, input)
                .and_then(|v| serde_json::to_value(v).checked("invalid_record")),
        )
    })
    .await
}
pub(crate) async fn consent(
    State(state): State<AppState>,
    Json(input): Json<d::SourceConsent>,
) -> ApiResult {
    external::api::response(
        external::run_sync(move || {
            state.plugins.domain_consent(&input)?;
            Ok(serde_json::json!({"updated":true}))
        })
        .await,
    )
}
pub(crate) async fn conversation(
    State(state): State<AppState>,
    Json(input): Json<d::UnifiedConversationRequest>,
) -> ApiResult {
    external::api::response(
        query::conversation(state, input)
            .await
            .and_then(|v| serde_json::to_value(v).checked("invalid_record")),
    )
}
pub(crate) async fn prepare_consent(
    State(state): State<AppState>,
    Json(query): Json<d::SourceConsentQuery>,
) -> ApiResult {
    external::api::response(
        external::run_sync(move || {
            serde_json::to_value(state.plugins.domain_prepare_consent(&query)?)
                .checked("invalid_record")
        })
        .await,
    )
}
