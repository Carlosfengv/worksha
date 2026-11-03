//! Same-database staging only. No sealing, raw deletion, automatic policy activation
//! or background scan is performed here. This is the accounting prerequisite for them.
use super::{FactError, GatewayFact};
use crate::{
    agents::AgentDirectory,
    observation::{ObservationStore, sql::aggregates},
    requests::{Request, Summary},
};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use sha2::{Digest, Sha256};
use std::cell::RefCell;

const MAX_BATCH: usize = 64;
const COLUMNS: [&str; 5] = [
    "key",
    "source_fingerprint",
    "projection_digest",
    "state",
    "value",
];
mod committed;
mod filter;
pub use filter::{AgentFilter, GatewayFilter};
mod queries;
mod source_clock;
mod source_snapshot;
mod tools;
mod validation_cache;
pub(super) use tools::connector_expiration;
pub use tools::{ToolExpiry, ToolFilter, ToolSummary};
mod packed;
pub use packed::{PackedBatch, PackedCoverage, PackedKind, PackedPreview};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LedgerError {
    Unavailable,
    InvalidRecord,
    WrongStore,
    UnsupportedVersion,
    BatchLimit,
    ProtectedFact,
    IncompleteFacts,
    SourceChanged,
    ProjectionDowngrade,
    InvalidQuery,
    Projection(FactError),
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StageResult {
    pub inserted: usize,
    pub updated: usize,
    pub unchanged: usize,
}

pub struct FactLedger<'db> {
    db: &'db Connection,
    store_id: String,
    namespace: String,
    // Only set by the fully validated, SQL-query-only packed preview builder.
    immutable_snapshot: bool,
    source: Option<source_snapshot::Source<'db>>,
    validation_cache: RefCell<validation_cache::Cache>,
}

fn unavailable(_: impl std::fmt::Display) -> LedgerError {
    LedgerError::Unavailable
}
fn content_epoch_read(
    db: &Connection,
) -> Result<crate::observation::content_epoch::ContentEpoch, LedgerError> {
    crate::observation::content_epoch::read(db).map_err(|error| match error {
        crate::observation::content_epoch::EpochError::Unavailable => LedgerError::Unavailable,
        _ => LedgerError::SourceChanged,
    })
}
fn invalid(_: impl std::fmt::Display) -> LedgerError {
    LedgerError::InvalidRecord
}
fn valid_uuid(value: &str) -> bool {
    uuid::Uuid::parse_str(value)
        .is_ok_and(|id| id.get_version_num() == 4 && id.to_string() == value)
}
fn digest(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}
fn scope_digest(store_id: &str, namespace: &str) -> String {
    digest(&format!("retention-fact-scope-v1\0{store_id}\0{namespace}"))
}
fn scope_digest_v2(store_id: &str, namespace: &str) -> String {
    digest(&format!("retention-fact-scope-v2\0{store_id}\0{namespace}"))
}
fn valid_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
}
fn exists(db: &Connection, name: &str) -> Result<bool, LedgerError> {
    db.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
        [name],
        |r| r.get(0),
    )
    .map_err(unavailable)
}
fn identity(db: &Connection, store_id: &str) -> Result<String, LedgerError> {
    let (count, version, owner, namespace, fingerprint): (i64, i64, String, String, String) = db.query_row(
        "SELECT (SELECT count(*) FROM retention_fact_scope),version,store_id,namespace,identity_digest FROM retention_fact_scope WHERE id=1",[],
        |r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?)),
    ).map_err(invalid)?;
    if count != 1 || version < 1 || !valid_uuid(&owner) || !valid_uuid(&namespace) {
        return Err(LedgerError::InvalidRecord);
    }
    if ![1, 2].contains(&version) {
        return Err(LedgerError::UnsupportedVersion);
    }
    let expected = if version == 1 {
        scope_digest(&owner, &namespace)
    } else {
        scope_digest_v2(&owner, &namespace)
    };
    if fingerprint != expected {
        return Err(LedgerError::InvalidRecord);
    }
    if owner != store_id {
        return Err(LedgerError::WrongStore);
    }
    Ok(namespace)
}

