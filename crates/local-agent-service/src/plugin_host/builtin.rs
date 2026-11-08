use super::RuntimeConfiguration;
use crate::{ApiResult, AppState};
use axum::{
    Json,
    extract::{MatchedPath, State},
    http::StatusCode,
};
use local_agent_core::{
    Store,
    plugin::{BuiltinPlugin, PluginError, builtin::*},
};
use std::{collections::BTreeSet, sync::Arc};

#[cfg(test)]
mod tests;

pub(super) fn selected(
    plugins: &[Arc<dyn BuiltinPlugin>],
    composition: &Composition,
) -> Result<BTreeSet<String>, PluginError> {
    let ids: BTreeSet<_> = plugins
        .iter()
        .map(|plugin| plugin.descriptor().id.to_owned())
        .collect();
    if [
        &composition.disabled_plugin_ids,
        &composition.removed_plugin_ids,
        &composition.opted_in_plugin_ids,
    ]
    .into_iter()
    .any(|list| {
        list.len() > 256
            || list.windows(2).any(|pair| pair[0] >= pair[1])
            || list
                .iter()
                .any(|id| !ids.contains(id) && !super::features::deferred(id))
    }) {
        return Err(PluginError::new(
            "invalid_input",
            "功能配置包含未知或重复的插件，请重新读取目录",
        ));
    }
    if composition.opted_in_plugin_ids.iter().any(|id| {
        !plugins
            .iter()
            .any(|plugin| plugin.opt_in() && plugin.descriptor().id == id)
    }) {
        return Err(PluginError::new(
            "invalid_input",
            "组合包含未登记的可选插件",
        ));
    }
    let mut enabled: BTreeSet<String> = plugins
        .iter()
        .filter(|plugin| {
            let descriptor = plugin.descriptor();
            (!plugin.opt_in()
                || composition
                    .opted_in_plugin_ids
                    .iter()
                    .any(|id| id == descriptor.id))
                && !composition
                    .disabled_plugin_ids
                    .iter()
                    .any(|id| id == descriptor.id)
                && !composition
                    .removed_plugin_ids
                    .iter()
                    .any(|id| id == descriptor.id)
                && match composition.profile {
                    Profile::Desktop => true,
                    Profile::Recovery => false,
                    Profile::Background => !descriptor.provides.iter().any(|key| {
                        key.name == "collector"
                            || plugin.capability_metadata(key).kind
                                == CapabilityKind::UiContribution
                    }),
                }
        })
        .map(|plugin| plugin.descriptor().id.to_owned())
        .collect();
    // Stop owned support without overwriting the user's explicit support choices.
    for plugin in plugins {
        if super::features::owner(plugin.as_ref()).is_some_and(|id| !enabled.contains(id)) {
            enabled.remove(plugin.descriptor().id);
        }
    }
    Ok(enabled)
}
fn unavailable() -> PluginError {
    PluginError::new("unavailable", "插件状态服务暂不可用，请核查原操作结果")
}
fn error(error: PluginError) -> (StatusCode, Json<serde_json::Value>) {
    let unknown = error.code == "unavailable";
    (
        if unknown || error.code == "busy" {
            StatusCode::SERVICE_UNAVAILABLE
        } else {
            StatusCode::CONFLICT
        },
        Json(
            serde_json::json!({"code":error.code,"message":error.message,"outcomeUnknown":unknown}),
        ),
    )
}
fn catalog_locked(
    state: &AppState,
    config: &RuntimeConfiguration,
    store: &Store,
) -> Result<CompositionCatalog, PluginError> {
    if !Arc::ptr_eq(&state.sessions.builtin_host(), &state.readonly.host) {
        return Err(PluginError::new("unavailable", "会话来源尚未接入当前功能"));
    }
    let saved = store.builtin_composition(config.default_profile)?;
    Ok(CompositionCatalog {
        schema_version: 1,
        store_id: store.database_id().map_err(|_| unavailable())?,
        revision: saved.revision,
        composition: saved.composition,
        effective_profile: config.effective_profile,
        host: state.readonly.host.snapshot()?,
        problem: config.problem.clone(),
        impact: ImpactContext {
            active_tasks: state
                .workbench
                .as_ref()
                .is_some_and(|workbench| workbench.has_active_task()),
            model_services: store
                .model_services()
                .map_err(|_| unavailable())?
                .into_iter()
                .map(|service| service.name)
                .collect(),
        },
    })
}
fn read(state: &AppState) -> Result<CompositionCatalog, PluginError> {
    let config = state.readonly.configuration()?;
    let store = state.store.lock().map_err(|_| unavailable())?;
    catalog_locked(state, &config, &store)
}
pub(crate) async fn catalog(State(state): State<AppState>) -> ApiResult {
    crate::blocking::run(move || {
        read(&state)
            .map(|catalog| Json(serde_json::json!(catalog)))
            .map_err(error)
    })
    .await
    .map_err(|_| error(unavailable()))?
}
pub(crate) async fn operation(
    State(state): State<AppState>,
    Json(query): Json<CompositionOperation>,
) -> ApiResult {
    crate::blocking::run(move || {
        let config = state.readonly.configuration().map_err(error)?;
        let store = state.store.lock().map_err(|_| error(unavailable()))?;
        let receipt = store.builtin_composition_operation(&query).map_err(error)?;
        let catalog = catalog_locked(&state, &config, &store).map_err(error)?;
        Ok(Json(serde_json::json!(CompositionReply {
            receipt,
            catalog
        })))
    })
    .await
    .map_err(|_| error(unavailable()))?
}
pub(crate) async fn change(
    State(state): State<AppState>,
    Json(input): Json<CompositionChange>,
) -> ApiResult {
    crate::blocking::management(move || {
        apply(&state, input)
            .map(|reply| Json(serde_json::json!(reply)))
            .map_err(error)
    })
    .await
}
pub(crate) fn enable_iam(
    state: &AppState,
    operation_id: String,
    bootstrap: local_agent_core::iam::Bootstrap,
) -> Result<(), PluginError> {
    let catalog = read(state)?;
    let mut composition = catalog.composition;
    composition
        .opted_in_plugin_ids
        .push(local_agent_core::iam::PLUGIN_ID.into());
    composition.opted_in_plugin_ids.sort();
    composition.opted_in_plugin_ids.dedup();
    composition
        .disabled_plugin_ids
        .retain(|id| id != local_agent_core::iam::PLUGIN_ID);
    composition
        .removed_plugin_ids
        .retain(|id| id != local_agent_core::iam::PLUGIN_ID);
    apply_inner(
        state,
        CompositionChange {
            store_id: catalog.store_id,
            operation_id,
            expected_revision: catalog.revision,
            expected_generation: catalog.host.generation,
            expected_impact: Some(catalog.impact),
            composition,
        },
        Some(&bootstrap),
    )?;
    Ok(())
}

