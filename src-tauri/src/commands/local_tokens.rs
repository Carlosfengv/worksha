//! Keep capability admission and the numbered action on one native connection.
use crate::backend::{Backend, ConnectionContext};
use local_agent_core::providers::{
    BindingDraft,
    credential_scope::{IdentityV3, NamespaceState},
    local_tokens::{CANCELLATION_LIMIT, LIMIT, PENDING_LIMIT, Protocol, Status},
    operations::valid_id,
};
use serde_json::{Value, json};

fn error(message: &str) -> String {
    json!({"code":"local_token_protocol_unavailable","outcomeUnknown":false,"message":message})
        .to_string()
}
async fn protocol(context: &ConnectionContext) -> Result<Protocol, String> {
    let value = context
        .request(reqwest::Method::GET, "/api/local-tokens/protocol", None)
        .await?;
    let protocol: Protocol = serde_json::from_value(value)
        .map_err(|_| error("当前后台无法核查本机入口，请升级并重新读取配置"))?;
    if protocol.version != 2
        || protocol.schema_version != local_agent_core::Store::SCHEMA_VERSION
        || !valid_id(&protocol.store_id)
        || !valid_id(&protocol.namespace_id)
        || protocol.operation_limit != LIMIT
        || protocol.pending_limit != PENDING_LIMIT
        || protocol.cancellation_limit != CANCELLATION_LIMIT
        || protocol.pending.len() > PENDING_LIMIT
        || protocol.pending.iter().any(|operation| {
            let id = &operation.request.identity;
            operation.version != 1
                || operation.status != Status::Pending
                || operation.preview_id.is_some()
                || id.store_id != protocol.store_id
                || id.namespace_id != protocol.namespace_id
                || !valid_id(&id.operation_id)
                || operation.owner_id != id.operation_id
                || !valid_id(&operation.credential_ref)
        })
        || protocol
            .pending
            .iter()
            .enumerate()
            .any(|(index, operation)| {
                protocol.pending[..index]
                    .iter()
                    .any(|other| other.owner_id == operation.owner_id)
            })
    {
        return Err(error("本机入口能力或原来源不可核查，未发送创建请求"));
    }
    Ok(protocol)
}
async fn context(backend: &Backend, identity: &IdentityV3) -> Result<ConnectionContext, String> {
    if !valid_id(&identity.store_id)
        || !valid_id(&identity.namespace_id)
        || !valid_id(&identity.operation_id)
    {
        return Err(error("原本机入口身份或编号无效，未发送请求"));
    }
    let context = backend.context()?;
    let protocol = protocol(&context).await?;
    if protocol.store_id != identity.store_id
        || protocol.namespace_id != identity.namespace_id
        || !matches!(protocol.namespace_state, NamespaceState::Active)
    {
        return Err(error("请连接原后台并核查原凭据恢复空间，未转移原操作"));
    }
    Ok(context)
}
pub(super) async fn prepare_draft(
    backend: &Backend,
    draft: BindingDraft,
    dynamic: bool,
) -> Result<Value, String> {
    let context = if draft.provider_id.is_none() && !dynamic && draft.operation.is_none() {
        backend.context()?
    } else {
        if draft.provider_id.is_none() {
            return Err(error("本机入口缺少原供应商"));
        }
        context(
            backend,
            draft
                .operation
                .as_ref()
                .ok_or_else(|| error("请重新读取配置并保留原入口编号"))?,
        )
        .await?
    };
    context
        .request(
            reqwest::Method::POST,
            if dynamic {
                "/api/dynamic-connections"
            } else {
                "/api/provider-switches"
            },
            Some(serde_json::to_value(draft).map_err(|_| error("原配置草稿无效"))?),
        )
        .await
}
pub(super) async fn prepare_service(backend: &Backend, input: Value) -> Result<Value, String> {
    let identity: IdentityV3 = serde_json::from_value(
        input
            .get("operation")
            .cloned()
            .ok_or_else(|| error("请重新读取模型服务并保留原入口编号"))?,
    )
    .map_err(|_| error("模型服务原入口身份无效"))?;
    context(backend, &identity)
        .await?
        .request(
            reqwest::Method::POST,
            "/api/model-service-connections",
            Some(input),
        )
        .await
}
async fn action(backend: &Backend, input: IdentityV3, action: &str) -> Result<Value, String> {
    context(backend, &input)
        .await?
        .request(
            reqwest::Method::POST,
            &format!("/api/local-tokens/{action}"),
            Some(serde_json::to_value(input).map_err(|_| error("原入口身份无效"))?),
        )
        .await
}
#[tauri::command]
pub(crate) async fn get_local_token_protocol(
    backend: tauri::State<'_, Backend>,
) -> Result<Value, String> {
    serde_json::to_value(protocol(&backend.context()?).await?)
        .map_err(|_| error("本机入口能力回复无效"))
}
#[tauri::command]
pub(crate) async fn get_local_token_operation(
    backend: tauri::State<'_, Backend>,
    input: IdentityV3,
) -> Result<Value, String> {
    action(&backend, input, "query").await
}
#[tauri::command]
pub(crate) async fn continue_local_token_operation(
    backend: tauri::State<'_, Backend>,
    input: IdentityV3,
) -> Result<Value, String> {
    action(&backend, input, "continue").await
}
#[tauri::command]
pub(crate) async fn cancel_local_token_operation(
    backend: tauri::State<'_, Backend>,
    input: IdentityV3,
) -> Result<Value, String> {
    action(&backend, input, "cancel").await
}
#[cfg(test)]
mod tests;
