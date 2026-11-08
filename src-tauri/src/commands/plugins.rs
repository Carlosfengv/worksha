//! Native user selections never accept a renderer-supplied filesystem path.
use crate::backend::Backend;
use local_agent_core::plugin::external::{ActionRequest, InstallRequest, StateRequest, ViewQuery};
use tauri::Manager;
use tauri::path::BaseDirectory;
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_opener::OpenerExt;

#[tauri::command]
pub(crate) async fn get_builtin_plugins(
    backend: tauri::State<'_, Backend>,
) -> Result<serde_json::Value, String> {
    backend
        .request(reqwest::Method::GET, "/api/builtin-plugins", None)
        .await
}
#[tauri::command]
pub(crate) async fn configure_builtin_plugins(
    backend: tauri::State<'_, Backend>,
    input: local_agent_core::plugin::builtin::CompositionChange,
) -> Result<serde_json::Value, String> {
    backend
        .request(
            reqwest::Method::POST,
            "/api/builtin-plugins/changes",
            Some(serde_json::to_value(input).map_err(|_| "内置组合参数无效")?),
        )
        .await
}
#[tauri::command]
pub(crate) async fn get_builtin_plugin_operation(
    backend: tauri::State<'_, Backend>,
    query: local_agent_core::plugin::builtin::CompositionOperation,
) -> Result<serde_json::Value, String> {
    backend
        .request(
            reqwest::Method::POST,
            "/api/builtin-plugins/operations",
            Some(serde_json::to_value(query).map_err(|_| "组合操作查询参数无效")?),
        )
        .await
}

#[tauri::command]
pub(crate) async fn list_plugins(
    backend: tauri::State<'_, Backend>,
) -> Result<serde_json::Value, String> {
    backend
        .request(reqwest::Method::GET, "/api/plugins", None)
        .await
}
#[tauri::command]
pub(crate) async fn inspect_plugin_upload(
    app: tauri::AppHandle,
    backend: tauri::State<'_, Backend>,
) -> Result<serde_json::Value, String> {
    let window = app.get_webview_window("main").ok_or("应用窗口不可用")?;
    let selected = tokio::task::spawn_blocking(move || {
        app.dialog()
            .file()
            .set_parent(&window)
            .set_title("上传插件：检查兼容性、来源与权限")
            .add_filter("Local Agent 插件", &["laplug"])
            .blocking_pick_file()
    })
    .await
    .map_err(|_| "未完成包选择")?;
    let Some(selected) = selected else {
        return Ok(serde_json::Value::Null);
    };
    let path = selected.into_path().map_err(|_| "请选择本机文件")?;
    backend
        .request(
            reqwest::Method::POST,
            "/api/plugins/upload/inspect",
            Some(serde_json::json!({"path":path})),
        )
        .await
}

#[tauri::command]
pub(crate) async fn inspect_bundled_dsh_package(
    app: tauri::AppHandle,
    backend: tauri::State<'_, Backend>,
) -> Result<serde_json::Value, String> {
    let path = app
        .path()
        .resolve("dsh/dsh-desktop.laplug", BaseDirectory::Resource)
        .map_err(|_| "无法定位随附的 DSH 来源包")?;
    if !path.is_file() {
        return Err("当前应用缺少随附的 DSH 来源包，请重新安装应用。".into());
    }
    let review = backend
        .request(
            reqwest::Method::POST,
            "/api/plugins/upload/inspect",
            Some(serde_json::json!({"path":path})),
        )
        .await?;
    if review["kind"] != "source" || review["review"]["manifest"]["id"] != "net.worksha.dsh-desktop"
    {
        return Err("随附的 DSH 来源包与应用声明不符。".into());
    }
    Ok(review)
}

