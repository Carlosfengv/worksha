//! Activity metadata is distinct from host-owned monotonic and numeric evidence.
//! A source event timestamp or collection duration never establishes a live token rate.
use super::*;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
pub mod query;

pub const MAX_ACTIVITIES: usize = 4096;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ActivityRequest {
    pub invocation: Invocation,
    pub source_id: String,
    /// Host wall clock at the beginning of collection, separate from event time.
    pub observed_at: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ActivityInput {
    /// Existing bounded, redacted user excerpt; never tool/system arguments.
    pub text: String,
    pub at: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ActivityObservation {
    pub id: String,
    pub source_id: String,
    pub session_id: String,
    pub message_id: Option<String>,
    pub product_id: Option<String>,
    pub attribution: Option<String>,
    pub title: String,
    pub model: Option<String>,
    pub phase: String,
    pub step: String,
    pub user_input: Option<ActivityInput>,
    pub tool_names: Vec<String>,
    pub message_started_at: Option<String>,
    pub event_at: String,
    pub stale: bool,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ActivitySnapshot {
    pub source_id: String,
    pub source_revision: String,
    pub observed_at: String,
    /// Host monotonic collection duration, not message latency or generation time.
    pub collection_micros: Decimal,
    pub activities: Vec<ActivityObservation>,
    pub limited: bool,
}
pub trait ActivitySource {
    fn collect_activity(&mut self, request: &ActivityRequest) -> Result<ActivitySnapshot>;
}
impl ActivitySnapshot {
    pub fn validate(&self, request: &ActivityRequest) -> Result<()> {
        let mut ids = std::collections::HashSet::new();
        if self.activities.len() > MAX_ACTIVITIES
            || self.activities.len() as u64 > request.invocation.budget.max_records.value()
        {
            return Err(Failure::new(ErrorCode::ResourceLimit));
        }
        if self.source_id != request.source_id
            || self.source_revision != request.invocation.invocation_id
            || self.observed_at != request.observed_at
            || super::timestamp(&self.observed_at).is_none()
            || self.activities.iter().any(|a| {
                a.source_id != request.source_id
                    || a.id.is_empty()
                    || a.session_id.is_empty()
                    || !ids.insert(&a.id)
                    || super::timestamp(&a.event_at).is_none()
                    || a.message_started_at
                        .as_ref()
                        .is_some_and(|t| super::timestamp(t).is_none())
                    || a.user_input
                        .as_ref()
                        .is_some_and(|i| super::timestamp(&i.at).is_none())
            })
        {
            return Err(Failure::new(ErrorCode::InvalidInput));
        }
        Ok(())
    }
}
