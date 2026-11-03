//! Explicit builtin/external writes share the original receipt and cancellation.
use crate::{ApiResult, AppState, json_value};
use axum::{Json, extract::State};
use local_agent_core::{
    domain::{
        provider_connection::ConnectionRequest,
        provider_source_operation::{SourceChoice, SourceOperationAction, SourceRequest},
    },
    providers::{
        ConfigError,
        operations::{Kind, Operation, Status},
    },
};
use std::time::Duration;
fn unavailable() -> ConfigError {
    ConfigError::new(
        "catalog_operation_unavailable",
        "原来源操作尚不可核查，请保留原请求；没有改用其他来源或重新发送",
    )
}
async fn lookup(state: &AppState, input: SourceRequest) -> Result<Option<Operation>, ConfigError> {
    let state = state.clone();
    crate::blocking::run(move || {
        let manager = state.providers.lock().map_err(|_| unavailable())?;
        let store = state.store.lock().map_err(|_| unavailable())?;
        let operation = store.existing_provider_operation(&input.operation)?;
        if let Some(op) = &operation {
            // Original cancellation can precede admission. It grants no authority.
            if op.status != Status::Cancelled {
                manager.verify_source_operation(&input)?;
            }
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
async fn ensure(state: AppState, input: SourceRequest) -> Result<(), ConfigError> {
    crate::blocking::run(move || {
        let manager = state.providers.lock().map_err(|_| unavailable())?;
        let store = state.store.lock().map_err(|_| unavailable())?;
        let op = store
            .existing_provider_operation(&input.operation)?
            .ok_or_else(unavailable)?;
        if op.status != Status::Pending {
            return Err(ConfigError::new(
                "operation_cancelled",
                "原操作已结束，没有继续发送验证请求",
            ));
        }
        manager.verify_source_operation(&input)?;
        manager.verify_catalog_source_intent(&store, &input.operation)
    })
    .await
    .map_err(crate::blocking::config_error)?
}
pub(crate) async fn action(
    State(state): State<AppState>,
    Json(action): Json<SourceOperationAction>,
) -> ApiResult {
    use super::catalog_connection::error;
    let (input, commit) = match action {
        SourceOperationAction::Commit { request } => (request, true),
        SourceOperationAction::Query { request } => (request, false),
    };
    if let Some(op) = lookup(&state, input.clone()).await.map_err(error)? {
        return json_value(op);
    }
    if !commit {
        return Err(error(ConfigError::new(
            "operation_not_found",
            "未查到原结果；保留原请求，没有重新发送",
        )));
    }
    if let SourceChoice::External { catalog } = &input.source {
        let connection = ConnectionRequest {
            operation: input.operation.clone(),
            catalog: catalog.clone(),
        };
        return super::catalog_connection::commit_selected(state, connection, Some(input)).await;
    }
    if input.operation.kind == Kind::Save {
        let op = crate::blocking::run(move || {
            let manager = state.providers.lock().map_err(|_| unavailable())?;
            let store = state.store.lock().map_err(|_| unavailable())?;
            if let Some(op) = store.existing_provider_operation(&input.operation)? {
                if op.status != Status::Cancelled {
                    manager.verify_source_operation(&input)?;
                }
                return Ok(op);
            }
            manager.save_source_provider(&store, &input, || Ok(()))
        })
        .await
        .map_err(|e| error(crate::blocking::config_error(e)))?
        .map_err(error)?;
        return json_value(op);
    }
    let _permit = state
        .verification_capacity
        .clone()
        .try_acquire_owned()
        .map_err(|_| error(ConfigError::new("busy", "已有连接验证正在进行，请稍后核查")))?;
    let (provider, secret) =
        crate::provider_connect::prepare(&state, input.operation.provider.clone())
            .await
            .map_err(error)?;
    let copy = state.clone();
    let frozen = input.clone();
    let start = crate::blocking::run(move || {
        let manager = copy.providers.lock().map_err(|_| unavailable())?;
        let store = copy.store.lock().map_err(|_| unavailable())?;
        if let Some(op) = store.existing_provider_operation(&frozen.operation)? {
            if op.status != Status::Cancelled {
                manager.verify_source_operation(&frozen)?;
            }
            return Ok((false, op));
        }
        let start = store.start_provider_operation(&frozen.operation)?;
        if let Err(reason) = manager.record_source_operation(&store, &frozen) {
            return Ok((
                false,
                store.fail_provider_connection(&frozen.operation, &reason)?,
            ));
        }
        Ok((start.started, start.operation))
    })
    .await
    .map_err(|e| error(crate::blocking::config_error(e)))?
    .map_err(error)?;
    if !start.0 {
        return json_value(start.1);
    }
    let work = crate::provider_connect::detect_checked(
        &state.provider_adapters,
        &state.provider_client,
        provider,
        secret,
        || ensure(state.clone(), input.clone()),
    );
    let stopped = async {
        loop {
            tokio::time::sleep(Duration::from_millis(50)).await;
            ensure(state.clone(), input.clone()).await?;
        }
    };
    let detected = tokio::select! {
        result = tokio::time::timeout(Duration::from_secs(45), work) => result.unwrap_or_else(|_| Err(ConfigError::new("connection_timeout", "连接超时；保留原操作，没有重新发送"))),
        result = stopped => result,
    };
    let op = crate::blocking::run(move || {
        let manager = state.providers.lock().map_err(|_| unavailable())?;
        let store = state.store.lock().map_err(|_| unavailable())?;
        let result = detected.and_then(|detected| {
            manager.verify_source_operation(&input)?;
            manager.stage_catalog_source(&store, &input.operation, &detected.provider)?;
            manager.finish_provider_operation(
                &store,
                &input.operation,
                detected.provider,
                chrono::Utc::now().to_rfc3339(),
            )
        });
        let op = match result {
            Ok(op) => op,
            Err(reason) => store.fail_provider_connection(&input.operation, &reason)?,
        };
        let _ = manager.finish_catalog_source(&store, &input.operation);
        let _ = manager.finish_catalog_operation_record(&store, &input.operation);
        Ok(op)
    })
    .await
    .map_err(|e| error(crate::blocking::config_error(e)))?
    .map_err(error)?;
    json_value(op)
}