#[tauri::command]
pub(crate) async fn open_bundled_dsh_bridges(app: tauri::AppHandle) -> Result<(), String> {
    let path = app
        .path()
        .resolve("dsh", BaseDirectory::Resource)
        .map_err(|_| "无法定位随附的 DSH 桥接")?;
    if !path.join("bridge/package.json").is_file()
        || !path.join("connection-bridge/package.json").is_file()
    {
        return Err("当前应用缺少随附的 DSH 桥接，请重新安装应用。".into());
    }
    app.opener()
        .open_path(path.to_string_lossy(), None::<&str>)
        .map_err(|_| "无法打开随附的 DSH 桥接文件夹".into())
}
#[tauri::command]
pub(crate) async fn inspect_plugin_package(
    app: tauri::AppHandle,
    backend: tauri::State<'_, Backend>,
) -> Result<serde_json::Value, String> {
    let window = app.get_webview_window("main").ok_or("应用窗口不可用")?;
    let selected = tokio::task::spawn_blocking(move || {
        app.dialog()
            .file()
            .set_parent(&window)
            .set_title("选择本地插件包以检查来源与权限")
            .add_filter("Local Agent 插件", &["laplug"])
            .blocking_pick_file()
    })
    .await
    .map_err(|_| "未完成包选择")?;
    let Some(selected) = selected else {
        return Ok(serde_json::Value::Null);
    };
    let selected = selected.into_path().map_err(|_| "请选择本机文件")?;
    backend
        .request(
            reqwest::Method::POST,
            "/api/plugins/inspect",
            Some(serde_json::json!({"path":selected})),
        )
        .await
}
#[tauri::command]
pub(crate) async fn install_plugin_package(
    backend: tauri::State<'_, Backend>,
    input: InstallRequest,
) -> Result<serde_json::Value, String> {
    backend
        .request(
            reqwest::Method::POST,
            "/api/plugins/install",
            Some(serde_json::to_value(input).map_err(|_| "插件安装参数无效")?),
        )
        .await
}
#[tauri::command]
pub(crate) async fn change_plugin_state(
    backend: tauri::State<'_, Backend>,
    input: StateRequest,
) -> Result<serde_json::Value, String> {
    backend
        .request(
            reqwest::Method::POST,
            "/api/plugins/state",
            Some(serde_json::to_value(input).map_err(|_| "插件状态参数无效")?),
        )
        .await
}
#[tauri::command]
pub(crate) async fn authorize_plugin_source(
    app: tauri::AppHandle,
    backend: tauri::State<'_, Backend>,
    plugin_id: String,
    digest: String,
) -> Result<serde_json::Value, String> {
    let plugins = backend
        .request(reqwest::Method::GET, "/api/plugins", None)
        .await?;
    let valid = plugins["items"].as_array().is_some_and(|items| {
        items.iter().any(|p| {
            p["manifest"]["id"] == plugin_id
                && p["digest"] == digest
                && p["enabled"] == true
                && p["manifest"]["capabilities"]
                    .as_array()
                    .is_some_and(|caps| caps.iter().any(|v| v == "session.source@1"))
        })
    });
    if !valid {
        return Err("请先启用对应的只读来源插件".into());
    }
    let window = app.get_webview_window("main").ok_or("应用窗口不可用")?;
    let selected = tokio::task::spawn_blocking(move || {
        app.dialog()
            .file()
            .set_parent(&window)
            .set_title("授权此插件读取所选 JSONL（包含全部正文）")
            .add_filter("JSONL 会话文件", &["jsonl"])
            .blocking_pick_file()
    })
    .await
    .map_err(|_| "未完成来源选择")?;
    let Some(selected) = selected else {
        return Ok(serde_json::Value::Null);
    };
    let selected = selected.into_path().map_err(|_| "请选择本机会话文件")?;
    backend
        .request(
            reqwest::Method::POST,
            "/api/plugins/source",
            Some(serde_json::json!({"pluginId":plugin_id,"digest":digest,"path":selected})),
        )
        .await
}
#[tauri::command]
pub(crate) async fn query_plugin_view(
    backend: tauri::State<'_, Backend>,
    query: ViewQuery,
) -> Result<serde_json::Value, String> {
    backend
        .request(
            reqwest::Method::POST,
            "/api/plugins/view",
            Some(serde_json::to_value(query).map_err(|_| "插件查询参数无效")?),
        )
        .await
}
#[tauri::command]
pub(crate) async fn perform_plugin_action(
    backend: tauri::State<'_, Backend>,
    input: ActionRequest,
) -> Result<serde_json::Value, String> {
    backend
        .request(
            reqwest::Method::POST,
            "/api/plugins/action",
            Some(serde_json::to_value(input).map_err(|_| "插件操作参数无效")?),
        )
        .await
}

