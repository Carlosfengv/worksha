use crate::backend::{Backend, ConnectionContext, query_path};
use reqwest::Method;
use serde_json::{Value, json};
use tauri::Manager;
use tauri_plugin_dialog::DialogExt;

#[tauri::command]
pub(crate) async fn find_workbench_session(
    backend: tauri::State<'_, Backend>,
    session_id: String,
) -> Result<Value, String> {
    let query = query_path("/api/workbench/session", json!({"sessionId":session_id}))?;
    backend.request(Method::GET, &query, None).await
}

#[tauri::command]
pub(crate) async fn get_workbench_routes(
    backend: tauri::State<'_, Backend>,
    agent: Option<String>,
) -> Result<Value, String> {
    let query = query_path("/api/workbench/routes", json!({"agent":agent}))?;
    backend.request(Method::GET, &query, None).await
}

#[tauri::command]
pub(crate) async fn read_workbench_tasks(
    backend: tauri::State<'_, Backend>,
    task_id: Option<String>,
    revision: Option<u64>,
) -> Result<Value, String> {
    let query = query_path(
        "/api/workbench/tasks",
        json!({"taskId":task_id,"revision":revision}),
    )?;
    backend.request(Method::GET, &query, None).await
}

#[tauri::command]
pub(crate) async fn read_workbench_operation(
    backend: tauri::State<'_, Backend>,
    kind: String,
    operation_id: String,
    store_id: String,
) -> Result<Value, String> {
    let query = query_path(
        "/api/workbench/operation",
        json!({"kind":kind,"operationId":operation_id,"storeId":store_id}),
    )?;
    backend.context()?.request(Method::GET, &query, None).await
}

async fn select_task_directory(
    app: tauri::AppHandle,
    context: &ConnectionContext,
    project_id: Option<String>,
    agent: Option<String>,
    parent_task_id: Option<String>,
) -> Result<Option<std::path::PathBuf>, String> {
    if context.persistent() {
        return Err("任务工作台需要桌面应用自带的本地服务".into());
    }
    let agent = agent.as_deref().unwrap_or("opencode");
    let agent_name = match agent {
        "opencode" => "OpenCode",
        "claude" => "Claude Code",
        _ => return Err("未知任务 Agent".into()),
    };
    let routes = query_path("/api/workbench/routes", json!({"agent":agent}))?;
    context.request(Method::GET, &routes, None).await?;
    let expected = if let Some(id) = parent_task_id.as_ref() {
        uuid::Uuid::parse_str(id).map_err(|_| "交接任务编号无效")?;
        let query = query_path("/api/workbench/tasks", json!({"taskId":id}))?;
        let parent = context.request(Method::GET, &query, None).await?;
        let path = parent["task"]["directory"]
            .as_str()
            .ok_or("原任务目录不可用")?;
        Some(
            std::path::Path::new(path)
                .canonicalize()
                .map_err(|_| "原任务目录不可访问")?,
        )
    } else if let Some(id) = project_id {
        if !id.starts_with("project:")
            || id.len() != 72
            || !id[8..].bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err("项目 ID 无效".into());
        }
        let project = context
            .request(Method::GET, &format!("/api/projects/{id}"), None)
            .await?;
        let path = project["project"]["path"].as_str().ok_or("项目目录未知")?;
        Some(
            std::path::Path::new(path)
                .canonicalize()
                .map_err(|_| "项目目录不可访问")?,
        )
    } else {
        None
    };
    let window = app.get_webview_window("main").ok_or("应用窗口不可用")?;
    let _ = window.set_focus();
    let start = expected.clone();
    let selected = tokio::task::spawn_blocking(move || {
        let dialog = app
            .dialog()
            .file()
            .set_title(format!("选择目录并允许 {agent_name} 在此执行任务"))
            .set_parent(&window)
            .set_can_create_directories(false);
        match start {
            Some(path) => dialog.set_directory(path).blocking_pick_folder(),
            None => dialog.blocking_pick_folder(),
        }
    })
    .await
    .map_err(|_| "未能打开项目目录选择器")?;
    let Some(selected) = selected else {
        return Ok(None);
    };
    let directory = selected
        .into_path()
        .map_err(|_| "请选择本机目录")?
        .canonicalize()
        .map_err(|_| "所选目录不可访问")?;
    if expected.as_ref().is_some_and(|path| path != &directory) {
        return Err("所选目录与当前项目不同，请重新选择".into());
    }
    context.ensure_current()?;
    Ok(Some(directory))
}

