//! Explicit catalog choice uses the original non-replayable operation ledger.
use crate::{ApiResult, AppState, json_value, plugin_host::external::catalog};
use axum::{Json, extract::State};
use local_agent_core::{
    Provider,
    domain::{provider_connection::ConnectionRequest, provider_source_operation::SourceRequest},
    providers::{
        ConfigError, ProviderManager,
        operations::{Kind, Operation, Request, Status},
    },
};
use std::time::Duration;

fn unavailable() -> ConfigError {
    ConfigError::new(
        "catalog_operation_unavailable",
        "原目录选择无法核查，已保留原操作；未改用内置目录或重新探测",
    )
}
pub(super) fn error(error: ConfigError) -> (axum::http::StatusCode, Json<serde_json::Value>) {
    if error.code == "catalog_operation_unavailable" {
        return (
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            Json(
                serde_json::json!({"code":error.code,"message":error.message,"outcomeUnknown":true}),
            ),
        );
    }
    super::error(error)
}
pub(crate) async fn scoped(
    State(state): State<AppState>,
    Json(input): Json<local_agent_core::domain::provider_connection::ScopedRequest>,
) -> ApiResult {
    let copy = state.clone();
    let scope = input.store_id.clone();
    super::operations::work(&state, move |store| {
        if store.provider_write_protocol()?.store_id != scope {
            return Err(ConfigError::new(
                "operation_store_changed",
                "目录授权属于原数据库，本次未执行",
            ));
        }
        Ok(())
    })
    .await
    .map_err(super::error)?;
    match input.request.action {
        local_agent_core::domain::provider_access::Action::Read { .. } => {
            let response = catalog::api::action(State(copy), Json(input.request)).await?;
            let scope = input.store_id;
            super::operations::work(&state, move |store| {
                if store.provider_write_protocol()?.store_id != scope {
                    return Err(ConfigError::new(
                        "operation_store_changed",
                        "目录来源已变化，未交付旧结果",
                    ));
                }
                Ok(())
            })
            .await
            .map_err(super::error)?;
            Ok(response)
        }
        action => crate::blocking::run(move || {
            let store = copy.store.lock().map_err(|_| super::error(unavailable()))?;
            if store
                .provider_write_protocol()
                .map_err(super::error)?
                .store_id
                != input.store_id
            {
                return Err(super::error(ConfigError::new(
                    "operation_store_changed",
                    "目录授权属于原数据库，本次未执行",
                )));
            }
            let providers = store.providers().map_err(|_| super::error(unavailable()))?;
            let q = local_agent_core::domain::provider_access::Query {
                plugin_id: input.request.plugin_id,
                digest: input.request.digest,
            };
            let result = match action {
                local_agent_core::domain::provider_access::Action::Bind {
                    target,
                    allow_private_network,
                } => copy
                    .plugins
                    .catalog_bind(&q, target, allow_private_network, &providers),
                local_agent_core::domain::provider_access::Action::Unbind { binding_id } => {
                    copy.plugins.catalog_unbind(&q, &binding_id, &providers)
                }
                _ => unreachable!(),
            }
            .map_err(|e| super::error(plugin_error(e)))?;
            json_value(result)
        })
        .await
        .map_err(|e| super::error(crate::blocking::config_error(e)))?,
    }
}
fn plugin_error(error: crate::plugin_host::external::Error) -> ConfigError {
    ConfigError::new(
        error.code,
        crate::plugin_host::external::message(error.code),
    )
}
fn choice(
    manager: &ProviderManager,
    input: &ConnectionRequest,
    operation: &Operation,
    source: Option<&SourceRequest>,
) -> Result<(), ConfigError> {
    if let Some(source) = source {
        if operation.status == Status::Cancelled && manager.verify_source_operation(source).is_err()
        {
            return Ok(());
        }
        return manager.verify_source_operation(source);
    }
    match manager.catalog_operation_selection(&input.operation) {
        Ok(saved) if saved == input.catalog => Ok(()),
        // This host-only failure happens before directory or model I/O. The
        // original SQL failure remains independently recoverable after file loss.
        Err(_)
            if operation.status == Status::Failed
                && operation
                    .error
                    .as_ref()
                    .is_some_and(|e| e.code == "catalog_operation_unavailable") =>
        {
            Ok(())
        }
        Err(_) if operation.status == Status::Cancelled => Ok(()),
        _ => Err(unavailable()),
    }
}
async fn lookup(
    state: &AppState,
    input: ConnectionRequest,
    source: Option<SourceRequest>,
) -> Result<Option<Operation>, ConfigError> {
    let state = state.clone();
    crate::blocking::run(move || {
        let manager = state.providers.lock().map_err(|_| unavailable())?;
        let store = state.store.lock().map_err(|_| unavailable())?;
        let operation = store.existing_provider_operation(&input.operation)?;
        if let Some(op) = &operation {
            choice(&manager, &input, op, source.as_ref())?;
            if op.status != Status::Pending {
                let _ = manager.finish_catalog_source(&store, &input.operation);
                let _ = manager.finish_catalog_operation_record(&store, &input.operation);
            }
        }
        Ok(operation)
    })
    .await
    .map_err(crate::blocking::config_error)?
}
pub(crate) async fn get(
    State(state): State<AppState>,
    Json(input): Json<ConnectionRequest>,
) -> ApiResult {
    let result = lookup(&state, input, None)
        .await
        .map_err(error)?
        .ok_or_else(|| {
            error(ConfigError::new(
                "operation_not_found",
                "未查到原结果；未重新探测，请保留原操作核查",
            ))
        })?;
    json_value(result)
}
async fn ensure(
    state: AppState,
    request: Request,
    lease: catalog::Lease,
    provider: Provider,
) -> Result<(), ConfigError> {
    crate::blocking::run(move || {
        let manager = state.providers.lock().map_err(|_| unavailable())?;
        let store = state.store.lock().map_err(|_| unavailable())?;
        let operation = store
            .existing_provider_operation(&request)?
            .ok_or_else(unavailable)?;
        if operation.status != Status::Pending {
            return Err(ConfigError::new(
                "operation_cancelled",
                "原操作已结束，本次不再发送验证请求",
            ));
        }
        manager.verify_catalog_source_intent(&store, &request)?;
        lease
            .ensure(
                &state.plugins,
                &store
                    .provider_by_id(&provider.id)
                    .map_err(|_| unavailable())?
                    .into_iter()
                    .collect::<Vec<_>>(),
                &provider,
            )
            .map_err(plugin_error)
    })
    .await
    .map_err(crate::blocking::config_error)?
}
pub(crate) async fn commit(
    State(state): State<AppState>,
    Json(input): Json<ConnectionRequest>,
) -> ApiResult {
    commit_selected(state, input, None).await
}
pub(super) async fn commit_selected(
    state: AppState,
    input: ConnectionRequest,
    source: Option<SourceRequest>,
) -> ApiResult {
    if let Some(operation) = lookup(&state, input.clone(), source.clone())
        .await
        .map_err(error)?
    {
        return json_value(operation);
    }
    if input.operation.kind == Kind::Save {
        return save(state, input, source).await;
    }
    let (provider, secret) =
        crate::provider_connect::prepare(&state, input.operation.provider.clone())
            .await
            .map_err(error)?;
    let copy = state.clone();
    let selection = input.catalog.clone();
    let proposed = provider.clone();
    let lease = crate::blocking::run(move || {
        let store = copy.store.lock().map_err(|_| unavailable())?;
        copy.plugins
            .catalog_connection_lease(
                &selection,
                &store.providers().map_err(|_| unavailable())?,
                &proposed,
            )
            .map_err(plugin_error)
    })
    .await
    .map_err(|e| error(crate::blocking::config_error(e)))?
    .map_err(error)?;
    let copy = state.clone();
    let frozen = input.clone();
    let explicit = source.clone();
    let start = crate::blocking::run(move || {
        let manager = copy.providers.lock().map_err(|_| unavailable())?;
        let store = copy.store.lock().map_err(|_| unavailable())?;
        if let Some(op) = store.existing_provider_operation(&frozen.operation)? {
            choice(&manager, &frozen, &op, explicit.as_ref())?;
            return Ok((false, op));
        }
        super::ensure_write_enabled(&copy)?;
        let start = store.start_provider_operation(&frozen.operation)?;
        let recorded = if let Some(source) = &explicit {
            manager.record_source_operation(&store, source)
        } else {
            manager.record_catalog_operation(&store, &frozen.operation, &frozen.catalog)
        };
        if recorded.is_err() {
            let failed = store.fail_provider_connection(&frozen.operation, &unavailable())?;
            return Ok((false, failed));
        }
        Ok((start.started, start.operation))
    })
    .await
    .map_err(|e| error(crate::blocking::config_error(e)))?
    .map_err(error)?;
    if !start.0 {
        return json_value(start.1);
    }
    let work = async {
        ensure(
            state.clone(),
            input.operation.clone(),
            lease.clone(),
            provider.clone(),
        )
        .await?;
        let models = catalog::read_for_connection(state.clone(), &input.catalog)
            .await
            .map_err(plugin_error)?;
        let _permit = state
            .verification_capacity
            .clone()
            .try_acquire_owned()
            .map_err(|_| ConfigError::new("busy", "已有连接验证正在进行；本次未发送模型请求"))?;
        crate::provider_connect::detect_catalog(
            &state.provider_adapters,
            &state.provider_client,
            provider.clone(),
            secret,
            models,
            || {
                ensure(
                    state.clone(),
                    input.operation.clone(),
                    lease.clone(),
                    provider.clone(),
                )
            },
        )
        .await
    };
    let stopped = async {
        loop {
            tokio::time::sleep(Duration::from_millis(50)).await;
            ensure(
                state.clone(),
                input.operation.clone(),
                lease.clone(),
                provider.clone(),
            )
            .await?;
        }
    };
    let detected = tokio::select! {
        result=tokio::time::timeout(Duration::from_secs(45),work)=>result.unwrap_or_else(|_|Err(ConfigError::new("connection_timeout","连接超时，原操作已保留；未自动重新发送"))),
        result=stopped=>result,
    };
    let copy = state.clone();
    let frozen = input.operation.clone();
    let operation = crate::blocking::run(move || {
        let manager = copy.providers.lock().map_err(|_| unavailable())?;
        let store = copy.store.lock().map_err(|_| unavailable())?;
        let result = detected.and_then(|detected| {
            super::ensure_write_enabled(&copy)?;
            lease
                .ensure(
                    &copy.plugins,
                    &store
                        .provider_by_id(&provider.id)
                        .map_err(|_| unavailable())?
                        .into_iter()
                        .collect::<Vec<_>>(),
                    &provider,
                )
                .map_err(plugin_error)?;
            manager.stage_catalog_source(&store, &frozen, &detected.provider)?;
            manager.finish_provider_operation(
                &store,
                &frozen,
                detected.provider,
                chrono::Utc::now().to_rfc3339(),
            )
        });
        let op = match result {
            Ok(op) => op,
            Err(error) => store.fail_provider_connection(&frozen, &error)?,
        };
        // A failed bookkeeping update cannot change a confirmed SQL outcome.
        let _ = manager.finish_catalog_source(&store, &frozen);
        let _ = manager.finish_catalog_operation_record(&store, &frozen);
        Ok(op)
    })
    .await
    .map_err(|e| error(crate::blocking::config_error(e)))?
    .map_err(error)?;
    json_value(operation)
}