impl ObservationStore {
    /// Explicit optional staging only; never called by open/save or startup.
    pub fn retention_ledger(&self, store_id: &str) -> Result<FactLedger<'_>, LedgerError> {
        FactLedger::prepare(&self.0, store_id, Some((&self.2, &self.1)))
    }
}

impl<'db> FactLedger<'db> {
    fn prepare(
        db: &'db Connection,
        store_id: &str,
        origin: Option<(
            &'db crate::observation::source_file::SourceFile,
            &'db crate::observation::content_epoch::ContentEpoch,
        )>,
    ) -> Result<Self, LedgerError> {
        if !valid_uuid(store_id) {
            return Err(LedgerError::InvalidRecord);
        }
        source_clock::no_shadows(db)?;
        if let Some((file, _)) = origin {
            file.check().map_err(source_snapshot::file_error)?;
        }
        let tx =
            Transaction::new_unchecked(db, TransactionBehavior::Immediate).map_err(unavailable)?;
        if let Some((_, expected)) = origin {
            source_clock::raw_schema(&tx)?;
            if &content_epoch_read(&tx)? != expected {
                return Err(LedgerError::SourceChanged);
            }
        }
        let namespace = if exists(&tx, "retention_fact_scope")? {
            identity(&tx, store_id)?
        } else {
            if exists(&tx, "retention_fact_staging")?
                || exists(&tx, "retention_tool_staging")?
                || exists(&tx, "retention_packed_blocks")?
                || exists(&tx, "retention_packed_members")?
                || exists(&tx, "retention_packed_watermark")?
                || source_clock::artifacts_exist(&tx)?
            {
                return Err(LedgerError::InvalidRecord);
            }
            let namespace = uuid::Uuid::new_v4().to_string();
            tx.execute_batch("CREATE TABLE retention_fact_scope(id INTEGER PRIMARY KEY CHECK(id=1),version INTEGER NOT NULL,store_id TEXT NOT NULL,namespace TEXT NOT NULL,identity_digest TEXT NOT NULL);
                CREATE TABLE retention_fact_staging(key TEXT PRIMARY KEY,source_fingerprint TEXT NOT NULL,projection_digest TEXT NOT NULL,state TEXT NOT NULL CHECK(state IN ('staged','sealed')),value TEXT NOT NULL);").map_err(unavailable)?;
            tx.execute(
                "INSERT INTO retention_fact_scope VALUES(1,1,?1,?2,?3)",
                params![store_id, namespace, scope_digest(store_id, &namespace)],
            )
            .map_err(unavailable)?;
            namespace
        };
        let columns = tx
            .prepare("PRAGMA table_info(retention_fact_staging)")
            .map_err(invalid)?
            .query_map([], |r| r.get::<_, String>(1))
            .map_err(invalid)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(invalid)?;
        if columns != COLUMNS {
            return Err(LedgerError::InvalidRecord);
        }
        let source = if let Some((file, epoch)) = origin {
            let marker = source_clock::prepare(&tx, store_id, &namespace)?;
            file.check().map_err(source_snapshot::file_error)?;
            Some(source_snapshot::Source::new(
                db,
                file,
                epoch,
                marker.lineage,
            ))
        } else {
            None
        };
        tx.commit().map_err(unavailable)?;
        aggregates::register(db).map_err(unavailable)?;
        db.execute_batch("PRAGMA temp_store=FILE;PRAGMA temp.cache_size=-32768;")
            .map_err(unavailable)?;
        Ok(FactLedger {
            db,
            store_id: store_id.into(),
            namespace,
            immutable_snapshot: false,
            source,
            validation_cache: RefCell::default(),
        })
    }
}

impl FactLedger<'_> {
    fn check_identity(&self) -> Result<(), LedgerError> {
        let result = (|| {
            source_clock::no_shadows(self.db)?;
            if let Some(source) = &self.source {
                source.check_file()?;
            }
            if identity(self.db, &self.store_id)? != self.namespace {
                return Err(LedgerError::InvalidRecord);
            }
            Ok(())
        })();
        if result.is_err() {
            self.release_query_validation_cache();
        }
        result
    }

