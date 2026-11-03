use crate::backend::{Backend, query_path};
use local_agent_core::projects::ProjectQuery;
use tauri::Manager;
use tauri_plugin_dialog::DialogExt;

#[tauri::command]
pub(crate) async fn set_project_hidden(
    backend: tauri::State<'_, Backend>,
    project_id: String,
    hidden: bool,
) -> Result<serde_json::Value, String> {
    if !local_agent_core::projects::visibility::valid_id(&project_id) {
        return Err("项目 ID 无效".into());
    }
    backend
        .request(
            reqwest::Method::PUT,
            "/api/project-visibility",
            Some(serde_json::json!({"projectId":project_id,"hidden":hidden})),
        )
        .await
}

#[tauri::command]
pub(crate) async fn select_project_directory(
    app: tauri::AppHandle,
    backend: tauri::State<'_, Backend>,
    project_id: String,
) -> Result<bool, String> {
    if !project_id.starts_with("project:")
        || project_id.len() != 72
        || !project_id[8..].bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err("项目 ID 无效".into());
    }
    let project = backend
        .request(
            reqwest::Method::GET,
            &format!("/api/projects/{project_id}"),
            None,
        )
        .await?;
    let root = project["project"]["path"]
        .as_str()
        .ok_or("项目目录未知")?
        .to_string();
    if root == "/" || dirs_forbidden_home(&root) {
        return Err("请选择具体的项目目录，不为整个用户目录或磁盘申请访问".into());
    }
    let expected = std::path::PathBuf::from(&root)
        .canonicalize()
        .map_err(|_| "项目目录当前不可访问")?;
    let window = app.get_webview_window("main").ok_or("应用窗口不可用")?;
    let _ = window.set_focus();
    let selected = tokio::task::spawn_blocking(move || {
        app.dialog()
            .file()
            .set_title("选择当前项目目录以允许读取")
            .set_parent(&window)
            .set_directory(root)
            .set_can_create_directories(false)
            .blocking_pick_folder()
    })
    .await
    .map_err(|_| "目录选择未完成")?;
    let Some(selected) = selected else {
        return Ok(false);
    };
    let selected = selected
        .into_path()
        .map_err(|_| "请选择本机目录")?
        .canonicalize()
        .map_err(|_| "所选目录不可访问")?;
    if selected != expected {
        return Err("所选目录与当前项目不同，请重新选择当前项目目录".into());
    }
    Ok(true)
}
fn dirs_forbidden_home(root: &str) -> bool {
    std::env::var_os("HOME")
        .is_some_and(|home| std::path::Path::new(&home) == std::path::Path::new(root))
}
#[tauri::command]
pub(crate) async fn query_projects(
    backend: tauri::State<'_, Backend>,
    query: ProjectQuery,
) -> Result<serde_json::Value, String> {
    backend
        .request(
            reqwest::Method::GET,
            &query_path("/api/projects", query)?,
            None,
        )
        .await
}
#[tauri::command]
pub(crate) async fn get_project(
    backend: tauri::State<'_, Backend>,
    project_id: String,
    query: ProjectQuery,
) -> Result<serde_json::Value, String> {
    if !project_id.starts_with("project:")
        || project_id.len() != 72
        || !project_id[8..].bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err("项目 ID 无效".into());
    }
    backend
        .request(
            reqwest::Method::GET,
            &query_path(&format!("/api/projects/{project_id}"), query)?,
            None,
        )
        .await
}
#[tauri::command]
pub(crate) async fn get_project_diff(
    backend: tauri::State<'_, Backend>,
    project_id: String,
    file_id: String,
) -> Result<serde_json::Value, String> {
    if !project_id.starts_with("project:")
        || project_id.len() != 72
        || !project_id[8..].bytes().all(|b| b.is_ascii_hexdigit())
        || file_id.len() != 64
        || !file_id.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err("差异 ID 无效".into());
    }
    backend
        .request(
            reqwest::Method::GET,
            &format!("/api/projects/{project_id}/diff/{file_id}"),
            None,
        )
        .await
}
