//! Typed domain queries are additive; the original 96 commands keep their scope.
use crate::backend::Backend;
use local_agent_core::domain::SourceConsentQuery;
use local_agent_core::domain::{SourceConsent, UnifiedConversationRequest, UnifiedSessionQuery};
use tauri::Manager;
use tauri_plugin_dialog::DialogExt;

#[tauri::command]
pub(crate) async fn query_unified_requests(
    backend: tauri::State<'_, Backend>,
    query: local_agent_core::domain::requests::query::UnifiedRequestQuery,
) -> Result<serde_json::Value, String> {
    backend
        .request(
            reqwest::Method::POST,
            "/api/domain/requests",
            Some(serde_json::to_value(query).map_err(|_| "请求查询参数无效")?),
        )
        .await
}

#[tauri::command]
pub(crate) async fn query_unified_activity(
    backend: tauri::State<'_, Backend>,
    query: local_agent_core::domain::activity::query::UnifiedActivityQuery,
) -> Result<serde_json::Value, String> {
    backend
        .request(
            reqwest::Method::POST,
            "/api/domain/activity",
            Some(serde_json::to_value(query).map_err(|_| "活动查询参数无效")?),
        )
        .await
}

#[tauri::command]
pub(crate) async fn get_fact_conversation(
    backend: tauri::State<'_, Backend>,
    query: UnifiedConversationRequest,
) -> Result<serde_json::Value, String> {
    backend
        .request(
            reqwest::Method::POST,
            "/api/domain/fact-conversation",
            Some(serde_json::to_value(query).map_err(|_| "事实正文参数无效")?),
        )
        .await
}

#[tauri::command]
pub(crate) async fn set_domain_project_visibility(
    backend: tauri::State<'_, Backend>,
    input: local_agent_core::domain::facts::query::ProjectVisibility,
) -> Result<serde_json::Value, String> {
    backend
        .request(
            reqwest::Method::POST,
            "/api/domain/project-visibility",
            Some(serde_json::to_value(input).map_err(|_| "项目显示参数无效")?),
        )
        .await
}

