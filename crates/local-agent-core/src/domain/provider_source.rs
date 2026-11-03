//! Host-only remembered source. Remembering an implementation grants no network access.
use super::{provider_access::Policy, provider_connection::Selection};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceQuery {
    pub store_id: String,
    pub provider_id: String,
    pub expected_revision: String,
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Choice {
    pub plugin_id: String,
    pub digest: String,
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum SourceKind {
    Builtin,
    External,
    Pending,
    Changed,
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceState {
    pub provider_id: String,
    pub expected_revision: String,
    pub source_revision: Option<String>,
    pub kind: SourceKind,
    pub choice: Option<Choice>,
    pub selection: Option<Selection>,
    pub authorized: bool,
    pub policy: Option<Policy>,
    pub error_code: Option<String>,
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum SourceAction {
    State {
        query: SourceQuery,
    },
    #[serde(rename_all = "camelCase")]
    Read {
        query: SourceQuery,
        source_revision: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    Select {
        query: SourceQuery,
        source_revision: Option<String>,
        catalog: Option<Selection>,
    },
}
pub fn schemas() -> serde_json::Value {
    serde_json::json!({
        "query":schemars::schema_for!(SourceQuery),"choice":schemars::schema_for!(Choice),
        "state":schemars::schema_for!(SourceState),"action":schemars::schema_for!(SourceAction),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn host_source_schemas_are_separate_and_reject_additional_authority() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../contracts/domain/provider-source");
        for (name, schema) in schemas().as_object().unwrap() {
            let stored: serde_json::Value = serde_json::from_slice(
                &std::fs::read(root.join(format!("{name}.schema.json"))).unwrap(),
            )
            .unwrap();
            assert_eq!(&stored, schema, "{name}");
        }
        let query =
            serde_json::json!({"storeId":"owned","providerId":"openai","expectedRevision":"1"});
        let action = serde_json::json!({"kind":"read","query":query,"sourceRevision":null});
        assert!(serde_json::from_value::<SourceAction>(action.clone()).is_ok());
        let mut invalid = action;
        invalid["endpoint"] = "https://other.invalid".into();
        assert!(serde_json::from_value::<SourceAction>(invalid).is_err());
        let mut invalid = query;
        invalid["headers"] = serde_json::json!({"authorization":"secret"});
        assert!(serde_json::from_value::<SourceQuery>(invalid).is_err());
    }
}
