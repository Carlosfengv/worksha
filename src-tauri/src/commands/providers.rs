use crate::backend::Backend;

#[tauri::command]
pub(crate) async fn perform_upstream_account(
    backend: tauri::State<'_, Backend>,
    provider_id: String,
    input: local_agent_core::providers::upstream_accounts::AccountAction,
) -> Result<serde_json::Value, String> {
    if provider_id.is_empty()
        || provider_id.len() > 80
        || !provider_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
    {
        return Err("Provider 标识无效".into());
    }
    backend
        .request(
            reqwest::Method::POST,
            &format!("/api/provider-upstream-accounts/{provider_id}"),
            Some(serde_json::to_value(input).map_err(|_| "上游账号操作无效")?),
        )
        .await
}

fn credential_identity(store_id: &str, operation_id: Option<&str>) -> Result<(), String> {
    let valid = local_agent_core::providers::operations::valid_id;
    if !valid(store_id) || operation_id.is_some_and(|id| !valid(id)) {
        return Err("凭据来源或操作编号无效".into());
    }
    Ok(())
}
#[cfg(test)]
mod legacy_admission_tests {
    use super::*;
    use crate::backend::test_server::serve;
    use serde_json::json;
    use std::sync::Arc;
    use tokio::sync::Notify;
    async fn operation(
        backend: &Backend,
        action: u8,
        id: String,
    ) -> Result<serde_json::Value, String> {
        if action == 0 {
            legacy_credential_create(
                backend,
                "Fixture".into(),
                "disposable-secret".into(),
                Some(id),
            )
            .await
        } else {
            legacy_credential_operation(backend, id, action == 2).await
        }
    }
    #[tokio::test]
    async fn all_legacy_commands_require_current_policy_before_access() {
        for action in 0..3 {
            for schema in [None, Some(10), Some(11)] {
                let server = serve(move |_| async move {
                    schema.map_or_else(|| "{}".into(), |schema| json!({"version":2,"storeId":uuid::Uuid::new_v4().to_string(),"schemaVersion":schema}).to_string())
                }).await;
                let backend = Backend::test_connection(server.endpoint.clone());
                assert!(
                    operation(&backend, action, uuid::Uuid::new_v4().to_string())
                        .await
                        .unwrap_err()
                        .contains("credential_protocol_unavailable")
                );
                let requests = server.requests.lock().unwrap();
                assert_eq!(requests.len(), 1);
                assert_eq!(requests[0].path, "/api/provider-credentials/legacy-policy");
                assert!(requests[0].body.is_null());
            }
        }
    }
    #[tokio::test]
    async fn reconnect_during_legacy_policy_never_redirects_an_operation() {
        for action in 0..3 {
            let hit = Arc::new(Notify::new());
            let release = Arc::new(Notify::new());
            let (entered, done) = (hit.clone(), release.clone());
            let a = serve(move |_| {
                let (hit, release) = (hit.clone(), release.clone());
                async move {
                    hit.notify_one(); release.notified().await;
                    json!({"version":2,"storeId":uuid::Uuid::new_v4().to_string(),"schemaVersion":12}).to_string()
                }
            }).await;
            let b = serve(|_| async { "{}".into() }).await;
            let backend = Arc::new(Backend::test_connection(a.endpoint.clone()));
            let worker = backend.clone();
            let task = tokio::spawn(async move {
                operation(&worker, action, uuid::Uuid::new_v4().to_string()).await
            });
            tokio::time::timeout(std::time::Duration::from_secs(4), entered.notified())
                .await
                .unwrap();
            backend.test_install_connection(b.endpoint.clone());
            done.notify_one();
            assert!(
                task.await
                    .unwrap()
                    .unwrap_err()
                    .contains("backend_connection_changed")
            );
            assert_eq!(a.requests.lock().unwrap().len(), 1);
            assert!(b.requests.lock().unwrap().is_empty());
        }
    }
    #[tokio::test]
    async fn current_policy_preserves_legacy_request_shapes_and_invalid_ids_send_nothing() {
        for action in 0..3 {
            let server = serve(|r| async move {
                if r.path == "/api/provider-credentials/legacy-policy" {
                    json!({"version":2,"storeId":uuid::Uuid::new_v4().to_string(),"schemaVersion":12}).to_string()
                } else { "{}".into() }
            }).await;
            let backend = Backend::test_connection(server.endpoint.clone());
            assert!(operation(&backend, action, "invalid".into()).await.is_err());
            assert!(server.requests.lock().unwrap().is_empty());
            let id = uuid::Uuid::new_v4().to_string();
            operation(&backend, action, id.clone()).await.unwrap();
            let requests = server.requests.lock().unwrap();
            assert_eq!(requests.len(), 2);
            if action == 0 {
                assert_eq!(requests[1].path, "/api/provider-credentials");
                assert_eq!(
                    requests[1].body,
                    json!({"operationId":id,"label":"Fixture","secret":"disposable-secret"})
                );
            } else {
                assert_eq!(
                    requests[1].path,
                    format!(
                        "/api/provider-credential-operations/{id}{}",
                        if action == 2 { "/cancel" } else { "" }
                    )
                );
                assert!(requests[1].body.is_null());
            }
        }
    }
}
#[tauri::command]
pub(crate) async fn get_provider_credential_protocol(
    backend: tauri::State<'_, Backend>,
    store_id: String,
) -> Result<serde_json::Value, String> {
    credential_identity(&store_id, None)?;
    backend
        .context()?
        .request(
            reqwest::Method::GET,
            &format!("/api/provider-credentials/protocol?storeId={store_id}"),
            None,
        )
        .await
}
#[tauri::command]
pub(crate) async fn create_scoped_provider_credential(
    backend: tauri::State<'_, Backend>,
    store_id: String,
    operation_id: String,
    label: String,
    secret: String,
) -> Result<serde_json::Value, String> {
    scoped_credential_create(&backend, store_id, operation_id, label, secret).await
}