fn apply(state: &AppState, input: CompositionChange) -> Result<CompositionReply, PluginError> {
    apply_inner(state, input, None)
}

fn apply_inner(
    state: &AppState,
    input: CompositionChange,
    bootstrap: Option<&local_agent_core::iam::Bootstrap>,
) -> Result<CompositionReply, PluginError> {
    use super::intake::Scope;
    let basis = read(state)?;
    let enabled = selected(&state.readonly.factories, &input.composition)?;
    let scoped = basis.effective_profile == input.composition.profile
        && basis.problem.is_none()
        && basis
            .host
            .plugins
            .iter()
            .all(|plugin| plugin.runtime_active == plugin.desired_enabled);
    let changed: BTreeSet<_> = basis
        .host
        .plugins
        .iter()
        .filter(|plugin| !scoped || plugin.runtime_active != enabled.contains(&plugin.id))
        .map(|plugin| plugin.id.clone())
        .collect();
    let retained = basis
        .host
        .plugins
        .iter()
        .filter(|plugin| !changed.contains(&plugin.id))
        .map(|plugin| plugin.id.clone())
        .collect();
    let scope =
        if scoped && bootstrap.is_none() {
            let mut blocked = changed.clone();
            for plugin in &basis.host.plugins {
                if changed.contains(&plugin.id) && !plugin.id.starts_with("builtin.") {
                    blocked.extend(
                        plugin
                            .requires
                            .iter()
                            .filter(|key| key.name == "business.feature")
                            .map(|key| key.instance.clone()),
                    );
                }
            }
            loop {
                let before = blocked.len();
                for plugin in &basis.host.plugins {
                    if plugin.requires.iter().any(|key| {
                        key.name == "business.feature" && blocked.contains(&key.instance)
                    }) {
                        blocked.insert(plugin.id.clone());
                    }
                }
                if blocked.len() == before {
                    break;
                }
            }
            if [
                "builtin.providers",
                "builtin.agents",
                "builtin.model-services",
            ]
            .into_iter()
            .any(|id| blocked.contains(id))
            {
                blocked.insert("gateway".into());
            }
            Scope::Plugins(blocked)
        } else {
            Scope::All
        };
    let mut transition = state
        .readonly
        .intake
        .transition_scope(std::time::Duration::from_secs(3), scope)?;
    let mut config = state.readonly.configuration()?;
    let store = state.store.lock().map_err(|_| unavailable())?;
    if let Some(receipt) = store.check_builtin_composition_replay(&input)? {
        return Ok(CompositionReply {
            receipt: Some(receipt),
            catalog: catalog_locked(state, &config, &store)?,
        });
    }
    if config.problem.as_deref() == Some("cleanup_failed") {
        return Err(PluginError::new(
            "restart_required",
            "上次资源回收未完成，请正常重启应用后核查",
        ));
    }
    let current = catalog_locked(state, &config, &store)?;
    if input
        .expected_impact
        .as_ref()
        .is_some_and(|impact| impact != &current.impact)
    {
        return Err(PluginError::new(
            "stale_snapshot",
            "受影响的服务或任务已变化，请重新读取并核对",
        ));
    }
    if state
        .workbench
        .as_ref()
        .is_some_and(|workbench| workbench.has_active_task())
    {
        return Err(PluginError::new(
            "busy",
            "工作台仍有执行中的任务，请等待结束后再修改功能",
        ));
    }
    if current.revision != input.expected_revision
        || current.host.generation != basis.host.generation
    {
        return Err(PluginError::new(
            "stale_snapshot",
            "功能配置已变化，请重新读取",
        ));
    }
    let revoked: Vec<_> = state
        .sessions
        .directory()
        .sources()
        .iter()
        .filter(|source| {
            state
                .sessions
                .source_plugin_id(&source.id)
                .is_some_and(|id| !enabled.contains(id))
        })
        .map(|source| source.id.clone())
        .collect();
    let factories: Vec<&dyn BuiltinPlugin> =
        state.readonly.factories.iter().map(AsRef::as_ref).collect();
    let mut receipt = None;
    // Retirement runs before the next graph publishes. Preserve shared request
    // views when their consumer remains selected, rather than inspecting a
    // temporarily stopped old lease from inside a cleanup callback.
    let retained_request_views = state.readonly.retain_request_views.swap(
        enabled.contains("builtin.requests"),
        std::sync::atomic::Ordering::AcqRel,
    );
    let result = state
        .readonly
        .host
        .reconfigure_retaining(
            &input.expected_generation,
            &factories,
            &enabled,
            &retained,
            || {
                receipt = Some(store.save_builtin_composition_with_iam(
                    &input,
                    config.default_profile,
                    &revoked,
                    bootstrap,
                )?);
                if bootstrap.is_some() {
                    state.iam.mark_enabled();
                }
                transition.keep_closed_on_failure();
                Ok(())
            },
        )
        .and_then(|next| {
            config.effective_profile = if next.cleanup_failed {
                Profile::Recovery
            } else {
                input.composition.profile
            };
            config.problem = next.cleanup_failed.then(|| "cleanup_failed".into());
            let source_problem =
                || PluginError::new("source_reconcile_failed", "无法同步会话来源授权");
            let authorized = store.native_sources().map_err(|_| source_problem())?;
            if !scoped
                || state.sessions.directory().sources().iter().any(|source| {
                    state
                        .sessions
                        .source_plugin_id(&source.id)
                        .is_some_and(|id| changed.contains(id))
                })
            {
                state
                    .sessions
                    .reconcile_plugin_sources(&authorized)
                    .map_err(|_| source_problem())?;
            }
            state
                .readonly
                .bind_changed_resources(state, |id| changed.contains(id))?;
            catalog_locked(state, &config, &store)
        });
    let catalog = match result {
        Ok(catalog) => catalog,
        Err(problem) if receipt.is_some() => {
            transition.block_all();
            state.readonly.stop();
            config.effective_profile = Profile::Recovery;
            let cleanup_failed = state
                .readonly
                .host
                .snapshot()
                .is_ok_and(|host| host.cleanup_failed);
            config.problem = Some(
                if cleanup_failed {
                    "cleanup_failed"
                } else {
                    problem.code
                }
                .into(),
            );
            transition.commit(false);
            return Err(unavailable());
        }
        Err(problem) => {
            state
                .readonly
                .retain_request_views
                .store(retained_request_views, std::sync::atomic::Ordering::Release);
            return Err(problem);
        }
    };
    transition.commit(catalog.effective_profile != Profile::Recovery && catalog.problem.is_none());
    Ok(CompositionReply { receipt, catalog })
}

