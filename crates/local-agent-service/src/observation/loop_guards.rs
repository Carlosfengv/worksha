use crate::{ApiResult, AppState, json_value, providers};
use axum::{Json, extract::State, http::StatusCode};
use local_agent_core::{providers::ConfigError, routing::loop_guard::Override};
use serde_json::json;

pub(crate) async fn alerts(State(state): State<AppState>) -> ApiResult {
    let readers = state.store_readers.clone();
    let value = crate::blocking::run(move || {
        readers.read(
            || ConfigError::new("unavailable", "本机熔断状态暂不可读取"),
            |store| Ok(json!({"storeId":store.database_id()?,"alerts":store.loop_alerts()?})),
        )
    })
    .await
    .map_err(|error| providers::error(crate::blocking::config_error(error)))?
    .map_err(providers::error)?;
    Ok(Json(value))
}

pub(crate) async fn release(
    State(state): State<AppState>,
    Json(input): Json<Override>,
) -> ApiResult {
    let receipt = crate::blocking::store(state.store, move |store| store.loop_override(&input))
        .await
        .map_err(|error| {
            if error.code == "local_agent_loop_dedup_capacity" {
                (StatusCode::CONFLICT, Json(json!(error)))
            } else {
                providers::error(error)
            }
        })?;
    json_value(receipt)
}
