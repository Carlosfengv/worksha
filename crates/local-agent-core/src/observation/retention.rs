//! Whitelisted statistical facts for retention, never a second request-detail store.
//! This projection is not deletion authorization. Native sources, receipts and backups
//! are outside its scope; cleanup must separately verify durable inputs and consent.
use crate::{
    agents::AgentDirectory,
    requests::{Request, Summary},
};
use chrono::{DateTime, Duration, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
mod agent_identity;
mod attribution;
pub mod impact;
mod ledger;
mod source;
pub use attribution::{AttributionCoverage, PartDimension};
pub use ledger::{AgentFilter, GatewayFilter};
pub use ledger::{FactLedger, LedgerError, StageResult, ToolExpiry, ToolFilter, ToolSummary};
pub use ledger::{PackedBatch, PackedCoverage, PackedKind, PackedPreview};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FactError {
    InvalidNamespace,
    NotGatewayRequest,
    InvalidRequest,
    InvalidRetentionWindow,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Protection {
    Active,
    UnknownOutcome,
    UnknownTime,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ExpirationCandidate {
    Expired,
    WithinWindow,
    Protected(Protection),
}

/// One logical request's statistical contribution. Internal keys are namespaced
/// pseudonyms, not authentication tokens or a promise of irreversible anonymity.
/// Only summary queries may expose these facts; never reconstruct a detail/CSV row.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GatewayFact {
    version: u32,
    namespace: String,
    key: String,
    agent: String,
    provider: Option<String>,
    model: Option<String>,
    started_at: Option<String>,
    completed_at: Option<String>,
    protection: Option<Protection>,
    stats: Summary,
    cache_in: String,
    cache_read: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    agent_identity: Option<agent_identity::AgentIdentity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    attribution: Option<attribution::Attribution>,
}

fn instant(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|at| at.to_utc())
}
fn normalized(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(SecondsFormat::Nanos, true)
}
fn key(namespace: &str, kind: &str, value: &str) -> String {
    // Fixed namespace length and a domain separator prevent cross-purpose reuse.
    let mut hash = Sha256::new();
    hash.update(b"local-agent-retention-fact-v1\0");
    hash.update(namespace.as_bytes());
    hash.update([0]);
    hash.update(kind.as_bytes());
    hash.update([0]);
    hash.update(value.as_bytes());
    format!("{:x}", hash.finalize())
}

impl GatewayFact {
    pub fn from_request(
        request: &Request,
        namespace: &str,
        directory: &AgentDirectory,
    ) -> Result<Self, FactError> {
        if !uuid::Uuid::parse_str(namespace)
            .is_ok_and(|id| id.get_version_num() == 4 && id.to_string() == namespace)
        {
            return Err(FactError::InvalidNamespace);
        }
        if request.source != "gateway" || request.native.is_some() {
            return Err(FactError::NotGatewayRequest);
        }
        if request.id.is_empty() || request.id.len() > 1024 || request.attempts.is_empty() {
            return Err(FactError::InvalidRequest);
        }
        let started = instant(&request.started_at);
        let mut completed = None;
        let mut unknown_time = started.is_none();
        let mut active = false;
        let mut unknown_outcome = false;
        let (mut input, mut cached) = (0u128, 0u128);
        for attempt in &request.attempts {
            let row = &attempt.observation;
            active |= row.status == "in_progress";
            unknown_outcome |= !["success", "error", "failed", "incomplete", "cancelled"]
                .contains(&row.status.as_str());
            match instant(&row.started_at).zip(row.ended_at.as_deref().and_then(instant)) {
                Some((begin, end)) if end >= begin => {
                    completed = Some(completed.map_or(end, |old: DateTime<Utc>| old.max(end)));
                }
                _ => unknown_time = true,
            }
            if let Some((i, c)) = row
                .usage
                .input_tokens
                .zip(row.usage.cached_input_tokens)
                .filter(|(i, c)| c <= i)
            {
                input += u128::from(i);
                cached += u128::from(c);
            }
        }
        let protection = if active {
            Some(Protection::Active)
        } else if unknown_outcome {
            Some(Protection::UnknownOutcome)
        } else if unknown_time {
            Some(Protection::UnknownTime)
        } else {
            None
        };
        Ok(Self {
            version: 1,
            namespace: namespace.into(),
            key: key(namespace, "request", &request.id),
            agent: directory.group(&request.agent_id, request.agent_product_id.as_deref()),
            provider: (!request.provider_key.is_empty())
                .then(|| key(namespace, "provider", &request.provider_key)),
            model: request
                .model
                .as_ref()
                .filter(|model| {
                    !model.is_empty() && model.len() <= 512 && !model.chars().any(char::is_control)
                })
                .cloned(),
            started_at: started.map(normalized),
            completed_at: if protection.is_none() {
                completed.map(normalized)
            } else {
                None
            },
            protection,
            stats: Summary::of([request]),
            cache_in: input.to_string(),
            cache_read: cached.to_string(),
            agent_identity: None,
            attribution: None,
        })
    }

    /// Full statistical attribution, using only recorded route evidence. Version
    /// one remains readable as summary-only; it never acquires these dimensions
    /// merely because the reader has been upgraded.
    /// Retained root statistics and ordered derived attempts must match the
    /// canonical reconstruction of `originals`. Context links remain annotations;
    /// the controller must separately validate their authority and route evidence.
    pub fn from_request_with_evidence(
        request: &Request,
        originals: &[super::ModelObservation],
        namespace: &str,
        directory: &AgentDirectory,
        evidence: &[crate::provider_usage::Evidence],
    ) -> Result<Self, FactError> {
        let mut fact = Self::from_request(request, namespace, directory)?;
        fact.attribution = Some(attribution::Attribution::from_request(
            request, originals, namespace, directory, evidence, false, &fact,
        )?);
        fact.version = 2;
        Ok(fact)
    }

    pub fn statistics(&self) -> &Summary {
        &self.stats
    }

    /// Version three preserves original Agent matching through later directory
    /// changes. Existing v1/v2 constructors keep their original serialized shape.
    /// These opaque identities are statistical evidence, never authorization.
    /// The same source-statistics checks as v2 apply before adding v3 identities.
    pub fn from_request_queryable(
        request: &Request,
        originals: &[super::ModelObservation],
        namespace: &str,
        directory: &AgentDirectory,
        evidence: &[crate::provider_usage::Evidence],
    ) -> Result<Self, FactError> {
        let mut fact = Self::from_request(request, namespace, directory)?;
        fact.attribution = Some(attribution::Attribution::from_request(
            request, originals, namespace, directory, evidence, true, &fact,
        )?);
        fact.version = 3;
        fact.agent_identity = Some(agent_identity::AgentIdentity::new(
            namespace,
            &request.agent_id,
            request.agent_product_id.as_deref(),
        ));
        if directory.family(&fact.agent).is_none() {
            fact.agent = key(namespace, "agent-group", &fact.agent);
        }
        Ok(fact)
    }
    pub fn cache_counters(&self) -> (&str, &str) {
        (&self.cache_in, &self.cache_read)
    }

    /// Classify the time boundary only. An expired fact does not permit deletion.
    pub fn expiration_candidate(
        &self,
        days: u16,
        now: DateTime<Utc>,
    ) -> Result<ExpirationCandidate, FactError> {
        if ![7, 30, 90].contains(&days) {
            return Err(FactError::InvalidRetentionWindow);
        }
        let cutoff = now
            .checked_sub_signed(Duration::days(i64::from(days)))
            .ok_or(FactError::InvalidRetentionWindow)?;
        if let Some(reason) = self.protection {
            return Ok(ExpirationCandidate::Protected(reason));
        }
        let Some((started, completed)) = self
            .started_at
            .as_deref()
            .and_then(instant)
            .zip(self.completed_at.as_deref().and_then(instant))
        else {
            return Ok(ExpirationCandidate::Protected(Protection::UnknownTime));
        };
        Ok(if started < cutoff && completed < cutoff {
            ExpirationCandidate::Expired
        } else {
            ExpirationCandidate::WithinWindow
        })
    }
}

#[cfg(test)]
mod test_support;
#[cfg(test)]
mod tests;
