//! Frozen, read-only request queries. Response records and logical requests are
//! selected as separate units. Source claims never become host request evidence.
use super::*;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RequestFilter {
    pub start: Option<String>,
    pub end: Option<String>,
    pub product_id: Option<String>,
    pub session_id: Option<String>,
    pub project_id: Option<String>,
    pub provider_key: Option<String>,
    pub model: Option<String>,
    pub outcome: Option<String>,
    pub search: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UnifiedRequestQuery {
    pub scope: QueryScope,
    pub unit: RequestUnit,
    pub filter: RequestFilter,
    /// Event time ordered to milliseconds; unknown times always follow known times.
    pub descending: bool,
    /// Omitted requests keep the original event-time order and JSON shape.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sort: Option<RequestSort>,
    /// Opt in to frozen host matching/billing evidence, never guest claims.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include_evidence: Option<bool>,
    pub snapshot_token: Option<String>,
    pub cursor: Option<String>,
}
impl UnifiedRequestQuery {
    pub fn validate(&self) -> Result<()> {
        self.scope.validate()?;
        for time in [&self.filter.start, &self.filter.end].into_iter().flatten() {
            if timestamp(time).is_none() || time.len() > 64 {
                return Err(Failure::new(ErrorCode::InvalidInput));
            }
        }
        if self
            .filter
            .start
            .as_ref()
            .zip(self.filter.end.as_ref())
            .is_some_and(|(a, b)| {
                chrono::DateTime::parse_from_rfc3339(a).unwrap()
                    >= chrono::DateTime::parse_from_rfc3339(b).unwrap()
            })
            || [
                &self.filter.product_id,
                &self.filter.session_id,
                &self.filter.project_id,
                &self.filter.provider_key,
                &self.filter.model,
                &self.filter.outcome,
                &self.filter.search,
            ]
            .into_iter()
            .flatten()
            .any(|s| s.is_empty() || s.len() > 512)
            || self
                .snapshot_token
                .as_ref()
                .is_some_and(|s| s.is_empty() || s.len() > 80)
            || self.cursor.as_ref().is_some_and(|s| s.len() > 160)
            || (self.cursor.is_some() && self.snapshot_token.is_none())
        {
            return Err(Failure::new(ErrorCode::InvalidInput));
        }
        Ok(())
    }
    pub fn same_selection(&self, other: &Self) -> bool {
        self.scope == other.scope
            && self.unit == other.unit
            && self.filter == other.filter
            && self.descending == other.descending
            && self.sort == other.sort
            && self.include_evidence == other.include_evidence
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum RequestSort {
    Time,
    Tokens,
    Duration,
    Ttft,
    CostUsd,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum RequestEvidence {
    HostObserved,
    SourceReported,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RequestQueryRecord {
    pub id: String,
    pub source_id: String,
    pub unit: RequestUnit,
    pub product_id: Option<String>,
    pub session_ref: Option<String>,
    pub session_title: Option<String>,
    pub project_ref: Option<String>,
    pub project_name: Option<String>,
    pub occurred_at: Option<String>,
    pub ended_at: Option<String>,
    pub model: Option<String>,
    pub provider_key: String,
    pub provider_name: String,
    pub outcome: String,
    pub usage: Usage,
    pub usage_missing: Decimal,
    pub attempts: Option<Decimal>,
    pub duration_ms: Option<f64>,
    pub ttft_ms: Option<f64>,
    pub throughput: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UnifiedRequestItem {
    pub record: RequestQueryRecord,
    pub evidence: RequestEvidence,
    /// Only host records can open the existing request/attempt/billing detail.
    pub host_request_id: Option<String>,
    /// Not an exact host link and never used for deduplication.
    pub reported_response_id: Option<String>,
    pub reported_duration_ms: Option<Decimal>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub evidence_detail: Option<RequestEvidenceDetail>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RequestSourceSummary {
    pub source_id: String,
    pub unit: RequestUnit,
    pub evidence: RequestEvidence,
    pub coverage: Coverage,
    pub records: Option<Decimal>,
    pub usage: Usage,
    pub usage_missing: Option<Decimal>,
    /// Source-reported responses do not establish an HTTP attempt count.
    pub attempts: Option<Decimal>,
    /// Matching non-time filters but excluded from a dated range when time is unknown.
    pub unknown_time: Option<Decimal>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub evidence_summary: Option<RequestEvidenceSummary>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RequestMoney {
    #[schemars(regex(pattern = r"^[A-Z][A-Z0-9]{0,15}$"))]
    pub currency: String,
    /// Finite nonnegative decimal text from the existing host monetary value.
    /// No currency conversion or claim of greater precision is introduced.
    #[schemars(regex(pattern = r"^(0|[1-9][0-9]*)(\.[0-9]+)?$"), length(max = 400))]
    pub amount: String,
    pub parts: Decimal,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RequestBilling {
    pub actual: Vec<RequestMoney>,
    pub estimated: Vec<RequestMoney>,
    /// Missing billing evidence parts, not missing request/response count.
    pub unknown_parts: Decimal,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum NativeMatchState {
    ExactResponseId,
    Unlinked,
    Ambiguous,
    IdentityConflict,
    Unknown,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExactResponseLink {
    pub event_id: String,
    pub session_id: String,
    pub response_id: String,
    pub attempt_id: String,
    pub usage_conflict: bool,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(
    tag = "kind",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum RequestMatching {
    NativeResponse {
        state: NativeMatchState,
        response_id: Option<String>,
        logical_request_id: Option<String>,
        attempt_id: Option<String>,
        usage_conflict: bool,
    },
    GatewayRequest {
        matched_responses: Decimal,
        links: Vec<ExactResponseLink>,
        links_limited: bool,
    },
    SourceReported,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RequestEvidenceDetail {
    pub matching: RequestMatching,
    pub billing: Option<RequestBilling>,
    /// Overflow in captured attempt totals is unknown, never a capped value.
    pub overflow_fields: Vec<String>,
    /// Preserves distinctions between source event timing and host HTTP timing.
    pub timing_note: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(
    tag = "kind",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum RequestMatchingSummary {
    NativeResponses {
        exact: Decimal,
        unlinked: Decimal,
        ambiguous: Decimal,
        identity_conflicts: Decimal,
        unknown: Decimal,
        usage_conflicts: Decimal,
    },
    GatewayRequests {
        with_exact_links: Decimal,
        without_confirmed_links: Decimal,
        matched_responses: Decimal,
        usage_conflict_links: Decimal,
    },
    SourceReported {
        unverified_records: Option<Decimal>,
    },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RequestEvidenceSummary {
    pub matching: RequestMatchingSummary,
    pub billing: Option<RequestBilling>,
    /// Missing underlying usage parts, distinct from records with incomplete usage.
    pub usage_missing_parts: Option<Decimal>,
    /// A sum outside u64 is unknown; original per-record values remain intact.
    pub overflow_fields: Vec<String>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UnifiedRequestPage {
    pub items: Vec<UnifiedRequestItem>,
    pub snapshot_token: String,
    pub next_cursor: Option<String>,
    pub as_of: String,
    pub source_states: Vec<SourceState>,
    /// Full filtered scope, never just the current page. No cross-source total.
    pub source_summaries: Vec<RequestSourceSummary>,
    pub applied_query: UnifiedRequestQuery,
    pub overlap_unknown: bool,
}
