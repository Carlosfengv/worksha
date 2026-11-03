//! Host-owned export of an existing unified request selection. No path or guest
//! execution capability crosses this contract; the original export stays separate.
use super::query::{RequestSourceSummary, UnifiedRequestQuery};
use crate::domain::{Decimal, SourceState};
use crate::requests::export::ExportError;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
pub mod csv;

pub const API_VERSION: &str = "1.0.0";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Begin {
    pub operation_id: String,
    pub snapshot_token: String,
}
impl Begin {
    pub fn validate(&self) -> Result<(), ExportError> {
        if uuid::Uuid::parse_str(&self.operation_id).is_err()
            || self.snapshot_token.is_empty()
            || self.snapshot_token.len() > 80
        {
            return Err(ExportError::Invalid);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Info {
    pub api_version: String,
    pub operation_id: String,
    pub snapshot_token: String,
    /// Request/response records only, excluding the CSV header and scope row.
    pub count: Decimal,
    pub as_of: String,
    pub applied_query: UnifiedRequestQuery,
    pub source_states: Vec<SourceState>,
    pub source_summaries: Vec<RequestSourceSummary>,
    pub overlap_unknown: bool,
}
impl Info {
    pub fn validate(&self) -> Result<(), ExportError> {
        Begin {
            operation_id: self.operation_id.clone(),
            snapshot_token: self.snapshot_token.clone(),
        }
        .validate()?;
        self.applied_query
            .validate()
            .map_err(|_| ExportError::Invalid)?;
        let expected = self
            .applied_query
            .scope
            .source_ids
            .iter()
            .collect::<BTreeSet<_>>();
        let states = self
            .source_states
            .iter()
            .map(|s| &s.source_id)
            .collect::<BTreeSet<_>>();
        let summaries = self
            .source_summaries
            .iter()
            .map(|s| &s.source_id)
            .collect::<BTreeSet<_>>();
        if self.api_version != API_VERSION
            || self.count.value() > i64::MAX as u64
            || chrono::DateTime::parse_from_rfc3339(&self.as_of).is_err()
            || self.applied_query.snapshot_token.is_some()
            || self.applied_query.cursor.is_some()
            || states != expected
            || summaries != expected
            || states.len() != self.source_states.len()
            || summaries.len() != self.source_summaries.len()
            || self
                .source_summaries
                .iter()
                .any(|s| s.unit != self.applied_query.unit)
        {
            return Err(ExportError::Invalid);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Read {
    pub offset: Decimal,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Chunk {
    pub operation_id: String,
    pub offset: Decimal,
    pub next_offset: Option<Decimal>,
    pub rows: Decimal,
    pub csv: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Finish {
    pub next_offset: Decimal,
    pub rows: Decimal,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(
    tag = "status",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum SaveReply {
    Saved {
        info: Box<Info>,
        file_name: Option<String>,
    },
    Cancelled {},
}

pub fn schemas() -> serde_json::Value {
    serde_json::json!({
        "begin": schemars::schema_for!(Begin), "info": schemars::schema_for!(Info),
        "read": schemars::schema_for!(Read), "chunk": schemars::schema_for!(Chunk),
        "finish": schemars::schema_for!(Finish), "save-reply": schemars::schema_for!(SaveReply),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn unified_export_committed_schemas_agree_with_rust_serialization() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../contracts/domain/request-export");
        for (name, schema) in schemas().as_object().unwrap() {
            let committed: serde_json::Value = serde_json::from_slice(
                &std::fs::read(root.join(format!("{name}.schema.json"))).unwrap(),
            )
            .unwrap();
            assert_eq!(*schema, committed, "{name}");
        }
    }

    #[test]
    fn unified_export_inputs_reject_paths_foreign_fields_and_noncanonical_offsets() {
        let operation = uuid::Uuid::new_v4().to_string();
        let input = json!({"operationId":operation,"snapshotToken":"opaque-snapshot"});
        serde_json::from_value::<Begin>(input.clone())
            .unwrap()
            .validate()
            .unwrap();
        let mut path = input;
        path["path"] = json!("/ungranted/destination");
        assert!(serde_json::from_value::<Begin>(path).is_err());
        for offset in [
            json!(0),
            json!("-1"),
            json!("01"),
            json!("1e3"),
            json!("18446744073709551616"),
        ] {
            assert!(serde_json::from_value::<Read>(json!({"offset":offset})).is_err());
        }
        assert_eq!(
            serde_json::from_value::<Read>(json!({"offset":"9007199254740993"}))
                .unwrap()
                .offset
                .value(),
            9_007_199_254_740_993
        );
        assert!(
            serde_json::from_value::<SaveReply>(json!({"status":"cancelled","count":"0"})).is_err()
        );
        assert!(serde_json::from_value::<SaveReply>(json!({"status":"cancelled"})).is_ok());
    }
}