#[tauri::command]
pub(crate) async fn query_plugin_extension(
    backend: tauri::State<'_, Backend>,
    query: local_agent_core::plugin::advanced::Query,
) -> Result<serde_json::Value, String> {
    backend
        .request(
            reqwest::Method::POST,
            "/api/plugin-extensions/status",
            Some(serde_json::to_value(query).map_err(|_| "扩展查询参数无效")?),
        )
        .await
}
#[tauri::command]
pub(crate) async fn perform_plugin_extension(
    backend: tauri::State<'_, Backend>,
    input: local_agent_core::plugin::advanced::Request,
) -> Result<serde_json::Value, String> {
    backend
        .request(
            reqwest::Method::POST,
            "/api/plugin-extensions/action",
            Some(serde_json::to_value(input).map_err(|_| "扩展操作参数无效")?),
        )
        .await
}
#[tauri::command]
pub(crate) async fn authorize_plugin_resource(
    app: tauri::AppHandle,
    backend: tauri::State<'_, Backend>,
    query: local_agent_core::plugin::advanced::Query,
) -> Result<serde_json::Value, String> {
    let input = serde_json::to_value(&query).map_err(|_| "扩展授权参数无效")?;
    let policy = backend
        .request(
            reqwest::Method::POST,
            "/api/plugin-extensions/status",
            Some(input.clone()),
        )
        .await?;
    let config = policy["policy"]["config"].is_object();
    if !config && !policy["policy"]["connector"].is_string() {
        return Err("此插件未声明文件或连接配置能力".into());
    }
    let window = app.get_webview_window("main").ok_or("应用窗口不可用")?;
    let selected = tokio::task::spawn_blocking(move || {
        app.dialog()
            .file()
            .set_parent(&window)
            .set_title(if config {
                "授权所选 JSON 配置文件的预览、确认修改与恢复"
            } else {
                "授权读取所选 MCP 连接文件；stdio 服务以当前用户权限运行"
            })
            .add_filter("JSON 文件", &["json"])
            .blocking_pick_file()
    })
    .await
    .map_err(|_| "未完成文件选择")?;
    let Some(selected) = selected else {
        return Ok(serde_json::Value::Null);
    };
    let path = selected.into_path().map_err(|_| "请选择本机文件")?;
    backend
        .request(
            reqwest::Method::POST,
            "/api/plugin-extensions/resource",
            Some(serde_json::json!({"pluginId":query.plugin_id,"digest":query.digest,"path":path})),
        )
        .await
}
#[tauri::command]
pub(crate) async fn export_plugin_connector(
    app: tauri::AppHandle,
    backend: tauri::State<'_, Backend>,
    query: local_agent_core::plugin::advanced::Query,
) -> Result<serde_json::Value, String> {
    let window = app.get_webview_window("main").ok_or("应用窗口不可用")?;
    let selected = tokio::task::spawn_blocking(move || {
        app.dialog()
            .file()
            .set_parent(&window)
            .set_title("保存 MCP 连接（包含本机访问凭据，请仅交给可信客户端）")
            .set_file_name("local-agent-mcp.json")
            .blocking_save_file()
    })
    .await
    .map_err(|_| "未完成保存位置选择")?;
    let Some(selected) = selected else {
        return Ok(serde_json::Value::Null);
    };
    let path = selected.into_path().map_err(|_| "请选择本机保存位置")?;
    let value = backend
        .request(
            reqwest::Method::POST,
            "/api/plugin-extensions/export",
            Some(serde_json::to_value(query).map_err(|_| "连接导出参数无效")?),
        )
        .await?;
    local_agent_core::providers::files::atomic_write(
        &path,
        &serde_json::to_vec_pretty(&value).map_err(|_| "连接导出格式无效")?,
        0o600,
    )
    .map_err(|_| "连接文件保存未完成")?;
    Ok(serde_json::json!({"saved":true}))
}
