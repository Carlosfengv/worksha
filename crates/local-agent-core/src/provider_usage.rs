//! Provider history is attributed by recorded identity or retained route evidence,
//! never by the current endpoint, display name or model catalog alone.
use crate::{
    Provider,
    observation::ModelObservation,
    provider_cost::{self, Cost},
    sessions::{CallSummary, Usage},
};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashSet};

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Query {
    pub source: String,
    pub agent: Option<String>,
    pub days: Option<u16>,
    pub provider_id: Option<String>,
    pub model: Option<String>,
    pub cursor: Option<String>,
}
impl Query {
    pub fn validate(&self) -> Result<(), &'static str> {
        self.validate_with_directory(crate::agents::builtin_directory())
    }
    pub fn validate_with_directory(
        &self,
        directory: &crate::agents::AgentDirectory,
    ) -> Result<(), &'static str> {
        if !["native", "gateway"].contains(&self.source.as_str())
            || self
                .agent
                .as_deref()
                .is_some_and(|a| !directory.valid_filter(a))
            || self.days.is_some_and(|d| ![1, 7, 30].contains(&d))
            || self
                .provider_id
                .as_ref()
                .is_some_and(|s| s.len() > 200 || s.is_empty())
            || self.model.as_ref().is_some_and(|s| s.chars().count() > 512)
            || self.cursor.as_ref().is_some_and(|s| s.len() > 100)
        {
            return Err("供应商用量筛选无效");
        }
        Ok(())
    }
}

#[derive(Serialize)]
pub struct Evidence {
    pub agent: String,
    pub provider_id: String,
    pub provider_name: String,
    pub endpoint: String,
    pub protocol: String,
    pub revision: u64,
    pub native_provider: Option<String>,
    pub model_aliases: Vec<(String, String)>,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Record {
    pub id: String,
    pub provider_id: Option<String>,
    pub provider: Option<String>,
    pub attribution: &'static str,
    pub agent_id: String,
    pub agent_product_id: Option<String>,
    pub model: Option<String>,
    pub time: Option<String>,
    pub status: String,
    pub usage: Usage,
    pub cost: Cost,
    pub duration_ms: Option<f64>,
    pub ttft_ms: Option<f64>,
    pub session_id: Option<String>,
    pub request_id: Option<String>,
}
fn unique<'a>(candidates: impl IntoIterator<Item = &'a Evidence>) -> (Option<String>, bool) {
    let mut provider_id = None;
    for evidence in candidates {
        if provider_id.is_some_and(|id| id != evidence.provider_id) {
            return (None, true);
        }
        provider_id = Some(evidence.provider_id.as_str());
    }
    (provider_id.map(str::to_owned), false)
}
pub fn gateway_records(rows: Vec<ModelObservation>, evidence: &[Evidence]) -> Vec<Record> {
    rows.into_iter()
        .map(|row| {
            let cost = provider_cost::for_provider(&row);
            let (provider_id, attribution) = if row.provider_id.is_some() {
                (row.provider_id, "provider_id")
            } else {
                let (id, conflict) = unique(evidence.iter().filter(|e| {
                    e.agent == row.agent_id
                        && e.revision == row.binding_revision
                        && e.provider_name == row.provider
                        && e.endpoint == row.endpoint
                        && e.protocol == row.protocol
                }));
                let state = if conflict {
                    "conflicting_evidence"
                } else if id.is_some() {
                    "route_snapshot"
                } else {
                    "unknown"
                };
                (id, state)
            };
            Record {
                cost,
                id: row.id.clone(),
                provider_id,
                provider: Some(row.provider),
                attribution,
                agent_id: row.agent_id,
                agent_product_id: row.agent_product_id,
                model: row.model,
                time: Some(row.started_at),
                status: row.status,
                usage: row.usage,
                duration_ms: row.duration_ms,
                ttft_ms: row.ttft_ms,
                session_id: row.session_id,
                request_id: Some(row.id),
            }
        })
        .collect()
}
pub fn native_records(
    rows: impl IntoIterator<Item = CallSummary>,
    evidence: &[Evidence],
) -> Vec<Record> {
    native_records_iter(rows.into_iter(), evidence).collect()
}
pub fn native_records_iter<'a>(
    rows: impl Iterator<Item = CallSummary> + 'a,
    evidence: &'a [Evidence],
) -> impl Iterator<Item = Record> + 'a {
    rows.into_iter().filter(|c| c.kind == "model").map(|call| {
        let candidates: Vec<_> = evidence
            .iter()
            .filter(|e| {
                e.agent == call.agent_id
                    && e.native_provider.is_some()
                    && e.native_provider == call.provider
            })
            .collect();
        let by_provider = !candidates.is_empty();
        let candidates = if by_provider {
            candidates
        } else {
            evidence
                .iter()
                .filter(|e| {
                    e.agent == call.agent_id
                        && e.model_aliases
                            .iter()
                            .any(|(alias, _)| Some(alias) == call.model.as_ref())
                })
                .collect()
        };
        let alias_models: BTreeSet<_> = candidates
            .iter()
            .flat_map(|e| &e.model_aliases)
            .filter(|(alias, _)| Some(alias) == call.model.as_ref())
            .map(|(_, model)| model.as_str())
            .collect();
        let (provider_id, conflict) = unique(candidates.iter().copied());
        let attribution = if conflict {
            "conflicting_evidence"
        } else if provider_id.is_some() {
            if by_provider {
                "native_provider"
            } else {
                "native_model"
            }
        } else {
            "unknown"
        };
        let model = if provider_id.is_some() && alias_models.len() == 1 {
            alias_models.first().map(|model| (*model).to_owned())
        } else {
            call.model
        };
        Record {
            cost: Cost::unknown("native_price_unconfirmed"),
            id: call.id,
            provider_id,
            provider: call.provider,
            attribution,
            agent_id: call.agent_id,
            agent_product_id: call.agent_product_id,
            model,
            time: call.timestamp,
            status: call.status,
            usage: call.usage.unwrap_or_default(),
            duration_ms: call.duration_ms.map(|d| d as f64),
            ttft_ms: None,
            session_id: Some(call.session_id),
            request_id: None,
        }
    })
}

