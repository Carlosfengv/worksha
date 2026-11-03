use super::read_retention_inventory;
use crate::backend::{Backend, test_server::serve};
use local_agent_core::observation::retention::impact::InventoryRequest;
use serde_json::json;

const STORE: &str = "00000000-0000-4000-8000-000000000001";
fn request(version: Option<u8>) -> InventoryRequest {
    let mut value = json!({"storeId":STORE,"expectedSettingsRevision":7,"days":30});
    if let Some(version) = version {
        value["inventoryVersion"] = json!(version);
    }
    serde_json::from_value(value).unwrap()
}
#[tokio::test]
async fn omitted_and_explicit_v1_keep_the_old_service_wire_contract() {
    for version in [None, Some(1)] {
        let server=serve(|request|async move {
            assert_eq!(request.method,"GET");
            // Models the legacy server's deny_unknown_fields query parser.
            assert_eq!(request.path,format!("/api/settings/retention-inventory?storeId={STORE}&expectedSettingsRevision=7&days=30"));
            json!({"version":1,"legacy":"accepted"}).to_string()
        }).await;
        let backend = Backend::test_connection(server.endpoint.clone());
        assert_eq!(
            read_retention_inventory(&backend, request(version))
                .await
                .unwrap(),
            json!({"version":1,"legacy":"accepted"})
        );
        let requests = server.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].authorization, "Bearer export-fixture-token");
    }
}
#[tokio::test]
async fn only_explicit_v2_negotiates_the_broader_inventory_scope() {
    let server=serve(|request|async move {
        assert_eq!(request.method,"GET");assert_eq!(request.path,format!("/api/settings/retention-inventory?storeId={STORE}&expectedSettingsRevision=7&days=30&inventoryVersion=2"));json!({"version":2,"activityCaches":{}}).to_string()
    }).await;
    let backend = Backend::test_connection(server.endpoint.clone());
    let value = read_retention_inventory(&backend, request(Some(2)))
        .await
        .unwrap();
    assert_eq!(value["version"], 2);
    assert_eq!(server.requests.lock().unwrap().len(), 1);
}
#[tokio::test]
async fn unsupported_inventory_formats_never_contact_a_service() {
    let server = serve(|_| async { panic!("invalid format reached the service") }).await;
    let backend = Backend::test_connection(server.endpoint.clone());
    for version in [0, 3] {
        assert!(
            read_retention_inventory(&backend, request(Some(version)))
                .await
                .is_err()
        );
    }
    assert!(server.requests.lock().unwrap().is_empty());
}