async fn scoped_credential_create(
    backend: &Backend,
    store_id: String,
    operation_id: String,
    label: String,
    secret: String,
) -> Result<serde_json::Value, String> {
    credential_identity(&store_id, Some(&operation_id))?;
    let context = backend.context()?;
    let protocol = context
        .request(
            reqwest::Method::GET,
            &format!("/api/provider-credentials/protocol?storeId={store_id}"),
            None,
        )
        .await?;
    if protocol.as_object().is_none_or(|row| row.len() != 3)
        || protocol["version"] != 2
        || protocol["storeId"] != store_id
        || protocol["operationLimit"] != 4096
    {
        return Err(serde_json::json!({"code":"credential_protocol_unavailable","outcomeUnknown":false,"message":"凭据协议或原数据库不匹配，未发送密钥。"}).to_string());
    }
    context.request(reqwest::Method::POST,"/api/provider-credentials/scoped",Some(serde_json::json!({"storeId":store_id,"operationId":operation_id,"label":label,"secret":secret}))).await
}

#[cfg(test)]
mod credential_scope_tests {
    use super::*;
    use crate::backend::test_server::serve;
    use std::sync::Arc;
    use tokio::sync::Notify;
    #[tokio::test]
    async fn reconnect_during_credential_preflight_never_sends_secret_to_either_backend() {
        let entered = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let store_id = uuid::Uuid::new_v4().to_string();
        let response_id = store_id.clone();
        let (hit, done) = (entered.clone(), release.clone());
        let a = serve(move |_| {
            let (hit, done, id) = (hit.clone(), done.clone(), response_id.clone());
            async move {
                hit.notify_one();
                done.notified().await;
                serde_json::json!({"version":2,"storeId":id,"operationLimit":4096}).to_string()
            }
        })
        .await;
        let b = serve(|_| async { "{}".into() }).await;
        let backend = Arc::new(Backend::test_connection(a.endpoint.clone()));
        let worker = backend.clone();
        let task = tokio::spawn(async move {
            scoped_credential_create(
                &worker,
                store_id,
                uuid::Uuid::new_v4().to_string(),
                "Fixture".into(),
                "disposable-secret".into(),
            )
            .await
        });
        tokio::time::timeout(std::time::Duration::from_secs(4), entered.notified())
            .await
            .unwrap();
        backend.test_install_connection(b.endpoint.clone());
        release.notify_one();
        assert!(
            task.await
                .unwrap()
                .unwrap_err()
                .contains("backend_connection_changed")
        );
        assert!(b.requests.lock().unwrap().is_empty());
        let requests = a.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method, "GET");
        assert!(requests[0].body.is_null());
    }
    #[tokio::test]
    async fn foreign_credential_protocol_never_receives_a_create_post() {
        let a=serve(|_|async{serde_json::json!({"version":2,"storeId":uuid::Uuid::new_v4().to_string(),"operationLimit":4096}).to_string()}).await;
        let backend = Backend::test_connection(a.endpoint.clone());
        let error = scoped_credential_create(
            &backend,
            uuid::Uuid::new_v4().to_string(),
            uuid::Uuid::new_v4().to_string(),
            "Fixture".into(),
            "disposable-secret".into(),
        )
        .await
        .unwrap_err();
        assert!(error.contains("credential_protocol_unavailable"));
        assert_eq!(a.requests.lock().unwrap().len(), 1);
    }
}
#[tauri::command]
pub(crate) async fn get_scoped_provider_credential_operation(
    backend: tauri::State<'_, Backend>,
    store_id: String,
    operation_id: String,
) -> Result<serde_json::Value, String> {
    credential_identity(&store_id, Some(&operation_id))?;
    backend
        .context()?
        .request(
            reqwest::Method::GET,
            &format!(
                "/api/provider-credential-operations/scoped/{operation_id}?storeId={store_id}"
            ),
            None,
        )
        .await
}
#[tauri::command]
pub(crate) async fn cancel_scoped_provider_credential_operation(
    backend: tauri::State<'_, Backend>,
    store_id: String,
    operation_id: String,
) -> Result<serde_json::Value, String> {
    credential_identity(&store_id, Some(&operation_id))?;
    backend
        .context()?
        .request(
            reqwest::Method::POST,
            "/api/provider-credential-operations/scoped/cancel",
            Some(serde_json::json!({"storeId":store_id,"operationId":operation_id})),
        )
        .await
}