    /// Release bounded semantic-check metadata. Query results, source membership
    /// and access permission are never retained by this cache. Drop also frees it.
    pub fn release_query_validation_cache(&self) {
        if let Ok(mut cache) = self.validation_cache.try_borrow_mut() {
            cache.clear();
        }
    }

    /// Summary-only compatibility path. A caller-supplied fingerprint is evidence
    /// to recheck later, never a deletion receipt or complete attribution proof.
    pub fn stage_batch(
        &self,
        rows: &[(&Request, [u8; 32])],
        directory: &AgentDirectory,
    ) -> Result<StageResult, LedgerError> {
        if rows.len() > MAX_BATCH {
            return Err(LedgerError::BatchLimit);
        }
        let tx = Transaction::new_unchecked(self.db, TransactionBehavior::Immediate)
            .map_err(unavailable)?;
        self.check_identity()?;
        let mut result = StageResult::default();
        for (request, source) in rows {
            let fact = GatewayFact::from_request(request, &self.namespace, directory)
                .map_err(LedgerError::Projection)?;
            let fingerprint = source
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>();
            self.persist(&tx, fact, fingerprint, &mut result)?;
        }
        tx.commit().map_err(unavailable)?;
        Ok(result)
    }

    /// Complete attribution needs the exact committed observations and every
    /// member of the current logical group, including later committed retries.
    /// Display requests cannot recreate historical endpoint identity.
    /// Retained statistics and ordered derived attempts must match the canonical
    /// reconstruction; context annotations and route evidence still need authority.
    /// This bounded staging operation neither seals facts nor deletes raw rows.
    pub fn stage_attributed_batch(
        &self,
        requests: &[&Request],
        directory: &AgentDirectory,
        evidence: &[crate::provider_usage::Evidence],
    ) -> Result<StageResult, LedgerError> {
        self.stage_gateway_batch(requests, directory, evidence, false)
    }

    /// Explicitly upgrade source-verified facts to v3 query identity evidence.
    /// Caller statistics and ordered derived attempts must match current raw rows;
    /// existing draft rows are not automatically recertified by a reader upgrade.
    /// Does not publish facts, activate policy, or permit original deletion.
    pub fn stage_queryable_batch(
        &self,
        requests: &[&Request],
        directory: &AgentDirectory,
        evidence: &[crate::provider_usage::Evidence],
    ) -> Result<StageResult, LedgerError> {
        self.stage_gateway_batch(requests, directory, evidence, true)
    }

