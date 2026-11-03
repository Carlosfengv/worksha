//! Read-only query contracts and a bounded merge window shared by native and legacy adapters.
use super::*;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::{cmp::Reverse, collections::BTreeMap};

pub const PAGE_SIZE: usize = 40;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum ScopeKind {
    Builtin,
    External,
    Combined,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct QueryScope {
    pub kind: ScopeKind,
    pub source_ids: Vec<String>,
}
impl QueryScope {
    /// Shape validation only. The host still resolves membership and consent.
    pub fn validate(&self) -> Result<()> {
        let unique = self
            .source_ids
            .iter()
            .collect::<std::collections::HashSet<_>>();
        if self.source_ids.is_empty()
            || self.source_ids.len() > 16
            || unique.len() != self.source_ids.len()
            || self
                .source_ids
                .iter()
                .any(|s| s.is_empty() || s.len() > 240)
        {
            return Err(Failure::new(ErrorCode::InvalidInput));
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionFilter {
    pub query: Option<String>,
    pub product_id: Option<String>,
    pub days: Option<u16>,
}
impl SessionFilter {
    pub fn validate(&self) -> Result<()> {
        if self.query.as_ref().is_some_and(|s| s.chars().count() > 300)
            || self.product_id.as_ref().is_some_and(|s| s.len() > 240)
            || self.days.is_some_and(|d| ![1, 7, 30].contains(&d))
        {
            return Err(Failure::new(ErrorCode::InvalidInput));
        }
        Ok(())
    }
    pub fn matches(&self, s: &SessionRecord, cutoff: Option<i64>) -> bool {
        self.product_id
            .as_ref()
            .is_none_or(|id| s.product_ids.contains(id))
            && self.query.as_ref().is_none_or(|q| {
                let q = q.trim().to_lowercase();
                s.title.to_lowercase().contains(&q) || s.native_id.to_lowercase().contains(&q)
            })
            && cutoff.is_none_or(|cutoff| {
                s.created_at
                    .as_deref()
                    .and_then(timestamp)
                    .is_some_and(|t| t >= cutoff)
            })
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UnifiedSessionQuery {
    pub scope: QueryScope,
    pub filter: SessionFilter,
    pub snapshot_token: Option<String>,
    pub cursor: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionItem {
    pub source_id: String,
    pub record: SessionRecord,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceState {
    pub source_id: String,
    pub authorization: Authorization,
    pub freshness: Freshness,
    pub coverage: Coverage,
    /// Host-owned fixed reason code, never guest text.
    pub problem: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionScopeSummary {
    pub source_id: String,
    pub sessions: Decimal,
    pub known_usage_sessions: Decimal,
    pub usage: Usage,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UnifiedSessionPage {
    pub items: Vec<SessionItem>,
    pub snapshot_token: String,
    pub next_cursor: Option<String>,
    pub as_of: String,
    pub coverage: Coverage,
    pub source_states: Vec<SourceState>,
    pub applied_filter: UnifiedSessionQuery,
    pub summary_ref: String,
    pub source_summaries: Vec<SessionScopeSummary>,
    /// Sources retain separate session cumulative units. No synthetic response/call totals.
    pub overlap_unknown: bool,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UnifiedConversationRequest {
    pub session_id: String,
    pub snapshot_token: String,
    pub cursor: Option<String>,
    pub anchor: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UnifiedConversation {
    pub source_id: String,
    pub snapshot_token: String,
    pub conversation: ConversationPage,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceConsent {
    pub plugin_id: String,
    pub digest: String,
    pub review_ref: String,
    pub enabled: bool,
    pub persist_metadata: bool,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceConsentQuery {
    pub plugin_id: String,
    pub digest: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceConsentReview {
    pub review_ref: String,
    pub plugin_id: String,
    pub digest: String,
    pub resource_label: Option<String>,
    pub source_authorized: bool,
    pub unified_authorized: bool,
    pub persist_metadata: bool,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceConsentStatus {
    pub plugin_id: String,
    pub digest: String,
    pub source_id: String,
    pub enabled: bool,
    pub persist_metadata: bool,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UnifiedCatalog {
    pub catalog: CatalogSnapshot,
    pub consents: Vec<SourceConsentStatus>,
    pub external_problem: Option<String>,
}

/// Internal cursor key: JSON numbers here never cross into a JavaScript number.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionOrder {
    // Known timestamps first, descending; missing timestamps last; ID breaks ties.
    pub time: Option<Reverse<i64>>,
    pub id: String,
}
pub fn timestamp(value: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|t| t.timestamp_millis())
}
impl SessionOrder {
    pub fn of(s: &SessionRecord) -> Self {
        Self {
            time: s
                .updated_at
                .as_deref()
                .or(s.created_at.as_deref())
                .and_then(timestamp)
                .map(Reverse),
            id: s.id.clone(),
        }
    }
}
impl Ord for SessionOrder {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.time
            .is_none()
            .cmp(&other.time.is_none())
            .then(self.time.cmp(&other.time))
            .then(self.id.cmp(&other.id))
    }
}
impl PartialOrd for SessionOrder {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
/// At most PAGE_SIZE+1 records are retained, independent of native history size.
pub struct SessionWindow {
    rows: BTreeMap<SessionOrder, SessionItem>,
    after: Option<SessionOrder>,
    summary: SessionScopeSummary,
}
impl SessionWindow {
    pub fn new(source_id: &str, after: Option<SessionOrder>) -> Self {
        Self {
            rows: BTreeMap::new(),
            after,
            summary: SessionScopeSummary {
                source_id: source_id.into(),
                sessions: 0u64.into(),
                known_usage_sessions: 0u64.into(),
                usage: Usage {
                    input_tokens: None,
                    output_tokens: None,
                    cached_input_tokens: None,
                    cache_write_input_tokens: None,
                    reasoning_output_tokens: None,
                    total_tokens: None,
                },
            },
        }
    }
    pub fn observe(&mut self, record: SessionRecord) -> Result<()> {
        self.summary.sessions = self
            .summary
            .sessions
            .value()
            .checked_add(1)
            .ok_or_else(|| Failure::new(ErrorCode::ResourceLimit))?
            .into();
        if record.usage.total_tokens.is_some() {
            self.summary.known_usage_sessions =
                (self.summary.known_usage_sessions.value() + 1).into();
        }
        macro_rules! add { ($($field:ident),*) => {$(
            if let Some(value) = &record.usage.$field {
                self.summary.usage.$field = Some(self.summary.usage.$field.as_ref().map_or(0, Decimal::value)
                    .checked_add(value.value()).ok_or_else(|| Failure::new(ErrorCode::ResourceLimit))?.into());
            }
        )*}; }
        add!(
            input_tokens,
            output_tokens,
            cached_input_tokens,
            cache_write_input_tokens,
            reasoning_output_tokens,
            total_tokens
        );
        let order = SessionOrder::of(&record);
        if self.after.as_ref().is_none_or(|a| &order > a) {
            self.rows.insert(
                order,
                SessionItem {
                    source_id: self.summary.source_id.clone(),
                    record,
                },
            );
            if self.rows.len() > PAGE_SIZE + 1 {
                self.rows.pop_last();
            }
        }
        Ok(())
    }
    pub fn finish(self) -> (Vec<SessionItem>, SessionScopeSummary) {
        (self.rows.into_values().collect(), self.summary)
    }
}
