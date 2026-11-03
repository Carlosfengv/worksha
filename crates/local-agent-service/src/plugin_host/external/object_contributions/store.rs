//! Separate metadata grants. Missing data grants no authority; damaged files are retained.
use super::super::{Checked, Result, fail, package};
use local_agent_core::{domain::Decimal, plugin::object_contributions as c};
use rusqlite::{Connection, OpenFlags, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, OpenOptions},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    time::{Duration, UNIX_EPOCH},
};

const MAX_DATABASE_BYTES: u64 = 8 * 1024 * 1024;
const MAX_GRANTS: i64 = 64 * 16;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Owner {
    pub slot: c::ObjectSlotQuery,
    pub descriptor_identity: String,
    pub activation_epoch: Decimal,
    pub grant_revision: Decimal,
}
impl Owner {
    fn validate(&self) -> Result<()> {
        self.slot.validate().checked("invalid_input")?;
        package::unhex::<32>(&self.descriptor_identity)?;
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Record {
    pub owner: Owner,
    pub legacy_file_scope: String,
    pub own_file_scope: String,
    pub revision: String,
    pub authorized: bool,
}
impl Record {
    fn validate(&self, slot: &c::ObjectSlotQuery) -> Result<()> {
        self.owner.validate().checked("invalid_record")?;
        if self.owner.slot.plugin_id != slot.plugin_id || self.owner.slot.slot_id != slot.slot_id {
            return Err(fail("invalid_record"));
        }
        package::unhex::<32>(&self.legacy_file_scope).checked("invalid_record")?;
        package::unhex::<32>(&self.own_file_scope).checked("invalid_record")?;
        uuid::Uuid::parse_str(&self.revision).checked("invalid_record")?;
        Ok(())
    }
}
#[derive(Clone, Serialize)]
struct Scopes {
    legacy: String,
    own: Option<String>,
    instance: String,
}
pub(super) struct Status {
    pub context_identity: String,
    pub authorized: bool,
}
pub(super) struct Store {
    path: PathBuf,
    legacy: PathBuf,
    legacy_scope: Option<String>,
    own_scope: Option<String>,
    instance: String,
}
fn file_scope(path: &Path) -> Result<Option<String>> {
    let meta = match fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(fail("unavailable")),
    };
    if !meta.is_file() || meta.file_type().is_symlink() || meta.nlink() != 1 {
        return Err(fail("invalid_record"));
    }
    let born = meta
        .created()
        .checked("unavailable")?
        .duration_since(UNIX_EPOCH)
        .checked("invalid_record")?
        .as_nanos()
        .to_string();
    Ok(Some(package::hash(
        &serde_json::to_vec(&(meta.dev(), meta.ino(), born)).checked("invalid_record")?,
    )))
}
fn sql<T>(result: rusqlite::Result<T>) -> Result<T> {
    result.map_err(|e| match e {
        rusqlite::Error::SqliteFailure(e, _)
            if matches!(
                e.code,
                rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked
            ) =>
        {
            fail("busy")
        }
        _ => fail("invalid_record"),
    })
}
impl Store {
    /// Construction performs no writes and cannot disable the original plugin host.
    pub(super) fn new(data: &Path) -> Self {
        let resolved = fs::canonicalize(data).unwrap_or_else(|_| data.to_path_buf());
        let data = resolved.as_path();
        let legacy = data.join("plugin-host.sqlite3");
        let path = data.join("object-contributions.sqlite3");
        Self {
            legacy_scope: file_scope(&legacy).ok().flatten(),
            own_scope: file_scope(&path).ok().flatten(),
            legacy,
            path,
            instance: uuid::Uuid::new_v4().to_string(),
        }
    }
    fn scopes(&mut self) -> Result<Scopes> {
        let current = file_scope(&self.legacy)?.ok_or(fail("invalid_record"))?;
        if self.legacy_scope.as_ref() != Some(&current) {
            return Err(fail("invalid_record"));
        }
        let own = file_scope(&self.path)?;
        if self
            .own_scope
            .as_ref()
            .is_some_and(|previous| own.as_ref() != Some(previous))
        {
            return Err(fail("invalid_record"));
        }
        if let Some(scope) = &own {
            if fs::metadata(&self.path).checked("unavailable")?.len() > MAX_DATABASE_BYTES {
                return Err(fail("resource_limit"));
            }
            self.own_scope = Some(scope.clone());
        }
        for suffix in ["-wal", "-shm"] {
            let mut name = self.path.as_os_str().to_os_string();
            name.push(suffix);
            match fs::symlink_metadata(PathBuf::from(name)) {
                Ok(meta) => {
                    if !meta.is_file() || meta.file_type().is_symlink() || meta.nlink() != 1 {
                        return Err(fail("invalid_record"));
                    }
                    if meta.len() > MAX_DATABASE_BYTES * 2 {
                        return Err(fail("resource_limit"));
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => return Err(fail("unavailable")),
            }
        }
        Ok(Scopes {
            legacy: current,
            own,
            instance: self.instance.clone(),
        })
    }
    fn connection(&mut self, create: bool) -> Result<Option<Connection>> {
        let before = self.scopes()?;
        if before.own.is_none() && !create {
            return Ok(None);
        }
        let fresh = before.own.is_none();
        if fresh {
            OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&self.path)
                .map_err(|e| {
                    if e.kind() == std::io::ErrorKind::AlreadyExists {
                        fail("stale_snapshot")
                    } else {
                        fail("unavailable")
                    }
                })?;
            self.own_scope = file_scope(&self.path)?;
        }
        let db = sql(Connection::open_with_flags(
            &self.path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        ))?;
        sql(db.busy_timeout(Duration::from_millis(25)))?;
        if fresh {
            sql(db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; BEGIN IMMEDIATE; CREATE TABLE object_grants(plugin_id TEXT NOT NULL,slot_id TEXT NOT NULL,record TEXT NOT NULL,PRIMARY KEY(plugin_id,slot_id)); PRAGMA user_version=1; COMMIT;"))?;
        } else {
            let version: i64 = sql(db.pragma_query_value(None, "user_version", |r| r.get(0)))?;
            if version != 1 {
                return Err(fail("invalid_record"));
            }
            let mode: String = sql(db.pragma_query_value(None, "journal_mode", |r| r.get(0)))?;
            if mode != "wal" {
                return Err(fail("invalid_record"));
            }
            sql(db.pragma_update(None, "synchronous", "FULL"))?;
        }
        self.scopes()?;
        Ok(Some(db))
    }
    fn read(db: &Connection, slot: &c::ObjectSlotQuery) -> Result<Option<Record>> {
        let mut stmt=sql(db.prepare("SELECT CASE WHEN length(CAST(record AS BLOB))<=4096 THEN record ELSE NULL END FROM object_grants WHERE plugin_id=?1 AND slot_id=?2 LIMIT 2"))?;
        let rows = sql(stmt.query_map(params![slot.plugin_id, slot.slot_id], |r| {
            r.get::<_, Option<String>>(0)
        }))?;
        let rows = sql(rows.collect::<rusqlite::Result<Vec<_>>>())?;
        if rows.len() > 1 {
            return Err(fail("invalid_record"));
        }
        let Some(row) = rows.into_iter().next() else {
            return Ok(None);
        };
        let row = row.ok_or(fail("invalid_record"))?;
        let record: Record = serde_json::from_str(&row).checked("invalid_record")?;
        record.validate(slot)?;
        Ok(Some(record))
    }
    pub(super) fn get(&mut self, slot: &c::ObjectSlotQuery) -> Result<Option<Record>> {
        slot.validate().checked("invalid_input")?;
        let Some(db) = self.connection(false)? else {
            return Ok(None);
        };
        sql(db.pragma_update(None, "query_only", true))?;
        let record = Self::read(&db, slot)?;
        self.scopes()?;
        Ok(record)
    }
    fn identity(owner: &Owner, scopes: &Scopes, record: Option<&Record>) -> Result<String> {
        let value = serde_json::json!({"owner":owner,"scopes":scopes,"record":record});
        Ok(package::hash(
            &serde_json::to_vec(&value).checked("invalid_record")?,
        ))
    }
    pub(super) fn status(&mut self, owner: &Owner) -> Result<Status> {
        owner.validate()?;
        let record = self.get(&owner.slot)?;
        let scopes = self.scopes()?;
        let authorized = record.as_ref().is_some_and(|r| {
            r.authorized
                && r.owner == *owner
                && r.legacy_file_scope == scopes.legacy
                && Some(&r.own_file_scope) == scopes.own.as_ref()
        });
        Ok(Status {
            context_identity: Self::identity(owner, &scopes, record.as_ref())?,
            authorized,
        })
    }
    /// A revoke writes a new tombstone, including when no grant was present.
    pub(super) fn apply(
        &mut self,
        owner: Owner,
        expected_context: &str,
        authorized: bool,
    ) -> Result<Record> {
        owner.validate()?;
        let old = self.get(&owner.slot)?;
        let before = self.scopes()?;
        if Self::identity(&owner, &before, old.as_ref())? != expected_context {
            return Err(fail("stale_snapshot"));
        }
        let mut db = self.connection(true)?.ok_or(fail("unavailable"))?;
        let scopes = self.scopes()?;
        let tx = sql(db.transaction_with_behavior(TransactionBehavior::Immediate))?;
        let current = Self::read(&tx, &owner.slot)?;
        if current.as_ref().map(|r| &r.revision) != old.as_ref().map(|r| &r.revision) {
            return Err(fail("stale_snapshot"));
        }
        if current.is_none()
            && sql(tx.query_row("SELECT count(*) FROM object_grants", [], |r| {
                r.get::<_, i64>(0)
            }))? >= MAX_GRANTS
        {
            return Err(fail("resource_limit"));
        }
        let record = Record {
            owner,
            legacy_file_scope: scopes.legacy,
            own_file_scope: scopes.own.ok_or(fail("invalid_record"))?,
            revision: uuid::Uuid::new_v4().to_string(),
            authorized,
        };
        record.validate(&record.owner.slot)?;
        sql(tx.execute("INSERT INTO object_grants VALUES(?1,?2,?3) ON CONFLICT(plugin_id,slot_id) DO UPDATE SET record=excluded.record",params![record.owner.slot.plugin_id,record.owner.slot.slot_id,serde_json::to_string(&record).checked("invalid_record")?]))?;
        self.scopes()?;
        sql(tx.commit())?;
        Ok(record)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn root() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("plugin-host.sqlite3"),
            b"owned legacy identity",
        )
        .unwrap();
        dir
    }
    fn owner() -> Owner {
        Owner {
            slot: c::ObjectSlotQuery {
                plugin_id: "org.example.details".into(),
                digest: "a".repeat(64),
                slot_id: "usage".into(),
            },
            descriptor_identity: "b".repeat(64),
            activation_epoch: 1_u64.into(),
            grant_revision: 1_u64.into(),
        }
    }
    #[test]
    fn missing_store_grants_nothing_and_revoke_prevents_late_grant_or_authority_reuse() {
        let root = root();
        let mut s = Store::new(root.path());
        let owner = owner();
        let first = s.status(&owner).unwrap();
        assert!(!first.authorized);
        assert!(!s.path.exists());
        s.apply(owner.clone(), &first.context_identity, true)
            .unwrap();
        let granted = s.status(&owner).unwrap();
        assert!(granted.authorized);
        s.apply(owner.clone(), &granted.context_identity, false)
            .unwrap();
        assert_eq!(
            s.apply(owner.clone(), &granted.context_identity, true)
                .unwrap_err()
                .code,
            "stale_snapshot"
        );
        assert!(!s.status(&owner).unwrap().authorized);
        let mut next = owner.clone();
        next.grant_revision = 2_u64.into();
        assert!(!s.status(&next).unwrap().authorized);
        let mut next = owner;
        next.slot.digest = "c".repeat(64);
        assert!(!s.status(&next).unwrap().authorized);
    }
    #[test]
    fn normal_writes_keep_file_authority_but_database_replacement_and_restart_do_not_restore_it() {
        let root = root();
        let mut s = Store::new(root.path());
        let owner = owner();
        let ctx = s.status(&owner).unwrap().context_identity;
        s.apply(owner.clone(), &ctx, true).unwrap();
        fs::write(&s.legacy, b"ordinary legacy metadata write").unwrap();
        assert!(s.status(&owner).unwrap().authorized);
        let mut reopened = Store::new(root.path());
        assert!(reopened.status(&owner).unwrap().authorized);
        assert_eq!(
            reopened.apply(owner.clone(), &ctx, true).unwrap_err().code,
            "stale_snapshot"
        );
        fs::rename(&s.legacy, root.path().join("legacy-retained")).unwrap();
        fs::write(&s.legacy, b"replacement legacy database").unwrap();
        assert_eq!(s.status(&owner).err().unwrap().code, "invalid_record");
        let mut after = Store::new(root.path());
        assert!(!after.status(&owner).unwrap().authorized);
    }
    #[test]
    fn future_or_malformed_data_is_preserved_and_scoped_records_cannot_redirect_grants() {
        let root = root();
        let path = root.path().join("object-contributions.sqlite3");
        let db = Connection::open(&path).unwrap();
        db.pragma_update(None, "user_version", 7).unwrap();
        drop(db);
        let before = fs::read(&path).unwrap();
        let mut s = Store::new(root.path());
        assert_eq!(s.status(&owner()).err().unwrap().code, "invalid_record");
        assert_eq!(fs::read(&path).unwrap(), before);
        fs::rename(&path, root.path().join("future-retained")).unwrap();
        let mut s = Store::new(root.path());
        let owner = owner();
        let ctx = s.status(&owner).unwrap().context_identity;
        s.apply(owner.clone(), &ctx, true).unwrap();
        let db = Connection::open(&path).unwrap();
        let raw: String = db
            .query_row("SELECT record FROM object_grants", [], |r| r.get(0))
            .unwrap();
        let mut record: Record = serde_json::from_str(&raw).unwrap();
        record.owner.slot.plugin_id = "org.other.details".into();
        db.execute(
            "UPDATE object_grants SET record=?1",
            [serde_json::to_string(&record).unwrap()],
        )
        .unwrap();
        drop(db);
        assert_eq!(s.status(&owner).err().unwrap().code, "invalid_record");
    }
    #[test]
    fn revoking_a_missing_grant_fences_an_older_initial_confirmation() {
        let root = root();
        let mut s = Store::new(root.path());
        let owner = owner();
        let ctx = s.status(&owner).unwrap().context_identity;
        s.apply(owner.clone(), &ctx, false).unwrap();
        assert!(!s.status(&owner).unwrap().authorized);
        assert_eq!(
            s.apply(owner, &ctx, true).unwrap_err().code,
            "stale_snapshot"
        );
    }
    #[test]
    fn capacity_rejects_new_grants_but_existing_revoke_and_metadata_reads_remain_usable() {
        let root = root();
        let mut s = Store::new(root.path());
        let owner = owner();
        let ctx = s.status(&owner).unwrap().context_identity;
        let record = s.apply(owner.clone(), &ctx, true).unwrap();
        let mut db = Connection::open(&s.path).unwrap();
        let tx = db.transaction().unwrap();
        for i in 1..MAX_GRANTS {
            let mut row = record.clone();
            row.owner.slot.slot_id = format!("slot{i}");
            tx.execute(
                "INSERT INTO object_grants VALUES(?1,?2,?3)",
                params![
                    row.owner.slot.plugin_id,
                    row.owner.slot.slot_id,
                    serde_json::to_string(&row).unwrap()
                ],
            )
            .unwrap();
        }
        tx.commit().unwrap();
        drop(db);
        let mut extra = owner.clone();
        extra.slot.slot_id = "overflow".into();
        let ctx = s.status(&extra).unwrap().context_identity;
        assert_eq!(
            s.apply(extra, &ctx, true).unwrap_err().code,
            "resource_limit"
        );
        let ctx = s.status(&owner).unwrap().context_identity;
        s.apply(owner.clone(), &ctx, false).unwrap();
        assert!(!s.status(&owner).unwrap().authorized);
    }
    #[test]
    fn linked_files_are_never_opened_as_authority_or_reset() {
        let root = root();
        let target = root.path().join("retained-data");
        fs::write(&target, b"keep this").unwrap();
        std::os::unix::fs::symlink(&target, root.path().join("object-contributions.sqlite3"))
            .unwrap();
        let mut s = Store::new(root.path());
        assert_eq!(s.status(&owner()).err().unwrap().code, "invalid_record");
        assert_eq!(fs::read(&target).unwrap(), b"keep this");
        fs::remove_file(root.path().join("object-contributions.sqlite3")).unwrap();
        fs::hard_link(
            root.path().join("plugin-host.sqlite3"),
            root.path().join("legacy-link"),
        )
        .unwrap();
        let mut s = Store::new(root.path());
        assert_eq!(s.status(&owner()).err().unwrap().code, "invalid_record");
    }
}
