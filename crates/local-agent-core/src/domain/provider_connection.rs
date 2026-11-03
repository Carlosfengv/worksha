//! Host-only catalog selection for the original durable Provider operation.
//! These DTOs are never part of a guest's payload or authority.
use super::provider_access::Target;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Selection {
    pub plugin_id: String,
    pub digest: String,
    pub binding_id: String,
    pub target: Target,
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConnectionRequest {
    pub operation: crate::providers::operations::Request,
    pub catalog: Selection,
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScopedRequest {
    pub store_id: String,
    pub request: super::provider_access::Request,
}
pub fn schemas() -> serde_json::Value {
    serde_json::json!({
        "selection": schemars::schema_for!(Selection),
        "connection-request": schemars::schema_for!(ConnectionRequest),
        "scoped-request": schemars::schema_for!(ScopedRequest),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn host_connection_schemas_are_separate_and_keep_legacy_request_fields() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../contracts/domain/provider-connection");
        for (name, schema) in schemas().as_object().unwrap() {
            let stored: serde_json::Value = serde_json::from_slice(
                &std::fs::read(root.join(format!("{name}.schema.json"))).unwrap(),
            )
            .unwrap();
            assert_eq!(&stored, schema, "{name}");
        }
        let request = schemars::schema_for!(crate::providers::operations::Request);
        let value = serde_json::to_value(request).unwrap();
        assert_eq!(value["properties"].as_object().unwrap().len(), 5);
        assert!(value["properties"].get("catalog").is_none());
        assert!(
            serde_json::from_value::<ScopedRequest>(
                serde_json::json!({"storeId":"owned","request":{},"headers":{}})
            )
            .is_err()
        );
    }
}
