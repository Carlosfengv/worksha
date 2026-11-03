//! A fixed native connection for preview, source admission and import results.
use crate::backend::{Backend, ConnectionContext};
use local_agent_core::providers::{
    credential_scope::NamespaceState,
    imports::{
        ImportRequest,
        operations::{LIMIT, PENDING_LIMIT, Protocol, validate},
    },
    operations::valid_id,
};
use serde_json::{Value, json};

fn error(message: &str) -> String {
    json!({"code":"native_import_protocol_unavailable","outcomeUnknown":false,"message":message})
        .to_string()
}
async fn protocol(context: &ConnectionContext) -> Result<Protocol, String> {
    let value = context
        .request(reqwest::Method::GET, "/api/provider-imports/protocol", None)
        .await?;
    let protocol: Protocol = serde_json::from_value(value)
        .map_err(|_| error("当前后台未提供原生导入结果核查，请升级后重新识别"))?;
    if protocol.version != 1
        || !valid_id(&protocol.store_id)
        || !valid_id(&protocol.namespace_id)
        || protocol.sequence > LIMIT as u64
        || protocol.operation_limit != LIMIT
        || protocol.pending_limit != PENDING_LIMIT
        || protocol.pending_requests.len() > PENDING_LIMIT
        || protocol.pending_requests.iter().any(|r| {
            validate(r).is_err()
                || r.operation.as_ref().is_none_or(|i| {
                    i.store_id != protocol.store_id
                        || i.namespace_id != protocol.namespace_id
                        || i.expected_sequence >= protocol.sequence
                })
        })
    {
        return Err(error("原生导入来源或能力不可核查，未提交导入"));
    }
    if !matches!(protocol.namespace_state, NamespaceState::Active) {
        return Err(error("请先核查原凭据恢复空间，再识别与导入本机配置"));
    }
    Ok(protocol)
}
async fn request_context(
    backend: &Backend,
    input: &ImportRequest,
) -> Result<ConnectionContext, String> {
    let identity =
        validate(input).map_err(|_| error("原生导入的原请求或操作编号无效，未发送请求"))?;
    let context = backend.context()?;
    let protocol = protocol(&context).await?;
    if identity.store_id != protocol.store_id
        || identity.namespace_id != protocol.namespace_id
        || identity.expected_sequence > protocol.sequence
    {
        return Err(error(
            "原导入的数据库、恢复空间或序号已变化，请连接原后台核查",
        ));
    }
    Ok(context)
}
pub(super) async fn submit(backend: &Backend, input: ImportRequest) -> Result<Value, String> {
    request_context(backend, &input)
        .await?
        .request(
            reqwest::Method::POST,
            "/api/provider-imports",
            Some(serde_json::to_value(input).map_err(|_| error("原导入请求无效"))?),
        )
        .await
}
async fn result(backend: &Backend, input: ImportRequest, cancel: bool) -> Result<Value, String> {
    request_context(backend, &input)
        .await?
        .request(
            reqwest::Method::POST,
            if cancel {
                "/api/provider-imports/operations/cancel"
            } else {
                "/api/provider-imports/operations/query"
            },
            Some(serde_json::to_value(input).map_err(|_| error("原导入请求无效"))?),
        )
        .await
}
async fn preview(backend: &Backend) -> Result<Value, String> {
    let context = backend.context()?;
    let protocol = protocol(&context).await?;
    let candidates = context
        .request(reqwest::Method::GET, "/api/provider-imports", None)
        .await?;
    Ok(json!({"protocol":protocol,"candidates":candidates}))
}
#[tauri::command]
pub(crate) async fn preview_native_provider_import(
    backend: tauri::State<'_, Backend>,
) -> Result<Value, String> {
    preview(&backend).await
}
#[tauri::command]
pub(crate) async fn get_native_provider_import(
    backend: tauri::State<'_, Backend>,
    input: ImportRequest,
) -> Result<Value, String> {
    result(&backend, input, false).await
}
#[tauri::command]
pub(crate) async fn cancel_native_provider_import(
    backend: tauri::State<'_, Backend>,
    input: ImportRequest,
) -> Result<Value, String> {
    result(&backend, input, true).await
}

#[cfg(test)]
mod tests;
