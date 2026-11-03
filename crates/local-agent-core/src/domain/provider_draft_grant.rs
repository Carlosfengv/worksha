//! Host-owned lifetimes for editor catalog grants, separate from guest permissions.
use super::{
    provider_access::Query, provider_connection::Selection, provider_draft_catalog::DraftEndpoint,
    provider_source_operation::SourceRequest,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DraftGrantOwner {
    pub store_id: String,
    pub owner_id: String,
}

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DraftGrantScope {
    pub catalog: Query,
    pub draft: DraftEndpoint,
    pub label: String,
    pub allow_private_network: bool,
}

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum DraftGrantAction {
    #[serde(rename_all = "camelCase")]
    Open {
        store_id: String,
        scope: DraftGrantScope,
    },
    Acquire {
        owner: DraftGrantOwner,
    },
    Attach {
        owner: DraftGrantOwner,
        request: Box<SourceRequest>,
    },
    Release {
        owner: DraftGrantOwner,
    },
}

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum DraftGrantState {
    Prepared,
    Acquired,
    Borrowed,
    Pending,
    Released,
}

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DraftGrantReceipt {
    pub owner: DraftGrantOwner,
    pub state: DraftGrantState,
    pub selection: Option<Selection>,
}

pub fn schemas() -> serde_json::Value {
    serde_json::json!({
        "owner":schemars::schema_for!(DraftGrantOwner),
        "scope":schemars::schema_for!(DraftGrantScope),
        "action":schemars::schema_for!(DraftGrantAction),
        "receipt":schemars::schema_for!(DraftGrantReceipt),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn draft_ownership_is_separate_strict_and_never_accepts_plain_keys_or_saved_targets() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../contracts/domain/provider-draft-grant");
        let schemas = schemas();
        assert_eq!(schemas.as_object().unwrap().len(), 4);
        for (name, schema) in schemas.as_object().unwrap() {
            let stored: serde_json::Value = serde_json::from_slice(
                &std::fs::read(root.join(format!("{name}.schema.json"))).unwrap(),
            )
            .unwrap();
            assert_eq!(schema, &stored, "{name}");
        }
        let owner = serde_json::json!({"storeId":"owned","ownerId":"host-issued"});
        assert!(serde_json::from_value::<DraftGrantAction>(serde_json::json!({
            "kind":"acquire", "owner":owner, "target":{"kind":"saved","providerId":"other","expectedRevision":"1"}
        })).is_err());
        assert!(
            serde_json::from_value::<DraftGrantAction>(serde_json::json!({
                "kind":"release", "owner":owner, "force":true
            }))
            .is_err()
        );
        assert!(serde_json::from_value::<DraftGrantScope>(serde_json::json!({
            "catalog":{"pluginId":"owned","digest":"digest"},"label":"Owned","allowPrivateNetwork":false,
            "draft":{"endpoint":"https://example.com","protocol":"responses","vendor":"custom","apiKey":"not-accepted"}
        })).is_err());
    }
}