#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Totals {
    pub cost: provider_cost::Totals,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub unknown_cost_reasons: BTreeMap<String, usize>,
    pub calls: usize,
    pub errors: usize,
    pub in_progress: usize,
    pub unknown_outcomes: usize,
    pub usage: Usage,
    pub reported: BTreeMap<&'static str, usize>,
    pub last_seen: Option<String>,
    pub latency_p95_ms: Option<f64>,
    pub latency_samples: usize,
    pub ttft_p95_ms: Option<f64>,
    pub ttft_samples: usize,
    #[serde(skip)]
    latencies: Vec<f64>,
    #[serde(skip)]
    ttfts: Vec<f64>,
}
impl Totals {
    pub(crate) fn add(&mut self, row: &Record) {
        self.calls += 1;
        self.cost.add(&row.cost);
        if row.cost.kind == "unknown"
            && let Some(reason) = row.cost.reason
        {
            *self.unknown_cost_reasons.entry(reason.into()).or_default() += 1;
        }
        self.errors += usize::from(["error", "failed"].contains(&row.status.as_str()));
        self.in_progress += usize::from(row.status == "in_progress");
        self.unknown_outcomes += usize::from(
            ![
                "success",
                "returned",
                "error",
                "failed",
                "cancelled",
                "in_progress",
            ]
            .contains(&row.status.as_str()),
        );
        self.usage.add(&row.usage);
        for (key, value) in [
            ("inputTokens", row.usage.input_tokens),
            ("outputTokens", row.usage.output_tokens),
            ("totalTokens", row.usage.total_tokens),
            ("cachedInputTokens", row.usage.cached_input_tokens),
            ("cacheWriteInputTokens", row.usage.cache_write_input_tokens),
            ("reasoningOutputTokens", row.usage.reasoning_output_tokens),
        ] {
            *self.reported.entry(key).or_default() += usize::from(value.is_some());
        }
        if let Some(time) = row
            .time
            .as_deref()
            .and_then(|time| DateTime::parse_from_rfc3339(time).ok())
            .map(|time| time.with_timezone(&Utc))
            && self
                .last_seen
                .as_ref()
                .and_then(|t| DateTime::parse_from_rfc3339(t).ok())
                .is_none_or(|old| time > old)
        {
            self.last_seen = Some(time.to_rfc3339());
        }
        if ["success", "returned", "error", "failed"].contains(&row.status.as_str()) {
            if let Some(ms) = row.duration_ms.filter(|n| n.is_finite() && *n >= 0.0) {
                self.latencies.push(ms);
            }
            if let Some(ms) = row.ttft_ms.filter(|n| n.is_finite() && *n >= 0.0) {
                self.ttfts.push(ms);
            }
        }
    }
    pub(crate) fn finish(&mut self) {
        fn p95(values: &mut [f64]) -> Option<f64> {
            values.sort_by(f64::total_cmp);
            (!values.is_empty()).then(|| values[(values.len() * 95).div_ceil(100) - 1])
        }
        self.latency_p95_ms = p95(&mut self.latencies);
        self.latency_samples = self.latencies.len();
        self.ttft_p95_ms = p95(&mut self.ttfts);
        self.ttft_samples = self.ttfts.len();
    }
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderTotal {
    pub provider_id: String,
    pub totals: Totals,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelTotal {
    pub model: Option<String>,
    pub totals: Totals,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderModelTotal {
    pub provider_id: String,
    pub model: Option<String>,
    pub totals: Totals,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub query: Query,
    pub as_of: String,
    pub source_as_of: String,
    pub source_state: String,
    pub summary: Totals,
    pub providers: Vec<ProviderTotal>,
    pub unattributed: Totals,
    pub selected: Option<Totals>,
    pub model_options: Vec<Option<String>>,
    pub models: Vec<ModelTotal>,
    pub provider_models: Vec<ProviderModelTotal>,
    pub calls: Vec<Record>,
    pub total_calls: usize,
    pub next_cursor: Option<String>,
    pub missing_time: usize,
}
pub fn aggregate(
    query: Query,
    providers: &[Provider],
    rows: Vec<Record>,
    source_as_of: String,
    source_state: String,
    now: DateTime<Utc>,
) -> Result<Snapshot, &'static str> {
    query.validate()?;
    let mut totals: BTreeMap<_, _> = providers
        .iter()
        .filter(|p| p.configured)
        .map(|p| (p.id.clone(), Totals::default()))
        .collect();
    if query
        .provider_id
        .as_deref()
        .is_some_and(|id| id != "_unknown" && !totals.contains_key(id))
    {
        return Err("未找到供应商连接");
    }
    let mut summary = Totals::default();
    let mut unattributed = Totals::default();
    let mut models: BTreeMap<Option<String>, Totals> = BTreeMap::new();
    let mut provider_models: BTreeMap<(String, Option<String>), Totals> = BTreeMap::new();
    let mut options = BTreeSet::new();
    let mut selected = query.provider_id.as_ref().map(|_| Totals::default());
    let mut calls = vec![];
    let mut missing_time = 0;
    let mut seen = HashSet::new();
    let start = query.days.map(|d| now - Duration::days(d.into()));
    for mut row in rows {
        if !seen.insert(row.id.clone())
            || !crate::agents::matches(
                query.agent.as_deref(),
                &row.agent_id,
                row.agent_product_id.as_deref(),
            )
        {
            continue;
        }
        let time = row
            .time
            .as_deref()
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|t| t.with_timezone(&Utc));
        if row
            .provider_id
            .as_ref()
            .is_some_and(|id| !totals.contains_key(id))
        {
            row.provider_id = None;
            row.attribution = "removed_provider";
        }
        let matches = query.provider_id.as_deref().is_some_and(|id| {
            if id == "_unknown" {
                row.provider_id.is_none()
            } else {
                row.provider_id.as_deref() == Some(id)
            }
        });
        if time.is_none()
            && (query.provider_id.is_none() || matches)
            && query
                .model
                .as_ref()
                .is_none_or(|m| m == row.model.as_deref().unwrap_or(""))
        {
            missing_time += 1;
        }
        if time.is_some_and(|t| t > now) || start.is_some_and(|s| time.is_none_or(|t| t < s)) {
            continue;
        }
        summary.add(&row);
        if let Some(id) = &row.provider_id {
            totals.get_mut(id).unwrap().add(&row);
            provider_models
                .entry((id.clone(), row.model.clone()))
                .or_default()
                .add(&row);
        } else {
            unattributed.add(&row);
        }
        if !matches {
            continue;
        }
        options.insert(row.model.clone());
        if query
            .model
            .as_ref()
            .is_some_and(|m| m != row.model.as_deref().unwrap_or(""))
        {
            continue;
        }
        selected.as_mut().unwrap().add(&row);
        models.entry(row.model.clone()).or_default().add(&row);
        calls.push(row);
    }
    summary.finish();
    unattributed.finish();
    totals.values_mut().for_each(Totals::finish);
    models.values_mut().for_each(Totals::finish);
    provider_models.values_mut().for_each(Totals::finish);
    if let Some(s) = &mut selected {
        s.finish();
    }
    calls.sort_by(|a, b| b.time.cmp(&a.time).then(a.id.cmp(&b.id)));
    let version = format!(
        "{:x}",
        Sha256::digest(
            serde_json::to_vec(&(
                &calls,
                &source_state,
                &query.source,
                &query.agent,
                query.days,
                &query.provider_id,
                &query.model
            ))
            .unwrap_or_default()
        )
    );
    let offset = if let Some(cursor) = &query.cursor {
        let (revision, offset) = cursor.split_once(':').ok_or("供应商调用分页无效")?;
        if revision != version {
            return Err("供应商调用已更新，请从第一页重新读取");
        }
        let offset = offset.parse::<usize>().map_err(|_| "供应商调用分页无效")?;
        if offset > calls.len() || !offset.is_multiple_of(40) {
            return Err("供应商调用分页无效");
        }
        offset
    } else {
        0
    };
    let total_calls = calls.len();
    Ok(Snapshot {
        query,
        as_of: now.to_rfc3339(),
        source_as_of,
        source_state,
        summary,
        providers: totals
            .into_iter()
            .map(|(provider_id, totals)| ProviderTotal {
                provider_id,
                totals,
            })
            .collect(),
        unattributed,
        selected,
        model_options: options.into_iter().collect(),
        models: models
            .into_iter()
            .map(|(model, totals)| ModelTotal { model, totals })
            .collect(),
        provider_models: provider_models
            .into_iter()
            .map(|((provider_id, model), totals)| ProviderModelTotal {
                provider_id,
                model,
                totals,
            })
            .collect(),
        calls: calls.into_iter().skip(offset).take(40).collect(),
        total_calls,
        next_cursor: (offset + 40 < total_calls).then(|| format!("{version}:{}", offset + 40)),
        missing_time,
    })
}
