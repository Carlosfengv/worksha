//! Surviving version bindings or publications never establish current consent.
//! They can establish that a missing control is unknown rather than unbound.
use super::*;
use std::{collections::HashSet, os::unix::fs::MetadataExt};

pub(super) fn database(data: &Path) -> Result<Connection> {
    for suffix in ["", "-wal", "-shm"] {
        match std::fs::symlink_metadata(data.join(format!("routing-policy.sqlite3{suffix}"))) {
            Ok(m) if m.is_file() && !m.file_type().is_symlink() && m.nlink() == 1 => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && !suffix.is_empty() => {}
            _ => return Err(fail("invalid_record")),
        }
    }
    let db = Connection::open_with_flags(
        data.join("routing-policy.sqlite3"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .checked("invalid_record")?;
    db.busy_timeout(std::time::Duration::from_millis(25))
        .checked("unavailable")?;
    let format: i64 = db
        .query_row("SELECT format FROM policy_meta", [], |r| r.get(0))
        .checked("invalid_record")?;
    if format != 1 {
        return Err(fail("invalid_record"));
    }
    Ok(db)
}
pub(super) fn missing_controls(data: &Path, known: &HashSet<&str>, presence: &mut Presence) {
    let Ok(db) = database(data) else {
        return;
    };
    // Keys use the existing covering index. Bound the number and size of keys;
    // do not deserialize every historical binding or publication request.
    let bindings = (|| -> Result<Vec<Option<String>>> {
        let mut query = db.prepare("SELECT CASE WHEN length(scope)<=128 THEN scope END FROM policy_bindings GROUP BY scope LIMIT 257").checked("invalid_record")?;
        query
            .query_map([], |r| r.get(0))
            .checked("invalid_record")?
            .collect::<std::result::Result<Vec<_>, _>>()
            .checked("invalid_record")
    })();
    let intents = (|| -> Result<Vec<Option<String>>> {
        let mut query = db.prepare("SELECT CASE WHEN length(record)<=2097152 AND json_valid(record) THEN CASE WHEN json_type(record,'$.status.storeId')='text' AND json_type(record,'$.status.serviceId')='text' AND length(json_extract(record,'$.status.storeId'))<=64 AND length(json_extract(record,'$.status.serviceId'))<=64 THEN json_extract(record,'$.status.storeId')||':'||json_extract(record,'$.status.serviceId') END END FROM policy_intents LIMIT 257").checked("invalid_record")?;
        query
            .query_map([], |r| r.get(0))
            .checked("invalid_record")?
            .collect::<std::result::Result<Vec<_>, _>>()
            .checked("invalid_record")
    })();
    // Damaged optional evidence cannot manufacture identities. Preserve the
    // original startup checks and every independently identifiable scope.
    for rows in [bindings, intents].into_iter().flatten() {
        if rows.len() > 256 {
            presence.unavailable = true;
        }
        for scope in rows.into_iter().flatten() {
            if known.contains(scope.as_str()) {
                continue;
            }
            let Some((store, service)) = scope.split_once(':') else {
                continue;
            };
            if uuid::Uuid::parse_str(store).is_ok() && uuid::Uuid::parse_str(service).is_ok() {
                presence.mark_unknown(store, service);
            }
        }
    }
}