#[tauri::command]
pub(crate) async fn get_provider_write_protocol(
    backend: tauri::State<'_, Backend>,
) -> Result<serde_json::Value, String> {
    backend
        .request(
            reqwest::Method::GET,
            "/api/provider-operations/protocol",
            None,
        )
        .await
}
#[tauri::command]
pub(crate) async fn commit_provider_operation(
    backend: tauri::State<'_, Backend>,
    request: local_agent_core::providers::operations::Request,
) -> Result<serde_json::Value, String> {
    backend
        .context()?
        .request(
            reqwest::Method::POST,
            "/api/provider-operations",
            Some(serde_json::to_value(request).map_err(|_| "服务商操作参数无效")?),
        )
        .await
}
#[tauri::command]
pub(crate) async fn get_provider_operation(
    backend: tauri::State<'_, Backend>,
    operation_id: String,
    store_id: String,
) -> Result<serde_json::Value, String> {
    if !local_agent_core::providers::operations::valid_id(&operation_id)
        || !local_agent_core::providers::operations::valid_id(&store_id)
    {
        return Err("服务商操作身份无效".into());
    }
    backend
        .context()?
        .request(
            reqwest::Method::GET,
            &format!("/api/provider-operations/{operation_id}?storeId={store_id}"),
            None,
        )
        .await
}
#[tauri::command]
pub(crate) async fn cancel_provider_operation(
    backend: tauri::State<'_, Backend>,
    request: local_agent_core::providers::operations::Request,
) -> Result<serde_json::Value, String> {
    backend
        .context()?
        .request(
            reqwest::Method::POST,
            "/api/provider-operations/cancel",
            Some(serde_json::to_value(request).map_err(|_| "服务商操作参数无效")?),
        )
        .await
}

#[tauri::command]
pub(crate) async fn connect_provider(
    backend: tauri::State<'_, Backend>,
    provider: local_agent_core::Provider,
) -> Result<serde_json::Value, String> {
    backend
        .request(
            reqwest::Method::POST,
            "/api/provider-connect",
            Some(serde_json::to_value(provider).map_err(|_| "连接信息无效")?),
        )
        .await
}

#[tauri::command]
pub(crate) async fn prepare_dynamic_connection(
    backend: tauri::State<'_, Backend>,
    draft: local_agent_core::providers::BindingDraft,
) -> Result<serde_json::Value, String> {
    super::local_tokens::prepare_draft(&backend, draft, true).await
}
#[tauri::command]
pub(crate) async fn prepare_dynamic_route(
    backend: tauri::State<'_, Backend>,
    input: serde_json::Value,
) -> Result<serde_json::Value, String> {
    backend
        .request(reqwest::Method::POST, "/api/dynamic-routes", Some(input))
        .await
}
#[tauri::command]
pub(crate) async fn dynamic_route_action(
    backend: tauri::State<'_, Backend>,
    change_id: String,
    action: String,
) -> Result<serde_json::Value, String> {
    uuid::Uuid::parse_str(&change_id).map_err(|_| "动态操作 ID 无效")?;
    if !["commit", "cancel", "restore"].contains(&action.as_str()) {
        return Err("动态操作无效".into());
    }
    backend
        .request(
            reqwest::Method::POST,
            &format!("/api/dynamic-routes/{change_id}"),
            Some(serde_json::json!({"action":action})),
        )
        .await
}

#[tauri::command]
pub(crate) async fn query_provider_usage(
    backend: tauri::State<'_, Backend>,
    query: local_agent_core::provider_usage::Query,
) -> Result<serde_json::Value, String> {
    query.validate().map_err(str::to_owned)?;
    backend
        .request(
            reqwest::Method::GET,
            &crate::backend::query_path("/api/provider-usage", query)?,
            None,
        )
        .await
}

