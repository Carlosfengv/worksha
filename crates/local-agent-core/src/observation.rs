//! Metadata-only request observations. Bodies and credentials never enter this model.
use crate::sessions::Usage;
use chrono::{DateTime, Utc};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::Path};
pub mod content_epoch;
pub mod retention;
mod source_file;
pub mod sql;
mod write;
pub use write::ObservationWriteError;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GatewayClient {
    pub id: String,
    pub name: String,
    pub peer: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityMessage {
    pub id: String,
    pub started_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RouteEvidence {
    pub binding_id: String,
    pub route_version_id: String,
    pub provider_revision: u64,
    pub boundary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service: Option<crate::model_services::ServiceSelection>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelObservation {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_rate_sample: Option<crate::sessions::activity_performance::RateSample>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub routing: Option<RouteEvidence>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activity_message: Option<ActivityMessage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client: Option<GatewayClient>,
    #[serde(default)]
    pub response_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_response_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream_response_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub downstream_status: Option<String>,
    #[serde(default)]
    pub sent_ms: Option<f64>,
    #[serde(default)]
    pub upstream_request_id: Option<String>,
    #[serde(default)]
    pub requested_model: Option<String>,
    #[serde(default)]
    pub routed_model: Option<String>,
    #[serde(default)]
    pub reported_model: Option<String>,
    #[serde(default)]
    pub first_output_kind: Option<String>,
    #[serde(default)]
    pub clock_id: Option<String>,
    #[serde(default)]
    pub start_tick_ms: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reported_cost: Option<crate::provider_cost::ReportedCost>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    #[serde(default)]
    pub agent_product_id: Option<String>,
    pub id: String,
    pub logical_id: Option<String>,
    pub agent_id: String,
    pub session_id: Option<String>,
    pub model: Option<String>,
    pub provider: String,
    pub endpoint: String,
    pub binding_revision: u64,
    pub started_at: String,
    pub ended_at: Option<String>,
    pub status: String,
    pub http_status: Option<u16>,
    pub ttft_ms: Option<f64>,
    pub first_text_ms: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_text_ms: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimated_text_tokens: Option<f64>,
    pub duration_ms: Option<f64>,
    pub usage: Usage,
    pub protocol: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_protocol: Option<String>,
    pub timing_issue: Option<String>,
    pub revision: u64,
}
impl ModelObservation {
    /// Legacy responseId named the upstream. It is also the client ID only on
    /// a pass-through protocol; a historical converted ID cannot be invented.
    pub fn native_response_id(&self) -> Option<&str> {
        self.client_response_id.as_deref().or_else(|| {
            self.client_protocol
                .as_deref()
                .is_none_or(|p| p == self.protocol)
                .then_some(self.response_id.as_deref())
                .flatten()
        })
    }
    pub fn average_text_speed(&self) -> Option<f64> {
        if self.status != "success" || self.timing_issue.is_some() {
            return None;
        }
        let duration = self.last_text_ms? - self.first_text_ms?;
        let tokens = self.estimated_text_tokens?;
        (duration >= 100.0 && tokens > 0.0 && duration.is_finite() && tokens.is_finite())
            .then_some(tokens * 1000.0 / duration)
    }
    pub fn throughput(&self) -> Option<f64> {
        self.usage
            .output_tokens
            .zip(self.duration_ms)
            .filter(|(_, ms)| *ms > 0.0 && self.status == "success")
            .map(|(tokens, ms)| tokens as f64 * 1000.0 / ms)
    }
}

pub struct ObservationStore(
    Connection,
    content_epoch::ContentEpoch,
    source_file::SourceFile,
);
pub struct ObservationRead {
    pub rows: Vec<ModelObservation>,
    pub invalid_records: usize,
}
impl ObservationStore {
    pub fn data_version(&self) -> Result<i64, String> {
        self.0
            .query_row("PRAGMA data_version", [], |row| row.get(0))
            .map_err(|_| "无法确认请求历史版本".into())
    }
    pub fn load_all(&self) -> Result<Vec<ModelObservation>, String> {
        Ok(self.read(None)?.rows)
    }
    pub fn latest_by_agent(&self) -> Result<Vec<ModelObservation>, String> {
        let mut statement = self.0.prepare("SELECT value FROM (SELECT value, ROW_NUMBER() OVER (PARTITION BY json_extract(value,'$.agentId'),json_extract(value,'$.agentProductId') ORDER BY started_at DESC,id DESC) AS position FROM model_observations WHERE json_valid(value)) WHERE position=1")
            .map_err(|_| "无法读取各 Agent 最近观测")?;
        let raw = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|_| "无法读取各 Agent 最近观测")?;
        let mut rows = vec![];
        for value in raw {
            if let Ok(row) = serde_json::from_str(&value.map_err(|_| "最近观测读取中断")?) {
                rows.push(row);
            }
        }
        Ok(rows)
    }
    pub fn directory(&self) -> std::path::PathBuf {
        Path::new(self.0.path().expect("observation store is file-backed"))
            .parent()
            .expect("database has a parent")
            .to_path_buf()
    }
    /// Invalid records remain untouched in SQLite and are counted explicitly.
    pub fn read(&self, limit: Option<usize>) -> Result<ObservationRead, String> {
        let mut stmt = self
            .0
            .prepare(
                "SELECT value FROM model_observations ORDER BY started_at DESC,id DESC LIMIT ?1",
            )
            .map_err(|_| "无法读取请求历史")?;
        let mut cursor = stmt
            .query([limit.map_or(-1, |n| n as i64)])
            .map_err(|_| "无法读取请求历史")?;
        let mut result = ObservationRead {
            rows: vec![],
            invalid_records: 0,
        };
        while let Some(row) = cursor.next().map_err(|_| "请求历史读取中断")? {
            match row
                .get::<_, String>(0)
                .ok()
                .and_then(|v| serde_json::from_str(&v).ok())
            {
                Some(value) => result.rows.push(value),
                None => result.invalid_records += 1,
            }
        }
        Ok(result)
    }
    pub fn open(directory: &Path) -> Result<Self, String> {
        std::fs::create_dir_all(directory).map_err(|_| "无法创建观测数据目录")?;
        let path = directory
            .canonicalize()
            .map_err(|_| "无法确认观测数据目录")?
            .join("observations.sqlite3");
        let source = source_file::SourceFile::open(path).map_err(|_| "观测数据库来源不可确认")?;
        let db = Connection::open(source.path()).map_err(|_| "无法打开观测数据库")?;
        source.check().map_err(|_| "观测数据库来源已变化")?;
        db.busy_timeout(std::time::Duration::from_secs(3))
            .map_err(|_| "观测数据库不可用")?;
        content_epoch::initialize(&db)
            .map_err(|_| "观测存储格式或内容版本不可确认，原记录已保留")?;
        let epoch = content_epoch::read(&db).map_err(|_| "观测内容版本不可确认")?;
        source.check().map_err(|_| "观测数据库来源已变化")?;
        Ok(Self(db, epoch, source))
    }
    pub fn load(&self) -> Result<Vec<ModelObservation>, String> {
        Ok(self.read(Some(100001))?.rows)
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OverviewQuery {
    pub agent: Option<String>,
    pub range: u16,
    pub metric: String,
    pub dimension: String,
    pub source: String,
}
impl OverviewQuery {
    pub fn validate(&self) -> Result<(), String> {
        self.validate_with_directory(crate::agents::builtin_directory())
    }
    pub fn validate_with_directory(
        &self,
        directory: &crate::agents::AgentDirectory,
    ) -> Result<(), String> {
        if self
            .agent
            .as_deref()
            .is_some_and(|a| !directory.valid_filter(a))
            || ![6, 12, 24, 168].contains(&self.range)
            || !["calls", "tokens", "cost"].contains(&self.metric.as_str())
            || !["agent", "model"].contains(&self.dimension.as_str())
            || !["native", "gateway"].contains(&self.source.as_str())
        {
            return Err("调用分析筛选无效".into());
        }
        Ok(())
    }
}
/// USD components retain sub-microdollar precision until display. A logical
/// request can include known billed attempts alongside an unknown retry.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalysisCost {
    pub actual_usd: Option<f64>,
    pub estimated_usd: Option<f64>,
    pub unknown_attempts: usize,
}
impl AnalysisCost {
    pub fn add(&mut self, cost: &crate::provider_cost::Cost) {
        if cost.currency != Some("USD") || cost.amount.is_none() {
            self.unknown_attempts += 1;
        } else if cost.kind == "actual" {
            self.actual_usd = Some(self.actual_usd.unwrap_or(0.0) + cost.amount.unwrap());
        } else {
            self.estimated_usd = Some(self.estimated_usd.unwrap_or(0.0) + cost.amount.unwrap());
        }
    }
    fn merge(&mut self, other: &Self) {
        if let Some(v) = other.actual_usd {
            self.actual_usd = Some(self.actual_usd.unwrap_or(0.0) + v);
        }
        if let Some(v) = other.estimated_usd {
            self.estimated_usd = Some(self.estimated_usd.unwrap_or(0.0) + v);
        }
        self.unknown_attempts += other.unknown_attempts;
    }
    fn micro_usd(&self) -> Option<f64> {
        (self.actual_usd.is_some() || self.estimated_usd.is_some())
            .then(|| (self.actual_usd.unwrap_or(0.0) + self.estimated_usd.unwrap_or(0.0)) * 1e6)
    }
}
#[derive(Debug, Clone)]
pub struct AnalysisRow {
    pub cost: AnalysisCost,
    pub agent: String,
    pub model: Option<String>,
    pub time: Option<String>,
    pub tokens: Option<u64>,
    pub provisional: bool,
    pub partial: bool,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalysisValue {
    pub id: String,
    pub value: Option<f64>,
    pub missing: usize,
    pub samples: usize,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalysisBucket {
    pub start: String,
    pub end: String,
    pub groups: Vec<AnalysisValue>,
    pub value: Option<f64>,
    pub missing: usize,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Analysis {
    pub cost: AnalysisCost,
    pub as_of: String,
    pub window_start: String,
    pub query: OverviewQuery,
    pub total: Option<f64>,
    pub samples: usize,
    pub reported: usize,
    pub missing: usize,
    pub missing_time: usize,
    pub provisional: usize,
    pub agents: Vec<AnalysisValue>,
    pub models: Vec<AnalysisValue>,
    pub buckets: Vec<AnalysisBucket>,
    pub coverage: &'static str,
    pub unit: &'static str,
}
#[derive(Default, Clone)]
struct Sum {
    value: f64,
    known: usize,
    missing: usize,
    samples: usize,
    reported: usize,
}
impl Sum {
    fn add(&mut self, value: Option<f64>, partial: bool) {
        self.samples += 1;
        if let Some(v) = value {
            self.value += v;
            self.known += 1;
            if partial {
                self.missing += 1;
            } else {
                self.reported += 1;
            }
        } else {
            self.missing += 1;
        }
    }
    fn value(&self) -> Option<f64> {
        (self.known > 0 || self.missing == 0).then_some(self.value)
    }
    fn finish(&self, id: String) -> AnalysisValue {
        AnalysisValue {
            id,
            value: self.value(),
            missing: self.missing,
            samples: self.samples,
        }
    }
}

pub fn analyze(
    query: OverviewQuery,
    rows: &[AnalysisRow],
    as_of: DateTime<Utc>,
) -> Result<Analysis, String> {
    query.validate()?;
    let seconds = match query.range {
        6 | 12 => 1800,
        24 => 3600,
        _ => 21600,
    };
    let end = as_of.timestamp_millis();
    let start = end - i64::from(query.range) * 3600 * 1000;
    let n = (i64::from(query.range) * 3600 / seconds) as usize;
    let mut buckets = vec![BTreeMap::<String, Sum>::new(); n];
    let mut totals = Sum::default();
    let mut cost = AnalysisCost::default();
    let mut agents = BTreeMap::<String, Sum>::new();
    let mut models = BTreeMap::<String, Sum>::new();
    let mut missing_time = 0;
    let mut provisional = 0;
    for row in rows {
        if !crate::agents::matches_group(query.agent.as_deref(), &row.agent) {
            continue;
        }
        let Some(time) = row
            .time
            .as_ref()
            .and_then(|t| DateTime::parse_from_rfc3339(t).ok())
            .map(|t| t.timestamp_millis())
        else {
            missing_time += 1;
            continue;
        };
        if time < start || time >= end {
            continue;
        }
        let value = match query.metric.as_str() {
            "calls" => Some(1.0),
            "tokens" => row.tokens.map(|n| n as f64),
            _ => row.cost.micro_usd(),
        };
        let model = row.model.clone().unwrap_or_else(|| "模型未知".into());
        let group = if query.dimension == "agent" {
            &row.agent
        } else {
            &model
        };
        let partial = (query.metric == "tokens" && row.partial)
            || (query.metric == "cost" && row.cost.unknown_attempts > 0);
        cost.merge(&row.cost);
        buckets[((time - start) / (seconds * 1000)) as usize]
            .entry(group.clone())
            .or_default()
            .add(value, partial);
        agents
            .entry(row.agent.clone())
            .or_default()
            .add(value, partial);
        models.entry(model).or_default().add(value, partial);
        totals.add(value, partial);
        provisional += usize::from(row.provisional);
    }
    let groups: Vec<_> = if query.dimension == "agent" {
        agents.keys().cloned().collect()
    } else {
        models.keys().cloned().collect()
    };
    let iso = |t| DateTime::from_timestamp_millis(t).unwrap().to_rfc3339();
    let buckets = buckets
        .into_iter()
        .enumerate()
        .map(|(i, b)| {
            let mut sum = Sum::default();
            let groups = groups
                .iter()
                .map(|id| {
                    let value = b.get(id).cloned().unwrap_or_default();
                    sum.value += value.value;
                    sum.known += value.known;
                    sum.missing += value.missing;
                    sum.samples += value.samples;
                    sum.reported += value.reported;
                    value.finish(id.clone())
                })
                .collect();
            AnalysisBucket {
                start: iso(start + i as i64 * seconds * 1000),
                end: iso(start + (i + 1) as i64 * seconds * 1000),
                groups,
                value: sum.value(),
                missing: sum.missing,
            }
        })
        .collect();
    Ok(Analysis {
        cost,
        as_of: as_of.to_rfc3339(),
        window_start: iso(start),
        unit: if query.metric == "cost" {
            "USD"
        } else if query.metric == "tokens" {
            "Token"
        } else {
            "次"
        },
        query,
        total: totals.value(),
        samples: totals.samples,
        reported: totals.reported,
        missing: totals.missing,
        missing_time,
        provisional,
        agents: agents.into_iter().map(|(id, sum)| sum.finish(id)).collect(),
        models: models.into_iter().map(|(id, sum)| sum.finish(id)).collect(),
        buckets,
        coverage: "partial",
    })
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TimingStats {
    pub success: usize,
    pub ttft_samples: usize,
    pub ttft_p50_ms: Option<f64>,
    pub ttft_p95_ms: Option<f64>,
    pub throughput_samples: usize,
    pub throughput_p50: Option<f64>,
    pub retry_groups_unmeasurable: usize,
}
pub fn timing_stats(
    rows: &[ModelObservation],
    query: &OverviewQuery,
    as_of: DateTime<Utc>,
) -> TimingStats {
    let start = as_of - chrono::Duration::hours(query.range.into());
    let mut attempts = BTreeMap::<String, (String, usize)>::new();
    for r in rows {
        let group = crate::agents::group(&r.agent_id, r.agent_product_id.as_deref());
        attempts
            .entry(crate::requests::logical_request_key(r))
            .or_insert((group, 0))
            .1 += 1;
    }
    let rows: Vec<_> = rows
        .iter()
        .filter(|r| {
            r.status == "success"
                && attempts
                    .get(&crate::requests::logical_request_key(r))
                    .is_some_and(|(_, n)| *n == 1)
                && crate::agents::matches(
                    query.agent.as_deref(),
                    &r.agent_id,
                    r.agent_product_id.as_deref(),
                )
                && DateTime::parse_from_rfc3339(&r.started_at)
                    .is_ok_and(|t| t >= start && t < as_of)
        })
        .collect();
    let mut ttft: Vec<_> = rows
        .iter()
        .filter_map(|r| r.ttft_ms)
        .filter(|v| v.is_finite() && *v >= 0.0)
        .collect();
    let mut speed: Vec<_> = rows.iter().filter_map(|r| r.throughput()).collect();
    fn quantile(v: &mut [f64], p: f64) -> Option<f64> {
        v.sort_by(f64::total_cmp);
        (!v.is_empty()).then(|| {
            v[((v.len() as f64 * p).ceil() as usize)
                .saturating_sub(1)
                .min(v.len() - 1)]
        })
    }
    TimingStats {
        retry_groups_unmeasurable: attempts
            .into_iter()
            .filter(|(_, (agent, count))| {
                *count > 1 && crate::agents::matches_group(query.agent.as_deref(), agent)
            })
            .count(),
        success: rows.len(),
        ttft_samples: ttft.len(),
        ttft_p50_ms: quantile(&mut ttft, 0.5),
        ttft_p95_ms: quantile(&mut ttft, 0.95),
        throughput_samples: speed.len(),
        throughput_p50: quantile(&mut speed, 0.5),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn product_filters_preserve_unknown_history_and_separate_retry_identity() {
        let now = Utc::now();
        let mut cli = request(now);
        cli.agent_product_id = Some("claude-code-cli".into());
        cli.logical_id = Some("same-id".into());
        let mut desktop = cli.clone();
        desktop.id = "desktop".into();
        desktop.agent_product_id = Some("claude-code-desktop".into());
        let mut unknown = cli.clone();
        unknown.id = "legacy".into();
        unknown.agent_product_id = None;
        let rows = [cli, desktop, unknown];
        let mut query = OverviewQuery {
            agent: Some("claude-code-cli".into()),
            range: 24,
            metric: "calls".into(),
            dimension: "agent".into(),
            source: "gateway".into(),
        };
        assert_eq!(timing_stats(&rows, &query, now).success, 1);
        query.agent = Some("unknown".into());
        assert_eq!(timing_stats(&rows, &query, now).success, 1);
        query.agent = None;
        assert_eq!(timing_stats(&rows, &query, now).success, 3);
        let analysis_rows: Vec<_> = rows
            .iter()
            .map(|r| AnalysisRow {
                cost: AnalysisCost {
                    unknown_attempts: 1,
                    ..Default::default()
                },
                agent: crate::agents::group(&r.agent_id, r.agent_product_id.as_deref()),
                model: r.model.clone(),
                time: Some(r.started_at.clone()),
                tokens: Some(10),
                provisional: false,
                partial: false,
            })
            .collect();
        let all = analyze(query.clone(), &analysis_rows, now).unwrap();
        assert_eq!(all.samples, 3);
        assert_eq!(all.agents.len(), 3);
        query.agent = Some("unknown".into());
        assert_eq!(analyze(query, &analysis_rows, now).unwrap().samples, 1);
    }
    pub(super) fn request(now: DateTime<Utc>) -> ModelObservation {
        ModelObservation {
            last_rate_sample: None,
            routing: None,
            activity_message: None,
            client: None,
            sent_ms: None,
            upstream_request_id: None,
            response_id: None,
            client_response_id: None,
            upstream_response_id: None,
            stop_reason: None,
            downstream_status: None,
            requested_model: None,
            routed_model: None,
            reported_model: None,
            first_output_kind: None,
            clock_id: None,
            start_tick_ms: None,
            reported_cost: None,
            provider_id: None,
            agent_product_id: None,
            id: "r1".into(),
            logical_id: None,
            agent_id: "claude".into(),
            session_id: None,
            model: Some("model".into()),
            provider: "provider".into(),
            endpoint: "https://example.com".into(),
            binding_revision: 1,
            started_at: (now - chrono::Duration::seconds(5)).to_rfc3339(),
            ended_at: Some(now.to_rfc3339()),
            status: "success".into(),
            http_status: Some(200),
            ttft_ms: Some(100.0),
            first_text_ms: Some(120.0),
            last_text_ms: None,
            estimated_text_tokens: None,
            duration_ms: Some(1000.0),
            usage: Usage {
                input_tokens: Some(10),
                output_tokens: Some(10),
                total_tokens: Some(20),
                ..Default::default()
            },
            protocol: "messages".into(),
            timing_issue: None,
            client_protocol: None,
            revision: 1,
        }
    }
    #[test]
    fn completed_text_speed_survives_storage_without_guessing_legacy_or_invalid_timings() {
        let dir = tempfile::tempdir().unwrap();
        let store = ObservationStore::open(dir.path()).unwrap();
        let mut row = request(Utc::now());
        assert_eq!(row.average_text_speed(), None);
        row.last_text_ms = Some(1120.0);
        row.estimated_text_tokens = Some(15.0);
        assert_eq!(row.average_text_speed(), Some(15.0));
        store.save(&row).unwrap();
        let saved = store.load().unwrap().remove(0);
        assert_eq!(saved.average_text_speed(), Some(15.0));
        row.status = "in_progress".into();
        assert_eq!(row.average_text_speed(), None);
        row.status = "success".into();
        row.last_text_ms = row.first_text_ms;
        assert_eq!(row.average_text_speed(), None);
        row.last_text_ms = Some(121.0);
        assert_eq!(row.average_text_speed(), None);
        row.last_text_ms = Some(1120.0);
        row.timing_issue = Some("incomplete parser".into());
        assert_eq!(row.average_text_speed(), None);
        let mut legacy = serde_json::to_value(request(Utc::now())).unwrap();
        legacy.as_object_mut().unwrap().remove("lastTextMs");
        legacy
            .as_object_mut()
            .unwrap()
            .remove("estimatedTextTokens");
        assert_eq!(
            serde_json::from_value::<ModelObservation>(legacy)
                .unwrap()
                .average_text_speed(),
            None
        );
    }
    #[test]
    fn observations_do_not_regress_and_success_distributions_exclude_invalid_samples() {
        let dir = tempfile::tempdir().unwrap();
        let store = ObservationStore::open(dir.path()).unwrap();
        let now = Utc::now();
        let valid = request(now);
        store.save(&valid).unwrap();
        let mut old = valid.clone();
        old.revision = 0;
        old.status = "in_progress".into();
        store.save(&old).unwrap();
        assert_eq!(store.load().unwrap()[0].status, "success");
        let mut failed = valid.clone();
        failed.id = "failed".into();
        failed.status = "error".into();
        failed.ttft_ms = Some(9999.0);
        let mut cancelled = failed.clone();
        cancelled.status = "cancelled".into();
        let mut running = failed.clone();
        running.status = "in_progress".into();
        let q = OverviewQuery {
            agent: None,
            range: 24,
            metric: "calls".into(),
            dimension: "agent".into(),
            source: "gateway".into(),
        };
        let stats = timing_stats(&[valid.clone(), failed, cancelled, running], &q, now);
        assert_eq!(stats.success, 1);
        assert_eq!(stats.ttft_samples, 1);
        assert_eq!(stats.ttft_p95_ms, Some(100.0));
        assert_eq!(stats.throughput_p50, Some(10.0));
        let mut a = valid.clone();
        a.logical_id = Some("retry".into());
        let mut b = a.clone();
        b.id = "attempt2".into();
        let stats = timing_stats(&[valid, a, b], &q, now);
        assert_eq!(stats.ttft_samples, 1);
        assert_eq!(stats.retry_groups_unmeasurable, 1);
    }
    #[test]
    fn analysis_preserves_unknown_zero_boundaries_and_totals() {
        let now = Utc::now();
        let start = now - chrono::Duration::hours(24);
        let rows = vec![
            AnalysisRow {
                cost: AnalysisCost {
                    unknown_attempts: 1,
                    ..Default::default()
                },
                agent: "claude".into(),
                model: Some("m1".into()),
                time: Some(start.to_rfc3339()),
                tokens: Some(100),
                provisional: false,
                partial: false,
            },
            AnalysisRow {
                cost: AnalysisCost {
                    unknown_attempts: 1,
                    ..Default::default()
                },
                agent: "codex".into(),
                model: None,
                time: Some((now - chrono::Duration::seconds(1)).to_rfc3339()),
                tokens: None,
                provisional: true,
                partial: false,
            },
            AnalysisRow {
                cost: AnalysisCost {
                    unknown_attempts: 1,
                    ..Default::default()
                },
                agent: "claude".into(),
                model: None,
                time: Some(now.to_rfc3339()),
                tokens: Some(900),
                provisional: false,
                partial: false,
            },
        ];
        let query = OverviewQuery {
            agent: None,
            range: 24,
            metric: "tokens".into(),
            dimension: "model".into(),
            source: "native".into(),
        };
        let a = analyze(query.clone(), &rows, now).unwrap();
        assert_eq!(a.samples, 2);
        assert_eq!(a.total, Some(100.0));
        assert_eq!(a.missing, 1);
        assert_eq!(a.provisional, 1);
        assert_eq!(a.buckets.len(), 24);
        assert_eq!(a.agents.iter().filter_map(|v| v.value).sum::<f64>(), 100.0);
        assert_eq!(a.models.iter().filter_map(|v| v.value).sum::<f64>(), 100.0);
        assert_eq!(a.buckets.iter().filter_map(|v| v.value).sum::<f64>(), 100.0);
        assert!(a.buckets.last().unwrap().value.is_none());
        let cost = analyze(
            OverviewQuery {
                metric: "cost".into(),
                ..query
            },
            &rows,
            now,
        )
        .unwrap();
        assert_eq!(cost.total, None);
        assert_eq!(cost.missing, 2);
        let mut partial = rows.clone();
        partial[0].partial = true;
        let a = analyze(
            OverviewQuery {
                agent: None,
                range: 24,
                metric: "tokens".into(),
                dimension: "agent".into(),
                source: "gateway".into(),
            },
            &partial,
            now,
        )
        .unwrap();
        assert_eq!(a.total, Some(100.0));
        assert_eq!(a.samples, 2);
        assert_eq!(a.reported, 0);
        assert_eq!(a.missing, 2);
    }
}
#[test]
fn malformed_history_is_reported_without_erasing_valid_or_invalid_rows() {
    let temp = tempfile::tempdir().unwrap();
    let store = ObservationStore::open(temp.path()).unwrap();
    store.save(&tests::request(Utc::now())).unwrap();
    store
        .0
        .execute(
            "INSERT INTO model_observations VALUES('broken','2026-10-21',1,'{broken')",
            [],
        )
        .unwrap();
    let read = store.read(None).unwrap();
    assert_eq!(read.rows.len(), 1);
    assert_eq!(read.invalid_records, 1);
    assert_eq!(
        store
            .0
            .query_row(
                "SELECT value FROM model_observations WHERE id='broken'",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
        "{broken"
    );
}
