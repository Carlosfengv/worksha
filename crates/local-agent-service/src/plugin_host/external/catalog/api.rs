use super::{Checked, Result, a, runtime};
use crate::{ApiResult, AppState};
use axum::{Json, Router, extract::State, http::StatusCode, routing::post};
use serde_json::{Value, json};

pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/provider-catalog/status", post(status))
        .route("/api/provider-catalog/action", post(action))
        .layer(axum::extract::DefaultBodyLimit::max(65536))
}
fn response(result: Result<Value>) -> ApiResult {
    result.map(Json).map_err(|e| {
        let status = match e.code {
            "permission_denied" | "grant_revoked" | "plugin_disabled" => StatusCode::FORBIDDEN,
            "stale_snapshot" => StatusCode::CONFLICT,
            "not_found" => StatusCode::NOT_FOUND,
            "unavailable" | "invalid_record" => StatusCode::SERVICE_UNAVAILABLE,
            _ => StatusCode::BAD_REQUEST,
        };
        (
            status,
            Json(json!({"code":e.code,"message":super::super::message(e.code)})),
        )
    })
}
async fn status(State(s): State<AppState>, Json(q): Json<a::Query>) -> ApiResult {
    response(
        super::super::run_sync(move || {
            let providers = s
                .store
                .lock()
                .checked("unavailable")?
                .providers()
                .checked("unavailable")?;
            serde_json::to_value(s.plugins.catalog_state(&q, &providers)?).checked("invalid_input")
        })
        .await,
    )
}
pub(crate) async fn action(State(s): State<AppState>, Json(r): Json<a::Request>) -> ApiResult {
    let q = a::Query {
        plugin_id: r.plugin_id,
        digest: r.digest,
    };
    match r.action {
        a::Action::Read { binding_id, target } => {
            response(runtime::read(s, q, binding_id, target).await)
        }
        action => response(
            super::super::run_sync(move || {
                let providers = s
                    .store
                    .lock()
                    .checked("unavailable")?
                    .providers()
                    .checked("unavailable")?;
                let value = match action {
                    a::Action::Bind {
                        target,
                        allow_private_network,
                    } => s
                        .plugins
                        .catalog_bind(&q, target, allow_private_network, &providers)?,
                    a::Action::Unbind { binding_id } => {
                        s.plugins.catalog_unbind(&q, &binding_id, &providers)?
                    }
                    a::Action::Read { .. } => return Err(super::fail("invalid_input")),
                };
                serde_json::to_value(value).checked("invalid_input")
            })
            .await,
        ),
    }
}