#[tauri::command]
pub(crate) async fn query_unified_facts(
    backend: tauri::State<'_, Backend>,
    query: local_agent_core::domain::facts::query::FactQuery,
) -> Result<serde_json::Value, String> {
    backend
        .request(
            reqwest::Method::POST,
            "/api/domain/facts",
            Some(serde_json::to_value(query).map_err(|_| "事实查询参数无效")?),
        )
        .await
}
#[tauri::command]
pub(crate) async fn list_full_plugins(
    backend: tauri::State<'_, Backend>,
) -> Result<serde_json::Value, String> {
    backend
        .request(reqwest::Method::GET, "/api/plugin-sources", None)
        .await
}
#[tauri::command]
pub(crate) async fn inspect_full_plugin_package(
    app: tauri::AppHandle,
    backend: tauri::State<'_, Backend>,
) -> Result<serde_json::Value, String> {
    let window = app.get_webview_window("main").ok_or("应用窗口不可用")?;
    let selected = tokio::task::spawn_blocking(move || {
        app.dialog()
            .file()
            .set_parent(&window)
            .set_title("选择完整只读来源插件包")
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
            "/api/plugin-sources/inspect",
            Some(serde_json::json!({"path":path})),
        )
        .await
}
#[tauri::command]
pub(crate) async fn install_full_plugin_package(
    backend: tauri::State<'_, Backend>,
    input: local_agent_core::plugin::full::Install,
) -> Result<serde_json::Value, String> {
    backend
        .request(
            reqwest::Method::POST,
            "/api/plugin-sources/install",
            Some(serde_json::to_value(input).map_err(|_| "插件安装参数无效")?),
        )
        .await
}
#[tauri::command]
pub(crate) async fn change_full_plugin_state(
    backend: tauri::State<'_, Backend>,
    input: local_agent_core::plugin::full::StateChange,
) -> Result<serde_json::Value, String> {
    backend
        .request(
            reqwest::Method::POST,
            "/api/plugin-sources/state",
            Some(serde_json::to_value(input).map_err(|_| "插件状态参数无效")?),
        )
        .await
}

#[tauri::command]
pub(crate) async fn refresh_domain_source(
    backend: tauri::State<'_, Backend>,
    query: SourceConsentQuery,
) -> Result<serde_json::Value, String> {
    backend
        .request(
            reqwest::Method::POST,
            "/api/domain/source-refresh",
            Some(serde_json::to_value(query).map_err(|_| "来源参数无效")?),
        )
        .await
}
#[tauri::command]
pub(crate) async fn authorize_domain_source(
    app: tauri::AppHandle,
    backend: tauri::State<'_, Backend>,
    query: SourceConsentQuery,
) -> Result<serde_json::Value, String> {
    let catalog = backend
        .request(reqwest::Method::GET, "/api/domain/directory", None)
        .await?;
    let active = catalog["catalog"]["providers"]
        .as_array()
        .is_some_and(|providers| {
            providers.iter().any(|p| {
                p["pluginId"] == query.plugin_id
                    && p["implementationIdentity"] == query.digest
                    && p["executionKind"] != "builtin"
                    && catalog["catalog"]["sources"]
                        .as_array()
                        .is_some_and(|sources| {
                            sources.iter().any(|s| {
                                s["providerKey"] == p["providerKey"]
                                    && s["status"]["enabled"] == true
                                    && s["status"]["runtimeActive"] == true
                            })
                        })
            })
        });
    if !active {
        return Err("请先启用对应的只读来源插件".into());
    }
    let window = app.get_webview_window("main").ok_or("应用窗口不可用")?;
    let selected = tokio::task::spawn_blocking(move || {
        app.dialog()
            .file()
            .set_parent(&window)
            .set_title("授权此来源读取所选 JSONL（包含全部正文）")
            .add_filter("JSONL 会话文件", &["jsonl"])
            .blocking_pick_file()
    })
    .await
    .map_err(|_| "未完成来源选择")?;
    let Some(selected) = selected else {
        return Ok(serde_json::Value::Null);
    };
    let path = selected.into_path().map_err(|_| "请选择本机文件")?;
    backend
        .request(
            reqwest::Method::POST,
            "/api/domain/source-authorize",
            Some(serde_json::json!({"pluginId":query.plugin_id,"digest":query.digest,"path":path})),
        )
        .await
}

#[tauri::command]
pub(crate) async fn query_domain_capabilities(
    backend: tauri::State<'_, Backend>,
) -> Result<serde_json::Value, String> {
    backend
        .request(reqwest::Method::GET, "/api/domain/capabilities", None)
        .await
}

#[tauri::command]
pub(crate) async fn query_domain_catalog(
    backend: tauri::State<'_, Backend>,
) -> Result<serde_json::Value, String> {
    backend
        .request(reqwest::Method::GET, "/api/domain/directory", None)
        .await
}
#[tauri::command]
pub(crate) async fn query_unified_sessions(
    backend: tauri::State<'_, Backend>,
    query: UnifiedSessionQuery,
) -> Result<serde_json::Value, String> {
    backend
        .request(
            reqwest::Method::POST,
            "/api/domain/sessions",
            Some(serde_json::to_value(query).map_err(|_| "统一会话参数无效")?),
        )
        .await
}
#[tauri::command]
pub(crate) async fn get_unified_conversation(
    backend: tauri::State<'_, Backend>,
    query: UnifiedConversationRequest,
) -> Result<serde_json::Value, String> {
    backend
        .request(
            reqwest::Method::POST,
            "/api/domain/conversation",
            Some(serde_json::to_value(query).map_err(|_| "统一正文参数无效")?),
        )
        .await
}
#[tauri::command]
pub(crate) async fn prepare_domain_source_consent(
    backend: tauri::State<'_, Backend>,
    query: SourceConsentQuery,
) -> Result<serde_json::Value, String> {
    backend
        .request(
            reqwest::Method::POST,
            "/api/domain/consent-review",
            Some(serde_json::to_value(query).map_err(|_| "展示范围参数无效")?),
        )
        .await
}
#[tauri::command]
pub(crate) async fn set_domain_source_consent(
    backend: tauri::State<'_, Backend>,
    input: SourceConsent,
) -> Result<serde_json::Value, String> {
    backend
        .request(
            reqwest::Method::POST,
            "/api/domain/consent",
            Some(serde_json::to_value(input).map_err(|_| "统一来源授权参数无效")?),
        )
        .await
}