#[tauri::command]
pub(crate) async fn create_workbench_task(
    app: tauri::AppHandle,
    task_id: String,
    project_id: Option<String>,
    route: String,
    agent: Option<String>,
    parent_task_id: Option<String>,
    scope: Option<Value>,
) -> Result<Value, String> {
    let context = app.state::<Backend>().context()?;
    let Some(directory) = select_task_directory(
        app,
        &context,
        project_id,
        agent.clone(),
        parent_task_id.clone(),
    )
    .await?
    else {
        return Ok(Value::Null);
    };
    let agent = agent.as_deref().unwrap_or("opencode");
    context
        .request(
            Method::POST,
            "/api/workbench/tasks",
            Some(json!({
                "taskId":task_id, "directory":directory, "route":route, "agent":agent,
                "parentTaskId":parent_task_id, "scope":scope
            })),
        )
        .await
}

#[tauri::command]
pub(crate) async fn send_workbench_message(
    backend: tauri::State<'_, Backend>,
    task_id: String,
    turn_id: String,
    text: String,
    scope: Option<Value>,
    expected_revision: Option<u64>,
) -> Result<Value, String> {
    backend
        .context()?
        .request(
            Method::POST,
            "/api/workbench/send",
            Some(json!({
                "taskId":task_id, "turnId":turn_id, "text":text,
                "scope":scope,"expectedRevision":expected_revision
            })),
        )
        .await
}

#[tauri::command]
pub(crate) async fn control_workbench_turn(
    backend: tauri::State<'_, Backend>,
    action: Value,
) -> Result<Value, String> {
    backend
        .request(Method::POST, "/api/workbench/control", Some(action))
        .await
}

#[tauri::command]
pub(crate) async fn open_workbench_desktop(
    backend: tauri::State<'_, Backend>,
    agent: String,
    handoff: String,
) -> Result<Value, String> {
    use std::{path::PathBuf, process::Stdio};
    use tokio::{io::AsyncWriteExt, process::Command, time};
    let name = match agent.as_str() {
        "opencode" => "OpenCode.app",
        "claude" => "Claude.app",
        _ => return Err("未知桌面客户端".into()),
    };
    if handoff.trim().is_empty() || handoff.len() > 24 * 1024 {
        return Err("交接内容为空或超过 24 KB".into());
    }
    let routes = query_path("/api/workbench/routes", json!({"agent":agent}))?;
    let context = backend.context()?;
    context.request(Method::GET, &routes, None).await?;
    context.ensure_current()?;
    let path = PathBuf::from("/Applications").join(name);
    if !path.join("Contents/Info.plist").is_file() {
        return Err(format!("未在「应用程序」中找到 {name}"));
    }
    let result = async {
        let mut clipboard = Command::new("/usr/bin/pbcopy")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|_| "无法打开系统剪贴板")?;
        let mut input = clipboard.stdin.take().ok_or("无法打开系统剪贴板")?;
        input
            .write_all(handoff.as_bytes())
            .await
            .map_err(|_| "无法复制交接内容")?;
        drop(input);
        if !clipboard.wait().await.is_ok_and(|status| status.success()) {
            return Err("无法复制交接内容");
        }
        let status = Command::new("/usr/bin/open")
            .arg("-a")
            .arg(path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .status()
            .await
            .map_err(|_| "已复制交接内容，桌面客户端启动失败")?;
        if !status.success() {
            return Err("已复制交接内容，桌面客户端启动失败");
        }
        Ok(json!({"opened":true}))
    };
    time::timeout(std::time::Duration::from_secs(8), result)
        .await
        .map_err(|_| "桌面客户端打开结果待确认".to_string())?
        .map_err(str::to_owned)
}

