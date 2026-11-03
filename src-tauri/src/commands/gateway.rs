use crate::backend::Backend;
use tauri::Manager;
use tauri_plugin_dialog::DialogExt;
mod operations;
mod save;
mod scoped_save;
mod status;

#[tauri::command]
pub(crate) async fn get_gateway_client_operation(
    backend: tauri::State<'_, Backend>,
    operation_id: String,
    store_id: String,
) -> Result<serde_json::Value, String> {
    operations::lookup(&backend.context()?, &operation_id, &store_id).await
}
#[tauri::command]
pub(crate) async fn commit_gateway_client_operation(
    backend: tauri::State<'_, Backend>,
    request: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let request: operations::Request = serde_json::from_value(request)
        .map_err(|_| operations::failure("invalid_input", false, "客户端撤销原请求不可读取"))?;
    let outcome = operations::revoke(&backend.context()?, &request, false).await?;
    serde_json::to_value(outcome).map_err(|_| operations::unknown())
}
#[tauri::command]
pub(crate) async fn save_gateway_client_operation(
    app: tauri::AppHandle,
    backend: tauri::State<'_, Backend>,
    input: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let input = scoped_save::Input::parse(input)?;
    let context = backend.context()?;
    let window = app.get_webview_window("main").ok_or("应用窗口不可用")?;
    let selected = tokio::task::spawn_blocking(move || {
        app.dialog()
            .file()
            .set_parent(&window)
            .set_title("保存客户端连接文件（包含此客户端密钥）")
            .add_filter("JSON 文件", &["json"])
            .set_file_name("agent-gateway-access.json")
            .blocking_save_file()
    })
    .await
    .map_err(|_| "选择保存位置失败")?;
    let target = selected
        .map(|s| s.into_path().map_err(|_| "请选择本机文件位置"))
        .transpose()?;
    scoped_save::create_at(&context, input, target).await
}

#[tauri::command]
pub(crate) async fn get_gateway_status(
    backend: tauri::State<'_, Backend>,
) -> Result<serde_json::Value, String> {
    status::read(&backend.context()?).await
}

#[tauri::command]
pub(crate) async fn create_gateway_client(
    app: tauri::AppHandle,
    backend: tauri::State<'_, Backend>,
    input: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let context = backend.context()?;
    let window = app.get_webview_window("main").ok_or("应用窗口不可用")?;
    let selected = tokio::task::spawn_blocking(move || {
        app.dialog()
            .file()
            .set_parent(&window)
            .set_title("保存客户端连接文件（包含此客户端密钥）")
            .add_filter("JSON 文件", &["json"])
            .set_file_name("agent-gateway-access.json")
            .blocking_save_file()
    })
    .await
    .map_err(|_| "选择保存位置失败")?;
    let target = selected
        .map(|s| s.into_path().map_err(|_| "请选择本机文件位置"))
        .transpose()?;
    save::create_at(&context, input, target).await
}

#[tauri::command]
pub(crate) async fn revoke_gateway_client(
    backend: tauri::State<'_, Backend>,
    id: String,
) -> Result<serde_json::Value, String> {
    uuid::Uuid::parse_str(&id).map_err(|_| "客户端 ID 无效")?;
    backend
        .request(
            reqwest::Method::POST,
            &format!("/api/gateway-lan/clients/{id}/revoke"),
            None,
        )
        .await
}
