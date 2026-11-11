//! A bounded, read-only inventory of committed gateway metadata, never consent,
//! a cleanup plan, a ledger publication, or a whole-product history count.
use super::{instant, source};
use chrono::{DateTime, Duration, Utc};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::Path;
mod activity;
mod connector;
mod read_source;
pub use activity::{ActivityImpact, ActivitySource, activity};
pub use connector::{ToolImpact, connector};
use read_source::ReadSource;

const MAX_ROW_BYTES: i64 = 2 * 1024 * 1024;
const MAX_ATTEMPTS: u64 = 1_000_000;
const MAX_SECONDS: u64 = 10;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Request {
    pub store_id: String,
    pub expected_settings_revision: u64,
    pub days: u16,
}

impl Request {
    pub fn valid(&self) -> bool {
        [7, 30, 90].contains(&self.days)
            && self.expected_settings_revision <= 9_007_199_254_740_991
            && uuid::Uuid::parse_str(&self.store_id)
                .is_ok_and(|id| id.get_version_num() == 4 && id.to_string() == self.store_id)
    }
}

/// Omitted format selects the original inventory, so existing strict readers
/// keep their v1 envelope. New callers explicitly request the broader scope.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InventoryRequest {
    pub store_id: String,
    pub expected_settings_revision: u64,
    pub days: u16,
    #[serde(default = "inventory_version")]
    pub inventory_version: u8,
}
fn inventory_version() -> u8 {
    1
}
impl InventoryRequest {
    pub fn scope(&self) -> Request {
        Request {
            store_id: self.store_id.clone(),
            expected_settings_revision: self.expected_settings_revision,
            days: self.days,
        }
    }
    pub fn valid(&self) -> bool {
        [1, 2].contains(&self.inventory_version) && self.scope().valid()
    }
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Protected {
    pub active: u64,
    pub unknown_outcome: u64,
    pub unknown_time: u64,
    pub unsupported_source: u64,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GatewayImpact {
    pub attempts: u64,
    pub logical_requests: u64,
    pub expired: u64,
    pub within_window: u64,
    pub protected: Protected,
    pub unreadable_attempts: u64,
    pub classification_complete: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    InvalidInput,
    SourceAbsent,
    SourceChanged,
    UnsupportedSchema,
    Unavailable,
    Limit,
}
fn unavailable(error: impl std::fmt::Display) -> Error {
    #[cfg(test)]
    if std::env::var("LOCAL_AGENT_SQL_PROFILE").as_deref() == Ok("1") {
        eprintln!("IMPACT_ERROR {error}");
    }
    let _ = error;
    Error::Unavailable
}

/// Complete scan or an explicit error; no page is reported as the collection.
/// A private disk-backed database holds only hashes and classification flags.
pub fn gateway(directory: &Path, days: u16, now: DateTime<Utc>) -> Result<GatewayImpact, Error> {
    scan(
        directory,
        days,
        now,
        MAX_ATTEMPTS,
        std::time::Duration::from_secs(MAX_SECONDS),
    )
}

fn scan(
    directory: &Path,
    days: u16,
    now: DateTime<Utc>,
    max_attempts: u64,
    budget: std::time::Duration,
) -> Result<GatewayImpact, Error> {
    if ![7, 30, 90].contains(&days) {
        return Err(Error::InvalidInput);
    }
    let cutoff = now
        .checked_sub_signed(Duration::days(i64::from(days)))
        .ok_or(Error::InvalidInput)?;
    let mut source = ReadSource::open(directory, "observations.sqlite3", budget)?;
    let started = source.started;
    let tx = source.db.transaction().map_err(unavailable)?;
    let version: u32 = tx
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .map_err(unavailable)?;
    let encoding: String = tx
        .query_row("PRAGMA encoding", [], |r| r.get(0))
        .map_err(unavailable)?;
    if ![0, 1].contains(&version)
        || encoding != "UTF-8"
        || !read_source::table(
            &tx,
            "model_observations",
            &[
                ("id", "TEXT", 0, 1),
                ("started_at", "TEXT", 1, 0),
                ("revision", "INTEGER", 1, 0),
                ("value", "TEXT", 1, 0),
            ],
        )?
    {
        return Err(Error::UnsupportedSchema);
    }
    if version == 1 {
        crate::observation::content_epoch::read(&tx).map_err(|_| Error::UnsupportedSchema)?;
    } else {
        let misplaced: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='observation_content_epoch')",
                [],
                |r| r.get(0),
            )
            .map_err(unavailable)?;
        if misplaced {
            return Err(Error::UnsupportedSchema);
        }
    }
    let work = Connection::open("").map_err(unavailable)?;
    work.execute_batch("PRAGMA temp_store=FILE; PRAGMA cache_size=-2048; CREATE TABLE groups(key BLOB PRIMARY KEY, expired INTEGER NOT NULL, flags INTEGER NOT NULL) WITHOUT ROWID; BEGIN;").map_err(unavailable)?;
    work.progress_handler(10_000, Some(move || started.elapsed() >= budget))
        .map_err(unavailable)?;
    let mut result = GatewayImpact {
        classification_complete: true,
        ..Default::default()
    };
    {
        let mut write = work.prepare("INSERT INTO groups VALUES(?1,?2,?3) ON CONFLICT(key) DO UPDATE SET expired=min(expired,excluded.expired),flags=flags | excluded.flags").map_err(unavailable)?;
        let mut stmt = tx.prepare("SELECT
            CASE WHEN typeof(id)='text' AND octet_length(id)<=1024 THEN id END,
            CASE WHEN typeof(started_at)='text' AND octet_length(started_at)<=128 THEN started_at END,
            CASE WHEN typeof(revision)='integer' THEN revision END,
            CASE WHEN typeof(value)='text' AND octet_length(value)<=?1 THEN value END
            FROM model_observations").map_err(unavailable)?;
        let mut cursor = stmt.query([MAX_ROW_BYTES]).map_err(unavailable)?;
        let mut inspected = 0;
        while let Some(row) = cursor.next().map_err(unavailable)? {
            inspected += 1;
            if inspected > max_attempts || started.elapsed() >= budget {
                return Err(Error::Limit);
            }
            let raw = row.get::<_, Option<String>>(3).ok().flatten();
            let Some((strict, observation)) = raw.as_deref().and_then(|raw| {
                source::observation(raw)
                    .map(|row| (true, row))
                    .ok()
                    .or_else(|| {
                        serde_json::from_str::<super::super::ModelObservation>(raw)
                            .ok()
                            .map(|row| (false, row))
                    })
            }) else {
                result.attempts += 1;
                result.unreadable_attempts += 1;
                result.classification_complete = false;
                continue;
            };
            result.attempts += u64::from(!observation.is_route_trace());
            let source_ok = strict
                && row.get::<_, String>(0).ok().as_ref() == Some(&observation.id)
                && !observation.id.is_empty()
                && row.get::<_, String>(1).ok().as_ref() == Some(&observation.started_at)
                && row
                    .get::<_, i64>(2)
                    .ok()
                    .and_then(|v| u64::try_from(v).ok())
                    == Some(observation.revision);
            result.classification_complete &= source_ok;
            let mut flags = if source_ok { 0 } else { 8 };
            if observation.status == "in_progress" {
                flags |= 1;
            }
            if !["success", "error", "failed", "incomplete", "cancelled"]
                .contains(&observation.status.as_str())
            {
                flags |= 2;
            }
            let time = instant(&observation.started_at)
                .zip(observation.ended_at.as_deref().and_then(instant))
                .filter(|(start, end)| end >= start);
            if time.is_none() {
                flags |= 4;
            }
            let expired = time.is_some_and(|(start, end)| start < cutoff && end < cutoff);
            // Reuse canonical grouping including retries across model-service Providers.
            let key = Sha256::digest(crate::requests::logical_request_key(&observation).as_bytes());
            write
                .execute(params![key.as_slice(), expired, flags])
                .map_err(unavailable)?;
        }
    }
    work.execute_batch("COMMIT").map_err(unavailable)?;
    // Unknown source takes precedence: these groups cannot be claimed fully checked.
    let mut stmt = work.prepare("SELECT CASE WHEN flags&8!=0 THEN 8 WHEN flags&1!=0 THEN 1 WHEN flags&2!=0 THEN 2 WHEN flags&4!=0 THEN 4 WHEN expired THEN 16 ELSE 0 END AS kind,COUNT(*) FROM groups GROUP BY kind").map_err(unavailable)?;
    let mut cursor = stmt.query([]).map_err(unavailable)?;
    while let Some(row) = cursor.next().map_err(unavailable)? {
        let count =
            u64::try_from(row.get::<_, i64>(1).map_err(unavailable)?).map_err(unavailable)?;
        result.logical_requests += count;
        match row.get::<_, u8>(0).map_err(unavailable)? {
            8 => result.protected.unsupported_source = count,
            1 => result.protected.active = count,
            2 => result.protected.unknown_outcome = count,
            4 => result.protected.unknown_time = count,
            16 => result.expired = count,
            _ => result.within_window = count,
        }
    }
    tx.commit().map_err(unavailable)?;
    source.finish()?;
    Ok(result)
}

#[cfg(test)]
mod tests;