#[tauri::command]
pub(crate) async fn export_workbench_opencode(
    app: tauri::AppHandle,
    backend: tauri::State<'_, Backend>,
    task_id: String,
) -> Result<bool, String> {
    uuid::Uuid::parse_str(&task_id).map_err(|_| "任务编号无效")?;
    let context = backend.context()?;
    let query = query_path("/api/workbench/tasks", json!({"taskId":task_id}))?;
    context.request(Method::GET, &query, None).await?;
    let window = app.get_webview_window("main").ok_or("应用窗口不可用")?;
    let _ = window.set_focus();
    let selected = tokio::task::spawn_blocking(move || {
        app.dialog()
            .file()
            .set_parent(&window)
            .set_title("保存 OpenCode 会话副本")
            .set_file_name("opencode-session.json")
            .add_filter("OpenCode JSON", &["json"])
            .blocking_save_file()
    })
    .await
    .map_err(|_| "导出位置选择未完成")?;
    let Some(selected) = selected else {
        return Ok(false);
    };
    let path = selected.into_path().map_err(|_| "请选择本机保存位置")?;
    let value = context
        .request(
            Method::POST,
            "/api/workbench/export-opencode",
            Some(json!({"taskId":task_id})),
        )
        .await?;
    let bytes = serde_json::to_vec_pretty(&value).map_err(|_| "无法序列化 OpenCode 导出文件")?;
    tokio::task::spawn_blocking(move || {
        local_agent_core::providers::files::atomic_write(&path, &bytes, 0o600)
            .map_err(|error| error.message)
    })
    .await
    .map_err(|_| "会话文件未保存")??;
    Ok(true)
}

#[tauri::command]
pub(crate) async fn read_workbench_live(
    backend: tauri::State<'_, Backend>,
) -> Result<Value, String> {
    backend
        .request(Method::GET, "/api/workbench/live", None)
        .await
}

#[tauri::command]
pub(crate) async fn read_workbench_live_board(
    backend: tauri::State<'_, Backend>,
) -> Result<Value, String> {
    backend
        .request(Method::GET, "/api/workbench/live-board", None)
        .await
}

#[tauri::command]
pub(crate) async fn control_workbench_live(
    backend: tauri::State<'_, Backend>,
    action: Value,
) -> Result<Value, String> {
    backend
        .request(Method::POST, "/api/workbench/live", Some(action))
        .await
}

#[tauri::command]
pub(crate) async fn get_workbench_shared_routes(
    backend: tauri::State<'_, Backend>,
) -> Result<Value, String> {
    backend
        .request(Method::GET, "/api/workbench/shared-routes", None)
        .await
}

#[tauri::command]
pub(crate) async fn create_workbench_shared(
    app: tauri::AppHandle,
    task_id: String,
    project_id: Option<String>,
    route: String,
    text: String,
    scope: Option<Value>,
    connection_id: Option<String>,
) -> Result<Value, String> {
    uuid::Uuid::parse_str(&task_id).map_err(|_| "任务编号无效")?;
    let context = app.state::<Backend>().context()?;
    let Some(directory) =
        select_task_directory(app, &context, project_id, Some("opencode".into()), None).await?
    else {
        return Ok(Value::Null);
    };
    context
        .request(
            Method::POST,
            "/api/workbench/shared",
            Some(json!({
                "taskId":task_id,"directory":directory,"route":route,"text":text,
                "scope":scope,"connectionId":connection_id
            })),
        )
        .await
}
