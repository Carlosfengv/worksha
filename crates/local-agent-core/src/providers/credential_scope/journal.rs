//! The configuration database owns recovery metadata; Vault values stay outside it.
use super::super::{ConfigError, ConfigTransactionGuard, fail, files, operations::valid_id};
use rusqlite::{
    Connection, OpenFlags, OptionalExtension, Transaction, TransactionBehavior, params,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    sync::Arc,
};
mod legacy_admission;

const MAX_REVISION: i64 = 9_007_199_254_740_991;
const MAX_VALUES: i64 = 2 * 4097;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NamespaceState {
    Active,
    Unverified,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LegacyState {
    Fresh,
    Imported,
    Unverified,
}
#[derive(Clone)]
pub struct Metadata {
    pub store_id: String,
    pub namespace_id: String,
    pub state: NamespaceState,
    pub legacy: LegacyState,
    count: i64,
    revision: i64,
}
pub(crate) struct Journal {
    path: PathBuf,
    file: Arc<fs::File>,
    store_id: String,
    namespace_id: String,
    domain: u32,
}
fn corrupt() -> ConfigError {
    ConfigError::new(
        "credential_recovery_corrupt",
        "凭据恢复账本不完整或格式不可核查；原数据已保留，未重新创建凭据",
    )
}
fn unavailable(_: impl std::fmt::Display) -> ConfigError {
    ConfigError::new(
        "credential_recovery_unavailable",
        "凭据恢复账本暂不可用，请保留原操作后核查",
    )
}
fn plugin_storage_error(message: String) -> ConfigError {
    ConfigError::new("plugin_storage_unavailable", &message)
}
pub(crate) fn unverified() -> ConfigError {
    ConfigError::new(
        "credential_namespace_unverified",
        "凭据恢复来源尚未核查，请恢复原材料或明确建立新的恢复空间；未发送密钥",
    )
}
pub(super) fn lock_at(path: &Path) -> Result<ConfigTransactionGuard, ConfigError> {
    files::checked_path(path)?;
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(unavailable)?;
    let m = file.metadata().map_err(unavailable)?;
    if !m.is_file()
        || m.nlink() != 1
        || m.uid() != unsafe { libc::geteuid() }
        || m.mode() & 0o777 != 0o600
    {
        return Err(corrupt());
    }
    fs2::FileExt::try_lock_exclusive(&file)
        .map_err(|_| ConfigError::new("busy", "另一个凭据恢复任务正在处理，请稍后核查"))?;
    Ok(ConfigTransactionGuard { file })
}
fn metadata(db: &Connection) -> Result<Metadata, ConfigError> {
    let version: u32 = db
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(unavailable)?;
    if !matches!(
        version,
        crate::Store::SCHEMA_VERSION | crate::iam::PROTECTED_SCHEMA_VERSION
    ) {
        return Err(corrupt());
    }
    metadata_at(db, version)
}
fn metadata_at(db: &Connection, expected_schema: u32) -> Result<Metadata, ConfigError> {
    let version: u32 = db
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .map_err(unavailable)?;
    if version != expected_schema {
        return Err(corrupt());
    }
    let row:(u32,String,String,String,String,i64,i64)=db.query_row("SELECT format,store_id,namespace_id,state,legacy_state,value_count,revision FROM credential_recovery_scope WHERE id=1",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?))).map_err(|_|corrupt())?;
    let actual: String = db
        .query_row(
            "SELECT store_id FROM model_service_operation_scope WHERE id=1",
            [],
            |r| r.get(0),
        )
        .map_err(|_| corrupt())?;
    let count: i64 = db
        .query_row("SELECT count(*) FROM credential_recovery_values", [], |r| {
            r.get(0)
        })
        .map_err(|_| corrupt())?;
    if row.0 != 1
        || !valid_id(&row.1)
        || row.1 != actual
        || !valid_id(&row.2)
        || row.5 != count
        || !(0..=MAX_VALUES).contains(&count)
        || !(0..=MAX_REVISION).contains(&row.6)
    {
        return Err(corrupt());
    }
    let state = match row.3.as_str() {
        "active" => NamespaceState::Active,
        "unverified" => NamespaceState::Unverified,
        _ => return Err(corrupt()),
    };
    let legacy = match row.4.as_str() {
        "fresh" => LegacyState::Fresh,
        "imported" => LegacyState::Imported,
        "unverified" => LegacyState::Unverified,
        _ => return Err(corrupt()),
    };
    let invalid:i64=db.query_row("SELECT count(*) FROM credential_recovery_values WHERE domain NOT IN(2,3) OR length(value)>4096 OR typeof(value)!='blob' OR length(value_hash)!=64 OR revision<1 OR revision>?1",[row.6],|r|r.get(0)).map_err(|_|corrupt())?;
    let new_count: i64 = db
        .query_row(
            "SELECT count(*) FROM credential_recovery_values WHERE domain=3",
            [],
            |r| r.get(0),
        )
        .map_err(|_| corrupt())?;
    if invalid != 0 || matches!(state, NamespaceState::Unverified) && new_count != 0 {
        return Err(corrupt());
    }
    Ok(Metadata {
        store_id: row.1,
        namespace_id: row.2,
        state,
        legacy,
        count,
        revision: row.6,
    })
}
fn valid_name(name: &str) -> bool {
    name == "protection-key.json"
        || name
            .strip_prefix("operations/")
            .and_then(|s| s.strip_suffix(".json"))
            .is_some_and(valid_id)
}
impl Journal {
    pub(crate) fn existing_for_legacy(data: &Path) -> Result<Option<Arc<Self>>, ConfigError> {
        let path = data.join("local-agent.sqlite3");
        match fs::symlink_metadata(&path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(unavailable(e)),
            Ok(_) => (),
        }
        files::checked_path(&path)?;
        let mut db = Connection::open_with_flags(
            &path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(unavailable)?;
        db.busy_timeout(std::time::Duration::from_secs(3))
            .map_err(unavailable)?;
        let tx = db.transaction().map_err(unavailable)?;
        let meta = metadata(&tx)?;
        Self::from_store(&tx, &path, &meta.store_id).map(Some)
    }
    pub(crate) fn same_source(&self, other: &Self) -> Result<bool, ConfigError> {
        self.check_file()?;
        other.check_file()?;
        Ok(self.path == other.path
            && self.store_id == other.store_id
            && self.namespace_id == other.namespace_id)
    }
    pub(crate) fn from_store(
        db: &Connection,
        path: &Path,
        store_id: &str,
    ) -> Result<Arc<Self>, ConfigError> {
        let meta = metadata(db)?;
        if meta.store_id != store_id {
            return Err(corrupt());
        }
        let path = path.canonicalize().map_err(unavailable)?;
        files::checked_path(&path)?;
        let file = Arc::new(
            fs::OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW)
                .open(&path)
                .map_err(unavailable)?,
        );
        let result = Arc::new(Self {
            path,
            file,
            store_id: store_id.into(),
            namespace_id: meta.namespace_id,
            domain: 3,
        });
        result.status()?;
        Ok(result)
    }
    pub(crate) fn domain(&self, domain: u32) -> Arc<Self> {
        Arc::new(Self {
            path: self.path.clone(),
            file: self.file.clone(),
            store_id: self.store_id.clone(),
            namespace_id: self.namespace_id.clone(),
            domain,
        })
    }
    pub(crate) fn namespace_id(&self) -> &str {
        &self.namespace_id
    }
    fn check_file(&self) -> Result<(), ConfigError> {
        let held = self.file.metadata().map_err(unavailable)?;
        let now = fs::symlink_metadata(&self.path).map_err(unavailable)?;
        if !held.is_file()
            || !now.is_file()
            || held.dev() != now.dev()
            || held.ino() != now.ino()
            || now.nlink() != 1
            || now.uid() != unsafe { libc::geteuid() }
        {
            return Err(corrupt());
        }
        Ok(())
    }
    fn connect(&self) -> Result<Connection, ConfigError> {
        self.check_file()?;
        let db = Connection::open_with_flags(
            &self.path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(unavailable)?;
        db.busy_timeout(std::time::Duration::from_secs(3))
            .map_err(unavailable)?;
        self.check_file()?;
        Ok(db)
    }
    fn validate(&self, db: &Connection) -> Result<Metadata, ConfigError> {
        let mut meta = metadata(db)?;
        if meta.store_id != self.store_id || meta.namespace_id != self.namespace_id {
            return Err(ConfigError::new(
                "credential_namespace_changed",
                "凭据恢复空间已变化，请保留原空间和编号核查",
            ));
        }
        if self.domain == 4 {
            let (count, revision) = super::super::local_tokens::crypto_metadata(
                db,
                &meta.store_id,
                &meta.namespace_id,
            )?;
            meta.count = count;
            meta.revision = revision;
        }
        Ok(meta)
    }
    fn allowed(&self, meta: &Metadata) -> Result<(), ConfigError> {
        if matches!(self.domain, 3 | 4) && !matches!(meta.state, NamespaceState::Active)
            || self.domain == 2 && matches!(meta.legacy, LegacyState::Unverified)
        {
            return Err(unverified());
        }
        Ok(())
    }
    pub(crate) fn status(&self) -> Result<Metadata, ConfigError> {
        let mut db = self.connect()?;
        let tx = db.transaction().map_err(unavailable)?;
        let meta = self.validate(&tx)?;
        tx.commit().map_err(unavailable)?;
        self.check_file()?;
        Ok(meta)
    }
    pub(crate) fn discovery_inventory(&self) -> Result<(i64, Vec<(u32, String)>), ConfigError> {
        let mut db = self.connect()?;
        let tx = db.transaction().map_err(unavailable)?;
        let meta = self.validate(&tx)?;
        self.allowed(&meta)?;
        let mut rows = {
            let mut query = tx.prepare("SELECT domain,substr(name,1,129) FROM credential_recovery_values WHERE name!='protection-key.json' ORDER BY domain,name LIMIT 8195").map_err(unavailable)?;
            query
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
                .map_err(unavailable)?
                .collect::<Result<Vec<(u32, String)>, _>>()
                .map_err(unavailable)?
        };
        if rows.len() > MAX_VALUES as usize {
            return Err(corrupt());
        }
        let admission = legacy_admission::validate(&tx, &meta.store_id)?;
        rows.extend(
            admission
                .ids
                .into_iter()
                .map(|id| (1, format!("operations/{id}.json"))),
        );
        tx.commit().map_err(unavailable)?;
        self.check_file()?;
        Ok((meta.revision, rows))
    }
    pub(crate) fn allow_new(&self) -> Result<bool, ConfigError> {
        let meta = self.status()?;
        self.allowed(&meta)?;
        Ok(matches!(self.domain, 3 | 4) || matches!(meta.legacy, LegacyState::Fresh))
    }
    pub(crate) fn lock(&self) -> Result<ConfigTransactionGuard, ConfigError> {
        self.check_file()?;
        lock_at(&self.path.with_file_name("credential-recovery.lock"))
    }
    pub(crate) fn local_owner_lock(&self) -> Result<ConfigTransactionGuard, ConfigError> {
        self.check_file()?;
        lock_at(&self.path.with_file_name("local-token-operations.lock"))
    }
    fn value_table(&self) -> &'static str {
        if self.domain == 4 {
            "local_token_values"
        } else {
            "credential_recovery_values"
        }
    }
    fn scope_table(&self) -> &'static str {
        if self.domain == 4 {
            "local_token_scope"
        } else {
            "credential_recovery_scope"
        }
    }
    pub(crate) fn reference(&self, id: &str) -> String {
        let digest = Sha256::digest(
            serde_json::to_vec(&(
                "credential-reference-v3",
                &self.store_id,
                &self.namespace_id,
                self.domain,
                id,
            ))
            .expect("string tuple serializes"),
        );
        let mut bytes: [u8; 16] = digest[..16].try_into().expect("digest has sixteen bytes");
        bytes[6] = (bytes[6] & 15) | 64;
        bytes[8] = (bytes[8] & 63) | 128;
        uuid::Uuid::from_bytes(bytes).to_string()
    }
    pub(crate) fn read(&self, name: &str) -> Result<Option<Vec<u8>>, ConfigError> {
        if !valid_name(name) {
            return Err(corrupt());
        }
        let mut db = self.connect()?;
        let tx = db.transaction().map_err(unavailable)?;
        self.allowed(&self.validate(&tx)?)?;
        let row: Option<(Vec<u8>, String)> = tx
            .query_row(
                &format!(
                    "SELECT substr(value,1,4097),value_hash FROM {} WHERE domain=?1 AND name=?2",
                    self.value_table()
                ),
                params![self.domain, name],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .map_err(unavailable)?;
        let bytes = row
            .map(|(value, hash)| {
                if value.len() > 4096 || files::hash(&value) != hash {
                    Err(corrupt())
                } else {
                    Ok(value)
                }
            })
            .transpose()?;
        tx.commit().map_err(unavailable)?;
        self.check_file()?;
        Ok(bytes)
    }
    pub(crate) fn write(&self, name: &str, value: &[u8]) -> Result<(), ConfigError> {
        if !valid_name(name) || value.len() > 4096 {
            return Err(corrupt());
        }
        let mut db = self.connect()?;
        let tx = db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(unavailable)?;
        let meta = self.validate(&tx)?;
        self.allowed(&meta)?;
        if meta.revision == MAX_REVISION {
            return Err(corrupt());
        }
        let old: Option<(String, i64)> = tx
            .query_row(
                &format!(
                    "SELECT value_hash,revision FROM {} WHERE domain=?1 AND name=?2",
                    self.value_table()
                ),
                params![self.domain, name],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .map_err(unavailable)?;
        if old.is_none() && name.starts_with("operations/") {
            let count: i64 = tx
                .query_row(
                    &format!(
                        "SELECT count(*) FROM {} WHERE domain=?1 AND name GLOB 'operations/*'",
                        self.value_table()
                    ),
                    [self.domain],
                    |r| r.get(0),
                )
                .map_err(unavailable)?;
            if count >= 4096 {
                return fail(
                    "credential_operation_capacity",
                    "凭据操作记录已达上限；保留既有回执和凭据，未创建新凭据",
                );
            }
        }
        let hash = files::hash(value);
        let added = old.is_none();
        let changed=match old {Some((old_hash,revision))=>tx.execute(&format!("UPDATE {} SET value=?1,value_hash=?2,revision=?3 WHERE domain=?4 AND name=?5 AND value_hash=?6 AND revision=?7",self.value_table()),params![value,hash,meta.revision+1,self.domain,name,old_hash,revision]),None=>tx.execute(&format!("INSERT INTO {} VALUES(?1,?2,?3,?4,?5)",self.value_table()),params![self.domain,name,value,hash,meta.revision+1])}.map_err(unavailable)?;
        if changed!=1 || tx.execute(&format!("UPDATE {} SET value_count=?1,revision=revision+1 WHERE id=1 AND revision=?2",self.scope_table()),params![meta.count+i64::from(added),meta.revision]).map_err(unavailable)?!=1{return Err(corrupt());}
        self.validate(&tx)?;
        let saved: (Vec<u8>, String) = tx
            .query_row(
                &format!(
                    "SELECT value,value_hash FROM {} WHERE domain=?1 AND name=?2",
                    self.value_table()
                ),
                params![self.domain, name],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .map_err(unavailable)?;
        if saved.0 != value || saved.1 != hash {
            return Err(corrupt());
        }
        self.check_file()?;
        tx.commit().map_err(unavailable)?;
        self.check_file()
    }
    pub(crate) fn initialize_namespace(&self, restore: bool) -> Result<Metadata, ConfigError> {
        let _guard = self.lock()?;
        let mut db = self.connect()?;
        let tx = db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(unavailable)?;
        let meta = self.validate(&tx)?;
        let archive = if restore && matches!(meta.legacy, LegacyState::Unverified) {
            Some(
                super::archive::read(
                    self.path.parent().expect("database has parent"),
                    &self.store_id,
                )?
                .ok_or_else(unverified)?,
            )
        } else {
            None
        };
        if restore && matches!(meta.legacy, LegacyState::Unverified) {
            let archive = archive.as_ref().expect("restore archive checked above");
            import(&tx, archive, meta.revision + 1)?;
            if tx.execute("UPDATE credential_recovery_scope SET state='active',legacy_state='imported',legacy_digest=?1,value_count=?2,revision=revision+1 WHERE id=1 AND revision=?3",params![archive.digest,meta.count+archive.values.len() as i64,meta.revision]).map_err(unavailable)?!=1{return Err(corrupt());}
        } else if !matches!(meta.state, NamespaceState::Active)
            && tx.execute("UPDATE credential_recovery_scope SET state='active',revision=revision+1 WHERE id=1 AND revision=?1",[meta.revision]).map_err(unavailable)?!=1 {
            return Err(corrupt());
        }
        let result = self.validate(&tx)?;
        self.check_file()?;
        tx.commit().map_err(unavailable)?;
        self.check_file()?;
        Ok(result)
    }
}
fn import(
    db: &Connection,
    archive: &super::archive::Archive,
    revision: i64,
) -> Result<(), ConfigError> {
    for (name, value) in &archive.values {
        if db
            .execute(
                "INSERT INTO credential_recovery_values VALUES(2,?1,?2,?3,?4)",
                params![name, value, files::hash(value), revision],
            )
            .map_err(unavailable)?
            != 1
        {
            return Err(corrupt());
        }
    }
    Ok(())
}
pub(crate) fn initialize(
    db: &Connection,
    path: &Path,
    original_version: u32,
    fresh: bool,
) -> Result<(), ConfigError> {
    if matches!(
        original_version,
        crate::Store::SCHEMA_VERSION | crate::iam::PROTECTED_SCHEMA_VERSION
    ) {
        let tx =
            Transaction::new_unchecked(db, TransactionBehavior::Immediate).map_err(unavailable)?;
        crate::plugin::initialize_composition(&tx, original_version)
            .map_err(plugin_storage_error)?;
        crate::iam::initialize(&tx, original_version)
            .map_err(|error| plugin_storage_error(error.message))?;
        return tx.commit().map_err(unavailable);
    }
    let parent = path
        .parent()
        .and_then(|p| p.canonicalize().ok())
        .ok_or_else(corrupt)?;
    let _guard = lock_at(&parent.join("credential-recovery.lock"))?;
    let tx = Transaction::new_unchecked(db, TransactionBehavior::Immediate).map_err(unavailable)?;
    if original_version == 15 {
        // The plugin reader barrier and its schema publish together. Preserve
        // existing credential evidence and identities without recreating them.
        let meta = metadata_at(&tx, original_version)?;
        legacy_admission::validate(&tx, &meta.store_id)?;
        crate::providers::imports::operations::validate_existing(&tx, &meta.store_id)?;
        super::super::local_tokens::validate_scope(&tx, &meta.store_id, &meta.namespace_id)?;
        crate::plugin::initialize_composition(&tx, original_version)
            .map_err(plugin_storage_error)?;
        crate::iam::initialize(&tx, original_version)
            .map_err(|error| plugin_storage_error(error.message))?;
        tx.pragma_update(None, "user_version", crate::Store::SCHEMA_VERSION)
            .map_err(unavailable)?;
        metadata(&tx)?;
        return tx.commit().map_err(unavailable);
    }
    if matches!(original_version, 10..=14) {
        // The reader barrier changes; existing evidence, identities and row
        // bytes do not. A broken old registry cannot be promoted as known.
        let meta = metadata_at(&tx, original_version)?;
        if original_version < 12 {
            legacy_admission::initialize(&tx, path, &meta.store_id, false)?;
        } else {
            legacy_admission::validate(&tx, &meta.store_id)?;
        }
        if original_version < 13 {
            crate::providers::imports::operations::initialize(&tx, &meta.store_id)?;
        } else {
            crate::providers::imports::operations::validate_existing(&tx, &meta.store_id)?;
        }
        if original_version == 14 {
            super::super::local_tokens::upgrade_previous(&tx, &meta.store_id, &meta.namespace_id)?;
        } else {
            super::super::local_tokens::initialize(&tx, &meta.store_id, &meta.namespace_id)?;
        }
        crate::plugin::initialize_composition(&tx, original_version)
            .map_err(plugin_storage_error)?;
        crate::iam::initialize(&tx, original_version)
            .map_err(|error| plugin_storage_error(error.message))?;
        tx.pragma_update(None, "user_version", crate::Store::SCHEMA_VERSION)
            .map_err(unavailable)?;
        metadata(&tx)?;
        return tx.commit().map_err(unavailable);
    }
    let reserved:i64=tx.query_row("SELECT count(*) FROM sqlite_master WHERE name IN('credential_recovery_scope','credential_recovery_values')",[],|r|r.get(0)).map_err(unavailable)?;
    if reserved != 0 {
        return Err(corrupt());
    }
    let store_id: String = tx
        .query_row(
            "SELECT store_id FROM model_service_operation_scope WHERE id=1",
            [],
            |r| r.get(0),
        )
        .map_err(unavailable)?;
    let (archive, verified_absent) = match super::archive::read(&parent, &store_id) {
        Ok(value) => {
            let absent = value.is_none();
            (value, absent)
        }
        Err(e) if e.code == "busy" => return Err(e),
        Err(_) => (None, false),
    };
    let (state, legacy) = if archive.is_some() {
        ("active", "imported")
    } else if fresh && verified_absent {
        ("active", "fresh")
    } else {
        ("unverified", "unverified")
    };
    tx.execute_batch("CREATE TABLE credential_recovery_scope(id INTEGER PRIMARY KEY CHECK(id=1),format INTEGER NOT NULL,store_id TEXT NOT NULL,namespace_id TEXT NOT NULL,state TEXT NOT NULL,legacy_state TEXT NOT NULL,legacy_digest TEXT,value_count INTEGER NOT NULL,revision INTEGER NOT NULL);
      CREATE TABLE credential_recovery_values(domain INTEGER NOT NULL CHECK(domain IN(2,3)),name TEXT NOT NULL,value BLOB NOT NULL CHECK(typeof(value)='blob' AND length(value)<=4096),value_hash TEXT NOT NULL,revision INTEGER NOT NULL,PRIMARY KEY(domain,name));").map_err(unavailable)?;
    let count = archive.as_ref().map_or(0, |a| a.values.len());
    if let Some(a) = &archive {
        import(&tx, a, 1)?;
    }
    if tx
        .execute(
            "INSERT INTO credential_recovery_scope VALUES(1,1,?1,?2,?3,?4,?5,?6,1)",
            params![
                store_id,
                uuid::Uuid::new_v4().to_string(),
                state,
                legacy,
                archive.as_ref().map(|a| &a.digest),
                count as i64
            ],
        )
        .map_err(unavailable)?
        != 1
    {
        return Err(corrupt());
    }
    crate::plugin::initialize_composition(&tx, original_version).map_err(plugin_storage_error)?;
    crate::iam::initialize(&tx, original_version)
        .map_err(|error| plugin_storage_error(error.message))?;
    tx.pragma_update(None, "user_version", crate::Store::SCHEMA_VERSION)
        .map_err(unavailable)?;
    let meta = metadata(&tx)?;
    legacy_admission::initialize(&tx, path, &meta.store_id, fresh)?;
    crate::providers::imports::operations::initialize(&tx, &meta.store_id)?;
    super::super::local_tokens::initialize(&tx, &meta.store_id, &meta.namespace_id)?;
    tx.commit().map_err(unavailable)
}
