//! Product-owned proxy metadata only. Native MCP history remains authorized,
//! read-only user input and is never imported through this connector ledger.
use super::super::{LedgerError, digest, invalid, same_shape, valid_digest};
use crate::observation::retention::{ExpirationCandidate, Protection, key};
use chrono::{DateTime, Datelike, SecondsFormat, Utc};
use serde::{Deserialize, Serialize, de::IgnoredAny};
use serde_json::Value;

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Outcome {
    Success,
    ToolError,
    ProtocolError,
    TransportError,
    Timeout,
    Cancelled,
    Incomplete,
    InProgress,
}
impl Outcome {
    fn complete_response(self) -> bool {
        matches!(self, Self::Success | Self::ToolError | Self::ProtocolError)
    }
    pub(super) fn protection(self) -> Option<Protection> {
        match self {
            Self::InProgress => Some(Protection::Active),
            Self::Success | Self::ToolError | Self::ProtocolError => None,
            // A proxy timeout/cancel/transport failure does not prove whether
            // the remote tool finished a side effect. Keep the source protected.
            _ => Some(Protection::UnknownOutcome),
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Source {
    identity: String,
    #[serde(rename = "rpcId")]
    _rpc_id: IgnoredAny,
    mcp_connection: String,
    tool: String,
    status: Outcome,
    duration_ms: Option<f64>,
    observed_at: String,
    #[serde(rename = "agentSession")]
    _agent_session: (),
    content: String,
    #[serde(rename = "modelTokens")]
    _model_tokens: (),
}
const SOURCE_FIELDS: [&str; 10] = [
    "identity",
    "rpcId",
    "mcpConnection",
    "tool",
    "status",
    "durationMs",
    "observedAt",
    "agentSession",
    "content",
    "modelTokens",
];

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ToolFact {
    version: u16,
    pub(super) namespace: String,
    pub(super) key: String,
    pub(super) connector: String,
    pub(super) tool: String,
    tool_label: Option<String>,
    // observedAt is a recorded event time, not an invented tool-start time.
    pub(super) recorded_at: Option<String>,
    pub(super) outcome: Outcome,
    duration_ms: Option<f64>,
}
pub(super) fn normalized(at: DateTime<Utc>) -> Option<String> {
    (0..=9999)
        .contains(&at.year())
        .then(|| at.to_rfc3339_opts(SecondsFormat::Nanos, true))
}
fn bounded(s: &str, bytes: usize) -> bool {
    !s.is_empty() && s.len() <= bytes && !s.chars().any(char::is_control)
}
impl Source {
    fn checked(plugin: &str, identity: &str, raw: &str) -> Result<Self, LedgerError> {
        let source: Source = serde_json::from_str(raw).map_err(invalid)?;
        // Source has already rejected unknown root fields. Inspect its ten names
        // without allocating another tree for potentially large private RPC IDs.
        let fields: std::collections::BTreeMap<String, IgnoredAny> =
            serde_json::from_str(raw).map_err(invalid)?;
        if fields.len() != SOURCE_FIELDS.len()
            || !SOURCE_FIELDS.iter().all(|k| fields.contains_key(*k))
            || !bounded(plugin, 256)
            || !bounded(identity, 8192)
            || source.identity != identity
            || !bounded(&source.mcp_connection, 4096)
            || source.content != "not-persisted"
            || source.tool.len() > 640
            || source.tool.chars().count() > 160
            || source.tool.chars().any(char::is_control)
            || source
                .duration_ms
                .is_some_and(|d| !d.is_finite() || d < 0.0 || !source.status.complete_response())
        {
            return Err(LedgerError::InvalidRecord);
        }
        Ok(source)
    }
}

fn expiration(
    outcome: Outcome,
    at: Option<DateTime<Utc>>,
    cutoff: DateTime<Utc>,
) -> ExpirationCandidate {
    if let Some(reason) = outcome.protection() {
        return ExpirationCandidate::Protected(reason);
    }
    match at {
        None => ExpirationCandidate::Protected(Protection::UnknownTime),
        Some(at) if at < cutoff => ExpirationCandidate::Expired,
        Some(_) => ExpirationCandidate::WithinWindow,
    }
}
pub(in crate::observation::retention) fn source_expiration(
    plugin: &str,
    identity: &str,
    raw: &str,
    cutoff: DateTime<Utc>,
) -> Result<ExpirationCandidate, LedgerError> {
    let source = Source::checked(plugin, identity, raw)?;
    let at = DateTime::parse_from_rfc3339(&source.observed_at)
        .ok()
        .map(|t| t.to_utc())
        .filter(|t| (0..=9999).contains(&t.year()));
    Ok(expiration(source.status, at, cutoff))
}

impl ToolFact {
    pub(super) fn expiration_at(&self, cutoff: DateTime<Utc>) -> ExpirationCandidate {
        expiration(
            self.outcome,
            self.recorded_at
                .as_deref()
                .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                .map(|t| t.to_utc()),
            cutoff,
        )
    }
    pub(super) fn from_source(
        plugin: &str,
        identity: &str,
        raw: &str,
        namespace: &str,
    ) -> Result<Self, LedgerError> {
        let source = Source::checked(plugin, identity, raw)?;
        // RPC IDs can contain private caller metadata. Validate their presence,
        // but never project them or the MCP connection into Agent/session scope.
        let identity = serde_json::to_string(&(plugin, identity)).map_err(invalid)?;
        Ok(Self {
            version: 1,
            namespace: namespace.into(),
            key: key(namespace, "connector-call", &identity),
            connector: key(namespace, "connector", plugin),
            tool: key(namespace, "connector-tool", &source.tool),
            tool_label: bounded(&source.tool, 640).then_some(source.tool),
            recorded_at: DateTime::parse_from_rfc3339(&source.observed_at)
                .ok()
                .and_then(|t| normalized(t.to_utc())),
            outcome: source.status,
            duration_ms: source.duration_ms,
        })
    }
    pub(super) fn validate(
        id: &str,
        namespace: &str,
        expected: &str,
        raw: &str,
    ) -> Result<Self, LedgerError> {
        let fact: Self = serde_json::from_str(raw).map_err(invalid)?;
        let value: Value = serde_json::from_str(raw).map_err(invalid)?;
        let canonical = serde_json::to_value(&fact).map_err(invalid)?;
        if fact.version != 1 {
            return Err(LedgerError::UnsupportedVersion);
        }
        if fact.key != id
            || fact.namespace != namespace
            || digest(raw) != expected
            || !valid_digest(id)
            || !valid_digest(&fact.connector)
            || !valid_digest(&fact.tool)
            || !same_shape(&value, &canonical)
            || fact
                .tool_label
                .as_deref()
                .is_some_and(|s| !bounded(s, 640) || s.chars().count() > 160)
            || fact
                .tool_label
                .as_deref()
                .is_some_and(|s| key(namespace, "connector-tool", s) != fact.tool)
            || fact.recorded_at.as_deref().is_some_and(|s| {
                DateTime::parse_from_rfc3339(s)
                    .ok()
                    .and_then(|t| normalized(t.to_utc()))
                    .as_deref()
                    != Some(s)
            })
            || fact
                .duration_ms
                .is_some_and(|d| !d.is_finite() || d < 0.0 || !fact.outcome.complete_response())
        {
            return Err(LedgerError::InvalidRecord);
        }
        Ok(fact)
    }
}
