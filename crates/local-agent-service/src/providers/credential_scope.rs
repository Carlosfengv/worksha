use super::{error, unavailable};
use crate::{ApiResult, AppState, json_value};
use axum::{
    Json,
    extract::{
        Query, State,
        rejection::{JsonRejection, QueryRejection},
    },
};
use local_agent_core::providers::{
    ConfigError,
    credential_scope::{
        CreateV3, CredentialScope, IdentityV3, Initialization, InitializationChoice,
    },
};
use serde::Deserialize;
use zeroize::Zeroizing;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Scope {
    store_id: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Operation {
    store_id: String,
    operation_id: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Create {
    store_id: String,
    operation_id: String,
    label: String,
    secret: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ScopeV3 {
    store_id: String,
    namespace_id: String,
}
pub(crate) async fn protocol_v3(
    State(state): State<AppState>,
    input: Result<Query<Scope>, QueryRejection>,
) -> ApiResult {
    let Query(input) = input.map_err(|_| invalid())?;
    crate::blocking::run(move || {
        let manager = state.providers.lock().map_err(|_| error(unavailable()))?;
        let scope = scope(&state, &input.store_id)?;
        json_value(
            manager
                .scoped_credential_protocol_v3(&scope)
                .map_err(error)?,
        )
    })
    .await
    .map_err(|e| error(crate::blocking::config_error(e)))?
}
pub(crate) async fn initialize_v3(
    State(state): State<AppState>,
    input: Result<Json<Initialization>, JsonRejection>,
) -> ApiResult {
    let Json(input) = input.map_err(|_| invalid())?;
    crate::blocking::run(move || {
        let manager = state.providers.lock().map_err(|_| error(unavailable()))?;
        let scope = scope(&state, &input.store_id)?;
        json_value(
            manager
                .initialize_credential_namespace(
                    &scope,
                    &input.namespace_id,
                    matches!(input.choice, InitializationChoice::Restore),
                )
                .map_err(error)?,
        )
    })
    .await
    .map_err(|e| error(crate::blocking::config_error(e)))?
}
pub(crate) async fn create_v3(
    State(state): State<AppState>,
    input: Result<Json<CreateV3>, JsonRejection>,
) -> ApiResult {
    let Json(input) = input.map_err(|_| invalid())?;
    let secret = Zeroizing::new(input.secret);
    crate::blocking::run(move || {
        let manager = state.providers.lock().map_err(|_| error(unavailable()))?;
        let scope = scope(&state, &input.store_id)?;
        json_value(
            manager
                .create_credential_v3(
                    &scope,
                    &input.namespace_id,
                    &input.operation_id,
                    &input.label,
                    secret,
                )
                .map_err(error)?,
        )
    })
    .await
    .map_err(|e| error(crate::blocking::config_error(e)))?
}
pub(crate) async fn query_v3(
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<String>,
    input: Result<Query<ScopeV3>, QueryRejection>,
) -> ApiResult {
    let Query(input) = input.map_err(|_| invalid())?;
    operation_v3(
        state,
        IdentityV3 {
            store_id: input.store_id,
            namespace_id: input.namespace_id,
            operation_id: id,
        },
        false,
    )
    .await
}
pub(crate) async fn cancel_v3(
    State(state): State<AppState>,
    input: Result<Json<IdentityV3>, JsonRejection>,
) -> ApiResult {
    let Json(input) = input.map_err(|_| invalid())?;
    operation_v3(state, input, true).await
}
async fn operation_v3(state: AppState, input: IdentityV3, cancel: bool) -> ApiResult {
    crate::blocking::run(move || {
        let manager = state.providers.lock().map_err(|_| error(unavailable()))?;
        let scope = scope(&state, &input.store_id)?;
        json_value(
            manager
                .credential_operation_v3(&scope, &input.namespace_id, &input.operation_id, cancel)
                .map_err(error)?,
        )
    })
    .await
    .map_err(|e| error(crate::blocking::config_error(e)))?
}
fn scope(
    state: &AppState,
    id: &str,
) -> Result<CredentialScope, (axum::http::StatusCode, Json<serde_json::Value>)> {
    state
        .store
        .lock()
        .map_err(|_| error(unavailable()))?
        .credential_scope(id)
        .map_err(error)
}
fn invalid() -> (axum::http::StatusCode, Json<serde_json::Value>) {
    let mut result = error(ConfigError::new(
        "invalid_credential_operation",
        "凭据来源请求格式无效，未执行密钥操作",
    ));
    result.1.0["outcomeUnknown"] = serde_json::json!(false);
    result
}
pub(crate) async fn protocol(
    State(state): State<AppState>,
    input: Result<Query<Scope>, QueryRejection>,
) -> ApiResult {
    let Query(input) = input.map_err(|_| invalid())?;
    crate::blocking::run(move || {
        let manager = state.providers.lock().map_err(|_| error(unavailable()))?;
        let scope = scope(&state, &input.store_id)?;
        json_value(manager.scoped_credential_protocol(&scope).map_err(error)?)
    })
    .await
    .map_err(|e| error(crate::blocking::config_error(e)))?
}
pub(crate) async fn create(
    State(state): State<AppState>,
    input: Result<Json<Create>, JsonRejection>,
) -> ApiResult {
    let Json(input) = input.map_err(|_| invalid())?;
    let secret = Zeroizing::new(input.secret);
    crate::blocking::run(move || {
        let manager = state.providers.lock().map_err(|_| error(unavailable()))?;
        let scope = scope(&state, &input.store_id)?;
        json_value(
            manager
                .create_scoped_credential(&scope, &input.operation_id, &input.label, secret)
                .map_err(error)?,
        )
    })
    .await
    .map_err(|e| error(crate::blocking::config_error(e)))?
}
pub(crate) async fn query(
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<String>,
    input: Result<Query<Scope>, QueryRejection>,
) -> ApiResult {
    let Query(input) = input.map_err(|_| invalid())?;
    operation(
        state,
        Operation {
            store_id: input.store_id,
            operation_id: id,
        },
        false,
    )
    .await
}
pub(crate) async fn cancel(
    State(state): State<AppState>,
    input: Result<Json<Operation>, JsonRejection>,
) -> ApiResult {
    let Json(input) = input.map_err(|_| invalid())?;
    operation(state, input, true).await
}
async fn operation(state: AppState, input: Operation, cancel: bool) -> ApiResult {
    crate::blocking::run(move || {
        let manager = state.providers.lock().map_err(|_| error(unavailable()))?;
        let scope = scope(&state, &input.store_id)?;
        json_value(
            manager
                .scoped_credential_operation(&scope, &input.operation_id, cancel)
                .map_err(error)?,
        )
    })
    .await
    .map_err(|e| error(crate::blocking::config_error(e)))?
}

pub(crate) async fn preview_legacy(
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<String>,
    input: Result<Query<ScopeV3>, QueryRejection>,
) -> ApiResult {
    let Query(input) = input.map_err(|_| invalid())?;
    crate::blocking::run(move || {
        let manager = state.providers.lock().map_err(|_| error(unavailable()))?;
        let scope = scope(&state, &input.store_id)?;
        json_value(
            manager
                .preview_legacy_credential(&scope, &input.namespace_id, &id)
                .map_err(error)?,
        )
    })
    .await
    .map_err(|e| error(crate::blocking::config_error(e)))?
}
pub(crate) async fn confirm_legacy(
    State(state): State<AppState>,
    input: Result<
        Json<local_agent_core::providers::credential_operations::legacy::Confirmation>,
        JsonRejection,
    >,
) -> ApiResult {
    let Json(input) = input.map_err(|_| invalid())?;
    crate::blocking::run(move || {
        let manager = state.providers.lock().map_err(|_| error(unavailable()))?;
        let scope = scope(&state, &input.store_id)?;
        json_value(
            manager
                .confirm_legacy_credential(&scope, &input)
                .map_err(|e| {
                    let mut response = error(e);
                    response.1.0["outcomeUnknown"] = serde_json::json!(true);
                    response
                })?,
        )
    })
    .await
    .map_err(|e| error(crate::blocking::config_error(e)))?
}

pub(crate) async fn legacy_policy(State(state): State<AppState>) -> ApiResult {
    crate::blocking::run(move || {
        let store = state.store.lock().map_err(|_| error(unavailable()))?;
        json_value(store.legacy_credential_policy().map_err(error)?)
    })
    .await
    .map_err(|e| error(crate::blocking::config_error(e)))?
}
pub(crate) async fn discovery(
    State(state): State<AppState>,
    input: Result<
        Query<local_agent_core::providers::credential_operations::discovery::Request>,
        QueryRejection,
    >,
) -> ApiResult {
    let Query(input) = input.map_err(|_| invalid())?;
    crate::blocking::run(move || {
        let manager = state.providers.lock().map_err(|_| error(unavailable()))?;
        let scope = scope(&state, &input.store_id)?;
        json_value(
            manager
                .discover_credentials(&scope, &input)
                .map_err(error)?,
        )
    })
    .await
    .map_err(|e| error(crate::blocking::config_error(e)))?
}
