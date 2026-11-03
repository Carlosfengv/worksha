//! Memory metadata and explicitly requested previews. Paths stay in the native broker.
use super::{Coverage, Decimal, ErrorCode, Failure, Invocation, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MemoryRequest {
    pub invocation: Invocation,
    pub source_id: String,
    pub product_id: String,
    pub file_id: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MemoryFile {
    pub id: String,
    pub name: String,
    pub scope_label: String,
    pub size_bytes: Decimal,
    pub modified_at: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MemoryPreview {
    pub id: String,
    pub text: String,
    pub truncated: bool,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MemorySnapshot {
    pub source_id: String,
    pub product_id: String,
    pub shared_product_ids: Vec<String>,
    pub files: Vec<MemoryFile>,
    pub selected: Option<MemoryPreview>,
    pub observed_at: String,
    /// The known default locations do not establish coverage of custom/remote memory.
    pub coverage: Coverage,
    pub limited: bool,
    /// Discovery is not evidence that a client or Session loaded these files.
    pub loaded_in_client: Option<bool>,
}
pub trait MemorySource {
    fn read_memory(&mut self, request: &MemoryRequest) -> Result<MemorySnapshot>;
}
impl MemorySnapshot {
    pub fn validate(&self, request: &MemoryRequest) -> Result<()> {
        let invalid = || Failure::new(ErrorCode::InvalidInput);
        if self.source_id != request.source_id
            || self.product_id != request.product_id
            || chrono::DateTime::parse_from_rfc3339(&self.observed_at).is_err()
            || self
                .files
                .iter()
                .map(|f| &f.id)
                .collect::<HashSet<_>>()
                .len()
                != self.files.len()
            || self
                .files
                .iter()
                .any(|f| f.id.is_empty() || f.id.len() > 240)
            || self
                .selected
                .as_ref()
                .is_some_and(|s| request.file_id.as_ref() != Some(&s.id))
            || request.file_id.is_some() != self.selected.is_some()
        {
            return Err(invalid());
        }
        if self.files.len() > 500
            || self
                .selected
                .as_ref()
                .is_some_and(|p| p.text.chars().count() > 16384)
        {
            return Err(Failure::new(ErrorCode::ResourceLimit));
        }
        // The broker authorizes the preview independently of the metadata pass: a file
        // can disappear after it was opened without changing the old read's semantics.
        Ok(())
    }
}
