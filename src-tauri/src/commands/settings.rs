use crate::backend::Backend;
use local_agent_core::{Provider, settings::SettingsUpdate};

#[tauri::command]
pub(crate) async fn preview_retention_inventory(
    backend: tauri::State<'_, Backend>,
    request: local_agent_core::observation::retention::impact::InventoryRequest,
) -> Result<serde_json::Value, String> {
    read_retention_inventory(&backend, request).await
}

async fn read_retention_inventory(
    backend: &Backend,
    request: local_agent_core::observation::retention::impact::InventoryRequest,
) -> Result<serde_json::Value, String> {
    if !request.valid() {
        return Err("留存预览参数无效".into());
    }
    // An old v1 service rejects unknown query fields. The default request must
    // retain its original wire shape; only v2 explicitly negotiates a format.
    let format = if request.inventory_version == 2 {
        "&inventoryVersion=2"
    } else {
        ""
    };
    backend
        .context()?
        .request(
            reqwest::Method::GET,
            &format!(
                "/api/settings/retention-inventory?storeId={}&expectedSettingsRevision={}&days={}{}",
                request.store_id, request.expected_settings_revision, request.days, format
            ),
            None,
        )
        .await
}

#[cfg(test)]
mod retention_tests;

#[tauri::command]
pub(crate) async fn preview_retention(
    backend: tauri::State<'_, Backend>,
    request: local_agent_core::observation::retention::impact::Request,
) -> Result<serde_json::Value, String> {
    if !request.valid() {
        return Err("留存预览参数无效".into());
    }
    backend
        .request(
            reqwest::Method::GET,
            &format!(
                "/api/settings/retention-preview?storeId={}&expectedSettingsRevision={}&days={}",
                request.store_id, request.expected_settings_revision, request.days
            ),
            None,
        )
        .await
}

#[tauri::command]
pub(crate) async fn get_snapshot(
    backend: tauri::State<'_, Backend>,
) -> Result<serde_json::Value, String> {
    backend
        .request(reqwest::Method::GET, "/api/snapshot", None)
        .await
}

#[tauri::command]
pub(crate) async fn save_settings(
    backend: tauri::State<'_, Backend>,
    settings: SettingsUpdate,
) -> Result<serde_json::Value, String> {
    backend
        .request(
            reqwest::Method::PUT,
            "/api/settings",
            Some(serde_json::to_value(settings).map_err(|_| "设置格式错误")?),
        )
        .await
}

#[tauri::command]
pub(crate) async fn get_settings_operation(
    backend: tauri::State<'_, Backend>,
    operation_id: String,
) -> Result<serde_json::Value, String> {
    uuid::Uuid::parse_str(&operation_id).map_err(|_| "设置操作编号无效")?;
    backend
        .request(
            reqwest::Method::GET,
            &format!("/api/settings/operations/{operation_id}"),
            None,
        )
        .await
}

#[tauri::command]
pub(crate) async fn save_provider(
    backend: tauri::State<'_, Backend>,
    provider: Provider,
) -> Result<serde_json::Value, String> {
    backend
        .request(
            reqwest::Method::PUT,
            "/api/providers",
            Some(serde_json::to_value(provider).map_err(|_| "服务商格式错误")?),
        )
        .await
}

#[tauri::command]
pub(crate) async fn get_scoped_settings_operation(
    backend: tauri::State<'_, Backend>,
    operation_id: String,
    store_id: String,
) -> Result<serde_json::Value, String> {
    uuid::Uuid::parse_str(&operation_id).map_err(|_| "设置操作编号无效")?;
    uuid::Uuid::parse_str(&store_id).map_err(|_| "设置数据库身份无效")?;
    backend
        .request(
            reqwest::Method::GET,
            &format!("/api/settings/scoped/operations/{operation_id}?storeId={store_id}"),
            None,
        )
        .await
}
