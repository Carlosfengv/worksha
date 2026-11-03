//! Shared token vocabulary; source-specific parsing stays in the adapter.
use serde::Serialize;

#[derive(Debug, Clone, Default, Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cached_input_tokens: Option<u64>,
    pub cache_write_input_tokens: Option<u64>,
    pub reasoning_output_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
}
impl Usage {
    pub(crate) fn add(&mut self, other: &Self) {
        fn add(a: Option<u64>, b: Option<u64>) -> Option<u64> {
            match (a, b) {
                (None, None) => None,
                _ => Some(a.unwrap_or(0).saturating_add(b.unwrap_or(0))),
            }
        }
        self.input_tokens = add(self.input_tokens, other.input_tokens);
        self.output_tokens = add(self.output_tokens, other.output_tokens);
        self.cached_input_tokens = add(self.cached_input_tokens, other.cached_input_tokens);
        self.cache_write_input_tokens = add(
            self.cache_write_input_tokens,
            other.cache_write_input_tokens,
        );
        self.reasoning_output_tokens =
            add(self.reasoning_output_tokens, other.reasoning_output_tokens);
        self.total_tokens = add(self.total_tokens, other.total_tokens);
    }
}
