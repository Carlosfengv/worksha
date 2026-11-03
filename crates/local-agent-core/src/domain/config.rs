//! Shared advice and result inspection; execution and native documents remain host-owned.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const API_VERSION: &str = "1.0.0";
pub const CREDENTIAL_PLACEHOLDER: &str = "${HOST_CREDENTIAL}";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FieldEdit {
    pub pointer: String,
    pub value: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NativeBinding {
    /// An opaque identity of the host's provider snapshot, never a credential reference.
    pub identity: String,
    pub endpoint: String,
    pub model: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum ConfigIntent {
    #[serde(rename_all = "camelCase")]
    NativeRoute {
        agent_id: String,
        binding: Option<NativeBinding>,
    },
    JsonFields {
        edits: Vec<FieldEdit>,
    },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfigRequest {
    pub target_revision: String,
    pub intent: ConfigIntent,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FieldsCandidate {
    pub edits: Vec<FieldEdit>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlanError {
    InvalidInput,
    PermissionDenied,
    StaleSnapshot,
    Unsupported,
}
impl PlanError {
    pub fn code(self) -> &'static str {
        match self {
            Self::InvalidInput => "invalid_input",
            Self::PermissionDenied => "permission_denied",
            Self::StaleSnapshot => "stale_snapshot",
            Self::Unsupported => "unsupported",
        }
    }
}
impl ConfigRequest {
    pub fn validate(&self) -> Result<(), PlanError> {
        if self.target_revision.is_empty()
            || self.target_revision.len() > 256
            || self.target_revision.chars().any(char::is_control)
        {
            return Err(PlanError::InvalidInput);
        }
        match &self.intent {
            ConfigIntent::NativeRoute { agent_id, binding } => {
                if agent_id.is_empty()
                    || agent_id.len() > 100
                    || agent_id.chars().any(char::is_control)
                    || binding
                        .as_ref()
                        .is_some_and(|b| b.identity.is_empty() || b.identity.len() > 256)
                {
                    return Err(PlanError::InvalidInput);
                }
            }
            ConfigIntent::JsonFields { edits } => {
                let mut fields = std::collections::BTreeSet::new();
                if edits.is_empty()
                    || edits.len() > 8
                    || edits
                        .iter()
                        .any(|e| !valid_pointer(&e.pointer) || !fields.insert(&e.pointer))
                {
                    return Err(PlanError::InvalidInput);
                }
            }
        }
        Ok(())
    }
    pub fn identity(&self) -> String {
        crate::providers::files::hash(
            &serde_json::to_vec(self).expect("serializable config advice"),
        )
    }
}
pub fn valid_pointer(s: &str) -> bool {
    s.starts_with('/')
        && s.len() <= 160
        && s.split('/').skip(1).all(|part| {
            !part.is_empty()
                && !["__proto__", "prototype", "constructor"].contains(&part)
                && part
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
        })
}

// Deliberately neither Serialize nor Debug: a trusted native document can contain
// pre-existing private fields. Only the JSON-fields job is ever sent to a guest.
pub enum ConfigOutput {
    NativeDocument(Vec<u8>),
    JsonFields(Vec<FieldEdit>),
}
pub struct ConfigProposal {
    pub request_identity: String,
    pub output: ConfigOutput,
}
pub enum ConfigWork {
    Ready(ConfigProposal),
    JsonProposal { edits: Vec<FieldEdit> },
}
pub trait ConfigPlanner {
    type Error;
    fn propose(&self, request: &ConfigRequest) -> Result<ConfigWork, Self::Error>;
    fn inspect_result(
        &self,
        request: &ConfigRequest,
        result: ConfigProposal,
    ) -> Result<ConfigProposal, Self::Error>;
}
pub fn inspect_result(
    request: &ConfigRequest,
    result: ConfigProposal,
) -> Result<ConfigProposal, PlanError> {
    request.validate()?;
    if result.request_identity != request.identity() {
        return Err(PlanError::StaleSnapshot);
    }
    match (&request.intent, &result.output) {
        (ConfigIntent::NativeRoute { .. }, ConfigOutput::NativeDocument(_)) => (),
        (ConfigIntent::JsonFields { edits }, ConfigOutput::JsonFields(actual))
            if edits == actual => {}
        _ => return Err(PlanError::PermissionDenied),
    }
    Ok(result)
}

/// The guest adapter executes its RPC after obtaining this finite job.
/// It never receives a native context, filename, helper path, backup or secret.
pub struct FiniteJsonPlanner;
impl ConfigPlanner for FiniteJsonPlanner {
    type Error = PlanError;
    fn propose(&self, request: &ConfigRequest) -> Result<ConfigWork, PlanError> {
        request.validate()?;
        match &request.intent {
            ConfigIntent::JsonFields { edits } => Ok(ConfigWork::JsonProposal {
                edits: edits.clone(),
            }),
            ConfigIntent::NativeRoute { .. } => Err(PlanError::PermissionDenied),
        }
    }
    fn inspect_result(
        &self,
        request: &ConfigRequest,
        result: ConfigProposal,
    ) -> Result<ConfigProposal, PlanError> {
        if !matches!(request.intent, ConfigIntent::JsonFields { .. }) {
            return Err(PlanError::PermissionDenied);
        }
        inspect_result(request, result)
    }
}

pub fn schemas() -> serde_json::Value {
    serde_json::json!({
        "config-request": schemars::schema_for!(ConfigRequest),
        "config-intent": schemars::schema_for!(ConfigIntent),
        "native-binding": schemars::schema_for!(NativeBinding),
        "field-edit": schemars::schema_for!(FieldEdit),
        "fields-candidate": schemars::schema_for!(FieldsCandidate),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn config_schema_does_not_publish_native_documents_or_change_old_contracts() {
        assert_eq!(API_VERSION, "1.0.0");
        let root =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../contracts/domain/config");
        let value = schemas();
        assert_eq!(value.as_object().unwrap().len(), 5);
        assert!(
            !value.to_string().contains("NativeDocument")
                && !value.to_string().contains("credentialRef")
        );
        for (name, schema) in value.as_object().unwrap() {
            let saved: serde_json::Value = serde_json::from_slice(
                &std::fs::read(root.join(format!("{name}.schema.json"))).unwrap(),
            )
            .unwrap();
            assert_eq!(&saved, schema, "{name}");
        }
    }
    fn request() -> ConfigRequest {
        ConfigRequest {
            target_revision: "owned-revision".into(),
            intent: ConfigIntent::JsonFields {
                edits: vec![
                    FieldEdit {
                        pointer: "/connection/model".into(),
                        value: "owned-model".into(),
                    },
                    FieldEdit {
                        pointer: "/connection/key".into(),
                        value: CREDENTIAL_PLACEHOLDER.into(),
                    },
                ],
            },
        }
    }
    #[test]
    fn finite_json_planning_rejects_changed_target_values_and_native_output() {
        let r = request();
        let planner = FiniteJsonPlanner;
        let ConfigWork::JsonProposal { edits } = planner.propose(&r).unwrap() else {
            panic!("expected a finite JSON job");
        };
        let original = edits.clone();
        let accepted = planner
            .inspect_result(
                &r,
                ConfigProposal {
                    request_identity: r.identity(),
                    output: ConfigOutput::JsonFields(edits),
                },
            )
            .ok()
            .unwrap();
        let mut changed = r.clone();
        changed.target_revision = "different-target".into();
        assert_eq!(
            planner.inspect_result(&changed, accepted).err().unwrap(),
            PlanError::StaleSnapshot
        );
        let mut edits = original;
        edits[0].value = "unapproved".into();
        assert_eq!(
            planner
                .inspect_result(
                    &r,
                    ConfigProposal {
                        request_identity: r.identity(),
                        output: ConfigOutput::JsonFields(edits)
                    }
                )
                .err()
                .unwrap(),
            PlanError::PermissionDenied
        );
        assert_eq!(
            planner
                .inspect_result(
                    &r,
                    ConfigProposal {
                        request_identity: r.identity(),
                        output: ConfigOutput::NativeDocument(b"private document".to_vec())
                    }
                )
                .err()
                .unwrap(),
            PlanError::PermissionDenied
        );
        let native = ConfigRequest {
            target_revision: "absent".into(),
            intent: ConfigIntent::NativeRoute {
                agent_id: "claude".into(),
                binding: None,
            },
        };
        assert_eq!(
            planner.propose(&native).err().unwrap(),
            PlanError::PermissionDenied
        );
    }
    #[test]
    fn bounded_fields_share_the_legacy_pointer_rules_and_keep_strict_dtos() {
        for pointer in [
            "",
            "/",
            "/a/",
            "/a/../b",
            "/a~1b",
            "/constructor/x",
            "/__proto__",
            "/中文",
            "/a\0",
        ] {
            assert!(!valid_pointer(pointer), "{pointer}");
        }
        let mut r = request();
        if let ConfigIntent::JsonFields { edits } = &mut r.intent {
            edits.push(edits[0].clone());
        }
        assert_eq!(r.validate().unwrap_err(), PlanError::InvalidInput);
        let encoded = serde_json::to_value(request()).unwrap();
        assert!(!encoded.to_string().contains("credentialRef"));
        let mut extra = encoded;
        extra["nativeDocument"] = serde_json::json!("forbidden");
        assert!(serde_json::from_value::<ConfigRequest>(extra).is_err());
        assert!(
            serde_json::from_value::<FieldsCandidate>(
                serde_json::json!({"edits":[{"pointer":"/a","value":{"secret":"x"}}]})
            )
            .is_err()
        );
    }
}
