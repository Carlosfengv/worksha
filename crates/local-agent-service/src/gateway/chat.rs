use crate::{ApiResult, AppState, json_value, providers};
use axum::{Json, extract::State, http::HeaderMap};
use local_agent_core::{
    providers::{ConfigError, files::hash},
    routing::chat::{Operation, OperationQuery, Receipt},
};
use serde::Deserialize;
use serde_json::json;

pub(super) fn token(headers: &HeaderMap) -> Option<&str> {
    let mut token = None;
    for name in ["authorization", "x-api-key"] {
        let mut values = headers.get_all(name).iter();
        if let Some(value) = values.next() {
            if token.is_some() || values.next().is_some() {
                return None;
            }
            let value = value.to_str().ok()?;
            token = Some(if name == "authorization" {
                value.strip_prefix("Bearer ")?
            } else {
                value
            });
        }
    }
    token.filter(|token| !token.is_empty() && token.len() <= 256)
}

pub(super) fn authenticate(
    store: &local_agent_core::Store,
    binding: &str,
    headers: &HeaderMap,
) -> Result<Option<u32>, ConfigError> {
    match token(headers) {
        Some(token) => store.chat_key_version(binding, &hash(token.as_bytes())),
        None => Ok(None),
    }
}

fn base_url(host: &str, binding: &str) -> String {
    format!("http://{host}/gateway/{binding}/v1")
}

fn reply(host: &str, receipt: Receipt) -> ApiResult {
    let url = base_url(host, &receipt.binding_id);
    json_value(json!({"receipt":receipt,"baseUrl":url}))
}

pub(crate) async fn list(State(state): State<AppState>) -> ApiResult {
    let readers = state.store_readers.clone();
    let host = state.gateway.host.clone();
    let value = crate::blocking::run(move || {
        readers.read(
            || ConfigError::new("unavailable", "本机连接暂不可读取"),
            |store| {
                let items = store.chat_bindings()?.into_iter().map(|entry| {
                    json!({
                        "baseUrl":base_url(&host, &entry.binding_id),
                        "bindingId":entry.binding_id,"name":entry.name,"keyVersion":entry.key_version,
                        "revoked":entry.revoked,"serviceId":entry.service_id
                    })
                }).collect::<Vec<_>>();
                Ok(json!({"storeId":store.database_id()?,"items":items,"pending":store.chat_pending_operations()?}))
            },
        )
    }).await.map_err(|error| providers::error(crate::blocking::config_error(error)))?
        .map_err(providers::error)?;
    Ok(Json(value))
}

pub(crate) async fn prepare(
    State(state): State<AppState>,
    Json(input): Json<Operation>,
) -> ApiResult {
    let host = state.gateway.host.clone();
    let receipt = crate::blocking::store(state.store, move |store| store.chat_prepare(&input))
        .await
        .map_err(providers::error)?;
    reply(&host, receipt)
}

pub(crate) async fn operation(
    State(state): State<AppState>,
    Json(query): Json<OperationQuery>,
) -> ApiResult {
    let readers = state.store_readers.clone();
    let receipt = crate::blocking::run(move || {
        readers.read(
            || ConfigError::new("unavailable", "本机连接回执暂不可读取"),
            |store| store.chat_operation(&query),
        )
    })
    .await
    .map_err(|error| providers::error(crate::blocking::config_error(error)))?
    .map_err(providers::error)?;
    reply(&state.gateway.host, receipt)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Finish {
    query: OperationQuery,
    verified_digest: Option<String>,
}

pub(crate) async fn finish(State(state): State<AppState>, Json(input): Json<Finish>) -> ApiResult {
    let receipt = crate::blocking::store(state.store, move |store| {
        store.chat_finish(&input.query, input.verified_digest.as_deref())
    })
    .await
    .map_err(providers::error)?;
    reply(&state.gateway.host, receipt)
}
