use crate::backend::{Backend, Diagnostics};
#[tauri::command]
pub(crate) async fn get_backend_diagnostics(
    backend: tauri::State<'_, Backend>,
) -> Result<Diagnostics, String> {
    backend.diagnostics()
}
#[tauri::command]
pub(crate) async fn reconnect_backend(
    app: tauri::AppHandle,
    backend: tauri::State<'_, Backend>,
) -> Result<Diagnostics, String> {
    let directory = local_agent_core::runtime::desktop_data_dir(&app.config().identifier)?;
    backend.reconnect(&directory).await
}
