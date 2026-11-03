use super::Usage;
use super::text::num;
use serde_json::Value;

impl Usage {
    pub(in crate::sessions) fn parse(v: &Value, claude: bool) -> Self {
        let cached = num(
            v,
            if claude {
                "cache_read_input_tokens"
            } else {
                "cached_input_tokens"
            },
        );
        let write = num(
            v,
            if claude {
                "cache_creation_input_tokens"
            } else {
                "cache_write_input_tokens"
            },
        );
        let input = num(v, "input_tokens").map(|n| {
            if claude {
                n.saturating_add(cached.unwrap_or(0))
                    .saturating_add(write.unwrap_or(0))
            } else {
                n
            }
        });
        let output = num(v, "output_tokens");
        Self {
            input_tokens: input,
            output_tokens: output,
            cached_input_tokens: cached,
            cache_write_input_tokens: write,
            reasoning_output_tokens: num(v, "reasoning_output_tokens"),
            total_tokens: input.zip(output).map(|(a, b)| a.saturating_add(b)),
        }
    }
    pub(in crate::sessions) fn merge_max(&mut self, other: &Self) {
        self.input_tokens = self.input_tokens.max(other.input_tokens);
        self.output_tokens = self.output_tokens.max(other.output_tokens);
        self.cached_input_tokens = self.cached_input_tokens.max(other.cached_input_tokens);
        self.cache_write_input_tokens = self
            .cache_write_input_tokens
            .max(other.cache_write_input_tokens);
        self.reasoning_output_tokens = self
            .reasoning_output_tokens
            .max(other.reasoning_output_tokens);
        self.total_tokens = self
            .input_tokens
            .zip(self.output_tokens)
            .map(|(a, b)| a.saturating_add(b));
    }
}
