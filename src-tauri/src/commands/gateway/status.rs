use crate::backend::ConnectionContext;
use serde::{Deserialize, Serialize};
use serde_json::Value;

fn invalid() -> String {
    serde_json::json!({
        "code":"transport_invalid_response",
        "outcomeUnknown":false,
        "message":"网关状态与后台身份不可核对，请重新读取当前后台。"
    })
    .to_string()
}

pub(super) async fn read(context: &ConnectionContext) -> Result<Value, String> {
    let value = context
        .request(reqwest::Method::GET, "/api/gateway-lan", None)
        .await?;
    context.ensure_current()?;
    project(value, context.persistent())
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Status {
    listen: Option<String>,
    concurrency: Option<usize>,
    active: Option<usize>,
    clients: Vec<Client>,
    #[serde(skip_serializing_if = "Option::is_none")]
    write_protocol: Option<Protocol>,
    persistent: bool,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Protocol {
    version: u32,
    store_id: String,
    max_clients: usize,
    secret_replay: bool,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Client {
    id: String,
    name: String,
    agent: String,
    provider_id: String,
    models: Vec<String>,
    concurrency: usize,
    active: usize,
    enabled: bool,
    created_at: String,
}
pub(super) fn project(mut value: Value, persistent: bool) -> Result<Value, String> {
    let object = value.as_object_mut().ok_or_else(invalid)?;
    if ["listen", "concurrency", "active", "clients"]
        .iter()
        .any(|key| !object.contains_key(*key))
    {
        return Err(invalid());
    }
    object.insert("persistent".into(), persistent.into());
    let status: Status = serde_json::from_value(value).map_err(|_| invalid())?;
    if status.clients.len() > 128 {
        return Err(invalid());
    }
    serde_json::to_value(status).map_err(|_| invalid())
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::{Backend, test_server::serve};
    use std::sync::Arc;
    use tokio::sync::Notify;

    fn empty_status() -> Value {
        serde_json::json!({"listen":null,"concurrency":null,"active":null,"clients":[]})
    }

    #[test]
    fn runtime_wire_omits_false_but_typed_identity_preserves_both_lifetimes() {
        use local_agent_core::runtime::{Identity, PROTOCOL};
        for persistent in [false, true] {
            let identity = Identity {
                protocol: PROTOCOL,
                version: env!("CARGO_PKG_VERSION").into(),
                instance_id: uuid::Uuid::new_v4().to_string(),
                pid: 42,
                persistent,
            };
            let value = serde_json::to_value(&identity).unwrap();
            assert_eq!(
                value.get("persistent"),
                persistent.then_some(&Value::Bool(true))
            );
            assert_eq!(
                serde_json::from_value::<Identity>(value.clone()).unwrap(),
                identity
            );
            for bad in [
                Value::Null,
                Value::String("false".into()),
                serde_json::json!(0),
            ] {
                let mut invalid = value.clone();
                invalid["persistent"] = bad;
                assert!(serde_json::from_value::<Identity>(invalid).is_err());
            }
        }
    }

    #[tokio::test]
    async fn status_reuses_verified_lifetime_without_a_second_runtime_read_or_mutation() {
        for persistent in [false, true] {
            let server = serve(move |request| async move {
                assert_eq!(request.method, "GET");
                assert_eq!(request.path, "/api/gateway-lan");
                let mut value = empty_status();
                value["persistent"] = (!persistent).into();
                value["token"] = "private-sentinel".into();
                value.to_string()
            })
            .await;
            let backend = Backend::test_connection_mode(server.endpoint.clone(), persistent);
            let context = backend.context().unwrap();
            let value = read(&context).await.unwrap();
            assert_eq!(value["persistent"], persistent);
            assert_eq!(value["listen"], Value::Null);
            assert_eq!(value["clients"], serde_json::json!([]));
            assert!(value.get("token").is_none());
            let calls = server.requests.lock().unwrap();
            assert_eq!(calls.len(), 1);
            assert_eq!(calls[0].authorization, "Bearer export-fixture-token");
        }
    }

    #[tokio::test]
    async fn connection_change_during_status_read_does_not_publish_old_lifetime_or_contact_new_server()
     {
        let entered = Arc::new(Notify::new());
        let released = Arc::new(Notify::new());
        let (hit, gate) = (entered.clone(), released.clone());
        let a = serve(move |_| {
            let (hit, gate) = (hit.clone(), gate.clone());
            async move {
                hit.notify_one();
                gate.notified().await;
                empty_status().to_string()
            }
        })
        .await;
        let b = serve(|_| async { empty_status().to_string() }).await;
        let backend = Backend::test_connection_mode(a.endpoint.clone(), true);
        let context = backend.context().unwrap();
        let task = tokio::spawn(async move { read(&context).await });
        tokio::time::timeout(std::time::Duration::from_secs(2), entered.notified())
            .await
            .unwrap();
        backend.test_install_connection(b.endpoint.clone());
        released.notify_one();
        let failure: Value = serde_json::from_str(&task.await.unwrap().unwrap_err()).unwrap();
        assert_eq!(failure["code"], "backend_connection_changed");
        assert_eq!(failure["outcomeUnknown"], false);
        assert_eq!(a.requests.lock().unwrap().len(), 1);
        assert!(b.requests.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn malformed_status_is_a_read_failure_without_claiming_a_pending_client_mutation() {
        let server = serve(|_| async { "{\"clients\":[]}".into() }).await;
        let backend = Backend::test_connection(server.endpoint.clone());
        let failure: Value =
            serde_json::from_str(&read(&backend.context().unwrap()).await.unwrap_err()).unwrap();
        assert_eq!(failure["code"], "transport_invalid_response");
        assert_eq!(failure["outcomeUnknown"], false);
        assert_eq!(
            failure["message"],
            "网关状态与后台身份不可核对，请重新读取当前后台。"
        );
        assert_eq!(server.requests.lock().unwrap().len(), 1);
    }
    #[test]
    fn projects_only_public_status_and_rejects_missing_structure() {
        let value = serde_json::json!({"listen":null,"concurrency":null,"active":null,"clients":[],"key":"untrusted","token":"untrusted"});
        assert_eq!(
            project(value, true).unwrap(),
            serde_json::json!({"listen":null,"concurrency":null,"active":null,"clients":[],"persistent":true})
        );
        for value in [
            Value::Null,
            serde_json::json!([]),
            serde_json::json!({}),
            serde_json::json!({"clients":[]}),
        ] {
            assert!(project(value, false).is_err());
        }
    }
}