/// Manual selection persists metadata and the source, without a directory read,
/// Vault access or model verification. The original Save receipt stays intact.
async fn save(
    state: AppState,
    input: ConnectionRequest,
    source: Option<SourceRequest>,
) -> ApiResult {
    let operation = crate::blocking::run(move || {
        let manager = state.providers.lock().map_err(|_| unavailable())?;
        let store = state.store.lock().map_err(|_| unavailable())?;
        if let Some(op) = store.existing_provider_operation(&input.operation)? {
            choice(&manager, &input, &op, source.as_ref())?;
            return Ok(op);
        }
        super::ensure_write_enabled(&state)?;
        let providers = store
            .provider_by_id(&input.operation.provider.id)
            .map_err(|_| unavailable())?
            .into_iter()
            .collect::<Vec<_>>();
        let lease = state
            .plugins
            .catalog_connection_lease(&input.catalog, &providers, &input.operation.provider)
            .map_err(plugin_error)?;
        let authorize = || {
            lease
                .ensure(&state.plugins, &providers, &input.operation.provider)
                .map_err(plugin_error)
        };
        if let Some(source) = &source {
            manager.save_source_provider(&store, source, authorize)
        } else {
            manager.save_catalog_provider(&store, &input.operation, &input.catalog, authorize)
        }
    })
    .await
    .map_err(|e| error(crate::blocking::config_error(e)))?
    .map_err(error)?;
    json_value(operation)
}
