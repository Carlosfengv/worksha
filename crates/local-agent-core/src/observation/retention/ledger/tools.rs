//! Explicit same-connector-database staging. This does not change the existing
//! 10,000-record cap, publish long-term reports or authorize day-based cleanup.
use super::*;
use crate::observation::retention::key;
use chrono::{DateTime, Duration, Utc};
use serde::Serialize;
mod fact;
pub(in crate::observation::retention) use fact::source_expiration as connector_expiration;
use fact::{ToolFact, normalized};

const TOOL_COLUMNS: [&str; 6] = [
    "key",
    "source_locator",
    "source_fingerprint",
    "projection_digest",
    "state",
    "value",
];

#[derive(Default)]
pub struct ToolFilter {
    pub plugin_id: Option<String>,
    pub tool: Option<String>,
    pub start: Option<DateTime<Utc>>,
    pub end: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolSummary {
    pub source: &'static str,
    pub coverage: &'static str,
    pub calls: u64,
    pub returned: u64,
    pub tool_errors: u64,
    pub protocol_errors: u64,
    pub transport_errors: u64,
    pub timeouts: u64,
    pub cancelled: u64,
    pub incomplete: u64,
    pub in_progress: u64,
    pub error_denominator: u64,
    pub error_rate: Option<f64>,
    pub duration_samples: u64,
    pub p95_ms: Option<f64>,
    pub last_recorded_at: Option<String>,
    pub unknown_time_calls: u64,
    pub excluded_unknown_time_calls: u64,
    pub unknown_agent_calls: u64,
    pub unknown_session_calls: u64,
    pub unknown_service_calls: u64,
    pub unknown_model_calls: u64,
    pub model_tokens: Option<u64>,
    pub model_usage_state: &'static str,
}

#[derive(Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolExpiry {
    pub expired: u64,
    pub within_window: u64,
    pub protected: u64,
}

