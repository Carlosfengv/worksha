//! Optional source-version protocol, installed only by explicit gateway staging.
//! Built-in SQL triggers also cover already-open/older metadata writers.
use super::*;
use crate::observation::content_epoch;

const TABLE: &str = "retention_source_clock";
const COLUMNS: &[(&str, &str, u8, u8)] = &[
    ("id", "INTEGER", 0, 1),
    ("format", "INTEGER", 1, 0),
    ("store_id", "TEXT", 1, 0),
    ("namespace", "TEXT", 1, 0),
    ("lineage", "TEXT", 1, 0),
    ("nonce", "BLOB", 1, 0),
    ("identity_digest", "TEXT", 1, 0),
];
const SCOPE_COLUMNS: &[(&str, &str, u8, u8)] = &[
    ("id", "INTEGER", 0, 1),
    ("version", "INTEGER", 1, 0),
    ("store_id", "TEXT", 1, 0),
    ("namespace", "TEXT", 1, 0),
    ("identity_digest", "TEXT", 1, 0),
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Marker {
    pub lineage: String,
    pub nonce: [u8; 16],
}

pub(super) fn no_shadows(db: &Connection) -> Result<(), LedgerError> {
    let shadows: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM temp.sqlite_master WHERE lower(name) GLOB 'retention_*' OR lower(name) IN ('model_observations','connector_calls','extension_version','observation_content_epoch'))", [], |r|r.get(0)).map_err(unavailable)?;
    if shadows {
        return Err(LedgerError::InvalidRecord);
    }
    Ok(())
}

pub(super) fn raw_schema(db: &Connection) -> Result<(), LedgerError> {
    no_shadows(db)?;
    content_epoch::table(db, "model_observations", content_epoch::RAW_COLUMNS).map_err(schema_error)
}

fn schema_error(error: content_epoch::EpochError) -> LedgerError {
    match error {
        content_epoch::EpochError::Unavailable => LedgerError::Unavailable,
        _ => LedgerError::InvalidRecord,
    }
}

fn binding(store_id: &str, namespace: &str, lineage: &str) -> String {
    digest(&format!(
        "retention-source-clock-v1\0{store_id}\0{namespace}\0{lineage}"
    ))
}

fn triggers(store_id: &str, namespace: &str, lineage: &str) -> Vec<(String, String)> {
    let identity = binding(store_id, namespace, lineage);
    ["INSERT", "UPDATE", "DELETE"].into_iter().map(|event| {
        let name = format!("retention_source_{}_v1",event.to_ascii_lowercase());
        let sql=format!("CREATE TRIGGER {name} AFTER {event} ON model_observations BEGIN
            SELECT CASE WHEN (SELECT count(*) FROM retention_source_clock)!=1 OR NOT EXISTS(SELECT 1 FROM retention_source_clock WHERE id=1 AND format=1 AND store_id='{store_id}' AND namespace='{namespace}' AND lineage='{lineage}' AND identity_digest='{identity}' AND typeof(nonce)='blob' AND length(nonce)=16) THEN RAISE(ABORT,'retention source version unavailable') END;
            UPDATE retention_source_clock SET nonce=randomblob(16) WHERE id=1;
            END");
        (name,sql)
    }).collect()
}

pub(super) fn artifacts_exist(db: &Connection) -> Result<bool, LedgerError> {
    db.query_row(
        "SELECT EXISTS(SELECT 1 FROM main.sqlite_master WHERE lower(name) GLOB 'retention_source_*')",
        [],
        |r| r.get(0),
    )
    .map_err(unavailable)
}

fn raw_triggers(db: &Connection) -> Result<Vec<(String, String)>, LedgerError> {
    db.prepare("SELECT name,sql FROM main.sqlite_master WHERE type='trigger' AND tbl_name='model_observations' ORDER BY name LIMIT 8")
        .map_err(unavailable)?.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .map_err(unavailable)?.collect::<Result<Vec<_>,_>>().map_err(invalid)
}

fn query_triggers() -> Vec<(String, String)> {
    let mut expected = crate::observation::sql::raw_tracking::TRIGGERS
        .iter()
        .map(|(name, sql)| (name.to_string(), sql.to_string()))
        .collect::<Vec<_>>();
    expected.sort_by(|a, b| a.0.cmp(&b.0));
    expected
}

/// Upgrade only a genuine legacy scope with no version artifacts. Version two
/// is a durable requirement: missing metadata is never treated as first install.
pub(super) fn prepare(
    tx: &Transaction<'_>,
    store_id: &str,
    namespace: &str,
) -> Result<Marker, LedgerError> {
    raw_schema(tx)?;
    let version: i64 = tx
        .query_row(
            "SELECT version FROM main.retention_fact_scope WHERE id=1",
            [],
            |r| r.get(0),
        )
        .map_err(invalid)?;
    if version == 1 {
        if artifacts_exist(tx)? {
            return Err(LedgerError::InvalidRecord);
        }
        let found = raw_triggers(tx)?;
        let scope_trigger: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM main.sqlite_master WHERE type='trigger' AND tbl_name='retention_fact_scope')", [], |r|r.get(0)).map_err(unavailable)?;
        if scope_trigger || (!found.is_empty() && found != query_triggers()) {
            return Err(LedgerError::InvalidRecord);
        }
        let lineage = uuid::Uuid::new_v4().to_string();
        tx.execute_batch("CREATE TABLE retention_source_clock(id INTEGER PRIMARY KEY CHECK(id=1),format INTEGER NOT NULL,store_id TEXT NOT NULL,namespace TEXT NOT NULL,lineage TEXT NOT NULL,nonce BLOB NOT NULL,identity_digest TEXT NOT NULL);").map_err(unavailable)?;
        if tx
            .execute(
                "INSERT INTO retention_source_clock VALUES(1,1,?1,?2,?3,randomblob(16),?4)",
                params![
                    store_id,
                    namespace,
                    lineage,
                    binding(store_id, namespace, &lineage)
                ],
            )
            .map_err(unavailable)?
            != 1
        {
            return Err(LedgerError::Unavailable);
        }
        for (_, sql) in triggers(store_id, namespace, &lineage) {
            tx.execute_batch(&sql).map_err(unavailable)?;
        }
        if tx.execute("UPDATE main.retention_fact_scope SET version=2,identity_digest=?1 WHERE id=1 AND version=1", [scope_digest_v2(store_id,namespace)]).map_err(unavailable)?!=1 {
            return Err(LedgerError::Unavailable);
        }
    } else if version != 2 {
        return Err(LedgerError::UnsupportedVersion);
    }
    read(tx, store_id, namespace)
}

