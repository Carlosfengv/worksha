use local_agent_core::domain::{Decimal, activity::query::ReportedActivityMeasurements};
use std::collections::BTreeMap;

pub(crate) struct Task {
    pub(crate) native_id: String,
    pub(crate) title: String,
    pub(crate) started_at: Option<String>,
    pub(crate) model: Option<String>,
    pub(crate) measurements: Option<ReportedActivityMeasurements>,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BridgeTiming {
    message_native_id: String,
    message_started_at_ms: Option<u64>,
    model: Option<String>,
    first_token_ms: Option<u64>,
    output_tokens: Option<Decimal>,
    completed_tokens: Option<Decimal>,
    completed_decode_ms: Option<u64>,
    running_tokens: Option<Decimal>,
    running_decode_ms: Option<u64>,
}

impl BridgeTiming {
    fn measurements(&self) -> Option<ReportedActivityMeasurements> {
        const MAX_DURATION_MS: u64 = 86_400_000;
        const MAX_EXACT_TOKENS: u64 = (1 << 53) - 1;
        let rate = |tokens: Option<&Decimal>, elapsed: Option<u64>| {
            let count = tokens?.value();
            let ms = elapsed?;
            (count <= MAX_EXACT_TOKENS && (1..=MAX_DURATION_MS).contains(&ms))
                .then(|| count as f64 * 1000.0 / ms as f64)
        };
        let reported = ReportedActivityMeasurements {
            first_token_ms: self.first_token_ms.filter(|v| *v <= MAX_DURATION_MS),
            running_tokens_per_second: rate(self.running_tokens.as_ref(), self.running_decode_ms),
            completed_tokens_per_second: rate(
                self.completed_tokens.as_ref(),
                self.completed_decode_ms,
            ),
            output_tokens: self
                .output_tokens
                .clone()
                .filter(|v| v.value() <= MAX_EXACT_TOKENS),
        };
        (reported.first_token_ms.is_some()
            || reported.running_tokens_per_second.is_some()
            || reported.completed_tokens_per_second.is_some()
            || reported.output_tokens.is_some())
        .then_some(reported)
    }
}

/// Live sources may supply chronological conversation events and timing evidence.
/// Only an authored message from the authorized frame can name a task.
pub(super) fn latest_tasks(frame: &str) -> BTreeMap<String, Task> {
    frame
        .lines()
        .filter_map(|line| {
            let mut row: serde_json::Value = serde_json::from_str(line).ok()?;
            let fact = &row["fact"];
            if fact["kind"] != "session" {
                return None;
            }
            let session = fact["value"]["nativeId"].as_str()?;
            let event = row["conversation"]
                .as_array()?
                .iter()
                .rev()
                .find(|event| event["kind"] == "message" && event["role"] == "user")?;
            let native_id = event["nativeId"].as_str()?;
            if native_id.is_empty()
                || native_id.len() > 256
                || native_id.chars().any(char::is_control)
            {
                return None;
            }
            let title = event["text"]
                .as_str()?
                .trim()
                .chars()
                .take(240)
                .collect::<String>();
            if title.is_empty() {
                return None;
            }
            let session = session.to_owned();
            let native_id = native_id.to_owned();
            let timing = serde_json::from_value::<BridgeTiming>(row["activityTiming"].take())
                .ok()
                .filter(|timing| timing.message_native_id == native_id);
            let measurements = timing.as_ref().and_then(BridgeTiming::measurements);
            let started_at = timing.as_ref().and_then(|timing| {
                let milliseconds = i64::try_from(timing.message_started_at_ms?).ok()?;
                Some(
                    chrono::DateTime::from_timestamp_millis(milliseconds)?
                        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                )
            });
            let model = timing.and_then(|timing| timing.model).filter(|model| {
                !model.trim().is_empty()
                    && model.len() <= 200
                    && !model.chars().any(char::is_control)
            });
            Some((
                session,
                Task {
                    native_id,
                    title,
                    started_at,
                    model,
                    measurements,
                },
            ))
        })
        .collect()
}
