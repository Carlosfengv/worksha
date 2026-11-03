use crate::backend::Backend;
use local_agent_core::plugin::policy::Action;
use tauri::Manager;
use tauri_plugin_dialog::DialogExt;
#[tauri::command]
pub(crate) async fn perform_routing_policy_recovery(
    backend: tauri::State<'_, Backend>,
    input: local_agent_core::domain::routing_policy_recovery::PolicyRecoveryAction,
) -> Result<serde_json::Value, String> {
    backend
        .request(
            reqwest::Method::POST,
            "/api/routing-policy-recovery",
            Some(serde_json::to_value(input).map_err(|_| "范围核查参数无效")?),
        )
        .await
}
#[tauri::command]
pub(crate) async fn perform_routing_policy(
    app: tauri::AppHandle,
    backend: tauri::State<'_, Backend>,
    input: Action,
) -> Result<serde_json::Value, String> {
    let mut input = input;
    if let Action::Inspect { path, .. } = &mut input {
        let window = app.get_webview_window("main").ok_or("应用窗口不可用")?;
        let selected = tokio::task::spawn_blocking(move || {
            app.dialog()
                .file()
                .set_parent(&window)
                .set_title("选择纯路由策略插件包")
                .add_filter("Local Agent 插件", &["laplug"])
                .blocking_pick_file()
        })
        .await
        .map_err(|_| "未完成包选择")?;
        let Some(selected) = selected else {
            return Ok(serde_json::Value::Null);
        };
        *path = selected
            .into_path()
            .map_err(|_| "请选择本机文件")?
            .to_string_lossy()
            .into_owned();
    }
    backend
        .request(
            reqwest::Method::POST,
            "/api/routing-policy",
            Some(serde_json::to_value(input).map_err(|_| "策略操作参数无效")?),
        )
        .await
}
