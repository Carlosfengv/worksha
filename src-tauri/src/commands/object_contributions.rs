use crate::backend::Backend;
use local_agent_core::plugin::object_contributions::ObjectContributionAction;

#[tauri::command]
pub(crate) async fn perform_object_contribution(
    backend: tauri::State<'_, Backend>,
    input: ObjectContributionAction,
) -> Result<serde_json::Value, String> {
    backend
        .request(
            reqwest::Method::POST,
            "/api/plugins/object-contributions",
            Some(serde_json::to_value(input).map_err(|_| "对象扩展参数无效")?),
        )
        .await
}
