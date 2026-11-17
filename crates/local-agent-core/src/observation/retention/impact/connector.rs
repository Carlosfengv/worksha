use super::{
    Error, MAX_ROW_BYTES, MAX_SECONDS, Protected,
    read_source::{self, ReadSource},
    unavailable,
};
use crate::observation::retention::{ExpirationCandidate, Protection, ledger};
use chrono::{DateTime, Duration, Utc};
use rusqlite::{Connection, OptionalExtension};
use serde::Serialize;
use std::{
    path::Path,
    time::{Duration as Budget, Instant},
};

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolImpact {
    pub records: u64,
    pub expired: u64,
    pub within_window: u64,
    pub protected: Protected,
    pub unreadable_records: u64,
    pub classification_complete: bool,
    pub capacity_limit: Option<u32>,
}

/// The saved product-proxy collection only. Missing prior history, native MCP
/// files and side-effect completion are not inferred from this inventory.
pub fn connector(directory: &Path, days: u16, now: DateTime<Utc>) -> Result<ToolImpact, Error> {
    scan(
        directory,
        days,
        now,
        1_000_000,
        Budget::from_secs(MAX_SECONDS),
    )
}
fn unique_identity(db: &Connection) -> Result<bool, Error> {
    let mut stmt=db.prepare("SELECT name FROM pragma_index_list('ext_adv_connector_calls') WHERE \"unique\"=1 AND partial=0").map_err(unavailable)?;
    let mut indexes = stmt.query([]).map_err(unavailable)?;
    while let Some(row) = indexes.next().map_err(unavailable)? {
        let name: String = row.get(0).map_err(unavailable)?;
        let parts = db
            .prepare("SELECT name,coll,desc FROM pragma_index_xinfo(?1) WHERE key=1 ORDER BY seqno")
            .map_err(unavailable)?
            .query_map([name], |r| {
                Ok((
                    r.get::<_, Option<String>>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, u8>(2)?,
                ))
            })
            .map_err(unavailable)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(unavailable)?;
        if parts
            == [
                (Some("plugin_id".into()), "BINARY".into(), 0),
                (Some("identity".into()), "BINARY".into(), 0),
            ]
        {
            return Ok(true);
        }
    }
    Ok(false)
}
fn capacity_limit(db: &Connection) -> Result<Option<u32>, Error> {
    let sql:Option<String>=db.query_row("SELECT sql FROM sqlite_master WHERE type='trigger' AND name='ext_adv_call_retention' AND tbl_name='ext_adv_connector_calls'",[],|r|r.get(0)).optional().map_err(unavailable)?;
    let normalized = sql.map(|s| {
        s.chars()
            .filter(|c| !c.is_whitespace())
            .collect::<String>()
            .to_ascii_lowercase()
            .replace("ifnotexists", "")
            .trim_end_matches(';')
            .to_owned()
    });
    Ok((normalized.as_deref()==Some("createtriggerext_adv_call_retentionafterinsertonext_adv_connector_callsbegindeletefromext_adv_connector_callswhereid<=new.id-10000;end")).then_some(10_000))
}
fn scan(
    directory: &Path,
    days: u16,
    now: DateTime<Utc>,
    max_records: u64,
    budget: Budget,
) -> Result<ToolImpact, Error> {
    let source = ReadSource::open(directory, "local-agent.sqlite3", budget)?;
    let result = scan_connection(&source.db, days, now, max_records, source.started, budget)?;
    source.finish()?;
    Ok(result)
}

impl crate::Store {
    pub fn connector_retention_impact(
        &self,
        days: u16,
        now: DateTime<Utc>,
    ) -> Result<ToolImpact, Error> {
        if !self.database_file_matches() {
            return Err(Error::SourceChanged);
        }
        let db = &self.connection;
        if !db.is_readonly("main").map_err(unavailable)? {
            return Err(Error::Unavailable);
        }
        let started = Instant::now();
        let budget = Budget::from_secs(MAX_SECONDS);
        db.progress_handler(10_000, Some(move || started.elapsed() >= budget))
            .map_err(unavailable)?;
        let result = scan_connection(db, days, now, 1_000_000, started, budget);
        db.progress_handler(0, None::<fn() -> bool>)
            .map_err(unavailable)?;
        if !self.database_file_matches() {
            return Err(Error::SourceChanged);
        }
        if started.elapsed() >= budget {
            return Err(Error::Limit);
        }
        result
    }
}

