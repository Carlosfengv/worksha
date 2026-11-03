use crate::backend::Backend;

#[tauri::command]
pub(crate) async fn commit_model_service_operation(
    backend: tauri::State<'_, Backend>,
    request: local_agent_core::model_services::operations::OperationRequest,
) -> Result<serde_json::Value, String> {
    backend
        .request(
            reqwest::Method::POST,
            "/api/model-service-operations",
            Some(serde_json::to_value(request).map_err(|_| "模型服务操作无效")?),
        )
        .await
}
#[tauri::command]
pub(crate) async fn get_model_service_operation(
    backend: tauri::State<'_, Backend>,
    operation_id: String,
    store_id: String,
) -> Result<serde_json::Value, String> {
    uuid::Uuid::parse_str(&operation_id).map_err(|_| "模型服务操作编号无效")?;
    uuid::Uuid::parse_str(&store_id).map_err(|_| "模型服务数据库身份无效")?;
    backend
        .request(
            reqwest::Method::GET,
            &format!("/api/model-service-operations/{operation_id}?storeId={store_id}"),
            None,
        )
        .await
}

#[tauri::command]
pub(crate) async fn get_model_services(
    backend: tauri::State<'_, Backend>,
) -> Result<serde_json::Value, String> {
    backend
        .request(reqwest::Method::GET, "/api/model-services", None)
        .await
}
#[tauri::command]
pub(crate) async fn save_model_service(
    backend: tauri::State<'_, Backend>,
    draft: local_agent_core::model_services::ServiceDraft,
) -> Result<serde_json::Value, String> {
    backend
        .request(
            reqwest::Method::PUT,
            "/api/model-services",
            Some(serde_json::to_value(draft).map_err(|_| "模型服务草稿无效")?),
        )
        .await
}
#[tauri::command]
pub(crate) async fn delete_model_service(
    backend: tauri::State<'_, Backend>,
    service_id: String,
    expected_revision: u64,
) -> Result<serde_json::Value, String> {
    uuid::Uuid::parse_str(&service_id).map_err(|_| "模型服务标识无效")?;
    backend
        .request(
            reqwest::Method::POST,
            &format!("/api/model-services/{service_id}/delete"),
            Some(serde_json::json!({"expectedRevision":expected_revision})),
        )
        .await
}
#[tauri::command]
pub(crate) async fn prepare_model_service_connection(
    backend: tauri::State<'_, Backend>,
    input: serde_json::Value,
) -> Result<serde_json::Value, String> {
    super::local_tokens::prepare_service(&backend, input).await
}

#[tauri::command]
pub(crate) async fn verify_model_service_member(
    backend: tauri::State<'_, Backend>,
    input: local_agent_core::model_services::operations::VerifyMemberRequest,
) -> Result<serde_json::Value, String> {
    backend
        .request(
            reqwest::Method::POST,
            "/api/model-service-members/verify",
            Some(serde_json::to_value(input).map_err(|_| "模型验证参数无效")?),
        )
        .await
}
