//! Numeric, message-bound activity measurements. No content or credentials.
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const RATE_WINDOW_MS: u32 = 5_000;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RateSample {
    pub at: i64,
    pub window_ms: u32,
    pub tokens_per_second: f64,
}
impl RateSample {
    pub fn valid(&self) -> bool {
        self.at > 0
            && self.window_ms == RATE_WINDOW_MS
            && self.tokens_per_second.is_finite()
            && self.tokens_per_second > 0.0
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MessageUsage {
    pub output_tokens: u64,
    pub reasoning_tokens: Option<u64>,
    pub at: String,
}
impl MessageUsage {
    pub fn valid(&self) -> bool {
        self.reasoning_tokens
            .is_none_or(|n| n <= self.output_tokens)
            && self.output_tokens <= 1_000_000_000_000
            && chrono::DateTime::parse_from_rfc3339(&self.at).is_ok()
    }
    /// Only the explicit receiving turn's cumulative usage is eligible.
    /// Thread/session counters and individual response counters are not substitutes.
    pub fn codex(value: &Value, session: &str, turn: &str) -> Option<Self> {
        let p = &value["payload"];
        if value["type"] != "token_usage_record"
            || p["thread_id"] != session
            || p["turn_id"] != turn
            || uuid::Uuid::parse_str(session).is_err()
            || uuid::Uuid::parse_str(turn).is_err()
        {
            return None;
        }
        let at = chrono::DateTime::parse_from_rfc3339(value["timestamp"].as_str()?).ok()?;
        if at > chrono::Utc::now() + chrono::Duration::minutes(5) {
            return None;
        }
        let usage = &p["turn_token_usage"];
        let result = Self {
            output_tokens: usage["output_tokens"].as_u64()?,
            reasoning_tokens: if usage["reasoning_output_tokens"].is_null() {
                None
            } else {
                Some(usage["reasoning_output_tokens"].as_u64()?)
            },
            at: at.with_timezone(&chrono::Utc).to_rfc3339(),
        };
        result.valid().then_some(result)
    }
    pub fn merge(target: &mut Option<Self>, incoming: Option<Self>) {
        if let Some(value) = incoming
            && value.valid()
            && target.as_ref().is_none_or(|old| old.at <= value.at)
        {
            *target = Some(value);
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivitySampling {
    pub at: i64,
    pub window_ms: u32,
    pub tokens_per_second: Option<f64>,
    pub recent: Option<RateSample>,
    pub reported: Option<MessageUsage>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn only_explicit_turn_totals_are_reported_and_reasoning_is_a_subset() {
        let session = "11111111-1111-4111-8111-111111111111";
        let turn = "22222222-2222-4222-8222-222222222222";
        let mut v = json!({"type":"token_usage_record","timestamp":"2026-10-25T01:00:00Z","payload":{
            "thread_id":session,"turn_id":turn,"thread_token_usage":{"output_tokens":999999},
            "usage":{"output_tokens":100},"turn_token_usage":{"output_tokens":900,"reasoning_output_tokens":700},
            "private_text":"DO NOT RETAIN"
        }});
        let result = MessageUsage::codex(&v, session, turn).unwrap();
        assert_eq!(result.output_tokens, 900);
        assert_eq!(result.reasoning_tokens, Some(700));
        assert!(
            !serde_json::to_string(&result)
                .unwrap()
                .contains("DO NOT RETAIN")
        );
        assert!(MessageUsage::codex(&v, session, session).is_none());
        assert!(MessageUsage::codex(&v, turn, turn).is_none());
        v["payload"]["turn_token_usage"]["reasoning_output_tokens"] = json!(901);
        assert!(MessageUsage::codex(&v, session, turn).is_none());
        v["payload"]["turn_token_usage"] = Value::Null;
        assert!(MessageUsage::codex(&v, session, turn).is_none());
        v["payload"]["turn_token_usage"] = json!({"output_tokens":0});
        assert_eq!(
            MessageUsage::codex(&v, session, turn)
                .unwrap()
                .output_tokens,
            0
        );
        v["payload"]["turn_token_usage"]["output_tokens"] = json!(-1);
        assert!(MessageUsage::codex(&v, session, turn).is_none());
    }
}
