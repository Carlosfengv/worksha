//! Explicit host repair of policy scope metadata. Never publishes or grants.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PolicyRecoveryScope {
    pub store_id: String,
    pub service_id: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PolicyRecoveryReference {
    pub scope: PolicyRecoveryScope,
    pub reference_id: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum PolicyRecoveryMode {
    Native {},
    PendingOriginal {},
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum PolicyRecoveryAction {
    #[serde(rename_all = "camelCase")]
    List {
        store_id: String,
    },
    Inspect {
        scope: PolicyRecoveryScope,
    },
    Resolve {
        reference: PolicyRecoveryReference,
        mode: PolicyRecoveryMode,
    },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum PolicyRecoveryState {
    Unchecked,
    Consistent,
    NeedsRecovery,
    Blocked,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum PolicyRecoveryProblem {
    MissingControl,
    MarkerMismatch,
    PendingPublication,
    IncompleteEvidence,
    Unavailable,
    Capacity,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PolicyRecoveryEntry {
    pub scope: PolicyRecoveryScope,
    pub service_name: Option<String>,
    /// None means unreadable, not deleted.
    pub service_exists: Option<bool>,
    pub state: PolicyRecoveryState,
    pub problem: Option<PolicyRecoveryProblem>,
    pub pending_operation_ids: Vec<String>,
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PolicyRecoveryInspection {
    pub entry: PolicyRecoveryEntry,
    pub reference: Option<PolicyRecoveryReference>,
    pub current_service_version: Option<String>,
    pub can_restore_native: bool,
    pub can_restore_pending: bool,
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum PolicyRecoveryReply {
    #[serde(rename_all = "camelCase")]
    Listed {
        store_id: String,
        entries: Vec<PolicyRecoveryEntry>,
        limited: bool,
    },
    Inspected {
        inspection: Box<PolicyRecoveryInspection>,
    },
    Resolved {
        reference: PolicyRecoveryReference,
        mode: PolicyRecoveryMode,
        revision: String,
    },
}
pub fn schemas() -> serde_json::Value {
    serde_json::json!({
        "action": schemars::schema_for!(PolicyRecoveryAction),
        "entry": schemars::schema_for!(PolicyRecoveryEntry),
        "inspection": schemars::schema_for!(PolicyRecoveryInspection),
        "reply": schemars::schema_for!(PolicyRecoveryReply),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn recovery_contract_is_strict_and_cannot_publish_or_force() {
        let reference = serde_json::json!({"scope":{"storeId":"host-store","serviceId":"host-service"},"referenceId":"host-reference"});
        for mode in ["native", "pending-original"] {
            let action =
                serde_json::json!({"kind":"resolve","reference":reference,"mode":{"kind":mode}});
            assert!(serde_json::from_value::<PolicyRecoveryAction>(action.clone()).is_ok());
            for field in ["force", "request", "path", "sql", "package", "grant"] {
                for nested in [false, true] {
                    let mut invalid = action.clone();
                    if nested {
                        invalid["mode"][field] = true.into();
                    } else {
                        invalid[field] = true.into();
                    }
                    assert!(serde_json::from_value::<PolicyRecoveryAction>(invalid).is_err());
                }
            }
        }
        for kind in ["publish", "resume", "cancel", "bind", "enable"] {
            assert!(
                serde_json::from_value::<PolicyRecoveryAction>(
                    serde_json::json!({"kind":kind,"reference":reference})
                )
                .is_err()
            );
        }
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../contracts/domain/routing-policy-recovery");
        for (name, schema) in schemas().as_object().unwrap() {
            let stored: serde_json::Value = serde_json::from_slice(
                &std::fs::read(root.join(format!("{name}.schema.json"))).unwrap(),
            )
            .unwrap();
            assert_eq!(schema, &stored, "{name}");
        }
    }
}
