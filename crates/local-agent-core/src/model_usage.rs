//! Model consumption from mutually exclusive observation sources, without billing inference.
use crate::sessions::Usage;
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashSet};

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UsageQuery {
    pub agent: String,
    pub source: String,
    pub days: Option<u16>,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub cursor: Option<String>,
}
impl UsageQuery {
    pub fn validate(&self) -> Result<(), &'static str> {
        self.validate_with_directory(crate::agents::builtin_directory())
    }
    pub fn validate_with_directory(
        &self,
        directory: &crate::agents::AgentDirectory,
    ) -> Result<(), &'static str> {
        if !directory.valid_filter(&self.agent)
            || !["native", "gateway"].contains(&self.source.as_str())
            || self.days.is_some_and(|days| ![1, 7, 30].contains(&days))
            || self
                .provider
                .as_ref()
                .is_some_and(|id| id.len() != 32 || !id.bytes().all(|b| b.is_ascii_hexdigit()))
            || self
                .model
                .as_ref()
                .is_some_and(|model| model.chars().count() > 200)
            || self
                .cursor
                .as_ref()
                .is_some_and(|cursor| cursor.len() > 100)
        {
            return Err("模型用量筛选无效");
        }
        Ok(())
    }
}
pub struct UsageRecord {
    pub cost: crate::provider_cost::Cost,
    pub id: String,
    pub model: Option<String>,
    pub provider: Option<String>,
    pub endpoint: Option<String>,
    pub route_revision: Option<u64>,
    pub time: Option<String>,
    pub usage: Usage,
    pub session_id: Option<String>,
    pub request_id: Option<String>,
}
#[derive(Clone, Default, Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Reported {
    input_tokens: usize,
    output_tokens: usize,
    cached_input_tokens: usize,
    cache_write_input_tokens: usize,
    reasoning_output_tokens: usize,
    total_tokens: usize,
}
#[derive(Clone, Default, Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UsageTotals {
    cost: crate::provider_cost::Totals,
    attempts: usize,
    missing_usage: usize,
    usage: Usage,
    reported: Reported,
}
impl UsageTotals {
    pub(crate) fn add(&mut self, usage: &Usage, cost: &crate::provider_cost::Cost) {
        self.cost.add(cost);
        self.attempts += 1;
        self.missing_usage += usize::from(usage.total_tokens.is_none());
        self.usage.add(usage);
        self.reported.input_tokens += usize::from(usage.input_tokens.is_some());
        self.reported.output_tokens += usize::from(usage.output_tokens.is_some());
        self.reported.cached_input_tokens += usize::from(usage.cached_input_tokens.is_some());
        self.reported.cache_write_input_tokens +=
            usize::from(usage.cache_write_input_tokens.is_some());
        self.reported.reasoning_output_tokens +=
            usize::from(usage.reasoning_output_tokens.is_some());
        self.reported.total_tokens += usize::from(usage.total_tokens.is_some());
    }
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageProvider {
    id: String,
    name: Option<String>,
    endpoint: Option<String>,
    route_revision: Option<u64>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelUsage {
    provider_id: String,
    provider: Option<String>,
    endpoint: Option<String>,
    route_revision: Option<u64>,
    model: Option<String>,
    totals: UsageTotals,
    last_seen: Option<String>,
    session_id: Option<String>,
    request_id: Option<String>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageSnapshot {
    pub query: UsageQuery,
    as_of: String,
    source_as_of: String,
    source_state: String,
    summary: UsageTotals,
    providers: Vec<UsageProvider>,
    items: Vec<ModelUsage>,
    total: usize,
    next_cursor: Option<String>,
    missing_time: usize,
}
fn hash(value: &[u8]) -> String {
    format!("{:x}", Sha256::digest(value))
}

/// The recorded tuple defines a model-history connection, independently of the
/// current Provider catalog. Keep every consumer on the same identity format.
pub(crate) fn provider_identity(
    provider: Option<&str>,
    endpoint: Option<&str>,
    revision: Option<u64>,
) -> String {
    hash(&serde_json::to_vec(&(provider, endpoint, revision)).unwrap_or_default())[..32].to_owned()
}

pub fn aggregate(
    query: UsageQuery,
    rows: Vec<UsageRecord>,
    source_as_of: String,
    source_state: String,
    now: DateTime<Utc>,
) -> Result<UsageSnapshot, &'static str> {
    query.validate()?;
    let start = query.days.map(|days| now - Duration::days(days.into()));
    let mut groups: BTreeMap<(String, Option<String>), ModelUsage> = BTreeMap::new();
    let mut providers = BTreeMap::new();
    let mut summary = UsageTotals::default();
    let mut missing_time = 0;
    let mut seen = HashSet::new();
    for row in rows {
        if !seen.insert(row.id) {
            continue;
        }
        let time = row
            .time
            .as_deref()
            .and_then(|time| DateTime::parse_from_rfc3339(time).ok())
            .map(|time| time.with_timezone(&Utc));
        let in_range = time.is_none_or(|time| time < now)
            && start.is_none_or(|start| time.is_some_and(|time| time >= start));
        let id = provider_identity(
            row.provider.as_deref(),
            row.endpoint.as_deref(),
            row.route_revision,
        );
        if in_range {
            providers
                .entry(id.clone())
                .or_insert_with(|| UsageProvider {
                    id: id.clone(),
                    name: row.provider.clone(),
                    endpoint: row.endpoint.clone(),
                    route_revision: row.route_revision,
                });
        }
        if query
            .provider
            .as_ref()
            .is_some_and(|provider| provider != &id)
            || query.model.as_ref().is_some_and(|model| {
                !row.model
                    .as_deref()
                    .unwrap_or("")
                    .to_lowercase()
                    .contains(&model.to_lowercase())
            })
        {
            continue;
        }
        if time.is_none() {
            missing_time += 1;
        }
        if !in_range {
            continue;
        }
        summary.add(&row.usage, &row.cost);
        let group = groups
            .entry((id.clone(), row.model.clone()))
            .or_insert_with(|| ModelUsage {
                provider_id: id,
                provider: row.provider,
                endpoint: row.endpoint,
                route_revision: row.route_revision,
                model: row.model,
                totals: UsageTotals::default(),
                last_seen: None,
                session_id: None,
                request_id: None,
            });
        group.totals.add(&row.usage, &row.cost);
        if group.last_seen.is_none() || time.as_ref().map(DateTime::to_rfc3339) > group.last_seen {
            group.last_seen = time.map(|time| time.to_rfc3339());
            group.session_id = row.session_id;
            group.request_id = row.request_id;
        }
    }
    let mut items: Vec<_> = groups.into_values().collect();
    items.sort_by(|a, b| {
        b.totals
            .usage
            .total_tokens
            .cmp(&a.totals.usage.total_tokens)
            .then(a.provider_id.cmp(&b.provider_id))
            .then(a.model.cmp(&b.model))
    });
    let total = items.len();
    let revision = hash(
        &serde_json::to_vec(&(
            &items,
            &source_state,
            &query.agent,
            &query.source,
            query.days,
            &query.provider,
            &query.model,
        ))
        .unwrap_or_default(),
    );
    let offset = match query.cursor.as_deref() {
        None => 0,
        Some(cursor) => {
            let (version, offset) = cursor.split_once(':').ok_or("模型用量分页无效")?;
            if version != revision {
                return Err("模型用量已更新，请从第一页重新读取");
            }
            let offset: usize = offset.parse().map_err(|_| "模型用量分页无效")?;
            if offset > total || !offset.is_multiple_of(20) {
                return Err("模型用量分页无效");
            }
            offset
        }
    };
    Ok(UsageSnapshot {
        query,
        as_of: now.to_rfc3339(),
        source_as_of,
        source_state,
        summary,
        providers: providers.into_values().collect(),
        items: items.into_iter().skip(offset).take(20).collect(),
        total,
        next_cursor: (offset + 20 < total).then(|| format!("{revision}:{}", offset + 20)),
        missing_time,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn query() -> UsageQuery {
        UsageQuery {
            agent: "codex".into(),
            source: "native".into(),
            days: None,
            provider: None,
            model: None,
            cursor: None,
        }
    }
    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-10-19T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }
    fn row(id: &str, model: &str, endpoint: &str) -> UsageRecord {
        UsageRecord {
            cost: crate::provider_cost::Cost::unknown("native_price_unconfirmed"),
            id: id.into(),
            model: Some(model.into()),
            provider: Some("openai".into()),
            endpoint: Some(endpoint.into()),
            route_revision: None,
            time: Some("2026-10-19T10:00:00Z".into()),
            usage: Usage {
                input_tokens: Some(10),
                output_tokens: Some(5),
                cached_input_tokens: Some(4),
                cache_write_input_tokens: None,
                reasoning_output_tokens: Some(2),
                total_tokens: Some(15),
            },
            session_id: Some("codex:test".into()),
            request_id: None,
        }
    }
    fn snapshot(query: UsageQuery, rows: Vec<UsageRecord>) -> UsageSnapshot {
        aggregate(query, rows, now().to_rfc3339(), "ready".into(), now()).unwrap()
    }
    #[test]
    fn grouping_preserves_provider_endpoints_unknowns_zero_subsets_and_deduplicates_ids() {
        let mut missing = row("missing", "alpha", "https://one.example/v1");
        missing.usage = Usage {
            input_tokens: Some(3),
            ..Usage::default()
        };
        let mut zero = row("zero", "beta", "https://one.example/v1");
        zero.usage = Usage {
            input_tokens: Some(0),
            output_tokens: Some(0),
            total_tokens: Some(0),
            ..Usage::default()
        };
        let result = snapshot(
            query(),
            vec![
                row("a", "alpha", "https://one.example/v1"),
                row("a", "alpha", "https://one.example/v1"),
                missing,
                zero,
                row("b", "alpha", "https://two.example/v1"),
            ],
        );
        assert_eq!(result.summary.attempts, 4);
        assert_eq!(result.summary.usage.total_tokens, Some(30));
        assert_eq!(result.summary.usage.input_tokens, Some(23));
        assert_eq!(result.summary.usage.cached_input_tokens, Some(8));
        assert_eq!(result.summary.missing_usage, 1);
        assert_eq!(result.summary.reported.total_tokens, 3);
        assert_eq!(result.summary.reported.cache_write_input_tokens, 0);
        assert_eq!(result.items.len(), 3);
        assert_eq!(result.providers.len(), 2);
        let encoded = serde_json::to_value(&result).unwrap();
        assert!(encoded.get("subscription").is_none());
        let mut filtered = query();
        filtered.provider = Some(result.providers[0].id.clone());
        filtered.model = Some("ALP".into());
        let selected = snapshot(
            filtered,
            vec![
                row("a", "alpha", "https://one.example/v1"),
                row("b", "alpha", "https://two.example/v1"),
                row("c", "beta", "https://one.example/v1"),
            ],
        );
        assert_eq!(selected.items.len(), 1);
        assert_eq!(selected.summary.attempts, 1);
    }
    #[test]
    fn same_endpoint_with_different_route_snapshots_does_not_merge_credentials() {
        let mut a = row("a", "m", "https://gateway.example/v1");
        a.route_revision = Some(1);
        let mut b = row("b", "m", "https://gateway.example/v1");
        b.route_revision = Some(2);
        let result = snapshot(query(), vec![a, b]);
        assert_eq!(result.items.len(), 2);
        assert_eq!(result.providers.len(), 2);
        assert_eq!(result.summary.usage.total_tokens, Some(30));
    }
    #[test]
    fn date_filters_use_call_time_and_keep_unknown_times_explicit() {
        let make_rows = || {
            let mut rows = vec![
                row("boundary", "m", "e"),
                row("old", "m", "e"),
                row("end", "m", "e"),
                row("unknown", "m", "e"),
            ];
            rows[0].time = Some((now() - Duration::days(1)).to_rfc3339());
            rows[1].time =
                Some((now() - Duration::days(1) - Duration::milliseconds(1)).to_rfc3339());
            rows[2].time = Some(now().to_rfc3339());
            rows[3].time = None;
            rows
        };
        let all = snapshot(query(), make_rows());
        assert_eq!(all.summary.attempts, 3);
        assert_eq!(all.missing_time, 1);
        let mut filtered = query();
        filtered.days = Some(1);
        let recent = snapshot(filtered, make_rows());
        assert_eq!(recent.summary.attempts, 1);
        assert_eq!(recent.missing_time, 1);
    }
    #[test]
    fn pagination_summarizes_all_matching_groups_and_rejects_changed_snapshots() {
        let rows = || {
            (0..25)
                .map(|i| row(&i.to_string(), &format!("model-{i}"), "endpoint"))
                .collect::<Vec<_>>()
        };
        let first = snapshot(query(), rows());
        assert_eq!(first.summary.attempts, 25);
        assert_eq!(first.summary.cost.unknown_calls, 25);
        assert_eq!(first.items.len(), 20);
        let mut next = query();
        next.cursor = first.next_cursor;
        let second = snapshot(next.clone(), rows());
        assert_eq!(second.items.len(), 5);
        assert_eq!(second.summary.usage.total_tokens, Some(375));
        assert_eq!(second.summary.cost.unknown_calls, 25);
        assert!(second.next_cursor.is_none());
        let mut changed = rows();
        changed.push(row("new", "new-model", "endpoint"));
        assert!(aggregate(next.clone(), changed, "time".into(), "ready".into(), now()).is_err());
        next.source = "gateway".into();
        assert!(aggregate(next, rows(), "time".into(), "ready".into(), now()).is_err());
        let empty = snapshot(query(), vec![]);
        assert!(empty.summary.usage.total_tokens.is_none());
        assert_eq!(empty.summary.attempts, 0);
    }
}
