use crate::{ApiResult, AppState, connections, error, json_value, native_error};
use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
};
use local_agent_core::settings::{SettingsError, SettingsUpdate};
mod retention;
pub(crate) use retention::inventory as retention_inventory;
pub(crate) use retention::preview as retention_preview;

fn settings_error(error: SettingsError) -> (StatusCode, Json<serde_json::Value>) {
    let status = match error.code {
        "conflict" | "operation_conflict" | "operation_store_changed" | "operation_not_scoped" => {
            StatusCode::CONFLICT
        }
        "operation_not_found" => StatusCode::NOT_FOUND,
        "unavailable" | "invalid_record" | "busy" => StatusCode::SERVICE_UNAVAILABLE,
        _ => StatusCode::BAD_REQUEST,
    };
    (status, Json(serde_json::json!(error)))
}

fn worker_error(error: crate::blocking::Error) -> (StatusCode, Json<serde_json::Value>) {
    let (code, message) = match error {
        crate::blocking::Error::Busy => ("busy", "设置读取或保存任务繁忙，请稍后重试"),
        crate::blocking::Error::Unavailable => {
            ("unavailable", "设置服务暂不可用，请保留原操作编号核查")
        }
    };
    settings_error(SettingsError { code, message })
}

pub(crate) async fn snapshot(State(state): State<AppState>) -> ApiResult {
    crate::blocking::run(move || {
        let store = state
            .store
            .lock()
            .map_err(|_| error("数据服务暂不可用".into()))?;
        let mut value = serde_json::to_value(store.snapshot().map_err(error)?)
            .map_err(|_| error("读取失败".into()))?;
        drop(store);
        let bindings = state.observation.statuses();
        let gateway_ready = bindings.iter().any(|binding| binding.enabled);
        value["sessionSources"] =
            serde_json::to_value(state.sessions.sources().map_err(native_error)?)
                .map_err(|_| error("读取失败".into()))?;
        value["observationBindings"] =
            serde_json::to_value(bindings).map_err(|_| error("读取失败".into()))?;
        value["agentConnections"] = serde_json::to_value(connections::snapshot(&state))
            .map_err(|_| error("读取连接状态失败".into()))?;
        value["gatewayStatus"] = serde_json::json!(if gateway_ready {
            "ready"
        } else {
            "not_configured"
        });
        Ok(Json(value))
    })
    .await
    .map_err(worker_error)?
}

pub(crate) async fn save_settings(
    State(state): State<AppState>,
    Json(settings): Json<SettingsUpdate>,
) -> ApiResult {
    crate::blocking::run(move || {
        let saved = state
            .store
            .lock()
            .map_err(|_| error("数据服务暂不可用".into()))?
            .save_settings(&settings)
            .map_err(settings_error)?;
        json_value(saved)
    })
    .await
    .map_err(worker_error)?
}

pub(crate) async fn settings_operation(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult {
    crate::blocking::run(move || {
        let saved = state
            .store
            .lock()
            .map_err(|_| error("数据服务暂不可用".into()))?
            .settings_operation(&id)
            .map_err(settings_error)?;
        json_value(saved)
    })
    .await
    .map_err(worker_error)?
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SettingsScope {
    store_id: String,
}

pub(crate) async fn scoped_operation(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(scope): Query<SettingsScope>,
) -> ApiResult {
    crate::blocking::run(move || {
        let store = state
            .store
            .lock()
            .map_err(|_| worker_error(crate::blocking::Error::Unavailable))?;
        json_value(
            store
                .scoped_settings_operation(&id, &scope.store_id)
                .map_err(settings_error)?,
        )
    })
    .await
    .map_err(worker_error)?
}
