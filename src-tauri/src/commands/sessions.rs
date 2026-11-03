use crate::backend::{Backend, query_path};
use local_agent_core::sessions::{CallQuery, SessionQuery};

#[tauri::command]
pub(crate) async fn query_agent_memory(
    backend: tauri::State<'_, Backend>,
    agent_id: String,
    file_id: Option<String>,
) -> Result<serde_json::Value, String> {
    let mut url =
        reqwest::Url::parse("http://localhost/api/agents/").map_err(|_| "查询路径无效")?;
    url.path_segments_mut()
        .map_err(|_| "查询路径无效")?
        .pop_if_empty()
        .push(&agent_id)
        .push("memory");
    backend
        .request(
            reqwest::Method::GET,
            &query_path(url.path(), serde_json::json!({"fileId":file_id}))?,
            None,
        )
        .await
}

#[tauri::command]
pub(crate) async fn query_sessions(
    backend: tauri::State<'_, Backend>,
    query: SessionQuery,
) -> Result<serde_json::Value, String> {
    backend
        .request(
            reqwest::Method::GET,
            &query_path("/api/sessions", query)?,
            None,
        )
        .await
}
#[tauri::command]
pub(crate) async fn query_calls(
    backend: tauri::State<'_, Backend>,
    query: CallQuery,
) -> Result<serde_json::Value, String> {
    backend
        .request(
            reqwest::Method::GET,
            &query_path("/api/calls", query)?,
            None,
        )
        .await
}
#[tauri::command]
pub(crate) async fn query_mcp(
    backend: tauri::State<'_, Backend>,
    query: local_agent_core::sessions::McpQuery,
) -> Result<serde_json::Value, String> {
    backend
        .request(reqwest::Method::GET, &query_path("/api/mcp", query)?, None)
        .await
}
#[tauri::command]
pub(crate) async fn get_conversation(
    backend: tauri::State<'_, Backend>,
    session_id: String,
    cursor: Option<String>,
    anchor: Option<String>,
) -> Result<serde_json::Value, String> {
    let mut url =
        reqwest::Url::parse("http://localhost/api/sessions/").map_err(|_| "查询路径无效")?;
    url.path_segments_mut()
        .map_err(|_| "查询路径无效")?
        .pop_if_empty()
        .push(&session_id);
    backend
        .request(
            reqwest::Method::GET,
            &query_path(
                url.path(),
                serde_json::json!({"cursor":cursor,"anchor":anchor}),
            )?,
            None,
        )
        .await
}
#[tauri::command]
pub(crate) async fn set_session_source(
    backend: tauri::State<'_, Backend>,
    agent_id: String,
    enabled: bool,
) -> Result<serde_json::Value, String> {
    backend
        .request(
            reqwest::Method::PUT,
            "/api/session-sources",
            Some(serde_json::json!({"agentId":agent_id,"enabled":enabled})),
        )
        .await
}
#[tauri::command]
pub(crate) async fn refresh_sessions(
    backend: tauri::State<'_, Backend>,
) -> Result<serde_json::Value, String> {
    backend
        .request(reqwest::Method::POST, "/api/sessions/refresh", None)
        .await
}

#[tauri::command]
pub(crate) async fn query_associated_costs(
    backend: tauri::State<'_, Backend>,
    query: local_agent_core::associated_cost::detail::Query,
) -> Result<serde_json::Value, String> {
    backend
        .request(
            reqwest::Method::GET,
            &query_path("/api/associated-costs", query)?,
            None,
        )
        .await
}
