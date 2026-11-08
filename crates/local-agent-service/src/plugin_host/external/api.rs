use super::diagnostics::Operation;
use super::{Checked, Result, fail, run_sync};
use crate::{ApiResult, AppState};
use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    routing::{get, post},
};
use local_agent_core::plugin::external::*;
use serde::Deserialize;
use serde_json::{Value, json};

pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .merge(super::full::api::routes())
        .merge(super::policy::api::routes())
        .merge(super::object_contributions::api::routes())
        .route("/api/plugins", get(list))
        .route("/api/plugins/inspect", post(inspect))
        .route("/api/plugins/upload/inspect", post(inspect_upload))
        .route("/api/plugins/install", post(install))
        .route("/api/plugins/state", post(change))
        .route("/api/plugins/source", post(source))
        .route("/api/plugins/view", post(view))
        .route("/api/plugins/action", post(action))
        .layer(axum::extract::DefaultBodyLimit::max(MAX_FRAME))
}
pub(crate) fn response(result: Result<Value>) -> ApiResult {
    result.map(Json).map_err(|error| {
        (
            match error.code {
                "permission_denied" | "grant_revoked" => StatusCode::FORBIDDEN,
                "not_found" => StatusCode::NOT_FOUND,
                "stale_snapshot" => StatusCode::CONFLICT,
                "busy" => StatusCode::SERVICE_UNAVAILABLE,
                _ => StatusCode::BAD_REQUEST,
            },
            Json(json!({"code":error.code,"message":super::message(error.code)})),
        )
    })
}
async fn list(State(state): State<AppState>) -> ApiResult {
    response(run_sync(move || {
        let items = state.plugins.list()?;
        let diagnostics = state.plugins.diagnostics(&items);
        let contributions = state.plugins.contribution_catalog(&items);
        Ok(
            json!({"items":items,"diagnostics":diagnostics,"contributions":contributions,"hostFeatures":[local_agent_core::plugin::contributions::OPTIONAL,local_agent_core::plugin::contributions::DETAILS,local_agent_core::plugin::contributions::NAVIGATION,local_agent_core::plugin::contributions::SERVICES],"developmentMode":state.plugins.development(),"apiVersion":API_VERSION,"runtime":"wasmtime-36.0.19","externalSqlite":"unsupported","externalProtocol":"explicit-binding","externalConfiguration":"finite-json-plan","mcp":"partial","dsh":"unverified","cleanup":"unsupported","redemption":"unsupported"}),
        )
    }).await)
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct InspectPath {
    pub path: String,
}
async fn inspect(State(state): State<AppState>, Json(request): Json<InspectPath>) -> ApiResult {
    let result = run_sync(move || {
        state
            .plugins
            .inspect(std::path::Path::new(&request.path))
            .and_then(|r| serde_json::to_value(r).checked("invalid_input"))
    })
    .await;
    response(result)
}
async fn inspect_upload(
    State(state): State<AppState>,
    Json(request): Json<InspectPath>,
) -> ApiResult {
    response(
        run_sync(move || {
            state
                .plugins
                .inspect_upload(std::path::Path::new(&request.path))
        })
        .await,
    )
}
async fn install(State(state): State<AppState>, Json(request): Json<InstallRequest>) -> ApiResult {
    let result = run_sync(move || {
        state
            .plugins
            .install(&request)
            .and_then(|r| serde_json::to_value(r).checked("invalid_input"))
    })
    .await;
    response(result)
}
async fn change(State(state): State<AppState>, Json(request): Json<StateRequest>) -> ApiResult {
    let operation = match request.action {
        StateAction::Enable => Operation::Enable,
        StateAction::Disable => Operation::Disable,
        StateAction::Remove => Operation::Remove,
        StateAction::Rollback => Operation::Rollback,
        StateAction::RevokeGrants => Operation::RevokeGrants,
        StateAction::RevokePublisher => Operation::RevokePublisher,
    };
    let mut observed = state
        .plugins
        .observe(&request.plugin_id, &request.digest, operation);
    let result = state
        .plugins
        .change(request)
        .await
        .and_then(|r| serde_json::to_value(r).checked("invalid_input"));
    observed.finish(&result);
    response(result)
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SourcePath {
    pub plugin_id: String,
    pub digest: String,
    pub path: String,
}
async fn source(State(state): State<AppState>, Json(request): Json<SourcePath>) -> ApiResult {
    response(
        run_sync(move || {
            state
                .plugins
                .grant_source(
                    &request.plugin_id,
                    &request.digest,
                    std::path::Path::new(&request.path),
                )
                .and_then(|r| serde_json::to_value(r).checked("invalid_input"))
        })
        .await,
    )
}
async fn view(State(state): State<AppState>, Json(query): Json<ViewQuery>) -> ApiResult {
    response(
        run_sync(move || {
            let value = state.plugins.query_view(&query)?;
            crate::domain::validate_plugin_report(&state, &value)?;
            Ok(value)
        })
        .await,
    )
}
async fn action(State(state): State<AppState>, Json(request): Json<ActionRequest>) -> ApiResult {
    let operation = match &request.action {
        PublicAction::AuthorizeReport => Operation::AuthorizeReport,
        PublicAction::Refresh { .. } => Operation::Refresh,
        PublicAction::ReadSession { .. } => Operation::ReadSession,
        PublicAction::BindConnection { .. } => Operation::BindConnection,
        PublicAction::SaveSettings { .. } => Operation::SaveSettings,
        PublicAction::CreateProvider { .. } => Operation::CreateProvider,
    };
    let mut observed = state
        .plugins
        .observe(&request.plugin_id, &request.digest, operation);
    let result = async {
        match request.action {
            PublicAction::AuthorizeReport => {
                run_sync(move || {
                    serde_json::to_value(
                        state
                            .plugins
                            .authorize_report(&request.plugin_id, &request.digest)?,
                    )
                    .checked("invalid_input")
                })
                .await
            }
            PublicAction::Refresh { view_id } => {
                let copy = state.clone();
                let id = request.plugin_id.clone();
                let digest = request.digest.clone();
                let target = view_id.clone();
                let (report, provider) = run_sync(move || {
                    let (p, package) = copy.plugins.verified_package(&id, &digest)?;
                    if p.manifest
                        .requires
                        .iter()
                        .any(|r| r == local_agent_core::plugin::object_contributions::FEATURE)
                    {
                        return Err(fail("object_context_required"));
                    }
                    if p.manifest.capabilities.iter().any(|c| c == "report.view@1") {
                        if let Some(view_id) = target {
                            if !p
                                .manifest
                                .views
                                .iter()
                                .any(|view| view.id == view_id && view.kind == ViewKind::Report)
                            {
                                return Err(fail("not_found"));
                            }
                            copy.plugins.require_view_dependencies(&package, &view_id)?;
                        }
                        return Ok((true, None));
                    }
                    if target.is_some() {
                        return Err(fail("invalid_input"));
                    }
                    let selected = copy.plugins.selection(&id)?;
                    let provider = if let Some(connection) = selected.connection {
                        copy.store
                            .lock()
                            .checked("unavailable")?
                            .providers()
                            .checked("unavailable")?
                            .into_iter()
                            .find(|p| p.id == connection.provider_id)
                    } else {
                        None
                    };
                    Ok((false, provider))
                })
                .await?;
                if report {
                    return refresh_report(&state, &request.plugin_id, &request.digest, view_id)
                        .await;
                }
                state
                    .plugins
                    .refresh_external(&request.plugin_id, &request.digest, provider)
                    .await
            }
            PublicAction::ReadSession {
                session_id,
                snapshot,
            } => {
                state
                    .plugins
                    .read_session(&request.plugin_id, &request.digest, &session_id, &snapshot)
                    .await
            }
            PublicAction::BindConnection {
                provider_id,
                allow_private_network,
            } => {
                run_sync(move || {
                    let provider = state
                        .store
                        .lock()
                        .checked("unavailable")?
                        .providers()
                        .checked("unavailable")?
                        .into_iter()
                        .find(|p| p.id == provider_id)
                        .ok_or(fail("not_found"))?;
                    serde_json::to_value(state.plugins.grant_connection(
                        &request.plugin_id,
                        &request.digest,
                        &provider,
                        allow_private_network,
                    )?)
                    .checked("invalid_input")
                })
                .await
            }
            PublicAction::SaveSettings { values } => {
                run_sync(move || {
                    serde_json::to_value(state.plugins.save_private_settings(
                        &request.plugin_id,
                        &request.digest,
                        &values,
                    )?)
                    .checked("invalid_input")
                })
                .await
            }
            PublicAction::CreateProvider { preset } => {
                run_sync(move || {
                    let selected = state.plugins.selection(&request.plugin_id)?;
                    if !selected.enabled || selected.digest != request.digest {
                        return Err(fail("plugin_disabled"));
                    }
                    let installed = state
                        .plugins
                        .verified(&request.plugin_id, &request.digest)?;
                    let p = super::presets::legacy_advice(
                        &installed.manifest,
                        &request.digest,
                        &preset,
                    )?;
                    let store = state.store.lock().checked("unavailable")?;
                    crate::providers::ensure_write_enabled(&state)
                        .map_err(|_| fail("plugin_disabled"))?;
                    let id = state.plugins.preset_provider_id(
                        &request.plugin_id,
                        &request.digest,
                        &preset,
                    )?;
                    let provider = local_agent_core::Provider {
                        id: id.clone(),
                        name: p.name.clone(),
                        vendor: "custom".into(),
                        endpoint: p
                            .endpoints
                            .get(&p.protocol)
                            .cloned()
                            .ok_or(fail("invalid_input"))?,
                        protocol: p.protocol.clone(),
                        model: p.model.clone(),
                        configured: true,
                        credential_ref: None,
                        credential_label: None,
                        models: if p.model.is_empty() {
                            vec![]
                        } else {
                            vec![p.model.clone()]
                        },
                        disabled_models: vec![],
                        model_catalog: None,
                        revision: 0,
                        category: "cloud".into(),
                    };
                    if !store
                        .providers()
                        .checked("unavailable")?
                        .iter()
                        .any(|p| p.id == id)
                    {
                        store.save_provider(provider).checked("invalid_input")?;
                    }
                    Ok(json!({"providerId":id,"verified":false}))
                })
                .await
            }
        }
    }
    .await;
    observed.finish(&result);
    response(result)
}
async fn refresh_report(
    state: &AppState,
    id: &str,
    digest: &str,
    view_id: Option<String>,
) -> Result<Value> {
    let plugins = state.plugins.clone();
    let id = id.to_owned();
    let digest = digest.to_owned();
    let before = run_sync(move || plugins.status(&id, &digest)).await?;
    if !before.enabled || !before.report_authorized {
        return Err(fail("permission_denied"));
    }
    if before
        .manifest
        .requires
        .iter()
        .any(|r| r == local_agent_core::plugin::contributions::SERVICES)
    {
        return crate::domain::refresh_plugin_report(state, before, view_id).await;
    }
    let view_ids: Vec<_> = before
        .manifest
        .views
        .iter()
        .filter(|view| {
            view.kind == ViewKind::Report && view_id.as_ref().is_none_or(|id| id == &view.id)
        })
        .map(|view| view.id.clone())
        .collect();
    let observation = state.observation.clone();
    let now = chrono::Utc::now();
    let value = run_sync(move || {
        observation
            .sql_query(|history| history.external_gateway_summary(now))
            .checked("resource_limit")
    })
    .await?;
    let count = |v: &Value| -> Result<String> {
        v.as_u64()
            .map(|n| n.to_string())
            .ok_or(fail("invalid_input"))
    };
    let usage = |key: &str| -> Result<Option<String>> {
        if value["usage"][key].is_null() {
            Ok(None)
        } else {
            Ok(Some(count(&value["usage"][key])?))
        }
    };
    let report = GatewayReport {
        requests: count(&value["requests"])?,
        attempts: count(&value["attempts"])?,
        input_tokens: usage("inputTokens")?,
        output_tokens: usage("outputTokens")?,
        total_tokens: usage("totalTokens")?,
        range_start: (now - chrono::Duration::hours(24)).to_rfc3339(),
        range_end: now.to_rfc3339(),
        sources: vec![],
    };
    let plugins = state.plugins.clone();
    run_sync(move || {
        plugins.publish_report(&before, &view_ids, report)?;
        Ok(json!({"refreshed":true}))
    })
    .await
}