    fn stage_gateway_batch(
        &self,
        requests: &[&Request],
        directory: &AgentDirectory,
        evidence: &[crate::provider_usage::Evidence],
        queryable: bool,
    ) -> Result<StageResult, LedgerError> {
        const MAX_ATTEMPTS: usize = 1024;
        const MAX_BYTES: usize = 8 * 1024 * 1024;
        if requests.len() > MAX_BATCH
            || requests
                .iter()
                .try_fold(0usize, |sum, r| sum.checked_add(r.attempts.len()))
                .is_none_or(|n| n > MAX_ATTEMPTS)
        {
            if let Some(source) = &self.source {
                source.clear();
            }
            return Err(LedgerError::BatchLimit);
        }
        let source = self.source.as_ref().ok_or(LedgerError::SourceChanged)?;
        // Own the previous index before any fallible scope/SQLite checks.
        let cached = source.detach()?;
        // Detect path replacement before SQLite could write an orphaned file.
        source.check_file()?;
        let started = std::time::Instant::now();
        let tx = Transaction::new_unchecked(self.db, TransactionBehavior::Immediate)
            .map_err(unavailable)?;
        self.check_identity()?;
        if requests.is_empty() {
            source.clear();
            source.header(&tx, &self.store_id, &self.namespace)?;
            tx.commit().map_err(unavailable)?;
            return Ok(StageResult::default());
        }
        let snapshot = source.take(&tx, &self.store_id, &self.namespace, cached, started)?;
        let members = snapshot.members(requests, started)?;
        let mut remaining = MAX_BYTES;
        let mut result = StageResult::default();
        for request in requests {
            let mut source = Vec::with_capacity(request.attempts.len());
            for attempt in &request.attempts {
                let (revision, raw): (i64, Option<String>) = tx.query_row(
                    "SELECT revision,CASE WHEN octet_length(value)<=?2 THEN value END FROM model_observations WHERE id=?1",
                    params![attempt.observation.id, remaining as i64], |r| Ok((r.get(0)?,r.get(1)?)),
                ).optional().map_err(unavailable)?.ok_or(LedgerError::SourceChanged)?;
                let raw = raw.ok_or(LedgerError::BatchLimit)?;
                remaining -= raw.len();
                snapshot.verify_raw(&members, &attempt.observation.id, &raw)?;
                let row: crate::observation::ModelObservation =
                    super::source::observation(&raw).map_err(|_| LedgerError::InvalidRecord)?;
                if row.id != attempt.observation.id
                    || u64::try_from(revision).ok() != Some(row.revision)
                {
                    return Err(LedgerError::SourceChanged);
                }
                source.push((row, raw));
            }
            source.sort_by(|a, b| a.0.id.cmp(&b.0.id));
            let mut hash = Sha256::new();
            hash.update(b"local-agent-retention-source-v1\0");
            for (_, raw) in &source {
                hash.update((raw.len() as u64).to_be_bytes());
                hash.update(raw.as_bytes());
            }
            let fingerprint = format!("{:x}", hash.finalize());
            let originals = source.into_iter().map(|(row, _)| row).collect::<Vec<_>>();
            let project = if queryable {
                GatewayFact::from_request_queryable
            } else {
                GatewayFact::from_request_with_evidence
            };
            let fact = project(request, &originals, &self.namespace, directory, evidence).map_err(
                |error| match error {
                    FactError::InvalidRequest => LedgerError::SourceChanged,
                    other => LedgerError::Projection(other),
                },
            )?;
            self.persist(&tx, fact, fingerprint, &mut result)?;
        }
        self.check_identity()?;
        let after = source.finish(
            &tx,
            &self.store_id,
            &self.namespace,
            &snapshot,
            result,
            started,
        )?;
        tx.commit().map_err(unavailable)?;
        source.keep(snapshot, after);
        Ok(result)
    }

    /// Release the optional work index when a staging campaign stops or moves
    /// on to read-only previews. Dropping the ledger also closes its private DB.
    pub fn release_source_snapshot(&self) {
        if let Some(source) = &self.source {
            source.clear();
        }
    }

