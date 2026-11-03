//! Host-only official evidence augments validated common model metadata.
use local_agent_core::domain::provider::{ModelCapabilities, ModelMetadata};
use serde::Serialize;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Metadata {
    pub(super) context_window: Option<u64>,
    max_input_tokens: Option<u64>,
    max_output_tokens: Option<u64>,
    pub(super) capabilities: ModelCapabilities,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(super) documentation: Vec<super::official::Evidence>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) context_alias: Option<super::official::ContextAlias>,
}
impl From<ModelMetadata> for Metadata {
    fn from(value: ModelMetadata) -> Self {
        Self {
            context_window: value.context_window,
            max_input_tokens: value.max_input_tokens,
            max_output_tokens: value.max_output_tokens,
            capabilities: value.capabilities,
            documentation: vec![],
            context_alias: None,
        }
    }
}
