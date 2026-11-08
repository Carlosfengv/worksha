use crate::backend::Backend;
use local_agent_core::domain::provider_access::{Query, Request};
use local_agent_core::domain::provider_connection::ConnectionRequest;
use local_agent_core::domain::provider_connection::ScopedRequest;
use local_agent_core::domain::provider_source::SourceAction;

#[tauri::command]
pub(crate) async fn query_provider_presets(
    backend: tauri::State<'_, Backend>,
    input: local_agent_core::domain::provider_presets::PresetAction,
) -> Result<serde_json::Value, String> {
    backend
        .request(
            reqwest::Method::POST,
            "/api/provider-presets",
            Some(serde_json::to_value(input).map_err(|_| "供应商预设查询参数无效")?),
        )
        .await
}

#[tauri::command]
pub(crate) async fn perform_provider_draft_recovery(
    backend: tauri::State<'_, Backend>,
    input: local_agent_core::domain::provider_draft_recovery::DraftRecoveryAction,
) -> Result<serde_json::Value, String> {
    backend
        .request(
            reqwest::Method::POST,
            "/api/provider-draft-recovery",
            Some(serde_json::to_value(input).map_err(|_| "草稿授权恢复参数无效")?),
        )
        .await
}

#[tauri::command]
pub(crate) async fn perform_provider_draft_grant(
    backend: tauri::State<'_, Backend>,
    input: local_agent_core::domain::provider_draft_grant::DraftGrantAction,
) -> Result<serde_json::Value, String> {
    backend
        .request(
            reqwest::Method::POST,
            "/api/provider-draft-grant",
            Some(serde_json::to_value(input).map_err(|_| "草稿授权参数无效")?),
        )
        .await
}

#[tauri::command]
pub(crate) async fn perform_provider_draft_catalog(
    backend: tauri::State<'_, Backend>,
    input: local_agent_core::domain::provider_draft_catalog::DraftCatalogAction,
) -> Result<serde_json::Value, String> {
    backend
        .request(
            reqwest::Method::POST,
            "/api/provider-draft-catalog",
            Some(serde_json::to_value(input).map_err(|_| "草稿目录参数无效")?),
        )
        .await
}

#[tauri::command]
pub(crate) async fn perform_provider_catalog_source(
    backend: tauri::State<'_, Backend>,
    input: SourceAction,
) -> Result<serde_json::Value, String> {
    backend
        .request(
            reqwest::Method::POST,
            "/api/provider-catalog-source",
            Some(serde_json::to_value(input).map_err(|_| "目录来源操作无效")?),
        )
        .await
}

#[tauri::command]
pub(crate) async fn perform_scoped_provider_catalog(
    backend: tauri::State<'_, Backend>,
    input: ScopedRequest,
) -> Result<serde_json::Value, String> {
    backend
        .request(
            reqwest::Method::POST,
            "/api/catalog-provider-scope",
            Some(serde_json::to_value(input).map_err(|_| "目录来源参数无效")?),
        )
        .await
}

#[tauri::command]
pub(crate) async fn commit_catalog_provider_operation(
    backend: tauri::State<'_, Backend>,
    input: ConnectionRequest,
) -> Result<serde_json::Value, String> {
    backend
        .request(
            reqwest::Method::POST,
            "/api/catalog-provider-operations",
            Some(serde_json::to_value(input).map_err(|_| "目录连接参数无效")?),
        )
        .await
}
#[tauri::command]
pub(crate) async fn get_catalog_provider_operation(
    backend: tauri::State<'_, Backend>,
    input: ConnectionRequest,
) -> Result<serde_json::Value, String> {
    backend
        .request(
            reqwest::Method::POST,
            "/api/catalog-provider-operations/query",
            Some(serde_json::to_value(input).map_err(|_| "目录连接查询无效")?),
        )
        .await
}

#[tauri::command]
pub(crate) async fn query_provider_catalog(
    backend: tauri::State<'_, Backend>,
    query: Query,
) -> Result<serde_json::Value, String> {
    backend
        .request(
            reqwest::Method::POST,
            "/api/provider-catalog/status",
            Some(serde_json::to_value(query).map_err(|_| "模型目录查询无效")?),
        )
        .await
}
#[tauri::command]
pub(crate) async fn perform_provider_catalog(
    backend: tauri::State<'_, Backend>,
    input: Request,
) -> Result<serde_json::Value, String> {
    backend
        .request(
            reqwest::Method::POST,
            "/api/provider-catalog/action",
            Some(serde_json::to_value(input).map_err(|_| "模型目录操作无效")?),
        )
        .await
}

#[tauri::command]
pub(crate) async fn perform_provider_source_operation(
    backend: tauri::State<'_, Backend>,
    input: local_agent_core::domain::provider_source_operation::SourceOperationAction,
) -> Result<serde_json::Value, String> {
    use local_agent_core::domain::provider_source_operation::SourceOperationAction;
    let (route, body) = match input {
        SourceOperationAction::Query { request } => (
            "/api/provider-source-operations/query",
            serde_json::to_value(request),
        ),
        commit => (
            "/api/provider-source-operations",
            serde_json::to_value(commit),
        ),
    };
    backend
        .request(
            reqwest::Method::POST,
            route,
            Some(body.map_err(|_| "来源操作参数无效")?),
        )
        .await
}
