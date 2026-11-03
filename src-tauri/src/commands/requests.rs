use crate::backend::{Backend, query_path};
use local_agent_core::requests::Query;
use tauri::Manager;
use tauri_plugin_dialog::DialogExt;
mod save;

#[tauri::command]
pub(crate) async fn query_request_dashboard(
    backend: tauri::State<'_, Backend>,
    query: Query,
) -> Result<serde_json::Value, String> {
    query.validate()?;
    backend
        .request(
            reqwest::Method::GET,
            &query_path("/api/request-dashboard", query)?,
            None,
        )
        .await
}
#[tauri::command]
pub(crate) async fn get_request_detail(
    backend: tauri::State<'_, Backend>,
    id: String,
) -> Result<serde_json::Value, String> {
    if id.is_empty() || id.len() > 1024 {
        return Err("请求 ID 无效".into());
    }
    backend
        .request(
            reqwest::Method::GET,
            &query_path("/api/request-detail", serde_json::json!({"id":id}))?,
            None,
        )
        .await
}
#[tauri::command]
pub(crate) async fn export_request_csv(
    app: tauri::AppHandle,
    backend: tauri::State<'_, Backend>,
    snapshot: String,
) -> Result<serde_json::Value, String> {
    if snapshot.len() > 100 {
        return Err("快照 ID 无效".into());
    }
    let context = backend.context()?;
    let Some(target) = select_export_target(app, "导出当前筛选的请求", "requests.csv").await?
    else {
        return Ok(serde_json::json!({"status":"cancelled","count":0}));
    };
    save::export_to(&context, snapshot, target).await
}
#[tauri::command]
pub(crate) async fn export_unified_request_csv(
    app: tauri::AppHandle,
    backend: tauri::State<'_, Backend>,
    snapshot_token: String,
) -> Result<serde_json::Value, String> {
    if snapshot_token.is_empty() || snapshot_token.len() > 80 {
        return Err("统一快照 ID 无效".into());
    }
    let context = backend.context()?;
    let Some(target) =
        select_export_target(app, "导出当前冻结范围的全部请求", "unified-requests.csv").await?
    else {
        return Ok(serde_json::json!(
            local_agent_core::domain::requests::export::SaveReply::Cancelled {}
        ));
    };
    save::export_unified_to(&context, snapshot_token, target).await
}
async fn select_export_target(
    app: tauri::AppHandle,
    title: &'static str,
    name: &'static str,
) -> Result<Option<std::path::PathBuf>, String> {
    let window = app.get_webview_window("main").ok_or("应用窗口不可用")?;
    let target = tokio::task::spawn_blocking(move || {
        let selected = app
            .dialog()
            .file()
            .set_parent(&window)
            .set_title(title)
            .add_filter("CSV 文件", &["csv"])
            .set_file_name(name)
            .blocking_save_file();
        selected
            .map(|selected| {
                selected
                    .into_path()
                    .map_err(|_| "请选择本机文件位置".to_string())
            })
            .transpose()
    })
    .await
    .map_err(|_| "保存位置选择未完成")??;
    Ok(target)
}
