//! Host-only Draft reads. The origin guards remembered choice, never supplies credentials.
use super::provider_source::SourceQuery;
use super::provider_source_operation::SourceChoice;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum DraftOrigin {
    #[serde(rename_all = "camelCase")]
    Saved {
        query: SourceQuery,
        source_revision: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    New {
        store_id: String,
        provider_id: String,
    },
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DraftEndpoint {
    pub endpoint: String,
    pub protocol: String,
    pub vendor: String,
    pub credential_ref: Option<String>,
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DraftCatalogRequest {
    pub origin: DraftOrigin,
    pub draft: DraftEndpoint,
    pub source: SourceChoice,
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum DraftCatalogAction {
    Check { request: DraftCatalogRequest },
    Read { request: DraftCatalogRequest },
}
pub fn schemas() -> serde_json::Value {
    serde_json::json!({
        "origin":schemars::schema_for!(DraftOrigin),
        "endpoint":schemars::schema_for!(DraftEndpoint),
        "request":schemars::schema_for!(DraftCatalogRequest),
        "action":schemars::schema_for!(DraftCatalogAction),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn draft_catalog_is_a_separate_strict_host_contract() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../contracts/domain/provider-draft-catalog");
        for (name, schema) in schemas().as_object().unwrap() {
            let stored: serde_json::Value = serde_json::from_slice(
                &std::fs::read(root.join(format!("{name}.schema.json"))).unwrap(),
            )
            .unwrap();
            assert_eq!(&stored, schema, "{name}");
        }
        let value = serde_json::json!({"kind":"check","request":{
            "origin":{"kind":"new","storeId":"owned","providerId":"new"},
            "draft":{"endpoint":"https://example.invalid","protocol":"responses","vendor":"custom","credentialRef":null},
            "source":{"kind":"builtin"}}});
        assert!(serde_json::from_value::<DraftCatalogAction>(value.clone()).is_ok());
        for pointer in [
            "/headers",
            "/request/draft/secret",
            "/request/source/catalog",
            "/request/origin/sourceRevision",
        ] {
            let mut invalid = value.clone();
            let (parent, field) = pointer.rsplit_once('/').unwrap();
            invalid
                .pointer_mut(parent)
                .unwrap()
                .as_object_mut()
                .unwrap()
                .insert(field.into(), serde_json::Value::Null);
            assert!(
                serde_json::from_value::<DraftCatalogAction>(invalid).is_err(),
                "{pointer}"
            );
        }
    }
}