fn scan_connection(
    db: &Connection,
    days: u16,
    now: DateTime<Utc>,
    max_records: u64,
    started: Instant,
    budget: Budget,
) -> Result<ToolImpact, Error> {
    if ![7, 30, 90].contains(&days) {
        return Err(Error::InvalidInput);
    }
    let cutoff = now
        .checked_sub_signed(Duration::days(i64::from(days)))
        .ok_or(Error::InvalidInput)?;
    let tx = db.unchecked_transaction().map_err(unavailable)?;
    let version: u32 = tx
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .map_err(unavailable)?;
    let encoding: String = tx
        .query_row("PRAGMA encoding", [], |r| r.get(0))
        .map_err(unavailable)?;
    if !matches!(
        version,
        crate::Store::SCHEMA_VERSION | crate::iam::PROTECTED_SCHEMA_VERSION
    ) || encoding != "UTF-8"
        || !read_source::table(
            &tx,
            "ext_adv_extension_version",
            &[("version", "INTEGER", 1, 0)],
        )?
        || !read_source::table(
            &tx,
            "ext_adv_connector_calls",
            &[
                ("id", "INTEGER", 0, 1),
                ("plugin_id", "TEXT", 1, 0),
                ("identity", "TEXT", 1, 0),
                ("record", "TEXT", 1, 0),
            ],
        )?
        || !unique_identity(&tx)?
    {
        return Err(Error::UnsupportedSchema);
    }
    let (n, version): (i64, Option<i64>) = tx
        .query_row(
            "SELECT COUNT(*),MIN(version) FROM ext_adv_extension_version",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(unavailable)?;
    if n != 1 || version != Some(1) {
        return Err(Error::UnsupportedSchema);
    }
    let mut result = ToolImpact {
        classification_complete: true,
        capacity_limit: capacity_limit(&tx)?,
        ..Default::default()
    };
    {
        let mut stmt = tx
            .prepare(
                "SELECT
            CASE WHEN typeof(id)='integer' THEN id END,
            CASE WHEN typeof(plugin_id)='text' AND octet_length(plugin_id)<=256 THEN plugin_id END,
            CASE WHEN typeof(identity)='text' AND octet_length(identity)<=8192 THEN identity END,
            CASE WHEN typeof(record)='text' AND octet_length(record)<=?1 THEN record END
            FROM ext_adv_connector_calls",
            )
            .map_err(unavailable)?;
        let mut rows = stmt.query([MAX_ROW_BYTES]).map_err(unavailable)?;
        while let Some(row) = rows.next().map_err(unavailable)? {
            result.records += 1;
            if result.records > max_records || started.elapsed() >= budget {
                return Err(Error::Limit);
            }
            let fields = row
                .get::<_, Option<i64>>(0)
                .ok()
                .flatten()
                .filter(|id| *id > 0)
                .zip(row.get::<_, Option<String>>(1).ok().flatten())
                .zip(row.get::<_, Option<String>>(2).ok().flatten())
                .zip(row.get::<_, Option<String>>(3).ok().flatten());
            let Some((((_, plugin), identity), raw)) = fields else {
                result.unreadable_records += 1;
                result.classification_complete = false;
                continue;
            };
            match ledger::connector_expiration(&plugin, &identity, &raw, cutoff) {
                Ok(ExpirationCandidate::Expired) => result.expired += 1,
                Ok(ExpirationCandidate::WithinWindow) => result.within_window += 1,
                Ok(ExpirationCandidate::Protected(Protection::Active)) => {
                    result.protected.active += 1
                }
                Ok(ExpirationCandidate::Protected(Protection::UnknownOutcome)) => {
                    result.protected.unknown_outcome += 1
                }
                Ok(ExpirationCandidate::Protected(Protection::UnknownTime)) => {
                    result.protected.unknown_time += 1
                }
                Err(_) => {
                    result.protected.unsupported_source += 1;
                    result.classification_complete = false;
                }
            }
        }
    }
    tx.commit().map_err(unavailable)?;
    Ok(result)
}

#[cfg(test)]
mod tests;
