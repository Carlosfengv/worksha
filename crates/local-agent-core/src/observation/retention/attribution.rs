use super::{FactError, GatewayFact, instant, key, normalized};
use crate::{
    agents::AgentDirectory,
    provider_cost, provider_usage,
    requests::{self, Request, Summary},
    sessions::Usage,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PartDimension {
    Provider,
    Model,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AttributionCoverage {
    pub complete_facts: usize,
    pub summary_only_facts: usize,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Attribution {
    pub evidence_key: String,
    // Opaque scope links preserve filtering, not native source authorization.
    pub session: Option<String>,
    pub project: Option<String>,
    pub attempts: Vec<AttemptFact>,
    pub parts: Vec<PartFact>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ModelIdentity {
    pub key: String,
    pub label: Option<String>,
}
impl ModelIdentity {
    fn new(value: Option<&str>, namespace: &str) -> Self {
        let value = value.unwrap_or("模型未知");
        Self {
            key: key(namespace, "model", value),
            label: (!value.is_empty()
                && value.len() <= 512
                && !value.chars().any(char::is_control))
            .then(|| value.to_owned()),
        }
    }
}

#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum ProviderAttribution {
    ProviderId,
    RouteSnapshot,
    Unknown,
    ConflictingEvidence,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct AttemptFact {
    pub key: String,
    pub agent: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_identity: Option<super::agent_identity::AgentIdentity>,
    pub provider_group: String,
    pub attributed_provider: Option<String>,
    pub attribution: ProviderAttribution,
    pub model_provider_group: String,
    pub model: ModelIdentity,
    pub routed_model: ModelIdentity,
    pub started_at: Option<String>,
    pub request_cost: provider_cost::Totals,
    pub request_basis: CostBasis,
    pub provider_basis: CostBasis,
    pub model_basis: CostBasis,
    pub provider_stats: ProviderStatistics,
    pub model_stats: crate::model_usage::UsageTotals,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct CostBasis {
    kind: String,
    reason: Option<String>,
    source: Option<String>,
    price: Option<PriceBasis>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PriceBasis {
    version: String,
    checked_at: String,
    valid_from: String,
    valid_until: String,
    model: String,
    period: String,
    input_per_million: f64,
    cached_input_per_million: f64,
    output_per_million: f64,
}
impl CostBasis {
    fn new(cost: &provider_cost::Cost) -> Self {
        Self {
            kind: cost.kind.into(),
            reason: cost.reason.map(str::to_owned),
            source: cost.source.map(str::to_owned),
            price: cost.price.as_ref().map(|p| PriceBasis {
                version: p.version.into(),
                checked_at: p.checked_at.into(),
                valid_from: p.valid_from.into(),
                valid_until: p.valid_until.into(),
                model: p.model.clone(),
                period: p.period.into(),
                input_per_million: p.input_per_million,
                cached_input_per_million: p.cached_input_per_million,
                output_per_million: p.output_per_million,
            }),
        }
    }
}

/// Owned persistence DTO: the live Provider totals deliberately use static map
/// keys and skipped scratch samples. Copy only their published statistical fields.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ProviderStatistics {
    cost: provider_cost::Totals,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    unknown_cost_reasons: BTreeMap<String, usize>,
    calls: usize,
    errors: usize,
    in_progress: usize,
    unknown_outcomes: usize,
    usage: Usage,
    reported: Reported,
    last_seen: Option<String>,
    latency_p95_ms: Option<f64>,
    latency_samples: usize,
    ttft_p95_ms: Option<f64>,
    ttft_samples: usize,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Reported {
    input_tokens: usize,
    output_tokens: usize,
    total_tokens: usize,
    cached_input_tokens: usize,
    cache_write_input_tokens: usize,
    reasoning_output_tokens: usize,
}
impl ProviderStatistics {
    fn from_record(row: &provider_usage::Record) -> Self {
        let mut totals = provider_usage::Totals::default();
        totals.add(row);
        totals.finish();
        let count = |name| *totals.reported.get(name).unwrap_or(&0);
        let reported = Reported {
            input_tokens: count("inputTokens"),
            output_tokens: count("outputTokens"),
            total_tokens: count("totalTokens"),
            cached_input_tokens: count("cachedInputTokens"),
            cache_write_input_tokens: count("cacheWriteInputTokens"),
            reasoning_output_tokens: count("reasoningOutputTokens"),
        };
        Self {
            cost: totals.cost,
            unknown_cost_reasons: totals.unknown_cost_reasons,
            calls: totals.calls,
            errors: totals.errors,
            in_progress: totals.in_progress,
            unknown_outcomes: totals.unknown_outcomes,
            usage: totals.usage,
            reported,
            last_seen: totals.last_seen,
            latency_p95_ms: totals.latency_p95_ms,
            latency_samples: totals.latency_samples,
            ttft_p95_ms: totals.ttft_p95_ms,
            ttft_samples: totals.ttft_samples,
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct PartFact {
    pub dimension: PartDimension,
    pub key: String,
    pub label: Option<String>,
    pub stats: Summary,
    pub cache_in: String,
    pub cache_read: String,
}

impl Attribution {
    pub(super) fn from_request(
        request: &Request,
        originals: &[crate::observation::ModelObservation],
        namespace: &str,
        directory: &AgentDirectory,
        evidence: &[provider_usage::Evidence],
        queryable: bool,
        projection: &GatewayFact,
    ) -> Result<Self, FactError> {
        if originals.len() != request.attempts.len() {
            return Err(FactError::InvalidRequest);
        }
        let original_by_id = originals
            .iter()
            .map(|row| (row.id.as_str(), row))
            .collect::<std::collections::HashMap<_, _>>();
        if original_by_id.len() != originals.len() {
            return Err(FactError::InvalidRequest);
        }
        let canonical =
            requests::logical_requests(originals.to_vec(), &std::collections::HashMap::new());
        if canonical.len() != 1 || canonical[0].id != request.id {
            return Err(FactError::InvalidRequest);
        }
        // Compare the actual retained root projection, not a second hand-written
        // list of derived fields. Context annotations are intentionally separate.
        let expected_projection = GatewayFact::from_request(&canonical[0], namespace, directory)?;
        if serde_json::to_value(projection).map_err(|_| FactError::InvalidRequest)?
            != serde_json::to_value(expected_projection).map_err(|_| FactError::InvalidRequest)?
        {
            return Err(FactError::InvalidRequest);
        }
        if queryable
            && (canonical[0].agent_id != request.agent_id
                || canonical[0].agent_product_id != request.agent_product_id
                || canonical[0].started_at != request.started_at)
        {
            return Err(FactError::InvalidRequest);
        }
        let view_by_id = canonical[0]
            .attempts
            .iter()
            .enumerate()
            .map(|(index, a)| (a.observation.id.as_str(), (index, a)))
            .collect::<std::collections::HashMap<_, _>>();
        let mut attempts = Vec::with_capacity(request.attempts.len());
        let mut seen = HashSet::new();
        for (index, attempt) in request.attempts.iter().enumerate() {
            let row = *original_by_id
                .get(attempt.observation.id.as_str())
                .ok_or(FactError::InvalidRequest)?;
            let (expected_index, expected) = view_by_id
                .get(row.id.as_str())
                .ok_or(FactError::InvalidRequest)?;
            if *expected_index != index
                || serde_json::to_value(expected).map_err(|_| FactError::InvalidRequest)?
                    != serde_json::to_value(attempt).map_err(|_| FactError::InvalidRequest)?
            {
                return Err(FactError::InvalidRequest);
            }
            if !seen.insert(&row.id) {
                return Err(FactError::InvalidRequest);
            }
            let provider = provider_usage::gateway_records(vec![row.clone()], evidence).remove(0);
            let attribution = match provider.attribution {
                "provider_id" => ProviderAttribution::ProviderId,
                "route_snapshot" => ProviderAttribution::RouteSnapshot,
                "unknown" => ProviderAttribution::Unknown,
                "conflicting_evidence" => ProviderAttribution::ConflictingEvidence,
                _ => return Err(FactError::InvalidRequest),
            };
            let model_cost = provider_cost::calculate(
                Some(&row.endpoint),
                row.model.as_deref(),
                Some(&row.started_at),
                &row.status,
                &row.usage,
                row.reported_cost.as_ref(),
                row.ended_at.as_deref(),
            );
            let mut model_stats = crate::model_usage::UsageTotals::default();
            model_stats.add(&row.usage, &model_cost);
            let mut request_cost = provider_cost::Totals::default();
            request_cost.add(&attempt.cost);
            let model_provider = crate::model_usage::provider_identity(
                Some(&row.provider),
                Some(&row.endpoint),
                Some(row.binding_revision),
            );
            let group = directory.group(&row.agent_id, row.agent_product_id.as_deref());
            let agent = if queryable && directory.family(&group).is_none() {
                key(namespace, "agent-group", &group)
            } else {
                group
            };
            attempts.push(AttemptFact {
                key: key(namespace, "attempt", &row.id),
                agent,
                agent_identity: queryable.then(|| {
                    super::agent_identity::AgentIdentity::new(
                        namespace,
                        &row.agent_id,
                        row.agent_product_id.as_deref(),
                    )
                }),
                provider_group: key(namespace, "provider", &attempt.provider_key),
                attributed_provider: provider
                    .provider_id
                    .as_ref()
                    .map(|id| key(namespace, "provider", id)),
                attribution,
                model_provider_group: key(namespace, "model-provider", &model_provider),
                model: ModelIdentity::new(row.model.as_deref(), namespace),
                routed_model: ModelIdentity::new(
                    row.routed_model.as_deref().or(row.model.as_deref()),
                    namespace,
                ),
                started_at: instant(&row.started_at).map(normalized),
                request_cost,
                request_basis: CostBasis::new(&attempt.cost),
                provider_basis: CostBasis::new(&provider.cost),
                model_basis: CostBasis::new(&model_cost),
                provider_stats: ProviderStatistics::from_record(&provider),
                model_stats,
            });
        }
        let mut parts = vec![];
        for dimension in [PartDimension::Provider, PartDimension::Model] {
            let mut groups = BTreeMap::<String, Vec<requests::Attempt>>::new();
            for attempt in &request.attempts {
                let group = match dimension {
                    PartDimension::Provider => attempt.provider_key.clone(),
                    PartDimension::Model => attempt
                        .observation
                        .routed_model
                        .clone()
                        .or(attempt.observation.model.clone())
                        .unwrap_or("模型未知".into()),
                };
                groups.entry(group).or_default().push(attempt.clone());
            }
            for (group, attempts) in groups {
                let fragment = requests::with_attempts(request, attempts);
                let projected = GatewayFact::from_request(&fragment, namespace, directory)?;
                let (part_key, label) = match dimension {
                    PartDimension::Provider => (key(namespace, "provider", &group), None),
                    PartDimension::Model => {
                        let model = ModelIdentity::new(Some(&group), namespace);
                        (model.key, model.label)
                    }
                };
                parts.push(PartFact {
                    dimension,
                    key: part_key,
                    label,
                    stats: projected.stats,
                    cache_in: projected.cache_in,
                    cache_read: projected.cache_read,
                });
            }
        }
        let mut evidence_digests = evidence
            .iter()
            .map(|entry| {
                serde_json::to_string(entry).map(|value| key(namespace, "evidence-entry", &value))
            })
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| FactError::InvalidRequest)?;
        evidence_digests.sort();
        evidence_digests.dedup();
        let evidence_key = key(namespace, "evidence-set", &evidence_digests.join("\0"));
        Ok(Self {
            evidence_key,
            session: request
                .session_id
                .as_ref()
                .map(|id| key(namespace, "session", id)),
            project: request
                .project_id
                .as_ref()
                .map(|id| key(namespace, "project", id)),
            attempts,
            parts,
        })
    }

    pub(super) fn valid(&self, expected_attempts: usize) -> bool {
        let digest = |s: &str| {
            s.len() == 64
                && s.bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        };
        if !digest(&self.evidence_key)
            || self.attempts.len() != expected_attempts
            || self.session.as_deref().is_some_and(|s| !digest(s))
            || self.project.as_deref().is_some_and(|s| !digest(s))
        {
            return false;
        }
        let unique = self
            .attempts
            .iter()
            .map(|a| a.key.as_str())
            .collect::<HashSet<_>>();
        if unique.len() != expected_attempts
            || self.attempts.iter().any(|a| {
                !digest(&a.key)
                    || !digest(&a.provider_group)
                    || !digest(&a.model_provider_group)
                    || !digest(&a.model.key)
                    || !digest(&a.routed_model.key)
                    || a.attributed_provider.as_deref().is_some_and(|s| !digest(s))
                    || a.provider_stats.calls != 1
            })
        {
            return false;
        }
        for dimension in [PartDimension::Provider, PartDimension::Model] {
            let expected = self
                .attempts
                .iter()
                .map(|a| match dimension {
                    PartDimension::Provider => a.provider_group.as_str(),
                    PartDimension::Model => a.routed_model.key.as_str(),
                })
                .collect::<HashSet<_>>();
            let actual = self
                .parts
                .iter()
                .filter(|p| p.dimension == dimension)
                .collect::<Vec<_>>();
            if actual.len() != expected.len()
                || actual
                    .iter()
                    .map(|p| p.key.as_str())
                    .collect::<HashSet<_>>()
                    != expected
                || actual
                    .iter()
                    .try_fold(0usize, |sum, p| sum.checked_add(p.stats.attempts))
                    != Some(expected_attempts)
                || actual.iter().any(|p| p.stats.requests != 1)
            {
                return false;
            }
        }
        true
    }
}

#[cfg(test)]
mod tests;