    fn persist(
        &self,
        tx: &Transaction<'_>,
        fact: GatewayFact,
        fingerprint: String,
        result: &mut StageResult,
    ) -> Result<(), LedgerError> {
        let value = serde_json::to_string(&fact).map_err(invalid)?;
        let projection = digest(&value);
        validate(&fact.key, &self.namespace, &projection, &value)?;
        let old: Option<(String,String,String,String)> = tx.query_row("SELECT state,source_fingerprint,projection_digest,value FROM retention_fact_staging WHERE key=?1",[&fact.key],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional().map_err(unavailable)?;
        if let Some((state, old_source, old_projection, old_value)) = old {
            if !valid_digest(&old_source) {
                return Err(LedgerError::InvalidRecord);
            }
            let old_fact = validate(&fact.key, &self.namespace, &old_projection, &old_value)?;
            if state != "staged" {
                return Err(LedgerError::ProtectedFact);
            }
            if old_fact.version > fact.version {
                return Err(LedgerError::ProjectionDowngrade);
            }
            if old_source == fingerprint && old_projection == projection {
                result.unchanged += 1;
                return Ok(());
            }
            let changed=tx.execute("UPDATE retention_fact_staging SET source_fingerprint=?2,projection_digest=?3,value=?4 WHERE key=?1 AND state='staged'",params![fact.key,fingerprint,projection,value]).map_err(unavailable)?;
            if changed != 1 {
                return Err(LedgerError::Unavailable);
            }
            result.updated += 1;
        } else {
            let order = packed::next_order(tx, PackedKind::Gateway)?;
            let changed = tx
                .execute(
                    "INSERT INTO retention_fact_staging(rowid,key,source_fingerprint,projection_digest,state,value) VALUES(?1,?2,?3,?4,'staged',?5)",
                    params![order,fact.key, fingerprint, projection, value],
                )
                .map_err(unavailable)?;
            if changed != 1 {
                return Err(LedgerError::Unavailable);
            }
            result.inserted += 1;
        }
        Ok(())
    }

    /// Internal accounting preview only. Staged data is never advertised as sealed
    /// long-term history or mixed into the existing request list/export endpoints.
    pub fn staging_summary(&self) -> Result<Summary, LedgerError> {
        let tx = self.db.unchecked_transaction().map_err(unavailable)?;
        self.checked_coverage()?;
        let summary=aggregates::ordered_summary(&tx,"(SELECT rowid fact_order,0 sub_order,json_extract(value,'$.stats') stats,json_extract(value,'$.cacheIn') cache_in,json_extract(value,'$.cacheRead') cache_read,json_extract(value,'$.stats.ttft.p50') ttft_sample,json_extract(value,'$.stats.duration.p50') duration_sample,json_extract(value,'$.stats.throughput.p50') throughput_sample FROM retention_fact_staging WHERE state='staged')").map_err(invalid)?;
        self.check_identity()?;
        tx.commit().map_err(unavailable)?;
        Ok(summary)
    }
}

fn validate(
    key: &str,
    namespace: &str,
    expected: &str,
    value: &str,
) -> Result<GatewayFact, LedgerError> {
    let fact: GatewayFact = serde_json::from_str(value).map_err(invalid)?;
    let parsed: serde_json::Value = serde_json::from_str(value).map_err(invalid)?;
    let canonical = serde_json::to_value(&fact).map_err(invalid)?;
    let input = fact.cache_in.parse::<u128>().map_err(invalid)?;
    let cached = fact.cache_read.parse::<u128>().map_err(invalid)?;
    let attribution_valid = match (fact.version, &fact.attribution, &fact.agent_identity) {
        (1, None, None) => true,
        (2, Some(attribution), None) => {
            attribution.valid(fact.stats.attempts)
                && attribution
                    .attempts
                    .iter()
                    .all(|a| a.agent_identity.is_none())
        }
        (3, Some(attribution), Some(agent)) => {
            agent.valid()
                && attribution.valid(fact.stats.attempts)
                && attribution
                    .attempts
                    .iter()
                    .all(|a| a.agent_identity.as_ref().is_some_and(|id| id.valid()))
        }
        _ => false,
    };
    if !attribution_valid
        || fact.key != key
        || fact.namespace != namespace
        || digest(value) != expected
        || !same_shape(&parsed, &canonical)
        || !valid_digest(key)
        || fact.stats.requests != 1
        || fact.stats.attempts == 0
        || fact.stats.usage_missing > 1
        || fact.stats.provisional > 1
        || fact.stats.cache_samples > fact.stats.attempts
        || cached > input
        || input.to_string() != fact.cache_in
        || cached.to_string() != fact.cache_read
    {
        return Err(LedgerError::InvalidRecord);
    }
    Ok(fact)
}

// Unknown nested fields must fail closed. Compare structural shape, not a typed
// float roundtrip's decimal spelling: arbitrary_precision preserves original
// number text, while typed f64 parsing can choose an adjacent representation.
// The exact stored bytes are independently checked against projection_digest.
fn same_shape(a: &serde_json::Value, b: &serde_json::Value) -> bool {
    use serde_json::Value;
    match (a, b) {
        (Value::Object(a), Value::Object(b)) => {
            a.len() == b.len()
                && a.iter()
                    .all(|(key, value)| b.get(key).is_some_and(|other| same_shape(value, other)))
        }
        (Value::Array(a), Value::Array(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(a, b)| same_shape(a, b))
        }
        (Value::Null, Value::Null)
        | (Value::Bool(_), Value::Bool(_))
        | (Value::Number(_), Value::Number(_))
        | (Value::String(_), Value::String(_)) => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests;
