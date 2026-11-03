use super::{
    Error, MAX_ROW_BYTES, MAX_SECONDS, Protected,
    read_source::{self, ReadSource},
    unavailable,
};
use crate::observation::retention::{ExpirationCandidate, Protection, ledger};
use chrono::{DateTime, Duration, Utc};
use rusqlite::{Connection, OptionalExtension};
use serde::Serialize;
use std::{path::Path, time::Duration as Budget};

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
    let mut stmt=db.prepare("SELECT name FROM pragma_index_list('connector_calls') WHERE \"unique\"=1 AND partial=0").map_err(unavailable)?;
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
    let sql:Option<String>=db.query_row("SELECT sql FROM sqlite_master WHERE type='trigger' AND name='call_retention' AND tbl_name='connector_calls'",[],|r|r.get(0)).optional().map_err(unavailable)?;
    let normalized = sql.map(|s| {
        s.chars()
            .filter(|c| !c.is_whitespace())
            .collect::<String>()
            .to_ascii_lowercase()
            .replace("ifnotexists", "")
            .trim_end_matches(';')
            .to_owned()
    });
    Ok((normalized.as_deref()==Some("createtriggercall_retentionafterinsertonconnector_callsbegindeletefromconnector_callswhereid<=new.id-10000;end")).then_some(10_000))
}
fn scan(
    directory: &Path,
    days: u16,
    now: DateTime<Utc>,
    max_records: u64,
    budget: Budget,
) -> Result<ToolImpact, Error> {
    if ![7, 30, 90].contains(&days) {
        return Err(Error::InvalidInput);
    }
    let cutoff = now
        .checked_sub_signed(Duration::days(i64::from(days)))
        .ok_or(Error::InvalidInput)?;
    let mut source = ReadSource::open(directory, "plugin-extensions.sqlite3", budget)?;
    let started = source.started;
    let tx = source.db.transaction().map_err(unavailable)?;
    let version: u32 = tx
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .map_err(unavailable)?;
    let encoding: String = tx
        .query_row("PRAGMA encoding", [], |r| r.get(0))
        .map_err(unavailable)?;
    if version != 0
        || encoding != "UTF-8"
        || !read_source::table(&tx, "extension_version", &[("version", "INTEGER", 1, 0)])?
        || !read_source::table(
            &tx,
            "connector_calls",
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
            "SELECT COUNT(*),MIN(version) FROM extension_version",
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
            FROM connector_calls",
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
    source.finish()?;
    Ok(result)
}

#[cfg(test)]
mod tests;