pub(crate) async fn guard(
    State(state): State<AppState>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let owner = super::features::owner_of_path(request.uri().path());
    guard_owned(state, request, next, owner).await
}
pub(crate) async fn guard_owned(
    state: AppState,
    request: axum::extract::Request,
    next: axum::middleware::Next,
    owner: Option<&'static str>,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    let path = request.uri().path();
    if matches!(
        path,
        "/api/snapshot"
            | "/api/session-sources"
            | "/api/builtin-plugins"
            | "/api/builtin-plugins/changes"
            | "/api/builtin-plugins/operations"
    ) || request
        .extensions()
        .get::<MatchedPath>()
        .is_some_and(|route| super::features::recovery_path(route.as_str(), request.method()))
    {
        return next.run(request).await;
    }
    let scope = if path.starts_with("/gateway/")
        || path.starts_with("/v1/")
        || path.starts_with("/capture/")
        || path.starts_with("/api/dynamic-")
        || path == "/api/agents/model-connection"
    {
        Some("gateway")
    } else {
        owner
    };
    let Some(permit) = state.readonly.intake.acquire_for(scope) else {
        return error(PluginError::new(
            "plugin_disabled",
            "相关功能已暂停或正在调整，可在插件中心检查并恢复",
        ))
        .into_response();
    };
    if let Some(owner) = owner {
        if super::features::deferred(owner) {
            return error(PluginError::new(
                "plugin_disabled",
                "此功能待接入普通插件，当前不可用。历史数据和配置仍保留",
            ))
            .into_response();
        }
        if !super::features::active(&state, owner) {
            return error(PluginError::new(
                "plugin_disabled",
                "此功能的插件已停用或卸载，请在插件中心恢复；历史数据和配置仍保留",
            ))
            .into_response();
        }
    }
    let response = next.run(request).await;
    let (parts, body) = response.into_parts();
    axum::response::Response::from_parts(
        parts,
        axum::body::Body::new(super::intake::ScopedBody::new(body, permit)),
    )
}
