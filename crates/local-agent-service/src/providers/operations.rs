//! Scoped durable results. Re-reading or replaying an ID never restarts inference.
use crate::{ApiResult, AppState, json_value};
use axum::{
    Json,
    extract::{
        Path, Query, State,
        rejection::{JsonRejection, QueryRejection},
    },
};
use local_agent_core::{
    Store,
    providers::{
        ConfigError,
        operations::{Kind, Operation, Request},
    },
};
use serde::Deserialize;
use std::time::Duration;

fn invalid_json(error: JsonRejection) -> (axum::http::StatusCode, Json<serde_json::Value>) {
    (
        error.status(),
        Json(
            serde_json::json!({"code":"invalid_input","outcomeUnknown":false,"message":"服务商操作参数无法读取，本次未提交。"}),
        ),
    )
}

fn unavailable() -> ConfigError {
    ConfigError::new(
        "service_unavailable",
        "服务商操作暂不可用，请保留原编号核查",
    )
}
pub(super) async fn work<T: Send + 'static>(
    state: &AppState,
    action: impl FnOnce(&Store) -> Result<T, ConfigError> + Send + 'static,
) -> Result<T, ConfigError> {
    let store = state.store.clone();
    crate::blocking::run(move || {
        let store = store.lock().map_err(|_| unavailable())?;
        action(&store)
    })
    .await
    .map_err(crate::blocking::config_error)?
}
pub(crate) async fn protocol(State(state): State<AppState>) -> ApiResult {
    json_value(
        work(&state, |store| store.provider_write_protocol())
            .await
            .map_err(super::error)?,
    )
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Scope {
    store_id: String,
}
pub(crate) async fn get(
    State(state): State<AppState>,
    Path(id): Path<String>,
    scope: Result<Query<Scope>, QueryRejection>,
) -> ApiResult {
    let Query(scope)=scope.map_err(|error|(error.status(),Json(serde_json::json!({"code":"invalid_input","outcomeUnknown":false,"message":"服务商操作查询参数无效，本次未提交新操作。"}))))?;
    json_value(
        work(&state, move |store| {
            store.provider_operation(&scope.store_id, &id)
        })
        .await
        .map_err(super::error)?,
    )
}
pub(crate) async fn cancel(
    State(state): State<AppState>,
    request: Result<Json<Request>, JsonRejection>,
) -> ApiResult {
    let Json(request) = request.map_err(invalid_json)?;
    json_value(
        work(&state, move |store| {
            store.cancel_provider_connection(&request)
        })
        .await
        .map_err(super::error)?,
    )
}
pub(crate) async fn commit(
    State(state): State<AppState>,
    request: Result<Json<Request>, JsonRejection>,
) -> ApiResult {
    let Json(request) = request.map_err(invalid_json)?;
    let lookup = request.clone();
    if let Some(operation) = work(&state, move |store| {
        store.existing_provider_operation(&lookup)
    })
    .await
    .map_err(super::error)?
    {
        return json_value(operation);
    }
    let check = request.clone();
    let copy = state.clone();
    crate::blocking::run(move || {
        let manager = copy.providers.lock().map_err(|_| unavailable())?;
        let store = copy.store.lock().map_err(|_| unavailable())?;
        manager.require_native_provider_operation(&store, &check)
    })
    .await
    .map_err(|e| super::error(crate::blocking::config_error(e)))?
    .map_err(super::catalog_connection::error)?;
    if request.kind == Kind::Save {
        let result = crate::blocking::run(move || {
            let manager = state.providers.lock().map_err(|_| unavailable())?;
            let _guard = manager.provider_edit_guard()?;
            let store = state.store.lock().map_err(|_| unavailable())?;
            super::ensure_write_enabled(&state)?;
            Ok::<_, ConfigError>(store.start_provider_operation(&request)?.operation)
        })
        .await
        .map_err(|e| super::error(crate::blocking::config_error(e)))?
        .map_err(super::error)?;
        return json_value(result);
    }
    let _permit = state
        .verification_capacity
        .clone()
        .try_acquire_owned()
        .map_err(|_| {
            super::error(ConfigError::new(
                "busy",
                "已有连接验证正在进行，请稍后查询原结果",
            ))
        })?;
    let (input, secret) = crate::provider_connect::prepare(&state, request.provider.clone())
        .await
        .map_err(super::error)?;
    let frozen = request.clone();
    let copy = state.clone();
    let start = work(&state, move |store| {
        super::ensure_write_enabled(&copy)?;
        store.start_provider_operation(&frozen)
    })
    .await
    .map_err(super::error)?;
    if !start.started {
        return json_value(start.operation);
    }
    let detected = tokio::time::timeout(
        Duration::from_secs(45),
        crate::provider_connect::detect(
            &state.provider_adapters,
            &state.provider_client,
            input,
            secret,
        ),
    )
    .await
    .unwrap_or_else(|_| {
        Err(ConfigError::new(
            "connection_timeout",
            "自动连接超时，本次未保存；可核查原操作后重试",
        ))
    });
    let result: Result<Operation, ConfigError> = match detected {
        Ok(detected) => {
            let copy = state.clone();
            let frozen = request.clone();
            // Keep the established manager -> store lock order. No network or Vault I/O under these locks.
            crate::blocking::run(move || {
                let manager = copy.providers.lock().map_err(|_| unavailable())?;
                let store = copy.store.lock().map_err(|_| unavailable())?;
                super::ensure_write_enabled(&copy)?;
                manager.finish_provider_operation(
                    &store,
                    &frozen,
                    detected.provider,
                    chrono::Utc::now().to_rfc3339(),
                )
            })
            .await
            .map_err(|e| super::error(crate::blocking::config_error(e)))?
        }
        Err(error) => Err(error),
    };
    let operation = match result {
        Ok(operation) => operation,
        Err(error) => work(&state, move |store| {
            store.fail_provider_connection(&request, &error)
        })
        .await
        .map_err(super::error)?,
    };
    json_value(operation)
}
