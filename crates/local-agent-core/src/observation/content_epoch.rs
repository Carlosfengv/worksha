//! A durable invalidation token for disposable detail projections. This is not
//! installation identity, retention policy, consent or permission to delete.
use rusqlite::{Connection, Transaction, TransactionBehavior, params};

const SCHEMA_VERSION: i64 = 1;
pub(super) const RAW_COLUMNS: &[(&str, &str, u8, u8)] = &[
    ("id", "TEXT", 0, 1),
    ("started_at", "TEXT", 1, 0),
    ("revision", "INTEGER", 1, 0),
    ("value", "TEXT", 1, 0),
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContentEpoch(String);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EpochError {
    Unavailable,
    InvalidRecord,
    UnsupportedVersion,
    Changed,
}

fn unavailable(_: impl std::fmt::Display) -> EpochError {
    EpochError::Unavailable
}

fn version(db: &Connection) -> Result<i64, EpochError> {
    db.query_row("PRAGMA main.user_version", [], |row| row.get(0))
        .map_err(unavailable)
}

pub(super) fn table(
    db: &Connection,
    name: &str,
    expected: &[(&str, &str, u8, u8)],
) -> Result<(), EpochError> {
    let ordinary: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM pragma_table_list WHERE schema='main' AND name=?1 AND type='table' AND wr=0 AND strict=0)", [name], |row| row.get(0)).map_err(unavailable)?;
    let found = db
        .prepare("SELECT name,type,\"notnull\",pk,hidden FROM pragma_table_xinfo(?1,'main') ORDER BY cid")
        .map_err(unavailable)?
        .query_map([name], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, u8>(2)?,
                row.get::<_, u8>(3)?,
                row.get::<_, u8>(4)?,
            ))
        })
        .map_err(unavailable)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(unavailable)?;
    if !ordinary
        || found.len() != expected.len()
        || !found
            .iter()
            .zip(expected)
            .all(|(a, b)| a.0 == b.0 && a.1 == b.1 && a.2 == b.2 && a.3 == b.3 && a.4 == 0)
    {
        return Err(EpochError::InvalidRecord);
    }
    Ok(())
}

/// Read only. Version zero is legacy and has no token; callers must not infer
/// new-install status or initialize missing metadata while checking a lease.
pub fn read(db: &Connection) -> Result<ContentEpoch, EpochError> {
    if db.is_autocommit() {
        let tx =
            Transaction::new_unchecked(db, TransactionBehavior::Deferred).map_err(unavailable)?;
        let epoch = read_snapshot(&tx)?;
        tx.commit().map_err(unavailable)?;
        Ok(epoch)
    } else {
        read_snapshot(db)
    }
}

fn read_snapshot(db: &Connection) -> Result<ContentEpoch, EpochError> {
    if version(db)? != SCHEMA_VERSION {
        return Err(EpochError::UnsupportedVersion);
    }
    table(
        db,
        "observation_content_epoch",
        &[
            ("id", "INTEGER", 0, 1),
            ("format", "INTEGER", 1, 0),
            ("value", "TEXT", 1, 0),
        ],
    )?;
    let mut statement = db.prepare_cached("SELECT id,format,CASE WHEN typeof(value)='text' AND octet_length(value)=36 THEN value END FROM main.observation_content_epoch ORDER BY id LIMIT 2").map_err(unavailable)?;
    let mut rows = statement.query([]).map_err(unavailable)?;
    let row = rows
        .next()
        .map_err(unavailable)?
        .ok_or(EpochError::InvalidRecord)?;
    let id: i64 = row.get(0).map_err(|_| EpochError::InvalidRecord)?;
    let format: i64 = row.get(1).map_err(|_| EpochError::InvalidRecord)?;
    let value: Option<String> = row.get(2).map_err(|_| EpochError::InvalidRecord)?;
    let value = value.ok_or(EpochError::InvalidRecord)?;
    if id != 1 || rows.next().map_err(unavailable)?.is_some() {
        return Err(EpochError::InvalidRecord);
    }
    if format != 1 {
        return Err(EpochError::UnsupportedVersion);
    }
    if !uuid::Uuid::parse_str(&value)
        .is_ok_and(|id| id.get_version_num() == 4 && id.to_string() == value)
    {
        return Err(EpochError::InvalidRecord);
    }
    Ok(ContentEpoch(value))
}

/// Rotate within the caller's SAME database transaction as its content change.
/// The caller must propagate an error and roll back, never commit after failure.
/// This primitive neither changes content nor grants deletion authorization.
pub fn advance(tx: &Transaction<'_>, expected: &ContentEpoch) -> Result<ContentEpoch, EpochError> {
    if tx.is_autocommit() {
        return Err(EpochError::Unavailable);
    }
    if read(tx)? != *expected {
        return Err(EpochError::Changed);
    }
    let next = ContentEpoch(uuid::Uuid::new_v4().to_string());
    let changed = tx.execute("UPDATE main.observation_content_epoch SET value=?1 WHERE id=1 AND format=1 AND value=?2", params![next.0, expected.0]).map_err(unavailable)?;
    if changed != 1 || read(tx)? != next {
        return Err(EpochError::Changed);
    }
    Ok(next)
}

pub(super) fn initialize(db: &Connection) -> Result<(), EpochError> {
    let before = version(db)?;
    if ![0, SCHEMA_VERSION].contains(&before) {
        return Err(EpochError::UnsupportedVersion);
    }
    if before == SCHEMA_VERSION {
        read(db)?;
        table(db, "model_observations", RAW_COLUMNS)?;
        // Current stores need no migration write lock. WAL readers must remain
        // openable while a different connection holds the writer transaction.
        db.execute_batch("PRAGMA journal_mode=WAL;")
            .map_err(unavailable)?;
        read(db)?;
        return Ok(());
    }
    db.execute_batch("PRAGMA journal_mode=WAL;")
        .map_err(unavailable)?;
    let tx = Transaction::new_unchecked(db, TransactionBehavior::Immediate).map_err(unavailable)?;
    match version(&tx)? {
        0 => {
            let existing: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM main.sqlite_master WHERE name='observation_content_epoch')", [], |row| row.get(0)).map_err(unavailable)?;
            if existing {
                return Err(EpochError::InvalidRecord);
            }
            tx.execute_batch("CREATE TABLE IF NOT EXISTS model_observations(id TEXT PRIMARY KEY, started_at TEXT NOT NULL, revision INTEGER NOT NULL, value TEXT NOT NULL);
                CREATE TABLE observation_content_epoch(id INTEGER PRIMARY KEY CHECK(id=1),format INTEGER NOT NULL,value TEXT NOT NULL);").map_err(unavailable)?;
            if tx
                .execute(
                    "INSERT INTO observation_content_epoch VALUES(1,1,?1)",
                    [uuid::Uuid::new_v4().to_string()],
                )
                .map_err(unavailable)?
                != 1
            {
                return Err(EpochError::InvalidRecord);
            }
            tx.pragma_update(None, "user_version", SCHEMA_VERSION)
                .map_err(unavailable)?;
        }
        SCHEMA_VERSION => {
            read(&tx)?;
        }
        _ => return Err(EpochError::UnsupportedVersion),
    }
    table(&tx, "model_observations", RAW_COLUMNS)?;
    tx.execute_batch(
        "CREATE INDEX IF NOT EXISTS observation_started ON model_observations(started_at);",
    )
    .map_err(unavailable)?;
    read(&tx)?;
    tx.commit().map_err(unavailable)
}

#[cfg(test)]
mod tests;