fn columns(db: &Connection, table: &str, expected: &[&str]) -> Result<(), LedgerError> {
    // Table names come only from the two fixed call sites below.
    let found = db
        .prepare(&format!("PRAGMA table_info({table})"))
        .map_err(invalid)?
        .query_map([], |r| r.get::<_, String>(1))
        .map_err(invalid)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(invalid)?;
    if found != expected {
        return Err(LedgerError::InvalidRecord);
    }
    Ok(())
}
fn source_schema(db: &Connection) -> Result<(), LedgerError> {
    let (n, version): (i64, Option<i64>) = db
        .query_row(
            "SELECT count(*),min(version) FROM extension_version",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(invalid)?;
    if n != 1 {
        return Err(LedgerError::InvalidRecord);
    }
    if version != Some(1) {
        return Err(LedgerError::UnsupportedVersion);
    }
    columns(
        db,
        "connector_calls",
        &["id", "plugin_id", "identity", "record"],
    )
}

impl<'db> FactLedger<'db> {
    /// Only product-owned proxy observations use this entry. Its database and
    /// store ID must be selected by the trusted host, never by a plugin guest.
    pub fn for_connector_store(db: &'db Connection, store_id: &str) -> Result<Self, LedgerError> {
        source_schema(db)?;
        if exists(db, "retention_tool_staging")? {
            columns(db, "retention_tool_staging", &TOOL_COLUMNS)?;
        }
        let ledger = Self::prepare(db, store_id, None)?;
        let tx =
            Transaction::new_unchecked(db, TransactionBehavior::Immediate).map_err(unavailable)?;
        ledger.check_identity()?;
        source_schema(&tx)?;
        create_staging(&tx)?;
        tx.commit().map_err(unavailable)?;
        Ok(ledger)
    }

    /// IDs locate committed connector_calls rows. All source bytes and identity
    /// evidence are read under the same write transaction; callers supply no facts.
    pub fn stage_tool_batch(&self, ids: &[i64]) -> Result<StageResult, LedgerError> {
        if ids.len() > MAX_BATCH {
            return Err(LedgerError::BatchLimit);
        }
        let tx = Transaction::new_unchecked(self.db, TransactionBehavior::Immediate)
            .map_err(unavailable)?;
        self.check_identity()?;
        source_schema(&tx)?;
        columns(&tx, "retention_tool_staging", &TOOL_COLUMNS)?;
        let mut remaining = 8 * 1024 * 1024usize;
        let mut result = StageResult::default();
        for id in ids {
            if *id <= 0 {
                return Err(LedgerError::InvalidQuery);
            }
            let (plugin,identity,raw):(String,String,Option<String>)=tx.query_row(
                "SELECT CASE WHEN length(CAST(plugin_id AS BLOB))<=256 THEN plugin_id END,CASE WHEN length(CAST(identity AS BLOB))<=8192 THEN identity END,CASE WHEN length(CAST(record AS BLOB))<=?2 THEN record END FROM connector_calls WHERE id=?1",
                params![id,remaining as i64],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)),
            ).optional().map_err(unavailable)?.ok_or(LedgerError::SourceChanged)?;
            let raw = raw.ok_or(LedgerError::BatchLimit)?;
            remaining -= raw.len();
            let fact = ToolFact::from_source(&plugin, &identity, &raw, &self.namespace)?;
            let locator = key(&self.namespace, "connector-row", &id.to_string());
            let mut hash = Sha256::new();
            hash.update(b"local-agent-connector-source-v1\0");
            hash.update(id.to_be_bytes());
            for part in [&plugin, &identity, &raw] {
                hash.update((part.len() as u64).to_be_bytes());
                hash.update(part.as_bytes());
            }
            let fingerprint = format!("{:x}", hash.finalize());
            let value = serde_json::to_string(&fact).map_err(invalid)?;
            let projection = digest(&value);
            ToolFact::validate(&fact.key, &self.namespace, &projection, &value)?;
            let old=tx.prepare("SELECT key,source_locator,source_fingerprint,projection_digest,state,CASE WHEN length(CAST(value AS BLOB))<=16384 THEN value END FROM retention_tool_staging WHERE key=?1 OR source_locator=?2 LIMIT 2").map_err(unavailable)?
                .query_map(params![fact.key,locator],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?,r.get::<_,String>(4)?,r.get::<_,String>(5)?))).map_err(unavailable)?
                .collect::<Result<Vec<_>,_>>().map_err(invalid)?;
            if old.len() > 1 {
                return Err(LedgerError::SourceChanged);
            }
            if let Some((old_key, old_locator, source, old_projection, state, old_value)) =
                old.first()
            {
                checked(
                    old_key,
                    old_locator,
                    source,
                    old_projection,
                    state,
                    old_value,
                    &self.namespace,
                )?;
                if state != "staged" {
                    return Err(LedgerError::ProtectedFact);
                }
                if old_key != &fact.key || old_locator != &locator {
                    return Err(LedgerError::SourceChanged);
                }
                if source == &fingerprint && old_projection == &projection {
                    result.unchanged += 1;
                    continue;
                }
                let changed=tx.execute("UPDATE retention_tool_staging SET source_fingerprint=?2,projection_digest=?3,value=?4 WHERE key=?1 AND state='staged'",params![fact.key,fingerprint,projection,value]).map_err(unavailable)?;
                if changed != 1 {
                    return Err(LedgerError::Unavailable);
                }
                result.updated += 1;
            } else {
                let order = super::packed::next_order(&tx, super::PackedKind::ConnectorTool)?;
                let changed = tx
                    .execute(
                        "INSERT INTO retention_tool_staging(rowid,key,source_locator,source_fingerprint,projection_digest,state,value) VALUES(?1,?2,?3,?4,?5,'staged',?6)",
                        params![order,fact.key, locator, fingerprint, projection, value],
                    )
                    .map_err(unavailable)?;
                if changed != 1 {
                    return Err(LedgerError::Unavailable);
                }
                result.inserted += 1;
            }
        }
        tx.commit().map_err(unavailable)?;
        Ok(result)
    }

    fn visit_tools(&self, mut visit: impl FnMut(ToolFact)) -> Result<(), LedgerError> {
        self.check_identity()?;
        columns(self.db, "retention_tool_staging", &TOOL_COLUMNS)?;
        let mut statement=self.db.prepare("SELECT key,source_locator,source_fingerprint,projection_digest,state,CASE WHEN length(CAST(value AS BLOB))<=16384 THEN value END FROM retention_tool_staging").map_err(unavailable)?;
        let mut rows = statement.query([]).map_err(unavailable)?;
        while let Some(row) = rows.next().map_err(unavailable)? {
            let fact = checked(
                &row.get::<_, String>(0).map_err(invalid)?,
                &row.get::<_, String>(1).map_err(invalid)?,
                &row.get::<_, String>(2).map_err(invalid)?,
                &row.get::<_, String>(3).map_err(invalid)?,
                &row.get::<_, String>(4).map_err(invalid)?,
                &row.get::<_, String>(5).map_err(invalid)?,
                &self.namespace,
            )?;
            if row.get::<_, String>(4).map_err(invalid)? == "staged" {
                visit(fact);
            }
        }
        Ok(())
    }

    /// Internal contribution check over staged proxy observations. The host still
    /// owes current source authorization and complete-report integration.
    pub fn tool_staging_summary(&self, filter: &ToolFilter) -> Result<ToolSummary, LedgerError> {
        let tx = self.db.unchecked_transaction().map_err(unavailable)?;
        if self.immutable_snapshot {
            self.check_identity()?;
        } else {
            self.visit_tools(|_| {})?;
        }
        let mut base = "state='staged'".to_owned();
        for (field, domain, value) in [
            ("connector", "connector", &filter.plugin_id),
            ("tool", "connector-tool", &filter.tool),
        ] {
            if let Some(value) = value {
                base.push_str(&format!(
                    " AND json_extract(value,'$.{field}')='{}'",
                    key(&self.namespace, domain, value)
                ));
            }
        }
        if filter.start.zip(filter.end).is_some_and(|(a, b)| a >= b) {
            return Err(LedgerError::InvalidQuery);
        }
        let unknown=tx.query_row(&format!("SELECT count(*) FROM retention_tool_staging WHERE {base} AND json_extract(value,'$.recordedAt') IS NULL"),[],|r|count_column(r,0)).map_err(invalid)?;
        let mut scope = base;
        for (bound, operator) in [(filter.start, ">="), (filter.end, "<")] {
            if let Some(bound) = bound {
                let time = normalized(bound).ok_or(LedgerError::InvalidQuery)?;
                scope.push_str(&format!(
                    " AND json_extract(value,'$.recordedAt'){operator}'{time}'"
                ));
            }
        }
        let from = format!(
            "(SELECT json_extract(value,'$.outcome') outcome,json_extract(value,'$.recordedAt') at,json_extract(value,'$.durationMs') duration_sample FROM retention_tool_staging WHERE {scope})"
        );
        let mut result=tx.query_row(&format!("SELECT count(*),count(CASE WHEN outcome='success' THEN 1 END),count(CASE WHEN outcome='tool_error' THEN 1 END),count(CASE WHEN outcome='protocol_error' THEN 1 END),count(CASE WHEN outcome='transport_error' THEN 1 END),count(CASE WHEN outcome='timeout' THEN 1 END),count(CASE WHEN outcome='cancelled' THEN 1 END),count(CASE WHEN outcome='incomplete' THEN 1 END),count(CASE WHEN outcome='in_progress' THEN 1 END),max(at),count(CASE WHEN at IS NULL THEN 1 END) FROM {from}"),[],|r| Ok(ToolSummary {
            source: "product_connector", coverage: "staged_only",
            calls:count_column(r,0)?,returned:count_column(r,1)?,tool_errors:count_column(r,2)?,protocol_errors:count_column(r,3)?,transport_errors:count_column(r,4)?,timeouts:count_column(r,5)?,cancelled:count_column(r,6)?,incomplete:count_column(r,7)?,in_progress:count_column(r,8)?,last_recorded_at:r.get(9)?,unknown_time_calls:count_column(r,10)?,
            error_denominator:0,error_rate:None,duration_samples:0,p95_ms:None,excluded_unknown_time_calls:0,unknown_agent_calls:0,unknown_session_calls:0,unknown_service_calls:0,unknown_model_calls:0,model_tokens:None,model_usage_state:"unattributed",
        })).map_err(invalid)?;
        let (samples, p95) =
            aggregates::percentile(&tx, &from, "duration_sample", 95).map_err(invalid)?;
        result.duration_samples = samples as u64;
        result.p95_ms = p95;
        result.error_denominator =
            result.returned + result.tool_errors + result.protocol_errors + result.transport_errors;
        let errors = result.tool_errors + result.protocol_errors + result.transport_errors;
        result.error_rate =
            (result.error_denominator > 0).then(|| errors as f64 / result.error_denominator as f64);
        result.unknown_agent_calls = result.calls;
        result.unknown_session_calls = result.calls;
        result.unknown_service_calls = result.calls;
        result.unknown_model_calls = result.calls;
        if filter.start.is_some() || filter.end.is_some() {
            result.excluded_unknown_time_calls = unknown;
        }
        tx.commit().map_err(unavailable)?;
        Ok(result)
    }

    /// Candidate counts use recorded event age. They never authorize raw deletion
    /// and do not invent a tool start time from wall-clock/monotonic differences.
    pub fn tool_expiration_counts(
        &self,
        days: u16,
        now: DateTime<Utc>,
    ) -> Result<ToolExpiry, LedgerError> {
        if ![7, 30, 90].contains(&days) {
            return Err(LedgerError::InvalidQuery);
        }
        let cutoff = now
            .checked_sub_signed(Duration::days(i64::from(days)))
            .ok_or(LedgerError::InvalidQuery)?;
        let tx = self.db.unchecked_transaction().map_err(unavailable)?;
        let mut result = ToolExpiry::default();
        self.visit_tools(|fact| match fact.expiration_at(cutoff) {
            crate::observation::retention::ExpirationCandidate::Expired => result.expired += 1,
            crate::observation::retention::ExpirationCandidate::WithinWindow => {
                result.within_window += 1
            }
            crate::observation::retention::ExpirationCandidate::Protected(_) => {
                result.protected += 1
            }
        })?;
        tx.commit().map_err(unavailable)?;
        Ok(result)
    }
}

