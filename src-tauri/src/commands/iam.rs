use crate::backend::Backend;
use reqwest::Method;
use serde_json::Value;
use tauri::Manager;
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};

#[tauri::command]
pub(crate) async fn get_iam_status(backend: tauri::State<'_, Backend>) -> Result<Value, String> {
    backend.request(Method::GET, "/api/iam/status", None).await
}

#[tauri::command]
pub(crate) async fn perform_iam(
    app: tauri::AppHandle,
    backend: tauri::State<'_, Backend>,
    input: Value,
) -> Result<Value, String> {
    let kind = input["kind"].as_str().ok_or("账号管理操作无效")?;
    let _transition = matches!(kind, "login" | "logout" | "initialize" | "recover")
        .then(|| backend.iam_transition())
        .transpose()?;
    let origin = backend.context()?;
    if kind == "initialize" {
        let window = app.get_webview_window("main").ok_or("应用窗口不可用")?;
        let confirmed = tokio::task::spawn_blocking(move || {
            app.dialog()
                .message("开启后，此实例始终要求账号登录，账号管理插件无法关闭或卸载。")
                .title("开启账号管理")
                .parent(&window)
                .kind(MessageDialogKind::Warning)
                .buttons(MessageDialogButtons::OkCancelCustom(
                    "创建并开启".into(),
                    "取消".into(),
                ))
                .blocking_show()
        })
        .await
        .map_err(|_| "未完成开启确认")?;
        if !confirmed {
            return Err(crate::backend::local_error(
                "cancelled",
                "已取消开启账号管理",
            ));
        }
    }
    let logout = kind == "logout";
    let login = kind == "login";
    let recover = kind == "recover";
    let response = origin.request(Method::POST, "/api/iam", Some(input)).await;
    if logout || (recover && response.is_ok()) {
        backend.set_iam_session(&origin, None)?;
    }
    let mut response = response?;
    if login {
        let token = response
            .as_object_mut()
            .and_then(|value| value.remove("session"))
            .and_then(|value| value.as_str().map(str::to_owned))
            .filter(|token| token.len() == 64 && token.bytes().all(|b| b.is_ascii_hexdigit()))
            .ok_or("账号会话无法建立")?;
        backend.set_iam_session(&origin, Some(token))?;
    }
    Ok(response)
}

pub(crate) fn guard_invocations(
    handler: impl Fn(tauri::ipc::Invoke) -> bool + Send + Sync + 'static,
) -> impl Fn(tauri::ipc::Invoke) -> bool + Send + Sync + 'static {
    use std::sync::Arc;
    let handler = Arc::new(handler);
    move |invoke| {
        if matches!(
            invoke.message.command(),
            "get_iam_status" | "perform_iam" | "get_backend_diagnostics" | "reconnect_backend"
            // These commands only forward to the authenticated management routes.
            | "get_builtin_plugins" | "configure_builtin_plugins" | "get_builtin_plugin_operation"
        ) {
            return handler(invoke);
        }
        let webview = invoke.message.webview();
        let handler = handler.clone();
        tauri::async_runtime::spawn(async move {
            let Some(backend) = webview.try_state::<Backend>() else {
                invoke.resolver.reject("尚未连接本地后台");
                return;
            };
            match backend
                .request(Method::GET, "/api/iam/admission", None)
                .await
            {
                Ok(reply) if reply["allowed"].as_bool() == Some(true) => {}
                Ok(_) => {
                    invoke.resolver.reject(crate::backend::local_error(
                        "iam_unavailable",
                        "无法确认账号操作权限",
                    ));
                    return;
                }
                Err(error) => {
                    invoke.resolver.reject(error);
                    return;
                }
            }
            let resolver = invoke.resolver.clone();
            if !handler(invoke) {
                resolver.reject("未注册的桌面命令");
            }
        });
        true
    }
}