pub(super) fn read(
    db: &Connection,
    store_id: &str,
    namespace: &str,
) -> Result<Marker, LedgerError> {
    raw_schema(db)?;
    content_epoch::table(db, "retention_fact_scope", SCOPE_COLUMNS).map_err(schema_error)?;
    content_epoch::table(db, TABLE, COLUMNS).map_err(schema_error)?;
    let version: i64 = db
        .query_row(
            "SELECT version FROM main.retention_fact_scope WHERE id=1",
            [],
            |r| r.get(0),
        )
        .map_err(invalid)?;
    if version != 2 {
        return Err(if version > 2 {
            LedgerError::UnsupportedVersion
        } else {
            LedgerError::InvalidRecord
        });
    }
    let row: (i64,i64,String,String,String,Vec<u8>,String) = db.query_row("SELECT (SELECT count(*) FROM main.retention_source_clock),format,
        CASE WHEN typeof(store_id)='text' AND octet_length(store_id)=36 THEN store_id END,
        CASE WHEN typeof(namespace)='text' AND octet_length(namespace)=36 THEN namespace END,
        CASE WHEN typeof(lineage)='text' AND octet_length(lineage)=36 THEN lineage END,
        CASE WHEN typeof(nonce)='blob' AND length(nonce)=16 THEN nonce END,
        CASE WHEN typeof(identity_digest)='text' AND octet_length(identity_digest)=64 THEN identity_digest END
        FROM main.retention_source_clock WHERE id=1",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?))).map_err(invalid)?;
    let (count, format, owner, scope, lineage, nonce, identity) = row;
    if format != 1 {
        return Err(LedgerError::UnsupportedVersion);
    }
    if count != 1
        || owner != store_id
        || scope != namespace
        || !valid_uuid(&lineage)
        || identity != binding(store_id, namespace, &lineage)
    {
        return Err(LedgerError::InvalidRecord);
    }
    let mut found = raw_triggers(db)?;
    let mut expected = triggers(store_id, namespace, &lineage);
    if found
        .iter()
        .any(|(name, _)| name.starts_with("observation_query_"))
    {
        expected.extend(query_triggers());
    }
    expected.sort_by(|a, b| a.0.cmp(&b.0));
    found.sort_by(|a, b| a.0.cmp(&b.0));
    let extra:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM main.sqlite_master WHERE type='trigger' AND tbl_name IN ('retention_source_clock','retention_fact_scope')) OR EXISTS(SELECT 1 FROM temp.sqlite_master WHERE type='trigger' AND tbl_name IN ('model_observations','retention_source_clock','retention_fact_scope'))",[],|r|r.get(0)).map_err(unavailable)?;
    if found != expected || extra || super::identity(db, store_id)? != namespace {
        return Err(LedgerError::InvalidRecord);
    }
    Ok(Marker {
        lineage,
        nonce: nonce.try_into().map_err(|_| LedgerError::InvalidRecord)?,
    })
}

#[cfg(test)]
mod tests;
