//! Shared strict, bounded scan of the committed raw corpus. Reuse retains this
//! decoder and membership contract rather than trusting display models.
use super::*;
use crate::observation::ModelObservation;
use std::{
    collections::{HashMap, HashSet, hash_map::Entry},
    time::{Duration, Instant},
};

const MAX_ROWS: usize = 1_000_000;
const MAX_ROW_BYTES: i64 = 2 * 1024 * 1024;
const SCAN_BUDGET: Duration = Duration::from_secs(10);

#[derive(Clone, Copy)]
pub(super) struct Limits {
    pub rows: usize,
    pub row_bytes: i64,
    pub budget: Duration,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            rows: MAX_ROWS,
            row_bytes: MAX_ROW_BYTES,
            budget: SCAN_BUDGET,
        }
    }
}

pub(super) fn expected<'a>(
    requests: &[&'a Request],
) -> Result<HashMap<&'a str, HashSet<&'a str>>, LedgerError> {
    let mut expected = HashMap::new();
    for request in requests {
        if request.id.len() > 1024
            || request
                .attempts
                .iter()
                .any(|a| a.observation.id.len() > 1024)
        {
            return Err(LedgerError::BatchLimit);
        }
        let ids = request
            .attempts
            .iter()
            .map(|a| a.observation.id.as_str())
            .collect::<HashSet<_>>();
        if ids.len() != request.attempts.len() {
            return Err(LedgerError::SourceChanged);
        }
        match expected.entry(request.id.as_str()) {
            Entry::Vacant(entry) => {
                entry.insert(ids);
            }
            Entry::Occupied(entry) if entry.get() != &ids => {
                return Err(LedgerError::SourceChanged);
            }
            Entry::Occupied(_) => {}
        }
    }
    Ok(expected)
}

pub(super) fn scan(
    db: &Transaction<'_>,
    limits: Limits,
    started: Instant,
    mut visit: impl FnMut(&ModelObservation, &str) -> Result<(), LedgerError>,
) -> Result<(), LedgerError> {
    if db.is_autocommit() {
        return Err(LedgerError::Unavailable);
    }
    source_clock::raw_schema(db)?;
    if started.elapsed() >= limits.budget {
        return Err(LedgerError::BatchLimit);
    }
    let mut statement = db
        .prepare(
            "SELECT
        CASE WHEN typeof(id)='text' AND octet_length(id)>0 AND octet_length(id)<=1024 THEN id END,
        CASE WHEN typeof(started_at)='text' AND octet_length(started_at)<=128 THEN started_at END,
        CASE WHEN typeof(revision)='integer' AND revision>=0 THEN revision END,
        CASE WHEN typeof(value)='text' AND octet_length(value)<=?1 THEN value END,
        typeof(value)='text'
        FROM main.model_observations ORDER BY rowid LIMIT ?2",
        )
        .map_err(unavailable)?;
    let limit =
        i64::try_from(limits.rows.saturating_add(1)).map_err(|_| LedgerError::BatchLimit)?;
    let mut cursor = statement
        .query(params![limits.row_bytes, limit])
        .map_err(unavailable)?;
    let mut scanned = 0;
    while let Some(row) = cursor.next().map_err(unavailable)? {
        scanned += 1;
        if scanned > limits.rows || started.elapsed() >= limits.budget {
            return Err(LedgerError::BatchLimit);
        }
        if !row.get::<_, bool>(4).map_err(invalid)? {
            return Err(LedgerError::InvalidRecord);
        }
        let raw = row
            .get::<_, Option<String>>(3)
            .map_err(invalid)?
            .ok_or(LedgerError::BatchLimit)?;
        let observation =
            super::super::source::observation(&raw).map_err(|_| LedgerError::InvalidRecord)?;
        if row.get::<_, Option<String>>(0).map_err(invalid)?.as_deref()
            != Some(observation.id.as_str())
            || row.get::<_, Option<String>>(1).map_err(invalid)?.as_deref()
                != Some(observation.started_at.as_str())
            || row
                .get::<_, Option<i64>>(2)
                .map_err(invalid)?
                .and_then(|v| u64::try_from(v).ok())
                != Some(observation.revision)
        {
            return Err(LedgerError::SourceChanged);
        }
        visit(&observation, &raw)?;
    }
    if started.elapsed() >= limits.budget {
        return Err(LedgerError::BatchLimit);
    }
    Ok(())
}

// Preserve the previous verifier as a regression oracle for its existing
// private limit/SQLite fencing tests. Production uses the same scan above.
#[cfg(test)]
fn verify(
    db: &Transaction<'_>,
    requests: &[&Request],
    started: Instant,
) -> Result<[u8; 32], LedgerError> {
    verify_bounded(db, requests, MAX_ROWS, SCAN_BUDGET, started)
}
#[cfg(test)]
fn verify_bounded(
    db: &Transaction<'_>,
    requests: &[&Request],
    max_rows: usize,
    budget: Duration,
    started: Instant,
) -> Result<[u8; 32], LedgerError> {
    if requests.is_empty() {
        return Ok([0; 32]);
    }
    let mut expected = expected(requests)?;
    let mut snapshot = Sha256::new();
    snapshot.update(b"local-agent-retention-committed-source-v1\0");
    scan(
        db,
        Limits {
            rows: max_rows,
            budget,
            ..Default::default()
        },
        started,
        |row, raw| {
            let key = crate::requests::logical_request_key(row);
            if let Some(ids) = expected.get_mut(key.as_str())
                && !ids.remove(row.id.as_str())
            {
                return Err(LedgerError::SourceChanged);
            }
            snapshot.update((raw.len() as u64).to_be_bytes());
            snapshot.update(raw.as_bytes());
            Ok(())
        },
    )?;
    if expected.values().any(|ids| !ids.is_empty()) {
        return Err(LedgerError::SourceChanged);
    }
    Ok(snapshot.finalize().into())
}

#[cfg(test)]
mod tests;
