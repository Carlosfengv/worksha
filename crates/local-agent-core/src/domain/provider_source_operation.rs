//! Host-only atomic source choice accompanying the original durable operation.
use super::provider_connection::Selection;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum SourceChoice {
    Builtin {},
    External { catalog: Selection },
}
impl SourceChoice {
    pub fn catalog(&self) -> Option<&Selection> {
        match self {
            Self::Builtin {} => None,
            Self::External { catalog } => Some(catalog),
        }
    }
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceRequest {
    pub operation: crate::providers::operations::Request,
    pub source: SourceChoice,
    pub expected_source_revision: Option<String>,
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum SourceOperationAction {
    Commit { request: SourceRequest },
    Query { request: SourceRequest },
}
pub fn schemas() -> serde_json::Value {
    serde_json::json!({
        "choice":schemars::schema_for!(SourceChoice),
        "request":schemars::schema_for!(SourceRequest),
        "action":schemars::schema_for!(SourceOperationAction),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn source_operation_contract_is_additive_and_never_extends_old_request() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../contracts/domain/provider-source-operation");
        for (name, schema) in schemas().as_object().unwrap() {
            let stored: serde_json::Value = serde_json::from_slice(
                &std::fs::read(root.join(format!("{name}.schema.json"))).unwrap(),
            )
            .unwrap();
            assert_eq!(&stored, schema, "{name}");
        }
        assert!(
            serde_json::from_value::<SourceChoice>(
                serde_json::json!({"kind":"builtin","catalog":null})
            )
            .is_err()
        );
        assert!(
            serde_json::from_value::<SourceChoice>(serde_json::json!({"kind":"external"})).is_err()
        );
        let old =
            serde_json::to_value(schemars::schema_for!(crate::providers::operations::Request))
                .unwrap();
        assert_eq!(old["properties"].as_object().unwrap().len(), 5);
    }
}
