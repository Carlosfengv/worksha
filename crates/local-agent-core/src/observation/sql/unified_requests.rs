//! Scoped SQL positions share the legacy request lease budget. Host payloads
//! stay immutable and are decoded one page at a time; external metadata is bounded.
use super::*;
use crate::domain::{self as d, requests::query as q};
use aggregates::quote;
use chrono::{DateTime, Utc};
mod evidence;
#[cfg(test)]
mod evidence_tests;
pub(super) mod export;
#[cfg(test)]
mod tests;
mod usage;

pub(super) fn usage_metadata(row: &requests::Request) -> String {
    usage::captured(row)
}

pub struct Publication {
    pub signature: String,
    pub states: Vec<d::SourceState>,
    pub native: Vec<d::SourceState>,
    pub authorizations: Vec<crate::sessions::ActivityAuthorization>,
    pub hidden: Vec<String>,
    pub external: Vec<q::UnifiedRequestItem>,
    pub summaries: Vec<q::RequestSourceSummary>,
    /// Host persistence/read health; never supplied by an external source.
    pub history_partial: bool,
}
#[derive(Clone)]
pub(super) struct Frozen {
    pub positions: String,
    pub created: DateTime<Utc>,
    signature: String,
    query: q::UnifiedRequestQuery,
    page: q::UnifiedRequestPage,
    authorizations: Vec<crate::sessions::ActivityAuthorization>,
    hidden: Vec<String>,
    total: usize,
}
pub(super) fn init(db: &Connection) -> Result<(), String> {
    db.execute_batch("CREATE TEMP TABLE unified_request_positions(snapshot INTEGER,pos INTEGER,host_v INTEGER,external_value TEXT,PRIMARY KEY(snapshot,pos)) WITHOUT ROWID;
        CREATE INDEX temp.unified_request_versions ON unified_request_positions(host_v);
        CREATE TEMP TABLE unified_request_input(id TEXT,source TEXT,time INTEGER,value TEXT);").map_err(fail)?;
    usage::init(db)
}
fn unavailable(_: impl std::fmt::Display) -> d::Failure {
    d::Failure::new(d::ErrorCode::Unavailable)
}
fn stale() -> d::Failure {
    d::Failure::new(d::ErrorCode::StaleSnapshot)
}
fn filter(
    input: &q::UnifiedRequestQuery,
    now: DateTime<Utc>,
    hidden: &[String],
    dates: bool,
) -> String {
    let f = &input.filter;
    let legacy = requests::Query {
        start: f.start.clone(),
        end: f.end.clone(),
        provider: f.provider_key.clone(),
        model: f.model.clone(),
        session: f.session_id.clone(),
        project: f.project_id.clone(),
        result: f.outcome.clone(),
        search: f.search.clone(),
        ..Default::default()
    };
    let mut sql = dashboard::filter(&legacy, now, dates);
    if let Some(id) = &f.product_id {
        sql += &format!(" AND r.product={}", quote(id));
    }
    // A hidden project loses its references; its independently authorized
    // requests remain readable unless this query explicitly selects it.
    if f.project_id.as_ref().is_some_and(|id| hidden.contains(id)) {
        sql += " AND 0";
    }
    sql
}
impl History {
    /// Metadata only: do not create a lease, expire unrelated queries or refresh history.
    pub fn object_request_query(&self, token: &str) -> d::Result<q::UnifiedRequestQuery> {
        let frozen = self
            .unified_requests
            .get(token)
            .filter(|f| (Utc::now() - f.created).num_seconds() < 300)
            .ok_or_else(stale)?;
        Ok(frozen.query.clone())
    }
    pub fn object_request_record(
        &self,
        token: &str,
        signature: &str,
        source: &str,
        id: &str,
        sessions: &crate::sessions::NativeSessions,
    ) -> d::Result<q::RequestQueryRecord> {
        self.check_unified_requests(token, signature, sessions)?;
        let frozen = self.unified_requests.get(token).ok_or_else(stale)?;
        if !frozen.query.scope.source_ids.iter().any(|s| s == source) {
            return Err(d::Failure::new(d::ErrorCode::GrantRevoked));
        }
        // Select immutable positions in SQL; decode at most two matching rows,
        // never materialize a million-row page or resolve against current history.
        let mut statement = self.db.prepare("SELECT p.external_value,r.value FROM unified_request_positions p LEFT JOIN record_meta m ON m.v=p.host_v LEFT JOIN record_payloads r ON r.v=p.host_v WHERE p.snapshot=?1 AND ((m.id=?2 AND m.family=?3) OR (p.external_value IS NOT NULL AND json_extract(p.external_value,'$.record.id')=?2 AND json_extract(p.external_value,'$.record.sourceId')=?3)) LIMIT 2").map_err(unavailable)?;
        let mut rows = statement
            .query(params![frozen.positions, id, source])
            .map_err(unavailable)?;
        let row = rows
            .next()
            .map_err(unavailable)?
            .ok_or_else(|| d::Failure::new(d::ErrorCode::NotFound))?;
        let external: Option<String> = row.get(0).map_err(unavailable)?;
        let host: Option<String> = row.get(1).map_err(unavailable)?;
        let item: q::UnifiedRequestItem = match (external, host) {
            (Some(raw), None) => serde_json::from_str(&raw).map_err(unavailable)?,
            (None, Some(raw)) => host_item(
                serde_json::from_str(&raw).map_err(unavailable)?,
                frozen.query.unit,
                &frozen.hidden,
                false,
            )?,
            _ => return Err(unavailable("missing immutable request")),
        };
        if rows.next().map_err(unavailable)?.is_some()
            || item.record.id != id
            || item.record.source_id != source
            || item.record.unit != frozen.query.unit
        {
            return Err(unavailable("ambiguous request"));
        }
        Ok(item.record)
    }
    /// Legacy object details retain their current-record semantics, without preparing sources.
    pub fn current_object_request(
        &self,
        id: &str,
        unit: d::requests::RequestUnit,
    ) -> d::Result<q::RequestQueryRecord> {
        let row = self
            .request_detail(id)
            .map_err(unavailable)?
            .ok_or_else(|| d::Failure::new(d::ErrorCode::NotFound))?;
        let expected = match unit {
            d::requests::RequestUnit::ModelResponse => "native",
            d::requests::RequestUnit::LogicalRequest => "gateway",
        };
        if row["id"].as_str() != Some(id) || row["source"].as_str() != Some(expected) {
            return Err(d::Failure::new(d::ErrorCode::InvalidInput));
        }
        Ok(host_item(row, unit, &[], false)?.record)
    }
    pub fn check_unified_requests(
        &self,
        token: &str,
        signature: &str,
        sessions: &crate::sessions::NativeSessions,
    ) -> d::Result<()> {
        let frozen = self.unified_requests.get(token).ok_or_else(stale)?;
        if frozen.signature != signature || (Utc::now() - frozen.created).num_seconds() >= 300 {
            return Err(stale());
        }
        for authorization in &frozen.authorizations {
            authorization
                .ensure_current(sessions)
                .map_err(|_| stale())?;
        }
        Ok(())
    }
    pub fn unified_request_page(
        &mut self,
        input: &q::UnifiedRequestQuery,
        now: DateTime<Utc>,
        signature: &str,
        publication: Option<Publication>,
    ) -> d::Result<q::UnifiedRequestPage> {
        input.validate()?;
        self.expire(now, input.snapshot_token.is_none())
            .map_err(unavailable)?;
        let token = if let Some(token) = &input.snapshot_token {
            let f = self.unified_requests.get(token).ok_or_else(stale)?;
            if f.signature != signature || !f.query.same_selection(input) {
                return Err(stale());
            }
            token.clone()
        } else {
            let mut p = publication.ok_or_else(stale)?;
            if p.signature != signature
                || p.external.len() > 8192
                || encode(&p.external).map_err(unavailable)?.len() > 16 * 1024 * 1024
            {
                return Err(d::Failure::new(d::ErrorCode::ResourceLimit));
            }
            let mut seen = HashSet::new();
            if p.native
                .iter()
                .any(|s| !input.scope.source_ids.contains(&s.source_id))
                || p.external.iter().any(|r| {
                    r.record.unit != input.unit
                        || r.record.unit != d::requests::RequestUnit::ModelResponse
                        || !input.scope.source_ids.contains(&r.record.source_id)
                        || r.evidence != q::RequestEvidence::SourceReported
                        || r.host_request_id.is_some()
                        || r.record.attempts.is_some()
                        || r.record.ttft_ms.is_some()
                        || r.record.duration_ms.is_some()
                        || r.record.throughput.is_some()
                        || r.evidence_detail.as_ref().is_some_and(|v| {
                            v.matching != q::RequestMatching::SourceReported
                                || v.billing.is_some()
                                || v.timing_note.is_some()
                                || !v.overflow_fields.is_empty()
                        })
                        || !seen.insert((&r.record.source_id, &r.record.id))
                })
            {
                return Err(d::Failure::new(d::ErrorCode::InvalidInput));
            }
            for row in &mut p.external {
                row.evidence_detail =
                    (input.include_evidence == Some(true)).then_some(q::RequestEvidenceDetail {
                        matching: q::RequestMatching::SourceReported,
                        billing: None,
                        timing_note: None,
                        overflow_fields: vec![],
                    });
            }
            for summary in &mut p.summaries {
                summary.evidence_summary =
                    (input.include_evidence == Some(true)).then(|| evidence::reported(summary));
            }
            let token = uuid::Uuid::new_v4().to_string();
            let owner: i64 = self
                .db
                .query_row(
                    "UPDATE position_clock SET seq=seq+1 RETURNING seq",
                    [],
                    |r| r.get(0),
                )
                .map_err(unavailable)?;
            let kind = if input.unit == d::requests::RequestUnit::ModelResponse {
                "native"
            } else {
                "gateway"
            };
            let partial = input.unit == d::requests::RequestUnit::LogicalRequest
                && (self.invalid > 0 || p.history_partial);
            if partial {
                for state in &mut p.states {
                    if p.native.iter().any(|s| s.source_id == state.source_id) {
                        state.coverage = d::Coverage::Partial;
                        state.freshness = d::Freshness::Failed;
                        state.problem = Some("partial_history".into());
                    }
                }
            }
            let ids: Vec<_> = p
                .native
                .iter()
                .filter(|s| {
                    input.unit == d::requests::RequestUnit::LogicalRequest
                        || s.coverage != d::Coverage::Unavailable
                })
                .map(|s| quote(&s.source_id))
                .collect();
            let scope = if ids.is_empty() {
                "0".into()
            } else {
                format!("r.family IN ({})", ids.join(","))
            };
            let base = format!(
                "records r JOIN current c ON c.v=r.v WHERE r.kind={} AND {scope}",
                quote(kind)
            );
            let selected = format!("{base} AND {}", filter(input, now, &p.hidden, true));
            let mut summaries = p.summaries;
            for source in &p.native {
                let usable = input.unit == d::requests::RequestUnit::LogicalRequest
                    || source.coverage != d::Coverage::Unavailable;
                let from = format!(
                    "(SELECT r.* FROM {selected} AND r.family={})",
                    quote(&source.source_id)
                );
                let totals = usable
                    .then(|| usage::summary(&self.db, &from))
                    .transpose()?;
                let summary = totals.as_ref().map(|(s, _)| s);
                let exact_usage = totals.as_ref().map(|(_, u)| u);
                let overflow_fields = exact_usage
                    .as_ref()
                    .map(|u| u.overflow_fields.clone())
                    .unwrap_or_default();
                if !overflow_fields.is_empty() {
                    for state in &mut p.states {
                        if state.source_id == source.source_id {
                            state.coverage = d::Coverage::Partial;
                            state
                                .problem
                                .get_or_insert_with(|| "counter_overflow".into());
                        }
                    }
                }
                let evidence_summary = if input.include_evidence == Some(true) {
                    summary
                        .as_ref()
                        .map(|s| {
                            evidence::summary(
                                &self.db,
                                &from,
                                input.unit,
                                s,
                                overflow_fields.clone(),
                                exact_usage.as_ref().and_then(|u| u.missing_parts.clone()),
                            )
                        })
                        .transpose()?
                } else {
                    None
                };
                let missing: Option<usize> = usable.then(|| self.db.query_row(&format!("SELECT count(*) FROM {base} AND r.family={} AND r.time IS NULL AND {}", quote(&source.source_id), filter(input, now, &p.hidden, false)), [], |r| sql_count(r, 0))).transpose().map_err(unavailable)?;
                summaries.push(q::RequestSourceSummary {
                    source_id: source.source_id.clone(),
                    unit: input.unit,
                    evidence: q::RequestEvidence::HostObserved,
                    coverage: if !overflow_fields.is_empty() {
                        d::Coverage::Partial
                    } else if input.unit == d::requests::RequestUnit::LogicalRequest {
                        if partial {
                            d::Coverage::Partial
                        } else {
                            d::Coverage::Complete
                        }
                    } else {
                        source.coverage.clone()
                    },
                    records: summary.as_ref().map(|s| (s.requests as u64).into()),
                    usage: exact_usage.map_or_else(
                        || (&crate::sessions::Usage::default()).into(),
                        |s| s.usage.clone(),
                    ),
                    usage_missing: summary.as_ref().map(|s| (s.usage_missing as u64).into()),
                    attempts: summary.as_ref().map(|s| (s.attempts as u64).into()),
                    unknown_time: missing.map(|n| (n as u64).into()),
                    evidence_summary,
                });
            }
            let transaction = self.db.unchecked_transaction().map_err(unavailable)?;
            transaction
                .execute("DELETE FROM unified_request_input", [])
                .map_err(unavailable)?;
            for row in &p.external {
                transaction
                    .execute(
                        "INSERT INTO unified_request_input VALUES(?1,?2,?3,?4)",
                        params![
                            row.record.id,
                            row.record.source_id,
                            row.record.occurred_at.as_deref().and_then(d::timestamp),
                            encode(row).map_err(unavailable)?
                        ],
                    )
                    .map_err(unavailable)?;
            }
            let direction = if input.descending { "DESC" } else { "ASC" };
            let (host_sort, external_sort) = match input.sort.unwrap_or(q::RequestSort::Time) {
                q::RequestSort::Time => ("r.time", "time"),
                q::RequestSort::Tokens => (
                    "json_extract(r.domain_usage,'$[0][5]')",
                    "json_extract(value,'$.record.usage.totalTokens')",
                ),
                q::RequestSort::Duration => ("r.domain_duration", "NULL"),
                q::RequestSort::Ttft => ("r.domain_ttft", "NULL"),
                q::RequestSort::CostUsd => ("r.cost_value", "NULL"),
            };
            let digits = if input.sort == Some(q::RequestSort::Tokens) {
                format!("length(sort_value) {direction},")
            } else {
                String::new()
            };
            transaction.execute(&format!("INSERT INTO unified_request_positions SELECT ?1,row_number() OVER(ORDER BY sort_value IS NULL,{digits}sort_value {direction},source,id)-1,host_v,external_value FROM (SELECT r.id,r.family source,r.v host_v,NULL external_value,{host_sort} sort_value FROM {selected} UNION ALL SELECT id,source,NULL,value,{external_sort} FROM unified_request_input)"),[owner]).map_err(unavailable)?;
            transaction
                .execute("DELETE FROM unified_request_input", [])
                .map_err(unavailable)?;
            let total: usize = transaction
                .query_row(
                    "SELECT count(*) FROM unified_request_positions WHERE snapshot=?1",
                    [owner],
                    |r| sql_count(r, 0),
                )
                .map_err(unavailable)?;
            transaction.commit().map_err(unavailable)?;
            self.unified_requests.insert(
                token.clone(),
                Frozen {
                    positions: owner.to_string(),
                    created: now,
                    signature: signature.into(),
                    query: input.clone(),
                    total,
                    authorizations: p.authorizations,
                    hidden: p.hidden,
                    page: q::UnifiedRequestPage {
                        items: vec![],
                        snapshot_token: token.clone(),
                        next_cursor: None,
                        as_of: now.to_rfc3339(),
                        source_states: p.states,
                        source_summaries: summaries,
                        applied_query: input.clone(),
                        overlap_unknown: true,
                    },
                },
            );
            token
        };
        let frozen = self.unified_requests.get(&token).ok_or_else(stale)?;
        let offset = input
            .cursor
            .as_ref()
            .map(|cursor| {
                cursor
                    .strip_prefix(&format!("{token}:"))
                    .ok_or_else(stale)
                    .and_then(|s| s.parse::<usize>().map_err(|_| stale()))
            })
            .transpose()?
            .unwrap_or(0);
        if offset > frozen.total {
            return Err(stale());
        }
        let mut statement = self.db.prepare("SELECT p.external_value,r.value FROM unified_request_positions p LEFT JOIN record_payloads r ON r.v=p.host_v WHERE p.snapshot=?1 AND p.pos>=?2 ORDER BY p.pos LIMIT 40").map_err(unavailable)?;
        let mut page = frozen.page.clone();
        page.items = statement
            .query_map(
                params![
                    frozen.positions,
                    i64::try_from(offset).map_err(unavailable)?
                ],
                |r| {
                    Ok((
                        r.get::<_, Option<String>>(0)?,
                        r.get::<_, Option<String>>(1)?,
                    ))
                },
            )
            .map_err(unavailable)?
            .map(|row| {
                let (external, host) = row.map_err(unavailable)?;
                match (external, host) {
                    (Some(raw), None) => serde_json::from_str(&raw).map_err(unavailable),
                    (None, Some(raw)) => host_item(
                        serde_json::from_str(&raw).map_err(unavailable)?,
                        input.unit,
                        &frozen.hidden,
                        input.include_evidence == Some(true),
                    ),
                    _ => Err(unavailable("missing immutable request")),
                }
            })
            .collect::<d::Result<_>>()?;
        page.applied_query = input.clone();
        page.next_cursor = offset
            .checked_add(40)
            .filter(|next| *next < frozen.total)
            .map(|next| format!("{token}:{next}"));
        Ok(page)
    }
}
fn host_item(
    row: Value,
    unit: d::requests::RequestUnit,
    hidden: &[String],
    include_evidence: bool,
) -> d::Result<q::UnifiedRequestItem> {
    let text = |key: &str| row[key].as_str().map(String::from);
    let required = |key| text(key).ok_or_else(|| unavailable("invalid host record"));
    let id = required("id")?;
    let project = text("projectId").filter(|id| !hidden.contains(id));
    let usage = usage::record(&row, unit)?;
    Ok(q::UnifiedRequestItem {
        evidence_detail: include_evidence
            .then(|| evidence::detail(&row, unit, usage.overflow_fields))
            .transpose()?,
        record: q::RequestQueryRecord {
            id: id.clone(),
            source_id: required("agentId")?,
            unit,
            product_id: text("agentProductId"),
            session_ref: text("sessionId"),
            session_title: text("sessionTitle"),
            project_name: project.as_ref().and_then(|_| text("projectName")),
            project_ref: project,
            occurred_at: text("startedAt").filter(|s| d::timestamp(s).is_some()),
            ended_at: text("endedAt").filter(|s| d::timestamp(s).is_some()),
            model: text("model"),
            provider_key: required("providerKey")?,
            provider_name: required("provider")?,
            outcome: required("status")?,
            usage: usage.usage,
            usage_missing: row["usageMissing"]
                .as_u64()
                .ok_or_else(|| unavailable("invalid usage count"))?
                .into(),
            attempts: Some(
                (row["attempts"]
                    .as_array()
                    .ok_or_else(|| unavailable("invalid attempt list"))?
                    .len() as u64)
                    .into(),
            ),
            duration_ms: row["durationMs"].as_f64(),
            ttft_ms: row["ttftMs"].as_f64(),
            throughput: row["throughput"].as_f64(),
        },
        evidence: q::RequestEvidence::HostObserved,
        host_request_id: Some(id),
        reported_response_id: None,
        reported_duration_ms: None,
    })
}
