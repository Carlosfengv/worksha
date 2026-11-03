//! A query reads published observations, never a transcript or guest. Units stay
//! separate: a native message, a gateway attempt and a reported event are not additive.
use super::super::*;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::{
    cmp::Reverse,
    collections::{BTreeMap, BTreeSet},
};

pub const MAX_ITEMS: usize = 256;
#[cfg(test)]
mod tests;
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UnifiedActivityQuery {
    pub scope: QueryScope,
    pub range_seconds: u32,
    pub product_id: Option<String>,
    pub session_id: Option<String>,
    /// Host gateway observations for selected built-in Agent families only.
    /// This does not grant native transcript access.
    pub include_gateway: bool,
    pub include_historical: bool,
}
impl UnifiedActivityQuery {
    pub fn validate(&self) -> Result<()> {
        self.scope.validate()?;
        if !(1..=86400).contains(&self.range_seconds)
            || self
                .product_id
                .as_ref()
                .is_some_and(|p| p.is_empty() || p.len() > 240)
            || self
                .session_id
                .as_ref()
                .is_some_and(|p| p.is_empty() || p.len() > 512)
            || self.include_gateway && self.scope.kind == ScopeKind::External
        {
            return Err(Failure::new(ErrorCode::InvalidInput));
        }
        Ok(())
    }
    pub fn matches(&self, row: &ActivityRecord, now_ms: i64) -> bool {
        self.product_id
            .as_ref()
            .is_none_or(|p| row.product_ids.contains(p))
            && self
                .session_id
                .as_ref()
                .is_none_or(|s| row.session_id.as_ref() == Some(s))
            && row.occurred_at.as_deref().is_none_or(|t| {
                timestamp(t).is_some_and(|t| {
                    t >= now_ms.saturating_sub(i64::from(self.range_seconds) * 1000)
                        && t <= now_ms + 1000
                })
            })
    }
}
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
pub enum ActivityKind {
    NativeMessage,
    GatewayAttempt,
    SourceEvent,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum ActivityState {
    Current,
    Expired,
    Historical,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RateObservation {
    /// Wall-clock anchor for the host sampling clock, never the source event timestamp.
    pub at_ms: Decimal,
    pub window_ms: Decimal,
    pub tokens_per_second: f64,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ActivityMeasurements {
    /// Source event duration; distinct from a runtime first-token measurement.
    pub reported_ttft_ms: Option<f64>,
    pub runtime_state: Option<String>,
    pub first_token_ms: Option<f64>,
    pub first_text_ms: Option<f64>,
    pub first_output_ms: Option<f64>,
    pub usage: Option<Usage>,
    /// Only a current, message-bound host sample can populate this field.
    pub live_rate: Option<RateObservation>,
    /// A prior observation, even when the activity has expired.
    pub recent_rate: Option<RateObservation>,
    pub historical_average_text_rate: Option<f64>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ActivityRecord {
    pub id: String,
    pub kind: ActivityKind,
    pub source_id: String,
    pub provider_key: String,
    pub source_revision: String,
    pub observed_at: String,
    pub occurred_at: Option<String>,
    pub session_id: Option<String>,
    pub message_id: Option<String>,
    pub product_ids: Vec<String>,
    pub unknown_client: bool,
    pub title: String,
    pub model: Option<String>,
    pub phase: String,
    pub state: ActivityState,
    pub tool_names: Vec<String>,
    pub logical_request_id: Option<String>,
    pub historical_duration_ms: Option<Decimal>,
    /// Absent for external source events. Guests cannot manufacture host metrics.
    pub measurements: Option<ActivityMeasurements>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ActivitySourceState {
    pub kind: ActivityKind,
    pub state: SourceState,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ActivitySummary {
    pub source_id: String,
    pub kind: ActivityKind,
    pub observed_records: Option<Decimal>,
    pub current: Option<Decimal>,
    pub expired: Option<Decimal>,
    pub historical: Option<Decimal>,
    pub unknown_time: Option<Decimal>,
    pub logical_requests: Option<Decimal>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UnifiedActivityPage {
    pub items: Vec<ActivityRecord>,
    pub as_of: String,
    pub source_states: Vec<ActivitySourceState>,
    pub source_summaries: Vec<ActivitySummary>,
    pub applied_query: UnifiedActivityQuery,
    /// The bounded result window, not the source's collection coverage.
    pub limited: bool,
    pub overlap_unknown: bool,
}

type Order = (bool, Option<Reverse<i64>>, String, ActivityKind, String);
#[derive(Default)]
pub struct ActivityWindow {
    rows: BTreeMap<Order, ActivityRecord>,
    counts: BTreeMap<(String, ActivityKind), [u64; 5]>,
    logical: BTreeMap<String, BTreeSet<String>>,
    limited: bool,
}
impl ActivityWindow {
    pub fn observe(&mut self, row: ActivityRecord) {
        let counts = self
            .counts
            .entry((row.source_id.clone(), row.kind))
            .or_default();
        counts[0] += 1;
        counts[match row.state {
            ActivityState::Current => 1,
            ActivityState::Expired => 2,
            ActivityState::Historical => 3,
        }] += 1;
        counts[4] += u64::from(row.occurred_at.is_none());
        if let Some(id) = &row.logical_request_id {
            self.logical
                .entry(row.source_id.clone())
                .or_default()
                .insert(id.clone());
        }
        let time = row.occurred_at.as_deref().and_then(timestamp).map(Reverse);
        self.rows.insert(
            (
                time.is_none(),
                time,
                row.id.clone(),
                row.kind,
                row.source_id.clone(),
            ),
            row,
        );
        if self.rows.len() > MAX_ITEMS {
            // Known times first, descending; missing time is last.
            self.rows.pop_last();
            self.limited = true;
        }
    }
    pub fn finish(
        mut self,
        states: &[ActivitySourceState],
    ) -> (Vec<ActivityRecord>, Vec<ActivitySummary>, bool) {
        let mut summaries = Vec::new();
        for state in states {
            let counts = self
                .counts
                .remove(&(state.state.source_id.clone(), state.kind));
            let counts = counts
                .or_else(|| (state.state.coverage != Coverage::Unavailable).then_some([0; 5]));
            summaries.push(ActivitySummary {
                source_id: state.state.source_id.clone(),
                kind: state.kind,
                observed_records: counts.map(|c| c[0].into()),
                current: counts.map(|c| c[1].into()),
                expired: counts.map(|c| c[2].into()),
                historical: counts.map(|c| c[3].into()),
                unknown_time: counts.map(|c| c[4].into()),
                logical_requests: (state.kind == ActivityKind::GatewayAttempt && counts.is_some())
                    .then(|| {
                        (self
                            .logical
                            .get(&state.state.source_id)
                            .map_or(0, |s| s.len()) as u64)
                            .into()
                    }),
            });
        }
        let items = self.rows.into_values().collect::<Vec<_>>();
        (items, summaries, self.limited)
    }
}
