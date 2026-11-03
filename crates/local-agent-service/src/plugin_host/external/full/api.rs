use super::super::{Checked, api::response, run_sync};
use crate::{ApiResult, AppState};
use axum::{
    Json, Router,
    extract::State,
    routing::{get, post},
};
use local_agent_core::plugin::full as f;

pub(in crate::plugin_host::external) fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/plugin-sources", get(list))
        .route("/api/plugin-sources/inspect", post(inspect))
        .route("/api/plugin-sources/install", post(install))
        .route("/api/plugin-sources/state", post(change))
        .route("/api/plugin-sources/source", post(source))
        .route("/api/plugin-sources/refresh", post(refresh))
}
async fn list(State(state): State<AppState>) -> ApiResult {
    response(
        run_sync(move || {
            Ok(serde_json::json!({"apiVersion":f::API_VERSION,"items":state.plugins.full_list()?}))
        })
        .await,
    )
}
async fn inspect(
    State(state): State<AppState>,
    Json(input): Json<super::super::api::InspectPath>,
) -> ApiResult {
    response(
        run_sync(move || {
            serde_json::to_value(
                state
                    .plugins
                    .full_inspect(std::path::Path::new(&input.path))?,
            )
            .checked("invalid_record")
        })
        .await,
    )
}
async fn install(State(state): State<AppState>, Json(input): Json<f::Install>) -> ApiResult {
    response(
        run_sync(move || {
            serde_json::to_value(state.plugins.full_install(&input)?).checked("invalid_record")
        })
        .await,
    )
}
async fn change(State(state): State<AppState>, Json(input): Json<f::StateChange>) -> ApiResult {
    response(
        state
            .plugins
            .full_change(input)
            .await
            .and_then(|v| serde_json::to_value(v).checked("invalid_record")),
    )
}
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SourcePath {
    package: f::PackageQuery,
    path: String,
}
async fn source(State(state): State<AppState>, Json(input): Json<SourcePath>) -> ApiResult {
    response(
        run_sync(move || {
            serde_json::to_value(
                state
                    .plugins
                    .full_grant_source(&input.package, std::path::Path::new(&input.path))?,
            )
            .checked("invalid_record")
        })
        .await,
    )
}
async fn refresh(State(state): State<AppState>, Json(input): Json<f::PackageQuery>) -> ApiResult {
    response(
        state
            .plugins
            .full_refresh(input)
            .await
            .and_then(|v| serde_json::to_value(v).checked("invalid_record")),
    )
}
