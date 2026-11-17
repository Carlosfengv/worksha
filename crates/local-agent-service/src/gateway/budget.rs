use crate::{ApiResult, AppState, iam::Actor, json_value, providers};
use axum::{
    Extension, Json,
    extract::{Path, State},
};
use local_agent_core::{
    providers::ConfigError,
    routing::budget::{Policy, Review},
};
use serde::Deserialize;
use serde_json::json;

pub(crate) async fn bindings(State(state): State<AppState>) -> ApiResult {
    let readers = state.store_readers.clone();
    let data = crate::blocking::run(move || {
        readers.read(unavailable, |store| {
            let items = store
                .dynamic_bindings()?
                .into_iter()
                .map(|b| {
                    json!({
                        "bindingId":b.id,"agentId":b.agent_id,
                        "revoked":b.scope.starts_with("revoked:"),
                        "model":b.target.provider.model
                    })
                })
                .collect::<Vec<_>>();
            Ok(json!({"storeId":store.database_id()?,"items":items}))
        })
    })
    .await
    .map_err(|error| providers::error(crate::blocking::config_error(error)))?
    .map_err(providers::error)?;
    Ok(Json(data))
}

fn unavailable() -> ConfigError {
    ConfigError::new("unavailable", "本机预算暂不可读取")
}

pub(crate) async fn view(State(state): State<AppState>, Path(binding): Path<String>) -> ApiResult {
    let readers = state.store_readers.clone();
    let view=crate::blocking::run(move|| readers.read(unavailable,|store|{
        if store.dynamic_binding(&binding)?.is_none(){return Err(ConfigError::new("not_found","未找到本机绑定"));}
        Ok(json!({"storeId":store.database_id()?,"policy":store.budget_policy(&binding)?,
             "attempts":store.budget_uncertain(&binding)?,"windows":store.budget_window_status(&binding,chrono::Utc::now())?}))
    })).await.map_err(|error|providers::error(crate::blocking::config_error(error)))?.map_err(providers::error)?;
    Ok(Json(view))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Save {
    store_id: String,
    policy: Policy,
}

pub(crate) async fn save(
    State(state): State<AppState>,
    Path(binding): Path<String>,
    Json(input): Json<Save>,
) -> ApiResult {
    let saved = crate::blocking::store(state.store, move |store| {
        store.save_budget_policy(&input.store_id, &binding, input.policy)
    })
    .await
    .map_err(providers::error)?;
    json_value(saved)
}

pub(crate) async fn review(
    State(state): State<AppState>,
    actor: Option<Extension<Actor>>,
    Json(input): Json<Review>,
) -> ApiResult {
    let actor = actor.map_or_else(|| "local-owner".to_string(), |Extension(actor)| actor.0);
    let receipt = crate::blocking::store(state.store, move |store| {
        store.budget_review(&input, &actor)
    })
    .await
    .map_err(providers::error)?;
    json_value(receipt)
}

pub(crate) async fn attempt(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult {
    let readers = state.store_readers.clone();
    let data = crate::blocking::run(move || {
        readers.read(unavailable,|store|{
        Ok(json!({"storeId":store.database_id()?,"attempt":store.budget_attempt_for_request(&id)?}))
    })
    })
    .await
    .map_err(|error| providers::error(crate::blocking::config_error(error)))?
    .map_err(providers::error)?;
    Ok(Json(data))
}

#[derive(Deserialize)]
pub(crate) struct WarningQuery {
    #[serde(default)]
    pending: bool,
}
pub(crate) async fn warnings(
    State(state): State<AppState>,
    axum::extract::Query(query): axum::extract::Query<WarningQuery>,
) -> ApiResult {
    let readers = state.store_readers.clone();
    let data = crate::blocking::run(move || {
        readers.read(unavailable,|store|{
        Ok(json!({"storeId":store.database_id()?,"warnings":store.budget_warnings(query.pending)?}))
    })
    })
    .await
    .map_err(|error| providers::error(crate::blocking::config_error(error)))?
    .map_err(providers::error)?;
    Ok(Json(data))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Notified {
    store_id: String,
    ids: Vec<String>,
}
pub(crate) async fn notified(
    State(state): State<AppState>,
    Json(input): Json<Notified>,
) -> ApiResult {
    crate::blocking::store(state.store, move |store| {
        store.budget_warnings_notified(&input.store_id, &input.ids)
    })
    .await
    .map_err(providers::error)?;
    json_value(json!({"updated":true}))
}
