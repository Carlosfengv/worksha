//! Read-only projections of already captured requests. Native responses and
//! gateway logical requests are different units; transport and billing remain host owned.
use super::*;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
pub mod export;
pub mod query;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum RequestUnit {
    ModelResponse,
    LogicalRequest,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RequestRead {
    pub provider_key: String,
    pub implementation_identity: String,
    pub read_revision: String,
    pub source_id: String,
    pub record_id: String,
    pub unit: RequestUnit,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RequestObservation {
    pub id: String,
    pub source_id: String,
    pub unit: RequestUnit,
    pub product_id: Option<String>,
    pub session_ref: Option<String>,
    pub session_title: Option<String>,
    pub project_ref: Option<String>,
    pub project_name: Option<String>,
    /// An event time on a native response is not an HTTP request start.
    pub occurred_at: Option<String>,
    pub ended_at: Option<String>,
    pub model: Option<String>,
    pub provider_key: String,
    pub provider_name: String,
    pub outcome: String,
    pub usage: Usage,
    pub usage_missing: Decimal,
    /// Host-retained attempt evidence, including evidence already attached to
    /// a native response. This count never changes the response's own unit.
    pub attempts: Decimal,
    pub duration_ms: Option<f64>,
    pub ttft_ms: Option<f64>,
    pub throughput: Option<f64>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RequestSnapshot {
    /// Exact host-bound read identity. It is not a filesystem/resource grant.
    pub read: RequestRead,
    pub record: RequestObservation,
}
pub trait RequestSource {
    fn read_request(&mut self, request: &RequestRead) -> Result<RequestSnapshot>;
}
impl RequestSnapshot {
    pub fn validate(&self, read: &RequestRead) -> Result<()> {
        if self.read != *read
            || self.record.id != read.record_id
            || self.record.source_id != read.source_id
            || self.record.unit != read.unit
            || [
                self.record.occurred_at.as_deref(),
                self.record.ended_at.as_deref(),
            ]
            .into_iter()
            .flatten()
            .any(|v| timestamp(v).is_none())
            || [
                self.record.duration_ms,
                self.record.ttft_ms,
                self.record.throughput,
            ]
            .into_iter()
            .flatten()
            .any(|v| !v.is_finite() || v < 0.0)
        {
            return Err(Failure::new(ErrorCode::InvalidInput));
        }
        Ok(())
    }
}