pub(super) fn create_staging(db: &Connection) -> Result<(), LedgerError> {
    db.execute_batch("CREATE TABLE IF NOT EXISTS retention_tool_staging(key TEXT PRIMARY KEY,source_locator TEXT NOT NULL UNIQUE,source_fingerprint TEXT NOT NULL,projection_digest TEXT NOT NULL,state TEXT NOT NULL CHECK(state IN ('staged','sealed')),value TEXT NOT NULL);").map_err(unavailable)?;
    columns(db, "retention_tool_staging", &TOOL_COLUMNS)
}

pub(super) fn validate_fact(
    id: &str,
    namespace: &str,
    expected: &str,
    value: &str,
) -> Result<(), LedgerError> {
    ToolFact::validate(id, namespace, expected, value).map(|_| ())
}

fn count_column(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<u64> {
    let value: i64 = row.get(index)?;
    value
        .try_into()
        .map_err(|_| rusqlite::Error::IntegralValueOutOfRange(index, value))
}

fn checked(
    id: &str,
    locator: &str,
    source: &str,
    projection: &str,
    state: &str,
    value: &str,
    namespace: &str,
) -> Result<ToolFact, LedgerError> {
    if !valid_digest(locator) || !valid_digest(source) || !["staged", "sealed"].contains(&state) {
        return Err(LedgerError::InvalidRecord);
    }
    ToolFact::validate(id, namespace, projection, value)
}

#[cfg(test)]
mod tests;
