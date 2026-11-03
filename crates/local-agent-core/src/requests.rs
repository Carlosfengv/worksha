//! Logical request read model shared by the dashboard, detail and frozen export.
pub(crate) mod csv;
pub mod domain_read;
pub mod export;
mod heatmap;
pub mod sources;
use crate::{observation::ModelObservation, provider_cost, sessions::Usage};
use chrono::{DateTime, Duration, Utc};
pub use heatmap::Heatmap;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
pub use sources::{NativeLink, NativeRecord, SourceReport};
use std::collections::{BTreeMap, HashMap};

#[derive(Clone, Default, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Query {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub search: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub issue: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub descending: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metric: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dimension: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub currency: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}
impl Query {
    pub fn validate(&self) -> Result<(), String> {
        self.validate_with_directory(crate::agents::builtin_directory())
    }
    pub fn validate_with_directory(
        &self,
        directory: &crate::agents::AgentDirectory,
    ) -> Result<(), String> {
        for v in [&self.start, &self.end].into_iter().flatten() {
            DateTime::parse_from_rfc3339(v).map_err(|_| "日期范围无效")?;
        }
        if self
            .source
            .as_deref()
            .is_some_and(|s| !["gateway", "native", "linked"].contains(&s))
            || self
                .start
                .as_ref()
                .zip(self.end.as_ref())
                .is_some_and(|(a, b)| time(a) >= time(b))
            || self
                .agent
                .as_deref()
                .is_some_and(|s| !directory.valid_filter(s))
            || self.result.as_deref().is_some_and(|s| {
                ![
                    "success",
                    "recovered",
                    "failed",
                    "cancelled",
                    "unknown",
                    "waiting",
                    "streaming",
                ]
                .contains(&s)
            })
            || self
                .sort
                .as_deref()
                .is_some_and(|s| !["time", "ttft", "duration", "tokens", "cost"].contains(&s))
            || self.metric.as_deref().is_some_and(|s| {
                !["calls", "tokens", "cost", "ttft", "duration", "throughput"].contains(&s)
            })
            || self
                .dimension
                .as_deref()
                .is_some_and(|s| !["agent", "model", "provider"].contains(&s))
            || self.issue.as_deref().is_some_and(|s| {
                ![
                    "rate_limit",
                    "authentication",
                    "upstream_rejection",
                    "upstream_error",
                    "timeout",
                    "connection",
                    "stream",
                    "protocol",
                    "unmeasurable",
                ]
                .contains(&s)
            })
            || self
                .currency
                .as_ref()
                .is_some_and(|s| s.len() != 3 || !s.bytes().all(|c| c.is_ascii_uppercase()))
            || [
                &self.provider,
                &self.model,
                &self.session,
                &self.project,
                &self.search,
                &self.snapshot,
                &self.cursor,
            ]
            .into_iter()
            .flatten()
            .any(|s| s.len() > 1024)
        {
            return Err("请求筛选无效".into());
        }
        Ok(())
    }
    pub fn same_filter(&self, other: &Self) -> bool {
        let mut a = self.clone();
        let mut b = other.clone();
        a.snapshot = None;
        a.cursor = None;
        b.snapshot = None;
        b.cursor = None;
        a == b
    }
}
fn time(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|t| t.with_timezone(&Utc))
}
fn valid(n: Option<f64>) -> Option<f64> {
    n.filter(|n| n.is_finite() && *n >= 0.0)
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Context {
    pub session_id: String,
    pub title: String,
    pub project_id: Option<String>,
    pub project_name: Option<String>,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Attempt {
    #[serde(flatten)]
    pub observation: ModelObservation,
    pub cost: provider_cost::Cost,
    pub issue: Option<String>,
    pub provider_key: String,
    pub backoff_ms: Option<f64>,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Request {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client: Option<crate::observation::GatewayClient>,
    pub source: String,
    pub native: Option<NativeRecord>,
    pub native_links: Vec<NativeLink>,
    pub id: String,
    pub agent_id: String,
    pub agent_product_id: Option<String>,
    pub session_id: Option<String>,
    pub session_title: Option<String>,
    pub project_id: Option<String>,
    pub project_name: Option<String>,
    pub started_at: String,
    pub ended_at: Option<String>,
    pub status: String,
    pub model: Option<String>,
    pub provider: String,
    pub provider_key: String,
    pub usage: Usage,
    pub usage_missing: usize,
    pub cost: provider_cost::Totals,
    pub duration_ms: Option<f64>,
    pub ttft_ms: Option<f64>,
    pub throughput: Option<f64>,
    pub timing_note: Option<String>,
    pub attempts: Vec<Attempt>,
}
pub fn provider_key(r: &ModelObservation) -> String {
    r.provider_id.clone().unwrap_or_else(|| {
        format!(
            "snapshot-{:x}",
            Sha256::digest(format!("{}\0{}", r.provider, r.endpoint))
        )
    })
}
/// Scope idempotency to the client, saved route and Session. A key reused in
/// another Session or Provider is a distinct logical request.
pub fn logical_request_key(r: &ModelObservation) -> String {
    if let (Some(id), Some(route)) = (&r.logical_id, &r.routing)
        && id.starts_with("model-service-")
        && route.service.is_some()
    {
        return format!(
            "logical-{:x}",
            Sha256::digest(format!(
                "{}\0{}\0{}\0{}",
                crate::agents::group(&r.agent_id, r.agent_product_id.as_deref()),
                route.binding_id,
                r.session_id.as_deref().unwrap_or(""),
                id
            ))
        );
    }
    r.logical_id
        .as_ref()
        .map(|id| {
            format!(
                "logical-{:x}",
                Sha256::digest(format!(
                    "{}\0{}\0{}\0{}\0{}",
                    crate::agents::group(&r.agent_id, r.agent_product_id.as_deref()),
                    r.binding_revision,
                    r.session_id.as_deref().unwrap_or(""),
                    provider_key(r),
                    id
                ))
            )
        })
        .unwrap_or_else(|| r.id.clone())
}
fn issue(r: &ModelObservation) -> Option<String> {
    if r.http_status == Some(429) {
        return Some("rate_limit".into());
    }
    if matches!(r.http_status, Some(401 | 403)) {
        return Some("authentication".into());
    }
    if r.http_status.is_some_and(|s| (400..500).contains(&s)) {
        return Some("upstream_rejection".into());
    }
    if r.http_status.is_some_and(|s| s >= 500) {
        return Some("upstream_error".into());
    }
    let note = r.timing_issue.as_deref().unwrap_or("");
    if r.status == "success" {
        return None;
    }
    Some(
        if note.contains("超时") {
            "timeout"
        } else if note.contains("连接") && r.http_status.is_none() {
            "connection"
        } else if r.status == "incomplete" || note.contains("断流") || note.contains("流事件")
        {
            "stream"
        } else if note.contains("协议") || note.contains("解析") || note.contains("转换") {
            "protocol"
        } else {
            return None;
        }
        .into(),
    )
}
fn success(s: &str) -> bool {
    ["success", "recovered"].contains(&s)
}
fn clean_endpoint(s: &str) -> String {
    url::Url::parse(s)
        .map(|mut u| {
            let _ = u.set_username("");
            let _ = u.set_password(None);
            u.set_query(None);
            u.set_fragment(None);
            u.to_string()
        })
        .unwrap_or_else(|_| "端点未知".into())
}
pub fn logical_requests(
    rows: Vec<ModelObservation>,
    contexts: &HashMap<String, Context>,
) -> Vec<Request> {
    let mut unique = HashMap::<String, ModelObservation>::new();
    for row in rows {
        if unique
            .get(&row.id)
            .is_none_or(|old| old.revision <= row.revision)
        {
            unique.insert(row.id.clone(), row);
        }
    }
    let mut groups = BTreeMap::<String, Vec<ModelObservation>>::new();
    for r in unique.into_values() {
        let key = logical_request_key(&r);
        groups.entry(key).or_default().push(r);
    }
    groups
        .into_iter()
        .map(|(id, mut rows)| {
            rows.sort_by(|a, b| {
                time(&a.started_at)
                    .cmp(&time(&b.started_at))
                    .then(a.id.cmp(&b.id))
            });
            let first = &rows[0];
            let last = rows.last().unwrap();
            let status = if rows.iter().any(|r| r.status == "in_progress") {
                if rows
                    .iter()
                    .any(|r| r.status == "in_progress" && r.ttft_ms.is_some())
                {
                    "streaming"
                } else {
                    "waiting"
                }
            } else if last.status == "success" {
                if rows.len() > 1 && rows.iter().any(|r| r.status != "success") {
                    "recovered"
                } else {
                    "success"
                }
            } else if last.status == "cancelled" {
                "cancelled"
            } else if ["error", "failed", "incomplete"].contains(&last.status.as_str()) {
                "failed"
            } else {
                "unknown"
            };
            let context = first.session_id.as_ref().and_then(|id| {
                contexts
                    .get(&format!("{}:{id}", first.agent_id))
                    .or_else(|| contexts.get(id))
            });
            let mut result = Request {
                client: first.client.clone(),
                source: "gateway".into(),
                native: None,
                native_links: vec![],
                id,
                agent_id: first.agent_id.clone(),
                agent_product_id: first.agent_product_id.clone(),
                session_id: context
                    .map(|c| c.session_id.clone())
                    .or(first.session_id.clone()),
                session_title: context.map(|c| c.title.clone()),
                project_id: context.and_then(|c| c.project_id.clone()),
                project_name: context.and_then(|c| c.project_name.clone()),
                started_at: first.started_at.clone(),
                ended_at: last.ended_at.clone(),
                status: status.into(),
                model: first.routed_model.clone().or(first.model.clone()),
                provider: first.provider.clone(),
                provider_key: provider_key(first),
                usage: Usage::default(),
                usage_missing: 0,
                cost: provider_cost::Totals::default(),
                duration_ms: None,
                ttft_ms: None,
                throughput: None,
                timing_note: None,
                attempts: vec![],
            };
            let complete_clock = first.clock_id.is_some()
                && rows
                    .iter()
                    .all(|r| r.clock_id == first.clock_id && valid(r.start_tick_ms).is_some());
            if rows.len() == 1 {
                result.duration_ms = valid(first.duration_ms);
                result.ttft_ms = valid(first.ttft_ms);
            } else if complete_clock {
                let origin = first.start_tick_ms.unwrap();
                if let Some(end) = valid(last.start_tick_ms).zip(valid(last.duration_ms)) {
                    result.duration_ms = valid(Some(end.0 + end.1 - origin));
                }
                result.ttft_ms = rows
                    .iter()
                    .find_map(|r| {
                        valid(r.start_tick_ms)
                            .zip(valid(r.ttft_ms))
                            .map(|(s, t)| s + t - origin)
                    })
                    .and_then(|n| valid(Some(n)));
            } else {
                result.timing_note =
                    Some("历史重试缺少同一单调时钟证据；仅展示各次尝试的计时".into());
            }
            result.throughput = success(status).then_some(()).and_then(|_| {
                valid(result.duration_ms)
                    .filter(|n| *n > 0.0)
                    .zip(last.usage.output_tokens.filter(|n| *n > 0))
                    .map(|(ms, n)| n as f64 * 1000.0 / ms)
            });
            let mut previous_end = None;
            for mut r in rows {
                let cost = provider_cost::calculate(
                    Some(&r.endpoint),
                    r.model.as_deref(),
                    Some(&r.started_at),
                    &r.status,
                    &r.usage,
                    r.reported_cost.as_ref(),
                    r.ended_at.as_deref(),
                );
                result.cost.add(&cost);
                result.usage.add(&r.usage);
                result.usage_missing += usize::from(r.usage.total_tokens.is_none());
                let backoff = if complete_clock {
                    previous_end
                        .zip(r.start_tick_ms)
                        .and_then(|(e, s)| valid(Some(s - e)))
                } else {
                    None
                };
                previous_end = r.start_tick_ms.zip(r.duration_ms).map(|(s, d)| s + d);
                let problem = issue(&r);
                let provider_key = provider_key(&r);
                r.endpoint = clean_endpoint(&r.endpoint);
                result.attempts.push(Attempt {
                    observation: r,
                    cost,
                    issue: problem,
                    provider_key,
                    backoff_ms: backoff,
                });
            }
            result
        })
        .collect()
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Distribution {
    pub samples: usize,
    pub p50: Option<f64>,
    pub p95: Option<f64>,
}
fn distribution(mut values: Vec<f64>) -> Distribution {
    values.retain(|v| v.is_finite() && *v >= 0.0);
    values.sort_by(f64::total_cmp);
    let percentile =
        |p: usize| (!values.is_empty()).then(|| values[(values.len() * p).div_ceil(100) - 1]);
    Distribution {
        samples: values.len(),
        p50: percentile(50),
        p95: percentile(95),
    }
}
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    pub requests: usize,
    pub attempts: usize,
    pub statuses: BTreeMap<String, usize>,
    pub issues: BTreeMap<String, usize>,
    pub success_rate: Option<f64>,
    pub usage: Usage,
    pub usage_missing: usize,
    pub provisional: usize,
    pub cost: provider_cost::Totals,
    pub cache_ratio: Option<f64>,
    pub cache_samples: usize,
    pub ttft: Distribution,
    pub duration: Distribution,
    pub throughput: Distribution,
}
impl Summary {
    pub fn of<'a>(rows: impl IntoIterator<Item = &'a Request>) -> Self {
        let mut s = Self::default();
        let mut ttft = vec![];
        let mut duration = vec![];
        let mut throughput = vec![];
        let mut cache_in = 0u128;
        let mut cache_read = 0u128;
        for r in rows {
            s.requests += 1;
            s.attempts += r.attempts.len();
            if r.native.is_some() {
                s.cost
                    .add(&provider_cost::Cost::unknown("native_price_unconfirmed"));
                if let Some((i, c)) = r
                    .usage
                    .input_tokens
                    .zip(r.usage.cached_input_tokens)
                    .filter(|(i, c)| c <= i)
                {
                    cache_in += u128::from(i);
                    cache_read += u128::from(c);
                    s.cache_samples += 1;
                }
            }
            *s.statuses.entry(r.status.clone()).or_default() += 1;
            s.provisional += usize::from(["waiting", "streaming"].contains(&r.status.as_str()));
            s.usage.add(&r.usage);
            s.usage_missing += usize::from(r.usage_missing > 0);
            for a in &r.attempts {
                s.cost.add(&a.cost);
                if let Some(i) = &a.issue {
                    *s.issues.entry(i.clone()).or_default() += 1;
                }
                if let Some((input, cached)) = a
                    .observation
                    .usage
                    .input_tokens
                    .zip(a.observation.usage.cached_input_tokens)
                    .filter(|(i, c)| c <= i)
                {
                    cache_in += u128::from(input);
                    cache_read += u128::from(cached);
                    s.cache_samples += 1;
                }
            }
            if success(&r.status) {
                if let Some(n) = r.ttft_ms {
                    ttft.push(n);
                }
                if let Some(n) = r.duration_ms {
                    duration.push(n);
                }
                if let Some(n) = r.throughput {
                    throughput.push(n);
                }
            }
        }
        let count = |k: &str| *s.statuses.get(k).unwrap_or(&0);
        let completed = count("success") + count("recovered") + count("failed");
        s.success_rate = (completed > 0 && s.attempts > 0)
            .then(|| (count("success") + count("recovered")) as f64 / completed as f64);
        s.cache_ratio = (cache_in > 0).then(|| cache_read as f64 / cache_in as f64);
        s.ttft = distribution(ttft);
        s.duration = distribution(duration);
        s.throughput = distribution(throughput);
        s
    }
    pub(crate) fn metric(&self, metric: &str, currency: &str) -> Option<f64> {
        match metric {
            "tokens" => self.usage.total_tokens.map(|n| n as f64),
            "cost" => {
                let values: Vec<_> = self
                    .cost
                    .actual
                    .iter()
                    .chain(&self.cost.estimated)
                    .filter(|c| c.currency == currency)
                    .collect();
                if self.requests == 0 {
                    Some(0.0)
                } else if values.is_empty() {
                    None
                } else {
                    Some(values.iter().map(|c| c.amount).sum())
                }
            }
            "ttft" => self.ttft.p50,
            "duration" => self.duration.p50,
            "throughput" => self.throughput.p50,
            _ => Some(self.requests as f64),
        }
    }
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Point {
    pub groups: Vec<Group>,
    pub start: String,
    pub end: String,
    pub value: Option<f64>,
    pub p95: Option<f64>,
    pub samples: usize,
    pub missing: usize,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Group {
    pub id: String,
    pub label: String,
    pub value: Option<f64>,
    pub samples: usize,
    pub missing: usize,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OptionItem {
    pub id: String,
    pub label: String,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Dashboard {
    pub query: Query,
    pub as_of: String,
    pub snapshot: String,
    pub summary: Summary,
    pub buckets: Vec<Point>,
    pub groups: Vec<Group>,
    pub items: Vec<Request>,
    pub total: usize,
    pub next_cursor: Option<String>,
    pub models: Vec<String>,
    pub providers: Vec<OptionItem>,
    pub source_error: Option<String>,
    pub coverage: String,
    pub recorded_start: Option<String>,
    pub recorded_end: Option<String>,
    pub missing_time: usize,
    pub source_report: Option<SourceReport>,
    pub heatmap: Heatmap,
}

pub(crate) fn with_attempts(row: &Request, attempts: Vec<Attempt>) -> Request {
    let mut fragment = row.clone();
    fragment.usage = Usage::default();
    fragment.usage_missing = 0;
    for attempt in &attempts {
        fragment.usage.add(&attempt.observation.usage);
        fragment.usage_missing += usize::from(attempt.observation.usage.total_tokens.is_none());
    }
    fragment.attempts = attempts;
    fragment
}

fn grouped_values(rows: &[Request], metric: &str, dimension: &str, currency: &str) -> Vec<Group> {
    let mut grouped = BTreeMap::<String, (String, Vec<Request>)>::new();
    for r in rows {
        if ["tokens", "cost"].contains(&metric) && dimension != "agent" && !r.attempts.is_empty() {
            let mut fragments = BTreeMap::<String, Vec<Attempt>>::new();
            for a in &r.attempts {
                let key = if dimension == "provider" {
                    a.provider_key.clone()
                } else {
                    a.observation
                        .routed_model
                        .clone()
                        .or(a.observation.model.clone())
                        .unwrap_or("模型未知".into())
                };
                fragments.entry(key).or_default().push(a.clone());
            }
            for (key, attempts) in fragments {
                let label = if dimension == "provider" {
                    attempts[0].observation.provider.clone()
                } else {
                    key.clone()
                };
                let fragment = with_attempts(r, attempts);
                grouped
                    .entry(key)
                    .or_insert_with(|| (label, vec![]))
                    .1
                    .push(fragment);
            }
        } else {
            let key = match dimension {
                "provider" => r.provider_key.clone(),
                "model" => r.model.clone().unwrap_or("模型未知".into()),
                _ => crate::agents::group(&r.agent_id, r.agent_product_id.as_deref()),
            };
            let label = if dimension == "provider" {
                r.provider.clone()
            } else {
                key.clone()
            };
            grouped
                .entry(key)
                .or_insert_with(|| (label, vec![]))
                .1
                .push(r.clone());
        }
    }
    let mut groups: Vec<_> = grouped
        .into_iter()
        .map(|(id, (label, rows))| {
            let s = Summary::of(&rows);
            Group {
                id,
                label,
                value: s.metric(metric, currency),
                samples: match metric {
                    "ttft" => s.ttft.samples,
                    "duration" => s.duration.samples,
                    "throughput" => s.throughput.samples,
                    _ => s.requests,
                },
                missing: if metric == "tokens" {
                    s.usage_missing
                } else if metric == "cost" {
                    s.cost.unknown_calls
                } else {
                    s.requests.saturating_sub(match metric {
                        "ttft" => s.ttft.samples,
                        "duration" => s.duration.samples,
                        "throughput" => s.throughput.samples,
                        _ => s.requests,
                    })
                },
            }
        })
        .collect();
    groups.sort_by(|a, b| {
        b.value
            .unwrap_or(-1.0)
            .total_cmp(&a.value.unwrap_or(-1.0))
            .then(a.id.cmp(&b.id))
    });
    groups
}
pub struct Snapshot {
    pub query: Query,
    pub created: DateTime<Utc>,
    pub dashboard: Dashboard,
    pub rows: Vec<Request>,
}
impl Snapshot {
    pub fn new(
        query: Query,
        mut rows: Vec<Request>,
        now: DateTime<Utc>,
        id: String,
        source_error: Option<String>,
    ) -> Result<Self, String> {
        query.validate()?;
        let mut models = BTreeMap::<String, ()>::new();
        let mut providers = BTreeMap::<String, String>::new();
        for r in &rows {
            if r.native.is_some() {
                if let Some(model) = &r.model {
                    models.insert(model.clone(), ());
                }
                providers.insert(r.provider_key.clone(), r.provider.clone());
            }
            for a in &r.attempts {
                if let Some(m) = a
                    .observation
                    .routed_model
                    .as_ref()
                    .or(a.observation.model.as_ref())
                {
                    models.insert(m.clone(), ());
                }
                providers.insert(a.provider_key.clone(), a.observation.provider.clone());
            }
        }
        let recorded_start = rows
            .iter()
            .filter_map(|r| time(&r.started_at))
            .min()
            .map(|t| t.to_rfc3339());
        let recorded_end = rows
            .iter()
            .filter_map(|r| time(&r.started_at))
            .max()
            .map(|t| t.to_rfc3339());
        let missing_time = rows
            .iter()
            .filter(|r| {
                time(&r.started_at).is_none()
                    && matches(
                        r,
                        &Query {
                            start: None,
                            end: None,
                            ..query.clone()
                        },
                    )
            })
            .count();
        rows.retain(|r| matches(r, &query) && time(&r.started_at).is_none_or(|t| t < now));
        let currency = query.currency.as_deref().unwrap_or("USD");
        let metric = query.metric.as_deref().unwrap_or("calls");
        let dimension = query.dimension.as_deref().unwrap_or("agent");
        let summary = Summary::of(&rows);
        let heatmap = Heatmap::new(&rows, now, &query);
        let start = query
            .start
            .as_deref()
            .and_then(time)
            .or_else(|| rows.iter().filter_map(|r| time(&r.started_at)).min())
            .unwrap_or(now - Duration::hours(24));
        let end = query.end.as_deref().and_then(time).unwrap_or(now).min(now);
        let span = (end - start).num_seconds().max(1);
        let step = if span <= 86400 {
            3600
        } else if span <= 86400 * 7 {
            86400 / 4
        } else if span <= 86400 * 60 {
            86400
        } else {
            ((span / 86400 / 52) + 1) * 86400
        };
        let mut buckets = vec![];
        let mut at = start;
        while at < end && buckets.len() < 100 {
            let next = (at + Duration::seconds(step)).min(end);
            let s = Summary::of(
                rows.iter()
                    .filter(|r| time(&r.started_at).is_some_and(|t| t >= at && t < next)),
            );
            let distribution = match metric {
                "ttft" => &s.ttft,
                "duration" => &s.duration,
                _ => &s.throughput,
            };
            let samples = if ["ttft", "duration", "throughput"].contains(&metric) {
                distribution.samples
            } else {
                s.requests
            };
            let bucket_rows: Vec<_> = rows
                .iter()
                .filter(|r| time(&r.started_at).is_some_and(|t| t >= at && t < next))
                .cloned()
                .collect();
            buckets.push(Point {
                groups: grouped_values(&bucket_rows, metric, dimension, currency),
                start: at.to_rfc3339(),
                end: next.to_rfc3339(),
                value: s.metric(metric, currency),
                p95: ["ttft", "duration"]
                    .contains(&metric)
                    .then_some(distribution.p95)
                    .flatten(),
                samples,
                missing: if metric == "tokens" {
                    s.usage_missing
                } else if metric == "cost" {
                    s.cost.unknown_calls
                } else {
                    s.requests.saturating_sub(samples)
                },
            });
            at = next;
        }
        let groups = grouped_values(&rows, metric, dimension, currency);
        let sort = query.sort.as_deref().unwrap_or("time");
        let desc = query.descending.unwrap_or(true);
        rows.sort_by(|a, b| {
            let value = |r: &Request| match sort {
                "ttft" => r.ttft_ms,
                "duration" => r.duration_ms,
                "tokens" => r.usage.total_tokens.map(|n| n as f64),
                "cost" => {
                    let s = Summary::of([r]);
                    s.metric("cost", currency)
                }
                _ => time(&r.started_at).map(|t| t.timestamp_millis() as f64),
            };
            match (value(a), value(b)) {
                (Some(a), Some(b)) => {
                    if desc {
                        b.total_cmp(&a)
                    } else {
                        a.total_cmp(&b)
                    }
                }
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (Some(_), None) => std::cmp::Ordering::Less,
                _ => std::cmp::Ordering::Equal,
            }
            .then(a.id.cmp(&b.id))
        });
        let total = rows.len();
        let dashboard = Dashboard {
            query: query.clone(),
            as_of: now.to_rfc3339(),
            snapshot: id,
            summary,
            buckets,
            groups,
            items: vec![],
            total,
            next_cursor: None,
            models: models.into_keys().collect(),
            providers: providers
                .into_iter()
                .map(|(id, label)| OptionItem { id, label })
                .collect(),
            source_error,
            coverage: "仅覆盖已记录的网关请求；原生记录与本地拒绝未计入".into(),
            recorded_start,
            recorded_end,
            missing_time,
            source_report: None,
            heatmap,
        };
        Ok(Self {
            query,
            created: now,
            dashboard,
            rows,
        })
    }
    pub fn page(&self, query: &Query) -> Result<Dashboard, String> {
        if !self.query.same_filter(query) {
            return Err("快照筛选已变化，请刷新请求".into());
        }
        let offset = if let Some(cursor) = &query.cursor {
            cursor
                .strip_prefix(&format!("{}:", self.dashboard.snapshot))
                .ok_or("分页游标不属于当前快照")?
                .parse::<usize>()
                .map_err(|_| "分页游标无效")?
        } else {
            0
        };
        if offset > self.rows.len() {
            return Err("分页游标超出范围".into());
        }
        let mut page = self.dashboard.clone();
        page.items = self.rows.iter().skip(offset).take(40).cloned().collect();
        page.next_cursor =
            (offset + 40 < self.rows.len()).then(|| format!("{}:{}", page.snapshot, offset + 40));
        Ok(page)
    }
    pub fn csv(&self) -> String {
        let mut text = csv::header();
        for row in &self.rows {
            csv::write(
                &mut text,
                &serde_json::to_value(row).expect("request metadata is serializable"),
            );
        }
        text
    }
}
fn csv_dangerous(s: &str) -> bool {
    s.trim_start_matches(char::is_whitespace)
        .starts_with(['=', '+', '-', '@'])
        || s.starts_with(['\t', '\r', '\n'])
}
fn csv_cell(s: &str) -> String {
    format!(
        "\"{}{}\"",
        if csv_dangerous(s) { "'" } else { "" },
        s.replace('"', "\"\"")
    )
}
pub(crate) fn matches(r: &Request, q: &Query) -> bool {
    let t = time(&r.started_at);
    crate::agents::matches(
        q.agent.as_deref(),
        &r.agent_id,
        r.agent_product_id.as_deref(),
    ) && q
        .start
        .as_deref()
        .and_then(time)
        .is_none_or(|s| t.is_some_and(|t| t >= s))
        && q.end
            .as_deref()
            .and_then(time)
            .is_none_or(|e| t.is_some_and(|t| t < e))
        && q.result.as_ref().is_none_or(|s| s == &r.status)
        && q.provider
            .as_ref()
            .is_none_or(|p| &r.provider_key == p || r.attempts.iter().any(|a| &a.provider_key == p))
        && q.model.as_ref().is_none_or(|m| {
            r.model.as_ref() == Some(m)
                || r.attempts.iter().any(|a| {
                    a.observation
                        .routed_model
                        .as_ref()
                        .or(a.observation.model.as_ref())
                        == Some(m)
                })
        })
        && q.session.as_ref().is_none_or(|s| {
            r.session_id.as_ref() == Some(s)
                || r.attempts
                    .iter()
                    .any(|a| a.observation.session_id.as_ref() == Some(s))
        })
        && q.project
            .as_ref()
            .is_none_or(|p| r.project_id.as_ref() == Some(p))
        && q.search.as_ref().is_none_or(|s| {
            format!(
                "{} {} {} {} {} {}",
                r.id,
                r.model.as_deref().unwrap_or(""),
                r.provider,
                r.session_title.as_deref().unwrap_or(""),
                r.client
                    .as_ref()
                    .map(|c| format!("{} {} {}", c.id, c.name, c.peer))
                    .unwrap_or_default(),
                r.attempts
                    .iter()
                    .map(|a| a.observation.id.as_str())
                    .collect::<Vec<_>>()
                    .join(" ")
            )
            .to_lowercase()
            .contains(&s.to_lowercase())
        })
        && q.issue.as_ref().is_none_or(|i| {
            if i == "unmeasurable" {
                r.ttft_ms.is_none() && !["waiting", "streaming"].contains(&r.status.as_str())
            } else {
                r.attempts.iter().any(|a| a.issue.as_ref() == Some(i))
                    || sources::native_issue(r) == Some(i.as_str())
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    pub(super) fn row(id: &str) -> ModelObservation {
        serde_json::from_value(json!({"id":id,"logicalId":null,"agentId":"codex","agentProductId":"codex-cli","sessionId":null,"model":"deepseek-flash","routedModel":"deepseek-flash","provider":"测试连接","providerId":"p","endpoint":"https://api.deepseek.com/v1","bindingRevision":1,"startedAt":"2026-10-19T01:00:00Z","endedAt":"2026-10-19T01:00:01Z","status":"success","httpStatus":200,"ttftMs":100.0,"firstTextMs":150.0,"durationMs":1000.0,"usage":{"inputTokens":100,"outputTokens":20,"totalTokens":120,"cachedInputTokens":80,"cacheWriteInputTokens":0},"protocol":"responses","timingIssue":null,"revision":1})).unwrap()
    }
    fn requests(rows: Vec<ModelObservation>) -> Vec<Request> {
        logical_requests(rows, &HashMap::new())
    }
    #[test]
    fn service_fallback_groups_across_providers_but_keeps_all_usage_and_external_keys_scoped() {
        let mut a = row("policy-a");
        let mut b = row("policy-b");
        a.agent_id = "opencode".into();
        b.agent_id = a.agent_id.clone();
        a.agent_product_id = Some("opencode".into());
        b.agent_product_id = a.agent_product_id.clone();
        a.session_id = Some("opencode:ses_policy".into());
        b.session_id = a.session_id.clone();
        a.logical_id = Some("model-service-internal-key".into());
        b.logical_id = a.logical_id.clone();
        b.provider_id = Some("other-provider".into());
        b.binding_revision = 7;
        b.started_at = "2026-10-19T01:00:02Z".into();
        a.status = "error".into();
        a.http_status = Some(503);
        let route:crate::observation::RouteEvidence=serde_json::from_value(json!({"bindingId":"binding","routeVersionId":"route-a","providerRevision":1,"boundary":"new_user_turn","service":{"serviceId":"service","serviceVersionId":"version","serviceName":"Coding","modelId":"model-a","modelName":"A","requestedAlias":"local-agent-default","mode":"default","capabilities":{"contextWindow":32000,"maxOutputTokens":8192,"tools":true,"thinking":false,"inputModalities":["text"]}}})).unwrap();
        a.routing = Some(route.clone());
        b.routing = Some(route);
        let rows = requests(vec![a.clone(), b.clone()]);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].attempts.len(), 2);
        assert_eq!(rows[0].usage.total_tokens, Some(240));
        assert_eq!(rows[0].status, "recovered");
        a.routing = None;
        b.routing = None;
        assert_eq!(requests(vec![a, b]).len(), 2);
    }
    fn snapshot(q: Query, rows: Vec<Request>) -> Snapshot {
        Snapshot::new(
            q,
            rows,
            time("2026-10-19T12:00:00Z").unwrap(),
            "frozen".into(),
            None,
        )
        .unwrap()
    }
    #[test]
    fn retries_share_one_result_and_clock_but_keep_every_charge() {
        let mut a = row("a");
        let mut b = row("b");
        a.logical_id = Some("same".into());
        b.logical_id = a.logical_id.clone();
        a.status = "error".into();
        a.http_status = Some(429);
        a.ttft_ms = None;
        a.clock_id = Some("process".into());
        b.clock_id = a.clock_id.clone();
        a.start_tick_ms = Some(1000.0);
        b.start_tick_ms = Some(3000.0);
        b.started_at = "2026-10-19T01:00:02Z".into();
        let rows = requests(vec![b, a]);
        assert_eq!(rows.len(), 1);
        let r = &rows[0];
        assert_eq!(r.status, "recovered");
        assert_eq!(r.attempts.len(), 2);
        assert_eq!(r.duration_ms, Some(3000.0));
        assert_eq!(r.ttft_ms, Some(2100.0));
        assert_eq!(r.attempts[1].backoff_ms, Some(1000.0));
        assert_eq!(r.usage.total_tokens, Some(240));
        assert_eq!(r.cost.estimated[0].calls, 1);
        assert_eq!(r.cost.unknown_calls, 1);
        let s = Summary::of(&rows);
        assert_eq!(s.requests, 1);
        assert_eq!(s.attempts, 2);
        assert_eq!(s.success_rate, Some(1.0));
        assert_eq!(s.issues["rate_limit"], 1);
    }
    #[test]
    fn unlinked_records_and_different_sessions_never_merge_and_revisions_deduplicate() {
        let mut a = row("a");
        let mut b = row("b");
        assert_eq!(requests(vec![a.clone(), b.clone()]).len(), 2);
        a.logical_id = Some("key".into());
        b.logical_id = a.logical_id.clone();
        a.session_id = Some("one".into());
        b.session_id = Some("two".into());
        assert_eq!(requests(vec![a, b]).len(), 2);
        let old = row("a");
        let mut new = old.clone();
        new.revision = 2;
        new.usage.total_tokens = Some(200);
        assert_eq!(requests(vec![new, old])[0].usage.total_tokens, Some(200));
    }
    #[test]
    fn legacy_retry_timings_stay_unknown_without_a_shared_clock() {
        let mut a = row("a");
        let mut b = row("b");
        a.logical_id = Some("same".into());
        b.logical_id = a.logical_id.clone();
        a.status = "error".into();
        let r = requests(vec![a, b]).remove(0);
        assert_eq!(r.duration_ms, None);
        assert_eq!(r.ttft_ms, None);
        assert_eq!(r.throughput, None);
        assert!(r.timing_note.is_some());
        assert_eq!(r.attempts[0].observation.ttft_ms, Some(100.0));
    }
    #[test]
    fn rates_and_distributions_exclude_cancelled_unknown_and_live_but_keep_real_zero() {
        let mut a = row("a");
        a.ttft_ms = Some(0.0);
        let mut b = row("b");
        b.status = "error".into();
        let mut c = row("c");
        c.status = "cancelled".into();
        let mut d = row("d");
        d.status = "interrupted".into();
        let mut e = row("e");
        e.status = "in_progress".into();
        let s = Summary::of(&requests(vec![a, b, c, d, e]));
        assert_eq!(s.success_rate, Some(0.5));
        assert_eq!(s.ttft.samples, 1);
        assert_eq!(s.ttft.p50, Some(0.0));
        assert_eq!(s.statuses["unknown"], 1);
        assert_eq!(s.provisional, 1);
    }
    #[test]
    fn cache_ratio_is_weighted_and_missing_is_not_zero() {
        let mut a = row("a");
        a.usage.input_tokens = Some(900);
        a.usage.cached_input_tokens = Some(0);
        let mut b = row("b");
        b.usage.cached_input_tokens = Some(100);
        let mut c = row("c");
        c.usage.cached_input_tokens = None;
        let s = Summary::of(&requests(vec![a, b, c]));
        assert_eq!(s.cache_samples, 2);
        assert_eq!(s.cache_ratio, Some(0.1));
    }
    #[test]
    fn cost_prefers_actual_and_separates_unknown_attempts() {
        let mut a = row("a");
        a.endpoint = "https://openrouter.ai/api/v1".into();
        a.reported_cost = Some(provider_cost::ReportedCost {
            amount: 0.002,
            currency: "USD".into(),
            source: "openrouter.usage.cost".into(),
        });
        let mut b = row("b");
        b.endpoint = "https://unknown.test/v1".into();
        let s = Summary::of(&requests(vec![a, b]));
        assert_eq!(s.cost.actual[0].amount, 0.002);
        assert!(s.cost.estimated.is_empty());
        assert_eq!(s.cost.unknown_calls, 1);
    }
    #[test]
    fn frozen_pages_detail_aliases_and_export_agree_across_new_data() {
        let rows = requests((0..85).map(|i| row(&format!("r{i:03}"))).collect());
        let s = snapshot(Query::default(), rows);
        let first = s.page(&Query::default()).unwrap();
        assert_eq!(first.total, 85);
        assert_eq!(first.items.len(), 40);
        let q = Query {
            snapshot: Some("frozen".into()),
            cursor: first.next_cursor,
            ..Default::default()
        };
        let second = s.page(&q).unwrap();
        assert_eq!(second.items.len(), 40);
        assert!(
            first
                .items
                .iter()
                .all(|a| second.items.iter().all(|b| a.id != b.id))
        );
        assert_eq!(s.csv().lines().count(), 86);
        let bad = Query {
            result: Some("failed".into()),
            ..q
        };
        assert!(s.page(&bad).is_err());
    }
    #[test]
    fn filters_use_request_time_and_full_set_sorting_with_unknown_last() {
        let mut a = row("a");
        a.ttft_ms = None;
        let mut b = row("b");
        b.ttft_ms = Some(200.0);
        let mut c = row("c");
        c.ttft_ms = Some(10.0);
        c.started_at = "2026-10-18T01:00:00Z".into();
        let q = Query {
            start: Some("2026-10-19T00:00:00+08:00".into()),
            end: Some("2026-10-20T00:00:00+08:00".into()),
            sort: Some("ttft".into()),
            descending: Some(false),
            ..Default::default()
        };
        let s = snapshot(q.clone(), requests(vec![a, b, c]));
        assert_eq!(s.rows.len(), 2);
        assert_eq!(s.rows[0].id, "b");
        assert_eq!(s.rows[1].id, "a");
        let p = s.page(&q).unwrap();
        assert_eq!(p.summary.requests, 2);
        assert_eq!(
            p.buckets
                .iter()
                .map(|b| b.value.unwrap_or(0.0))
                .sum::<f64>(),
            2.0
        );
    }
    #[test]
    fn consumption_groups_attribute_each_attempt_without_duplicate_request_counts() {
        let mut a = row("a");
        let mut b = row("b");
        a.logical_id = Some("key".into());
        b.logical_id = a.logical_id.clone();
        a.status = "error".into();
        b.routed_model = Some("other-model".into());
        let s = snapshot(
            Query {
                metric: Some("tokens".into()),
                dimension: Some("model".into()),
                ..Default::default()
            },
            requests(vec![a, b]),
        );
        assert_eq!(s.rows.len(), 1);
        assert_eq!(s.dashboard.groups.len(), 2);
        assert_eq!(
            s.dashboard
                .groups
                .iter()
                .map(|g| g.value.unwrap())
                .sum::<f64>(),
            240.0
        );
    }
    #[test]
    fn csv_escapes_formula_text_quotes_unicode_and_never_exports_endpoint_or_credentials() {
        let mut a = row("a");
        a.provider = "  =HYPERLINK(\"evil\")\n中文".into();
        a.endpoint = "https://user:password@api.deepseek.com/v1?secret=value".into();
        let s = snapshot(Query::default(), requests(vec![a]));
        let csv = s.csv();
        assert!(csv.starts_with('\u{feff}'));
        assert!(csv.contains("'  =HYPERLINK(\"\"evil\"\")"));
        assert!(csv.contains("+08:00"));
        assert!(!csv.contains("password"));
        assert!(
            !s.rows[0].attempts[0]
                .observation
                .endpoint
                .contains("secret")
        );
    }
    #[test]
    fn invalid_filters_and_empty_export_have_real_outcomes() {
        assert!(
            Query {
                start: Some("not a date".into()),
                ..Default::default()
            }
            .validate()
            .is_err()
        );
        assert!(
            Query {
                sort: Some("untrusted".into()),
                ..Default::default()
            }
            .validate()
            .is_err()
        );
        let s = snapshot(Query::default(), vec![]);
        assert_eq!(s.csv().lines().count(), 1);
        assert_eq!(s.dashboard.summary.requests, 0);
        assert_eq!(s.dashboard.summary.success_rate, None);
    }
}