#[tauri::command]
pub(crate) async fn discover_provider_imports(
    backend: tauri::State<'_, Backend>,
) -> Result<serde_json::Value, String> {
    backend
        .request(reqwest::Method::GET, "/api/provider-imports", None)
        .await
}
#[tauri::command]
pub(crate) async fn import_native_provider(
    backend: tauri::State<'_, Backend>,
    input: local_agent_core::providers::imports::ImportRequest,
) -> Result<serde_json::Value, String> {
    super::native_imports::submit(&backend, input).await
}
#[tauri::command]
pub(crate) async fn get_provider_status(
    backend: tauri::State<'_, Backend>,
) -> Result<serde_json::Value, String> {
    backend
        .request(reqwest::Method::GET, "/api/provider-status", None)
        .await
}
#[tauri::command]
pub(crate) async fn create_provider_credential(
    backend: tauri::State<'_, Backend>,
    label: String,
    secret: String,
    operation_id: Option<String>,
) -> Result<serde_json::Value, String> {
    legacy_credential_create(&backend, label, secret, operation_id).await
}
async fn legacy_credential_create(
    backend: &Backend,
    label: String,
    secret: String,
    operation_id: Option<String>,
) -> Result<serde_json::Value, String> {
    if operation_id
        .as_ref()
        .is_some_and(|id| !local_agent_core::providers::operations::valid_id(id))
    {
        return Err(serde_json::json!({"code":"invalid_credential_operation","outcomeUnknown":false,"message":"凭据操作编号无效，未发送请求。"}).to_string());
    }
    let mut input = serde_json::json!({"label":label,"secret":secret});
    if let Some(id) = operation_id {
        input["operationId"] = serde_json::json!(id);
    }
    super::credential_recovery::legacy_context(backend, None)
        .await?
        .request(
            reqwest::Method::POST,
            "/api/provider-credentials",
            Some(input),
        )
        .await
}
#[tauri::command]
pub(crate) async fn get_provider_credential_operation(
    backend: tauri::State<'_, Backend>,
    operation_id: String,
) -> Result<serde_json::Value, String> {
    legacy_credential_operation(&backend, operation_id, false).await
}
#[tauri::command]
pub(crate) async fn cancel_provider_credential_operation(
    backend: tauri::State<'_, Backend>,
    operation_id: String,
) -> Result<serde_json::Value, String> {
    legacy_credential_operation(&backend, operation_id, true).await
}
async fn legacy_credential_operation(
    backend: &Backend,
    operation_id: String,
    cancel: bool,
) -> Result<serde_json::Value, String> {
    if !local_agent_core::providers::operations::valid_id(&operation_id) {
        return Err(serde_json::json!({"code":"invalid_credential_operation","outcomeUnknown":false,"message":"凭据操作编号无效，未发送请求。"}).to_string());
    }
    let context = super::credential_recovery::legacy_context(backend, None).await?;
    context
        .request(
            if cancel {
                reqwest::Method::POST
            } else {
                reqwest::Method::GET
            },
            &format!(
                "/api/provider-credential-operations/{operation_id}{}",
                if cancel { "/cancel" } else { "" }
            ),
            None,
        )
        .await
}
#[tauri::command]
pub(crate) async fn fetch_provider_models(
    backend: tauri::State<'_, Backend>,
    input: serde_json::Value,
) -> Result<serde_json::Value, String> {
    backend
        .request(reqwest::Method::POST, "/api/provider-models", Some(input))
        .await
}
#[tauri::command]
pub(crate) async fn verify_provider(
    backend: tauri::State<'_, Backend>,
    provider_id: String,
    expected_revision: u64,
    model: Option<String>,
) -> Result<serde_json::Value, String> {
    if !provider_id
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        || provider_id.len() > 80
    {
        return Err("服务商 ID 无效".into());
    }
    backend
        .request(
            reqwest::Method::POST,
            &format!("/api/providers/{provider_id}/verify"),
            Some(serde_json::json!({"expectedRevision":expected_revision,"model":model})),
        )
        .await
}
#[tauri::command]
pub(crate) async fn prepare_provider_switch(
    backend: tauri::State<'_, Backend>,
    draft: local_agent_core::providers::BindingDraft,
) -> Result<serde_json::Value, String> {
    super::local_tokens::prepare_draft(&backend, draft, false).await
}
#[tauri::command]
pub(crate) async fn provider_switch_action(
    backend: tauri::State<'_, Backend>,
    transaction_id: String,
    action: String,
) -> Result<serde_json::Value, String> {
    uuid::Uuid::parse_str(&transaction_id).map_err(|_| "配置事务 ID 无效")?;
    if !["apply", "cancel", "restore", "defer"].contains(&action.as_str()) {
        return Err("未知配置操作".into());
    }
    backend
        .request(
            reqwest::Method::POST,
            &format!("/api/provider-switches/{transaction_id}/{action}"),
            None,
        )
        .await
}
