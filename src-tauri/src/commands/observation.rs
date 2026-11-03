use crate::backend::{Backend, query_path};
use local_agent_core::model_usage::UsageQuery;
use local_agent_core::observation::OverviewQuery;

#[tauri::command]
pub(crate) async fn query_agent_usage(
    backend: tauri::State<'_, Backend>,
    query: UsageQuery,
) -> Result<serde_json::Value, String> {
    query.validate().map_err(str::to_owned)?;
    backend
        .request(
            reqwest::Method::GET,
            &query_path("/api/agent-usage", query)?,
            None,
        )
        .await
}
#[tauri::command]
pub(crate) async fn query_overview(
    backend: tauri::State<'_, Backend>,
    query: OverviewQuery,
) -> Result<serde_json::Value, String> {
    backend
        .request(
            reqwest::Method::GET,
            &query_path("/api/overview", query)?,
            None,
        )
        .await
}
#[tauri::command]
pub(crate) async fn query_requests(
    backend: tauri::State<'_, Backend>,
    agent: Option<String>,
    offset: Option<usize>,
    id: Option<String>,
) -> Result<serde_json::Value, String> {
    backend
        .request(
            reqwest::Method::GET,
            &query_path(
                "/api/requests",
                serde_json::json!({"agent":agent,"offset":offset,"id":id}),
            )?,
            None,
        )
        .await
}
#[tauri::command]
pub(crate) async fn save_observation_binding(
    backend: tauri::State<'_, Backend>,
    agent_id: String,
    provider_id: String,
    expected_revision: u64,
    enabled: bool,
) -> Result<serde_json::Value, String> {
    backend.request(reqwest::Method::PUT,"/api/observation/bindings",Some(serde_json::json!({"agentId":agent_id,"providerId":provider_id,"expectedRevision":expected_revision,"enabled":enabled}))).await
}
