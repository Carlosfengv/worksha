use crate::backend::{Backend, ConnectionContext};
use local_agent_core::providers::{
    credential_scope::{CreateV3, IdentityV3, Initialization, NamespaceState, ProtocolV3},
    operations::valid_id,
};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
fn failure(code: &str, message: &str) -> String {
    json!({"code":code,"outcomeUnknown":false,"message":message}).to_string()
}
fn identity(store: &str, namespace: Option<&str>, operation: Option<&str>) -> Result<(), String> {
    if !valid_id(store)
        || namespace.is_some_and(|id| !valid_id(id))
        || operation.is_some_and(|id| !valid_id(id))
    {
        return Err(failure(
            "invalid_credential_operation",
            "凭据来源、恢复空间或操作编号无效，未发送请求。",
        ));
    }
    Ok(())
}
fn request<T: DeserializeOwned>(value: Value) -> Result<T, String> {
    serde_json::from_value(value).map_err(|_| {
        failure(
            "invalid_credential_operation",
            "凭据请求格式无效，未发送请求。",
        )
    })
}
async fn context(
    backend: &Backend,
    store: &str,
    namespace: &str,
    active: bool,
) -> Result<ConnectionContext, String> {
    context_from(backend.context()?, store, namespace, active).await
}
async fn context_from(
    context: ConnectionContext,
    store: &str,
    namespace: &str,
    active: bool,
) -> Result<ConnectionContext, String> {
    identity(store, Some(namespace), None)?;
    let value = context
        .request(
            reqwest::Method::GET,
            &format!("/api/provider-credentials/v3/protocol?storeId={store}"),
            None,
        )
        .await?;
    let protocol: ProtocolV3 = serde_json::from_value(value).map_err(|_| {
        failure(
            "credential_protocol_unavailable",
            "凭据恢复协议无法核查，未发送密钥。",
        )
    })?;
    if protocol.version != 3
        || protocol.store_id != store
        || protocol.namespace_id != namespace
        || protocol.operation_limit != 4096
    {
        return Err(failure(
            "credential_namespace_changed",
            "原凭据恢复空间不匹配，未发送请求。",
        ));
    }
    if active && !matches!(protocol.namespace_state, NamespaceState::Active) {
        return Err(failure(
            "credential_namespace_unverified",
            "请先核查凭据恢复空间，未发送密钥。",
        ));
    }
    Ok(context)
}
pub(super) async fn legacy_context(
    backend: &Backend,
    expected_store: Option<&str>,
) -> Result<ConnectionContext, String> {
    let context = backend.context()?;
    let value = context
        .request(
            reqwest::Method::GET,
            "/api/provider-credentials/legacy-policy",
            None,
        )
        .await?;
    let policy: local_agent_core::providers::credential_operations::LegacyPolicy =
        serde_json::from_value(value).map_err(|_| {
            failure(
                "credential_protocol_unavailable",
                "当前后台尚未提供旧凭据持久防重，请正常退出旧版服务后升级；未发送密钥或确认。",
            )
        })?;
    if policy.version != 2
        // Policy 2 was introduced with schema 12; native imports have their own capability.
        || policy.schema_version < 12
        || !valid_id(&policy.store_id)
    {
        return Err(failure(
            "credential_protocol_unavailable",
            "当前后台尚未提供旧凭据持久防重，未发送密钥或确认。",
        ));
    }
    if expected_store.is_some_and(|id| id != policy.store_id) {
        return Err(failure(
            "credential_store_changed",
            "旧凭据入口的原数据库已变化，未发送请求。",
        ));
    }
    Ok(context)
}
#[tauri::command]
pub(crate) async fn discover_provider_credentials(
    backend: tauri::State<'_, Backend>,
    request: Value,
) -> Result<Value, String> {
    discover(&backend, self::request(request)?).await
}
async fn discover(
    backend: &Backend,
    input: local_agent_core::providers::credential_operations::discovery::Request,
) -> Result<Value, String> {
    identity(&input.store_id, Some(&input.namespace_id), None)?;
    input.validate().map_err(|e| failure(&e.code, &e.message))?;
    let fixed = legacy_context(backend, Some(&input.store_id)).await?;
    let context = context_from(fixed, &input.store_id, &input.namespace_id, true).await?;
    let mut path = format!(
        "/api/provider-credentials/v3/discovery?storeId={}&namespaceId={}",
        input.store_id, input.namespace_id
    );
    if let Some(cursor) = &input.cursor {
        path.push_str("&cursor=");
        path.push_str(cursor);
    }
    context.request(reqwest::Method::GET, &path, None).await
}
#[tauri::command]
pub(crate) async fn get_provider_credential_protocol_v3(
    backend: tauri::State<'_, Backend>,
    store_id: String,
) -> Result<Value, String> {
    identity(&store_id, None, None)?;
    backend
        .context()?
        .request(
            reqwest::Method::GET,
            &format!("/api/provider-credentials/v3/protocol?storeId={store_id}"),
            None,
        )
        .await
}
#[tauri::command]
pub(crate) async fn initialize_provider_credential_namespace(
    backend: tauri::State<'_, Backend>,
    request: Value,
) -> Result<Value, String> {
    initialize(&backend, self::request(request)?).await
}
async fn initialize(backend: &Backend, input: Initialization) -> Result<Value, String> {
    let context = context(backend, &input.store_id, &input.namespace_id, false).await?;
    context
        .request(
            reqwest::Method::POST,
            "/api/provider-credentials/v3/initialize",
            Some(
                serde_json::to_value(input)
                    .map_err(|_| failure("invalid_credential_operation", "凭据恢复请求无效"))?,
            ),
        )
        .await
}
#[tauri::command]
pub(crate) async fn create_provider_credential_v3(
    backend: tauri::State<'_, Backend>,
    request: Value,
) -> Result<Value, String> {
    create(&backend, self::request(request)?).await
}
async fn create(backend: &Backend, input: CreateV3) -> Result<Value, String> {
    identity(
        &input.store_id,
        Some(&input.namespace_id),
        Some(&input.operation_id),
    )?;
    let context = context(backend, &input.store_id, &input.namespace_id, true).await?;
    context
        .request(
            reqwest::Method::POST,
            "/api/provider-credentials/v3",
            Some(
                serde_json::to_value(input)
                    .map_err(|_| failure("invalid_credential_operation", "凭据保存请求无效"))?,
            ),
        )
        .await
}
#[tauri::command]
pub(crate) async fn get_provider_credential_operation_v3(
    backend: tauri::State<'_, Backend>,
    request: Value,
) -> Result<Value, String> {
    let input: IdentityV3 = self::request(request)?;
    identity(
        &input.store_id,
        Some(&input.namespace_id),
        Some(&input.operation_id),
    )?;
    backend
        .context()?
        .request(
            reqwest::Method::GET,
            &format!(
                "/api/provider-credential-operations/v3/{}?storeId={}&namespaceId={}",
                input.operation_id, input.store_id, input.namespace_id
            ),
            None,
        )
        .await
}
#[tauri::command]
pub(crate) async fn cancel_provider_credential_operation_v3(
    backend: tauri::State<'_, Backend>,
    request: Value,
) -> Result<Value, String> {
    let input: IdentityV3 = self::request(request)?;
    identity(
        &input.store_id,
        Some(&input.namespace_id),
        Some(&input.operation_id),
    )?;
    let context = context(&backend, &input.store_id, &input.namespace_id, true).await?;
    context
        .request(
            reqwest::Method::POST,
            "/api/provider-credential-operations/v3/cancel",
            Some(
                serde_json::to_value(input)
                    .map_err(|_| failure("invalid_credential_operation", "凭据取消请求无效"))?,
            ),
        )
        .await
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::test_server::serve;
    use std::sync::Arc;
    use tokio::sync::Notify;
    #[tokio::test]
    async fn reconnect_during_v3_preflight_sends_neither_create_nor_initialization() {
        for initializing in [false, true] {
            let store = uuid::Uuid::new_v4().to_string();
            let namespace = uuid::Uuid::new_v4().to_string();
            let response=json!({"version":3,"storeId":store,"namespaceId":namespace,"namespaceState":"active","legacyState":"fresh","operationLimit":4096}).to_string();
            let entered = Arc::new(Notify::new());
            let released = Arc::new(Notify::new());
            let (hit, done) = (entered.clone(), released.clone());
            let a = serve(move |_| {
                let (hit, done, response) = (hit.clone(), done.clone(), response.clone());
                async move {
                    hit.notify_one();
                    done.notified().await;
                    response
                }
            })
            .await;
            let b = serve(|_| async { "{}".into() }).await;
            let backend = Arc::new(Backend::test_connection(a.endpoint.clone()));
            let worker = backend.clone();
            let task = tokio::spawn(async move {
                if initializing {
                    initialize(&worker,Initialization{store_id:store,namespace_id:namespace,choice:local_agent_core::providers::credential_scope::InitializationChoice::New}).await
                } else {
                    create(
                        &worker,
                        CreateV3 {
                            store_id: store,
                            namespace_id: namespace,
                            operation_id: uuid::Uuid::new_v4().to_string(),
                            label: "Fixture".into(),
                            secret: "disposable-fixture-key".into(),
                        },
                    )
                    .await
                }
            });
            tokio::time::timeout(std::time::Duration::from_secs(4), entered.notified())
                .await
                .unwrap();
            backend.test_install_connection(b.endpoint.clone());
            released.notify_one();
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
    }
    #[tokio::test]
    async fn unverified_namespace_never_receives_a_secret() {
        let store = uuid::Uuid::new_v4().to_string();
        let namespace = uuid::Uuid::new_v4().to_string();
        let response=json!({"version":3,"storeId":store,"namespaceId":namespace,"namespaceState":"unverified","legacyState":"unverified","operationLimit":4096}).to_string();
        let server = serve(move |_| {
            let response = response.clone();
            async move { response }
        })
        .await;
        let backend = Backend::test_connection(server.endpoint.clone());
        let error = create(
            &backend,
            CreateV3 {
                store_id: store,
                namespace_id: namespace,
                operation_id: uuid::Uuid::new_v4().to_string(),
                label: "Fixture".into(),
                secret: "disposable-key".into(),
            },
        )
        .await
        .unwrap_err();
        assert!(error.contains("credential_namespace_unverified"));
        assert_eq!(server.requests.lock().unwrap().len(), 1);
    }
    #[test]
    fn invalid_typed_requests_do_not_echo_supplied_secret_fields() {
        let error = request::<CreateV3>(json!({"unknown-private-field":"disposable-private-key"}))
            .err()
            .unwrap();
        assert!(!error.contains("unknown-private-field"));
        assert!(!error.contains("disposable-private-key"));
        assert!(error.contains("invalid_credential_operation"));
    }
}

#[tauri::command]
pub(crate) async fn preview_legacy_provider_credential(
    backend: tauri::State<'_, Backend>,
    request: Value,
) -> Result<Value, String> {
    let input: IdentityV3 = self::request(request)?;
    let fixed = legacy_context(&backend, Some(&input.store_id)).await?;
    let context = context_from(fixed, &input.store_id, &input.namespace_id, true).await?;
    legacy_preview(&context, &input).await
}
async fn legacy_preview(context: &ConnectionContext, input: &IdentityV3) -> Result<Value, String> {
    identity(
        &input.store_id,
        Some(&input.namespace_id),
        Some(&input.operation_id),
    )?;
    let value = context
        .request(
            reqwest::Method::GET,
            &format!(
                "/api/provider-credentials/v3/legacy/{}?storeId={}&namespaceId={}",
                input.operation_id, input.store_id, input.namespace_id
            ),
            None,
        )
        .await?;
    let preview: local_agent_core::providers::credential_operations::legacy::Preview =
        serde_json::from_value(value.clone()).map_err(|_| {
            failure(
                "transport_invalid_response",
                "旧记录核查回复无效，未发送迁移确认",
            )
        })?;
    if preview.version != 2 {
        return Err(failure(
            "credential_protocol_unavailable",
            "旧记录核查协议不支持持久防重，未发送迁移确认",
        ));
    }
    if preview.store_id != input.store_id
        || preview.namespace_id != input.namespace_id
        || preview.operation_id != input.operation_id
        || preview.source_digest.len() != 64
        || !preview
            .source_digest
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        || matches!(
            preview.phase,
            local_agent_core::providers::credential_operations::Status::Removed
        )
    {
        return Err(failure(
            "transport_invalid_response",
            "旧记录核查身份或格式不匹配，未发送迁移确认",
        ));
    }
    Ok(value)
}
#[tauri::command]
pub(crate) async fn confirm_legacy_provider_credential(
    backend: tauri::State<'_, Backend>,
    request: Value,
) -> Result<Value, String> {
    confirm_legacy(&backend, self::request(request)?).await
}
async fn confirm_legacy(
    backend: &Backend,
    input: local_agent_core::providers::credential_operations::legacy::Confirmation,
) -> Result<Value, String> {
    if !input.confirmed {
        return Err(failure(
            "credential_legacy_unverified",
            "请明确确认旧记录属于原后台，未发送迁移",
        ));
    }
    let fixed = legacy_context(backend, Some(&input.store_id)).await?;
    let context = context_from(fixed, &input.store_id, &input.namespace_id, true).await?;
    let preview = legacy_preview(
        &context,
        &IdentityV3 {
            store_id: input.store_id.clone(),
            namespace_id: input.namespace_id.clone(),
            operation_id: input.operation_id.clone(),
        },
    )
    .await?;
    if preview["sourceDigest"].as_str() != Some(&input.source_digest) {
        return Err(failure(
            "credential_legacy_source_changed",
            "旧记录材料已经变化，请重新核查；未发送迁移",
        ));
    }
    context
        .request(
            reqwest::Method::POST,
            "/api/provider-credentials/v3/legacy",
            Some(
                serde_json::to_value(input)
                    .map_err(|_| failure("invalid_credential_operation", "旧记录迁移请求无效"))?,
            ),
        )
        .await
}

#[cfg(test)]
mod legacy_tests {
    use super::*;
    use crate::backend::test_server::serve;
    use std::sync::Arc;
    use tokio::sync::Notify;
    fn input() -> local_agent_core::providers::credential_operations::legacy::Confirmation {
        local_agent_core::providers::credential_operations::legacy::Confirmation {
            store_id: uuid::Uuid::new_v4().to_string(),
            namespace_id: uuid::Uuid::new_v4().to_string(),
            operation_id: uuid::Uuid::new_v4().to_string(),
            source_digest: "a".repeat(64),
            confirmed: true,
        }
    }
    #[tokio::test]
    async fn current_legacy_confirmation_uses_one_connection_and_exact_fields() {
        let input = input();
        let expected = serde_json::to_value(&input).unwrap();
        let reply = expected.clone();
        let server = serve(move |request| {
            let reply = reply.clone();
            async move {
                match request.path.as_str() {
                    "/api/provider-credentials/legacy-policy" => json!({"version":2,"storeId":reply["storeId"],"schemaVersion":12}),
                    "/api/provider-credentials/v3/legacy" => json!({"accepted":true}),
                    p if p.contains("/protocol?") => json!({"version":3,"storeId":reply["storeId"],"namespaceId":reply["namespaceId"],"namespaceState":"active","legacyState":"fresh","operationLimit":4096}),
                    _ => json!({"version":2,"storeId":reply["storeId"],"namespaceId":reply["namespaceId"],"operationId":reply["operationId"],"sourceDigest":reply["sourceDigest"],"stage":"unclaimed","phase":"ready","label":"Fixture"}),
                }.to_string()
            }
        }).await;
        let backend = Backend::test_connection(server.endpoint.clone());
        assert_eq!(
            confirm_legacy(&backend, input).await.unwrap(),
            json!({"accepted":true})
        );
        let requests = server.requests.lock().unwrap();
        assert_eq!(requests.len(), 4);
        assert!(
            requests[..3]
                .iter()
                .all(|r| r.method == "GET" && r.body.is_null())
        );
        assert_eq!(requests[3].method, "POST");
        assert_eq!(requests[3].body, expected);
    }
    #[tokio::test]
    async fn weak_or_foreign_legacy_policy_never_receives_source_material_or_confirmation() {
        let input = input();
        for policy in [
            json!({}),
            json!({"version":1,"storeId":input.store_id,"schemaVersion":11}),
            json!({"version":2,"storeId":input.store_id,"schemaVersion":11}),
            json!({"version":3,"storeId":input.store_id,"schemaVersion":12}),
            json!({"version":2,"storeId":uuid::Uuid::new_v4().to_string(),"schemaVersion":12}),
        ] {
            let server = serve(move |_| {
                let policy = policy.to_string();
                async move { policy }
            })
            .await;
            let backend = Backend::test_connection(server.endpoint.clone());
            let error = confirm_legacy(&backend, self::input_for(&input))
                .await
                .unwrap_err();
            assert!(
                error.contains("credential_protocol_unavailable")
                    || error.contains("credential_store_changed")
            );
            let requests = server.requests.lock().unwrap();
            assert_eq!(requests.len(), 1);
            assert_eq!(requests[0].path, "/api/provider-credentials/legacy-policy");
            assert!(requests[0].body.is_null());
        }
    }
    fn input_for(
        input: &local_agent_core::providers::credential_operations::legacy::Confirmation,
    ) -> local_agent_core::providers::credential_operations::legacy::Confirmation {
        serde_json::from_value(serde_json::to_value(input).unwrap()).unwrap()
    }
    #[tokio::test]
    async fn changed_material_is_not_confirmed_and_unselected_consent_sends_nothing() {
        let input = input();
        let (store, namespace, id) = (
            input.store_id.clone(),
            input.namespace_id.clone(),
            input.operation_id.clone(),
        );
        let server=serve(move |request|{let(store,namespace,id)=(store.clone(),namespace.clone(),id.clone());async move{if request.path == "/api/provider-credentials/legacy-policy" {json!({"version":2,"storeId":store,"schemaVersion":12}).to_string()}else if request.path.contains("/protocol?"){json!({"version":3,"storeId":store,"namespaceId":namespace,"namespaceState":"active","legacyState":"fresh","operationLimit":4096}).to_string()}else{json!({"version":2,"storeId":store,"namespaceId":namespace,"operationId":id,"sourceDigest":"b".repeat(64),"stage":"unclaimed","phase":"ready","label":"Fixture"}).to_string()}}}).await;
        let backend = Backend::test_connection(server.endpoint.clone());
        assert!(
            confirm_legacy(&backend, input)
                .await
                .unwrap_err()
                .contains("credential_legacy_source_changed")
        );
        assert_eq!(server.requests.lock().unwrap().len(), 3);
        assert!(
            server
                .requests
                .lock()
                .unwrap()
                .iter()
                .all(|r| r.method == "GET")
        );
        let mut unchecked = self::input();
        unchecked.confirmed = false;
        let before = server.requests.lock().unwrap().len();
        assert!(confirm_legacy(&backend, unchecked).await.is_err());
        assert_eq!(server.requests.lock().unwrap().len(), before);
    }
    #[tokio::test]
    async fn reconnect_during_original_material_read_never_sends_confirmation() {
        let input = input();
        let (store, namespace, id) = (
            input.store_id.clone(),
            input.namespace_id.clone(),
            input.operation_id.clone(),
        );
        let entered = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let (hit, done) = (entered.clone(), release.clone());
        let a=serve(move |request|{let(store,namespace,id,hit,done)=(store.clone(),namespace.clone(),id.clone(),hit.clone(),done.clone());async move{if request.path == "/api/provider-credentials/legacy-policy" {json!({"version":2,"storeId":store,"schemaVersion":12}).to_string()}else if request.path.contains("/protocol?"){json!({"version":3,"storeId":store,"namespaceId":namespace,"namespaceState":"active","legacyState":"fresh","operationLimit":4096}).to_string()}else{hit.notify_one();done.notified().await;json!({"version":2,"storeId":store,"namespaceId":namespace,"operationId":id,"sourceDigest":"a".repeat(64),"stage":"unclaimed","phase":"ready","label":"Fixture"}).to_string()}}}).await;
        let b = serve(|_| async { "{}".into() }).await;
        let backend = Arc::new(Backend::test_connection(a.endpoint.clone()));
        let worker = backend.clone();
        let task = tokio::spawn(async move { confirm_legacy(&worker, input).await });
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
        assert_eq!(a.requests.lock().unwrap().len(), 3);
        assert!(a.requests.lock().unwrap().iter().all(|r| r.method == "GET"));
        assert!(b.requests.lock().unwrap().is_empty());
    }
}

#[cfg(test)]
mod discovery_tests {
    use super::*;
    use crate::backend::test_server::serve;
    use local_agent_core::providers::credential_operations::discovery::Request;
    use std::sync::Arc;
    use tokio::sync::Notify;
    fn input() -> Request {
        Request {
            store_id: uuid::Uuid::new_v4().to_string(),
            namespace_id: uuid::Uuid::new_v4().to_string(),
            cursor: None,
        }
    }
    fn reply(path: &str, input: &Request) -> String {
        if path == "/api/provider-credentials/legacy-policy" {
            json!({"version":2,"storeId":input.store_id,"schemaVersion":12}).to_string()
        } else if path.contains("/protocol?") {
            json!({"version":3,"storeId":input.store_id,"namespaceId":input.namespace_id,"namespaceState":"active","legacyState":"fresh","operationLimit":4096}).to_string()
        } else {
            json!({"version":1,"storeId":input.store_id,"namespaceId":input.namespace_id,"snapshot":"a".repeat(64),"pageSize":32,"totalCandidates":0,"scanned":0,"merged":0,"unrecognized":0,"items":[],"nextCursor":null}).to_string()
        }
    }
    #[tokio::test]
    async fn discovery_uses_only_gets_after_fixed_source_preflights_and_exact_cursor() {
        let mut input = input();
        input.cursor = Some(format!("{}:3:{}", "a".repeat(64), uuid::Uuid::new_v4()));
        let expected = format!(
            "/api/provider-credentials/v3/discovery?storeId={}&namespaceId={}&cursor={}",
            input.store_id,
            input.namespace_id,
            input.cursor.as_ref().unwrap()
        );
        let state = input.clone();
        let server = serve(move |r| {
            let state = state.clone();
            async move { reply(&r.path, &state) }
        })
        .await;
        let backend = Backend::test_connection(server.endpoint.clone());
        discover(&backend, input).await.unwrap();
        let requests = server.requests.lock().unwrap();
        assert_eq!(requests.len(), 3);
        assert!(
            requests
                .iter()
                .all(|r| r.method == "GET" && r.body.is_null())
        );
        assert_eq!(requests[2].path, expected);
    }
    #[tokio::test]
    async fn reconnect_at_each_discovery_step_cannot_forward_to_another_source() {
        for stage in 0..3 {
            let entered = Arc::new(Notify::new());
            let release = Arc::new(Notify::new());
            let (hit, done) = (entered.clone(), release.clone());
            let input = input();
            let state = input.clone();
            let a = serve(move |r| {
                let (state, hit, done) = (state.clone(), hit.clone(), done.clone());
                async move {
                    let index = if r.path == "/api/provider-credentials/legacy-policy" {
                        0
                    } else if r.path.contains("/protocol?") {
                        1
                    } else {
                        2
                    };
                    if index == stage {
                        hit.notify_one();
                        done.notified().await;
                    }
                    reply(&r.path, &state)
                }
            })
            .await;
            let b = serve(|_| async { "{}".into() }).await;
            let backend = Arc::new(Backend::test_connection(a.endpoint.clone()));
            let worker = backend.clone();
            let task = tokio::spawn(async move { discover(&worker, input).await });
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
            assert_eq!(a.requests.lock().unwrap().len(), stage + 1);
            assert!(b.requests.lock().unwrap().is_empty());
        }
    }
    #[tokio::test]
    async fn malformed_cursor_sends_no_http_and_weak_policy_never_receives_a_list_request() {
        let server = serve(|_| async { "{}".into() }).await;
        let backend = Backend::test_connection(server.endpoint.clone());
        for cursor in [
            "bad".into(),
            format!("{}:3:00000000-0000-0000-0000-000000000000", "a".repeat(64)),
            "x".repeat(129),
            "a&secret=private".into(),
        ] {
            let mut input = input();
            input.cursor = Some(cursor);
            assert!(
                discover(&backend, input)
                    .await
                    .unwrap_err()
                    .contains("invalid_credential_operation")
            );
        }
        assert!(server.requests.lock().unwrap().is_empty());
        assert!(
            discover(&backend, input())
                .await
                .unwrap_err()
                .contains("credential_protocol_unavailable")
        );
        assert_eq!(server.requests.lock().unwrap().len(), 1);
    }
    #[tokio::test]
    async fn discovery_reply_has_its_own_bounded_transport_limit() {
        let input = input();
        let state = input.clone();
        let server=serve(move|r|{let state=state.clone();async move{if r.path.contains("/discovery?"){json!({"payload":"x".repeat(local_agent_core::providers::credential_operations::discovery::MAX_RESPONSE_BYTES)}).to_string()}else{reply(&r.path,&state)}}}).await;
        let backend = Backend::test_connection(server.endpoint.clone());
        assert!(
            discover(&backend, input)
                .await
                .unwrap_err()
                .contains("transport_invalid_response")
        );
        assert_eq!(server.requests.lock().unwrap().len(), 3);
    }
}
