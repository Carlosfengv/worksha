//! Host recovery is independent of browser hints and never resubmits a request.
use super::provider_draft_grant::DraftGrantOwner;
use crate::providers::operations::Request;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum DraftRecoveryAction {
    #[serde(rename_all = "camelCase")]
    List {
        store_id: String,
    },
    Inspect {
        owner: DraftGrantOwner,
    },
    Finish {
        owner: DraftGrantOwner,
    },
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum DraftRecoveryState {
    Unchecked,
    Unattached,
    Pending,
    Committed,
    Failed,
    Cancelled,
    NotFound,
    Unavailable,
}

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum DraftRecoveryOwnership {
    Prepared,
    Managed,
    Shared,
    Missing,
}

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DraftRecoveryEntry {
    pub owner: DraftGrantOwner,
    pub label: String,
    pub plugin_id: Option<String>,
    pub digest: Option<String>,
    /// Scheme/host/port only; no credential reference or private endpoint path.
    pub endpoint_origin: Option<String>,
    pub ownership: DraftRecoveryOwnership,
    pub operation_id: Option<String>,
    pub state: DraftRecoveryState,
    pub can_finish: bool,
}

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DraftRecoveryInspection {
    pub entry: DraftRecoveryEntry,
    /// The original frozen input, for the existing query/cancel API only.
    pub request: Option<Box<Request>>,
}

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum DraftRecoveryReply {
    #[serde(rename_all = "camelCase")]
    Listed {
        store_id: String,
        entries: Vec<DraftRecoveryEntry>,
    },
    Inspected {
        inspection: Box<DraftRecoveryInspection>,
    },
    Finished {
        owner: DraftGrantOwner,
        released: bool,
    },
}

pub fn schemas() -> serde_json::Value {
    serde_json::json!({
        "action": schemars::schema_for!(DraftRecoveryAction),
        "entry": schemars::schema_for!(DraftRecoveryEntry),
        "inspection": schemars::schema_for!(DraftRecoveryInspection),
        "reply": schemars::schema_for!(DraftRecoveryReply),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn recovery_contract_is_strict_separate_and_cannot_submit_or_force() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../contracts/domain/provider-draft-recovery");
        let schemas = schemas();
        assert_eq!(schemas.as_object().unwrap().len(), 4);
        for (name, schema) in schemas.as_object().unwrap() {
            let stored: serde_json::Value = serde_json::from_slice(
                &std::fs::read(root.join(format!("{name}.schema.json"))).unwrap(),
            )
            .unwrap();
            assert_eq!(schema, &stored, "{name}");
        }
        let owner = serde_json::json!({"storeId":"original", "ownerId":"host-issued"});
        for kind in ["inspect", "finish"] {
            let action = serde_json::json!({"kind":kind,"owner":owner});
            assert!(serde_json::from_value::<DraftRecoveryAction>(action.clone()).is_ok());
            for field in ["force", "request", "apiKey", "path", "selection"] {
                let mut invalid = action.clone();
                invalid[field] = serde_json::json!(true);
                assert!(
                    serde_json::from_value::<DraftRecoveryAction>(invalid).is_err(),
                    "{kind}/{field}"
                );
            }
        }
        for kind in ["submit", "retry", "cancel", "acquire", "bind"] {
            assert!(
                serde_json::from_value::<DraftRecoveryAction>(
                    serde_json::json!({"kind":kind,"owner":owner})
                )
                .is_err()
            );
        }
        assert!(
            serde_json::from_value::<DraftRecoveryAction>(
                serde_json::json!({"kind":"list","storeId":"original","cursor":"arbitrary"})
            )
            .is_err()
        );
    }
}
