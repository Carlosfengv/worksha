use super::{Checked, Database, Manager, Result, fail, package};
use local_agent_core::{plugin::external::*, providers::vault::Vault};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, atomic::AtomicBool},
};

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SourceGrant {
    pub path: PathBuf,
    pub device: u64,
    pub inode: u64,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ConnectionGrant {
    pub provider_id: String,
    pub endpoint: String,
    pub credential_ref: Option<String>,
    pub revision: u64,
    pub allow_private_network: bool,
}
pub(super) enum Authorization {
    Source(SourceGrant),
    Connection(ConnectionGrant),
    Report,
}
#[derive(Clone)]
pub(crate) struct Selection {
    pub report: bool,
    pub digest: String,
    pub previous: Option<String>,
    pub enabled: bool,
    pub status: String,
    pub error: Option<String>,
    pub epoch: u64,
    pub revision: u64,
    pub source: Option<SourceGrant>,
    pub connection: Option<ConnectionGrant>,
}
#[derive(Clone)]
pub(crate) struct Installed {
    pub manifest: Manifest,
    pub digest: String,
    pub signing_key: Option<String>,
    pub path: PathBuf,
}
fn directory(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    if path.exists() {
        let m = std::fs::symlink_metadata(path).checked("unavailable")?;
        if m.file_type().is_symlink() || !m.is_dir() {
            return Err(fail("permission_denied"));
        }
    } else {
        std::fs::create_dir(path).checked("unavailable")?;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
            .checked("unavailable")?;
    }
    Ok(())
}
fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::{io::Write, os::unix::fs::OpenOptionsExt};
    let mut f = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(path)
        .checked("unavailable")?;
    f.write_all(bytes).checked("unavailable")?;
    f.sync_all().checked("unavailable")
}
/// Fixed host schemas only. Unknown or incomplete repositories are preserved;
/// initial DDL and its format marker commit atomically, including on SQLITE_FULL.
pub(super) fn initialize_database(
    db: &Connection,
    version_table: &str,
    version_column: &str,
    tables: &[&str],
    ddl: &str,
) -> Result<()> {
    let validate = |db: &Connection| -> Result<()> {
        let mut query = db
            .prepare(
                "SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'",
            )
            .checked("unavailable")?;
        let names = query
            .query_map([], |r| r.get::<_, String>(0))
            .checked("unavailable")?
            .collect::<rusqlite::Result<std::collections::BTreeSet<_>>>()
            .checked("invalid_record")?;
        if !names.contains(version_table) {
            return if names.is_empty() {
                Ok(())
            } else {
                Err(fail("invalid_record"))
            };
        }
        let mut query = db
            .prepare(&format!(
                "SELECT {version_column} FROM {version_table} LIMIT 2"
            ))
            .checked("invalid_record")?;
        let versions = query
            .query_map([], |r| r.get::<_, i64>(0))
            .checked("invalid_record")?
            .collect::<rusqlite::Result<Vec<_>>>()
            .checked("invalid_record")?;
        if versions.len() != 1 {
            return Err(fail("invalid_record"));
        }
        if versions[0] != 1 {
            return Err(fail("incompatible_api"));
        }
        if tables.iter().any(|name| !names.contains(*name)) {
            return Err(fail("invalid_record"));
        }
        Ok(())
    };
    // Check before any PRAGMA or recovery writes, then again under the write lock.
    validate(db)?;
    db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; PRAGMA foreign_keys=ON;")
        .checked("unavailable")?;
    let tx = rusqlite::Transaction::new_unchecked(db, rusqlite::TransactionBehavior::Immediate)
        .checked("unavailable")?;
    validate(&tx)?;
    tx.execute_batch(ddl).checked("unavailable")?;
    validate(&tx)?;
    tx.commit().checked("unavailable")
}
impl Manager {
    pub(crate) fn for_service(data: &Path, vault: Arc<dyn Vault>) -> Self {
        match Self::open(data, vault.clone()) {
            Ok(manager) => manager,
            Err(error) => {
                eprintln!(
                    "外部插件数据不可用（{}）；保留原文件，内置功能继续可用。",
                    error.code
                );
                let policy = super::policy::Host::new(data);
                let policy_presence = policy.presence.clone();
                Self {
                    objects: Mutex::new(super::object_contributions::Host::new(data)),
                    policy: Mutex::new(policy),
                    policy_presence,
                    installation: Mutex::new(()),
                    full: Mutex::new(super::full::Host::new(data)),
                    full_publisher_revisions: Mutex::new(BTreeMap::new()),
                    domain: Mutex::new(super::domain_store::Host::new(
                        data.join("domain-host.sqlite3"),
                    )),
                    diagnostics: super::diagnostics::Diagnostics::default(),
                    root: data.join("plugins"),
                    db: Database::Unavailable(error),
                    development: std::env::var("LOCAL_AGENT_PLUGIN_DEVELOPMENT").as_deref()
                        == Ok("1"),
                    cache: Mutex::new(BTreeMap::new()),
                    gates: Mutex::new(BTreeMap::new()),
                    connectors: Mutex::new(BTreeMap::new()),
                    activation: Arc::new(tokio::sync::Mutex::new(())),
                    workers: Arc::new(tokio::sync::Semaphore::new(4)),
                    catalog_instance: uuid::Uuid::new_v4().to_string(),
                    vault,
                    stopping: AtomicBool::new(false),
                    advanced: Mutex::new(
                        super::advanced::Store::open(data)
                            .expect("extension store initialization performs no I/O"),
                    ),
                }
            }
        }
    }
    pub(crate) fn open(data: &Path, vault: Arc<dyn Vault>) -> Result<Self> {
        let root = data.join("plugins");
        directory(&root)?;
        for name in ["packages", "staging"] {
            directory(&root.join(name))?;
        }
        let db_path = data.join("plugin-host.sqlite3");
        use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&db_path)
            .checked("permission_denied")?;
        let metadata = file.metadata().checked("permission_denied")?;
        if !metadata.is_file() || metadata.nlink() != 1 {
            return Err(fail("permission_denied"));
        }
        let db = Connection::open(&db_path).checked("unavailable")?;
        db.busy_timeout(std::time::Duration::from_millis(250))
            .checked("unavailable")?;
        initialize_database(&db, "host_version", "version", &["publishers", "packages", "selections", "private_settings", "provider_presets", "operations", "audit"], "
          CREATE TABLE IF NOT EXISTS host_version(version INTEGER NOT NULL);
          INSERT INTO host_version SELECT 1 WHERE NOT EXISTS(SELECT 1 FROM host_version);
          CREATE TABLE IF NOT EXISTS publishers(id TEXT PRIMARY KEY, public_key TEXT NOT NULL);
          CREATE TABLE IF NOT EXISTS packages(digest TEXT PRIMARY KEY,plugin_id TEXT NOT NULL,version TEXT NOT NULL,manifest TEXT NOT NULL,public_key TEXT,bytes INTEGER NOT NULL,removed INTEGER NOT NULL DEFAULT 0);
          CREATE TABLE IF NOT EXISTS selections(plugin_id TEXT PRIMARY KEY,digest TEXT NOT NULL,previous TEXT,enabled INTEGER NOT NULL DEFAULT 0,status TEXT NOT NULL DEFAULT 'installed',error TEXT,epoch INTEGER NOT NULL DEFAULT 1,revision INTEGER NOT NULL DEFAULT 1,source TEXT,connection TEXT,report INTEGER NOT NULL DEFAULT 0);
          CREATE TABLE IF NOT EXISTS private_settings(digest TEXT PRIMARY KEY,settings TEXT NOT NULL);
          CREATE TABLE IF NOT EXISTS provider_presets(plugin_id TEXT NOT NULL,digest TEXT NOT NULL,preset TEXT NOT NULL,provider_id TEXT NOT NULL,PRIMARY KEY(plugin_id,digest,preset));
          CREATE TABLE IF NOT EXISTS operations(id TEXT PRIMARY KEY,digest TEXT NOT NULL,phase TEXT NOT NULL,review TEXT NOT NULL);
          CREATE TABLE IF NOT EXISTS audit(id INTEGER PRIMARY KEY,plugin_id TEXT NOT NULL,digest TEXT NOT NULL,action TEXT NOT NULL,at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP);")?;
        db.execute_batch("CREATE TRIGGER IF NOT EXISTS audit_retention AFTER INSERT ON audit BEGIN DELETE FROM audit WHERE id <= NEW.id-10000; END;
          DELETE FROM operations WHERE phase NOT IN ('inspected','publishing') AND rowid < (SELECT coalesce(max(rowid),0)-256 FROM operations);").checked("unavailable")?;
        let version: u32 = db
            .query_row("SELECT version FROM host_version", [], |r| r.get(0))
            .checked("unavailable")?;
        if version != 1 {
            return Err(fail("incompatible_api"));
        }
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(db_path, std::fs::Permissions::from_mode(0o600))
            .checked("unavailable")?;
        // A published-but-uncommitted package is never selected, trusted or activated.
        db.execute(
            "UPDATE operations SET phase='interrupted' WHERE phase='publishing'",
            [],
        )
        .checked("unavailable")?;
        // Review references expire across restarts; no partially published package is trusted.
        db.execute(
            "UPDATE operations SET phase='expired' WHERE phase='inspected'",
            [],
        )
        .checked("unavailable")?;
        for entry in std::fs::read_dir(root.join("staging")).checked("unavailable")? {
            let entry = entry.checked("unavailable")?;
            if uuid::Uuid::parse_str(&entry.file_name().to_string_lossy()).is_ok()
                && entry.file_type().checked("unavailable")?.is_dir()
            {
                let package = entry.path().join("package.laplug");
                if package.exists() {
                    std::fs::remove_file(package).checked("unavailable")?;
                }
                std::fs::remove_dir(entry.path()).checked("unavailable")?;
            }
        }
        for author in std::fs::read_dir(root.join("packages")).checked("unavailable")? {
            let author = author.checked("unavailable")?;
            if !author.file_type().checked("unavailable")?.is_dir() {
                continue;
            }
            for version in std::fs::read_dir(author.path()).checked("unavailable")? {
                let version = version.checked("unavailable")?;
                if !version.file_type().checked("unavailable")?.is_dir() {
                    continue;
                }
                for digest in std::fs::read_dir(version.path()).checked("unavailable")? {
                    let digest = digest.checked("unavailable")?;
                    if !digest.file_type().checked("unavailable")?.is_dir() {
                        continue;
                    }
                    let name = digest.file_name().to_string_lossy().into_owned();
                    let exists: bool = db
                        .query_row(
                            "SELECT EXISTS(SELECT 1 FROM packages WHERE digest=?1 AND removed=0)",
                            [&name],
                            |r| r.get(0),
                        )
                        .checked("unavailable")?;
                    if !exists && package::unhex::<32>(&name).is_ok() {
                        let path = digest.path().join("package.laplug");
                        if path.exists() {
                            std::fs::remove_file(path).checked("unavailable")?;
                        }
                        std::fs::remove_dir(digest.path()).checked("unavailable")?;
                    }
                }
            }
        }
        let policy = super::policy::Host::new(data);
        let policy_presence = policy.presence.clone();
        let host = Self {
            objects: Mutex::new(super::object_contributions::Host::new(data)),
            policy: Mutex::new(policy),
            policy_presence,
            installation: Mutex::new(()),
            full: Mutex::new(super::full::Host::new(data)),
            full_publisher_revisions: Mutex::new(BTreeMap::new()),
            domain: Mutex::new(super::domain_store::Host::new(
                data.join("domain-host.sqlite3"),
            )),
            diagnostics: super::diagnostics::Diagnostics::default(),
            root,
            db: Database::Ready(Mutex::new(db)),
            development: std::env::var("LOCAL_AGENT_PLUGIN_DEVELOPMENT").as_deref() == Ok("1"),
            cache: Mutex::new(BTreeMap::new()),
            gates: Mutex::new(BTreeMap::new()),
            connectors: Mutex::new(BTreeMap::new()),
            activation: Arc::new(tokio::sync::Mutex::new(())),
            workers: Arc::new(tokio::sync::Semaphore::new(4)),
            catalog_instance: uuid::Uuid::new_v4().to_string(),
            vault,
            stopping: AtomicBool::new(false),
            advanced: Mutex::new(super::advanced::Store::open(data)?),
        };
        // Validate persisted selections without spawning guests or accessing their sources.
        let selected = host.list()?;
        for item in selected.iter().filter(|s| s.enabled) {
            if host.verified(&item.manifest.id, &item.digest).is_err() {
                host.mark_failed(&item.manifest.id, "signature_invalid")?;
            }
        }
        Ok(host)
    }
    pub(crate) fn development(&self) -> bool {
        self.development
    }
    pub(crate) fn inspect_upload(&self, path: &Path) -> Result<serde_json::Value> {
        self.ensure_running()?;
        let bytes = package::read_file(path, package::MAX_ARCHIVE)?;
        let inspected = package::inspect(&bytes, self.development)?;
        let (kind, review) = if inspected.manifest.source.is_some() {
            ("source", serde_json::to_value(self.full_inspect(path)?))
        } else if inspected.manifest.kind.is_some() {
            ("policy", serde_json::to_value(self.policy_inspect(path)?))
        } else {
            ("plugin", serde_json::to_value(self.inspect(path)?))
        };
        let review = review.checked("invalid_input")?;
        // Dispatch is host-owned. A file replaced during inspection cannot turn
        // a reviewed package into a different kind, identity, or signing key.
        if review["digest"] != inspected.digest {
            return Err(fail("stale_snapshot"));
        }
        Ok(serde_json::json!({"kind":kind,"review":review}))
    }
    pub(crate) fn inspect(&self, path: &Path) -> Result<PackageReview> {
        self.ensure_running()?;
        let full_root = self
            .full
            .try_lock()
            .checked("busy")?
            .cache_root()
            .to_owned();
        let bytes = package::read_file(path, package::MAX_ARCHIVE)?;
        let p = package::inspect(&bytes, self.development)?;
        // History sources and routing policies have separate authorization state.
        // They use the common package format, but must enter their capability host.
        if p.manifest.source.is_some() || p.manifest.kind.is_some() {
            return Err(fail("unsupported"));
        }
        let db = self.db.lock().checked("unavailable")?;
        let count: i64 = db
            .query_row(
                "SELECT count(*) FROM operations WHERE phase='inspected'",
                [],
                |r| r.get(0),
            )
            .checked("unavailable")?;
        if count >= 8 {
            let old: String = db
                .query_row(
                    "SELECT id FROM operations WHERE phase='inspected' ORDER BY rowid LIMIT 1",
                    [],
                    |r| r.get(0),
                )
                .checked("unavailable")?;
            uuid::Uuid::parse_str(&old).checked("invalid_input")?;
            let stage = self.root.join("staging").join(&old);
            if stage.join("package.laplug").exists() {
                std::fs::remove_file(stage.join("package.laplug")).checked("unavailable")?;
                std::fs::remove_dir(stage).checked("unavailable")?;
            }
            db.execute("UPDATE operations SET phase='expired' WHERE id=?1", [old])
                .checked("unavailable")?;
        }
        let trusted: Option<String> = db
            .query_row(
                "SELECT public_key FROM publishers WHERE id=?1",
                [&p.manifest.publisher],
                |r| r.get(0),
            )
            .optional()
            .checked("unavailable")?;
        if trusted.is_some() && trusted != p.signing_key {
            return Err(fail("publisher_changed"));
        }
        let reference = uuid::Uuid::new_v4().to_string();
        // The legacy database lock serializes staging writes for this format.
        super::installation::check_staging(
            [&self.root, &full_root],
            super::installation::HostKind::General,
            bytes.len() as u64,
        )?;
        let stage = self.root.join("staging").join(&reference);
        directory(&stage)?;
        write_private(&stage.join("package.laplug"), &bytes)?;
        let review = PackageReview {
            package_ref: reference.clone(),
            digest: p.digest.clone(),
            manifest: p.manifest,
            signing_key: p.signing_key.clone(),
            trusted_publisher: trusted.is_some(),
            development: p.signing_key.is_none(),
            compressed_bytes: bytes.len() as u64,
            expanded_bytes: p.expanded,
        };
        db.execute(
            "INSERT INTO operations(id,digest,phase,review) VALUES(?1,?2,'inspected',?3)",
            params![
                reference,
                p.digest,
                serde_json::to_string(&review).checked("invalid_input")?
            ],
        )
        .checked("unavailable")?;
        Ok(review)
    }
    pub(crate) fn install(&self, request: &InstallRequest) -> Result<Status> {
        self.ensure_running()?;
        let _installation = self.installation.try_lock().checked("busy")?;
        // Capture the optional inventory before holding the legacy database mutex.
        let (mut full_inventory, full_root) = {
            let full = self.full.try_lock().checked("busy")?;
            (full.installation_inventory()?, full.cache_root().to_owned())
        };
        full_inventory.extend(self.policy_inventory()?);
        uuid::Uuid::parse_str(&request.package_ref).checked("invalid_input")?;
        let mut db = self.db.lock().checked("unavailable")?;
        let (digest, phase, review): (String, String, String) = db
            .query_row(
                "SELECT digest,phase,review FROM operations WHERE id=?1",
                [&request.package_ref],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .checked("not_found")?;
        if digest != request.digest {
            return Err(fail("signature_invalid"));
        }
        let review: PackageReview = serde_json::from_str(&review).checked("invalid_input")?;
        if phase == "committed" {
            drop(db);
            return self.status(&review.manifest.id, &digest);
        }
        if phase != "inspected" {
            return Err(fail("stale_snapshot"));
        }
        let stage = self.root.join("staging").join(&request.package_ref);
        let bytes = package::read_file(&stage.join("package.laplug"), package::MAX_ARCHIVE)?;
        let package = package::inspect(&bytes, self.development)?;
        if package.digest != request.digest {
            return Err(fail("signature_invalid"));
        }
        let trusted: Option<String> = db
            .query_row(
                "SELECT public_key FROM publishers WHERE id=?1",
                [&package.manifest.publisher],
                |r| r.get(0),
            )
            .optional()
            .checked("unavailable")?;
        if trusted.is_some() && trusted != package.signing_key {
            return Err(fail("publisher_changed"));
        }
        if package.signing_key.is_some() && trusted.is_none() && !request.approve_publisher {
            return Err(fail("publisher_untrusted"));
        }
        super::installation::check(
            &db,
            &full_inventory,
            &super::installation::Entry {
                host_kind: super::installation::HostKind::General,
                id: package.manifest.id.clone(),
                version: package.manifest.version.clone(),
                digest: package.digest.clone(),
                publisher: package.manifest.publisher.clone(),
                key: package.signing_key.clone(),
                bytes: bytes.len() as u64,
            },
        )?;
        let target_file = self
            .root
            .join("packages")
            .join(&package.manifest.id)
            .join(&package.manifest.version)
            .join(&package.digest)
            .join("package.laplug");
        super::installation::check_archives(
            [&self.root, &full_root],
            &target_file,
            bytes.len() as u64,
        )?;
        let parent = self.root.join("packages").join(&package.manifest.id);
        directory(&parent)?;
        let parent = parent.join(&package.manifest.version);
        directory(&parent)?;
        let target = parent.join(&package.digest);
        db.execute(
            "UPDATE operations SET phase='publishing' WHERE id=?1",
            [&request.package_ref],
        )
        .checked("unavailable")?;
        if target.exists() && target.join("package.laplug").exists() {
            if package::hash(&package::read_file(
                &target.join("package.laplug"),
                package::MAX_ARCHIVE,
            )?) != package.digest
            {
                return Err(fail("signature_invalid"));
            }
            std::fs::remove_file(stage.join("package.laplug")).checked("unavailable")?;
            std::fs::remove_dir(&stage).checked("unavailable")?;
        } else {
            if target.exists() {
                std::fs::remove_dir(&target).checked("unavailable")?;
            }
            std::fs::rename(&stage, &target).checked("unavailable")?;
            std::fs::File::open(&parent)
                .checked("unavailable")?
                .sync_all()
                .checked("unavailable")?;
        }
        let tx = db.transaction().checked("unavailable")?;
        if let Some(key) = &package.signing_key {
            tx.execute(
                "INSERT OR IGNORE INTO publishers(id,public_key) VALUES(?1,?2)",
                params![package.manifest.publisher, key],
            )
            .checked("unavailable")?;
        }
        tx.execute("INSERT INTO packages(digest,plugin_id,version,manifest,public_key,bytes) VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(digest) DO UPDATE SET removed=0",params![package.digest,package.manifest.id,package.manifest.version,serde_json::to_string(&package.manifest).checked("invalid_input")?,package.signing_key,bytes.len() as i64]).checked("unavailable")?;
        tx.execute("INSERT OR IGNORE INTO selections(plugin_id,digest,status) VALUES(?1,?2,'awaiting_permission')",params![package.manifest.id,package.digest]).checked("unavailable")?;
        let previous_settings: Option<String> = tx.query_row("SELECT p.settings FROM private_settings p JOIN selections s ON s.digest=p.digest WHERE s.plugin_id=?1",[&package.manifest.id],|r|r.get(0)).optional().checked("unavailable")?;
        let previous_settings: BTreeMap<String, String> = previous_settings
            .map(|v| serde_json::from_str(&v))
            .transpose()
            .checked("invalid_input")?
            .unwrap_or_default();
        let defaults: BTreeMap<_, _> = package
            .manifest
            .settings
            .iter()
            .map(|s| {
                (
                    s.key.clone(),
                    previous_settings
                        .get(&s.key)
                        .filter(|v| s.options.contains(v))
                        .unwrap_or(&s.default)
                        .clone(),
                )
            })
            .collect();
        tx.execute(
            "INSERT OR IGNORE INTO private_settings(digest,settings) VALUES(?1,?2)",
            params![
                package.digest,
                serde_json::to_string(&defaults).checked("invalid_input")?
            ],
        )
        .checked("unavailable")?;
        tx.execute(
            "UPDATE operations SET phase='committed' WHERE id=?1",
            [&request.package_ref],
        )
        .checked("unavailable")?;
        tx.execute(
            "INSERT INTO audit(plugin_id,digest,action) VALUES(?1,?2,'installed')",
            params![package.manifest.id, package.digest],
        )
        .checked("unavailable")?;
        tx.commit().checked("unavailable")?;
        drop(db);
        self.status(&package.manifest.id, &package.digest)
    }
    pub(crate) fn installed(&self, id: &str, digest: &str) -> Result<Installed> {
        if !package::valid_id(id) || package::unhex::<32>(digest).is_err() {
            return Err(fail("invalid_input"));
        }
        let db = self.db.lock().checked("unavailable")?;
        let (manifest,key):(String,Option<String>)=db.query_row("SELECT manifest,public_key FROM packages WHERE plugin_id=?1 AND digest=?2 AND removed=0",params![id,digest],|r|Ok((r.get(0)?,r.get(1)?))).checked("not_found")?;
        let manifest: Manifest = serde_json::from_str(&manifest).checked("invalid_input")?;
        let path = self
            .root
            .join("packages")
            .join(id)
            .join(&manifest.version)
            .join(digest)
            .join("package.laplug");
        Ok(Installed {
            manifest,
            digest: digest.into(),
            signing_key: key,
            path,
        })
    }
    pub(crate) fn verified(&self, id: &str, digest: &str) -> Result<Installed> {
        if super::full::legacy_conflict(&self.root, id)? {
            return Err(fail("capability_conflict"));
        }
        let installed = self.installed(id, digest)?;
        let bytes = package::read_file(&installed.path, package::MAX_ARCHIVE)?;
        let package = package::inspect(&bytes, self.development)?;
        if package.digest != digest || package.manifest.id != id {
            return Err(fail("signature_invalid"));
        }
        if let Some(key) = &package.signing_key {
            let trusted: Option<String> = self
                .db
                .lock()
                .checked("unavailable")?
                .query_row(
                    "SELECT public_key FROM publishers WHERE id=?1",
                    [&package.manifest.publisher],
                    |r| r.get(0),
                )
                .optional()
                .checked("unavailable")?;
            if trusted.as_ref() != Some(key) {
                return Err(fail("publisher_untrusted"));
            }
        }
        Ok(installed)
    }
    pub(crate) fn selection(&self, id: &str) -> Result<Selection> {
        self.db.lock().checked("unavailable")?.query_row("SELECT digest,previous,enabled,status,error,epoch,revision,source,connection,report FROM selections WHERE plugin_id=?1",[id],|r|{
            let source:Option<String>=r.get(7)?;let connection:Option<String>=r.get(8)?;
            Ok(Selection{report:r.get(9)?,digest:r.get(0)?,previous:r.get(1)?,enabled:r.get(2)?,status:r.get(3)?,error:r.get(4)?,epoch:r.get::<_,i64>(5)? as u64,revision:r.get::<_,i64>(6)? as u64,source:source.and_then(|s|serde_json::from_str(&s).ok()),connection:connection.and_then(|s|serde_json::from_str(&s).ok())})
        }).checked("not_found")
    }
    pub(crate) fn status(&self, id: &str, digest: &str) -> Result<Status> {
        let p = self.installed(id, digest)?;
        let s = self.selection(id)?;
        let selected = s.digest == digest;
        let settings: String = self
            .db
            .lock()
            .checked("unavailable")?
            .query_row(
                "SELECT settings FROM private_settings WHERE digest=?1",
                [digest],
                |r| r.get(0),
            )
            .checked("unavailable")?;
        Ok(Status {
            report_authorized: selected && s.report,
            manifest: p.manifest,
            digest: p.digest,
            signing_key: p.signing_key,
            enabled: selected && s.enabled,
            runtime_status: if selected {
                s.status
            } else {
                "installed".into()
            },
            error_code: if selected { s.error } else { None },
            activation_epoch: s.epoch.to_string(),
            grant_revision: s.revision.to_string(),
            source_authorized: selected && s.source.is_some(),
            connection_authorized: selected && s.connection.is_some(),
            source_label: if selected {
                s.source.map(|g| g.path.display().to_string())
            } else {
                None
            },
            connection_label: if selected {
                s.connection.map(|g| g.endpoint)
            } else {
                None
            },
            previous_digest: s.previous,
            settings: serde_json::from_str(&settings).checked("invalid_input")?,
        })
    }
    pub(crate) fn list(&self) -> Result<Vec<Status>> {
        let rows: Vec<(String, String)> = {
            let db = self.db.lock().checked("unavailable")?;
            let mut stmt=db.prepare("SELECT plugin_id,digest FROM packages WHERE removed=0 ORDER BY plugin_id,version,digest").checked("unavailable")?;
            stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
                .checked("unavailable")?
                .collect::<std::result::Result<_, _>>()
                .checked("unavailable")?
        };
        rows.iter()
            .map(|(id, digest)| self.status(id, digest))
            .collect()
    }
    pub(crate) fn mark_failed(&self, id: &str, code: &'static str) -> Result<()> {
        self.db
            .lock()
            .checked("unavailable")?
            .execute(
                "UPDATE selections SET status='failed',error=?2,epoch=epoch+1 WHERE plugin_id=?1",
                params![id, code],
            )
            .checked("unavailable")?;
        self.cancel_connector(id)?;
        self.cache.lock().checked("unavailable")?.remove(id);
        Ok(())
    }
    pub(super) fn dependencies(&self, candidate: &Installed) -> Result<()> {
        let target = &candidate.manifest;
        let mut graph: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut packages = self.list()?;
        packages.sort_by(|a, b| {
            semver::Version::parse(&a.manifest.version)
                .unwrap()
                .cmp(&semver::Version::parse(&b.manifest.version).unwrap())
                .then(a.enabled.cmp(&b.enabled))
        });
        let active: std::collections::BTreeSet<_> = packages
            .iter()
            .filter(|p| p.enabled && p.runtime_status == "active")
            .map(|p| p.manifest.id.clone())
            .collect();
        let mut selected = BTreeMap::new();
        for s in packages {
            if !selected.contains_key(&s.manifest.id)
                || s.enabled
                || !active.contains(&s.manifest.id)
            {
                selected.insert(s.manifest.id.clone(), (s.manifest, s.digest));
            }
        }
        selected.insert(
            target.id.clone(),
            (target.clone(), candidate.digest.clone()),
        );
        // Expand only reachable dependencies. A corrupt unrelated optional package
        // must not prevent another plugin from being activated.
        let mut pending = vec![target.id.clone()];
        while let Some(id) = pending.pop() {
            if graph.contains_key(&id) {
                continue;
            }
            let Some((manifest, digest)) = selected.get(&id) else {
                continue;
            };
            let mut requires = manifest.requires.clone();
            if super::contributions::opted_in(manifest) {
                let p = self.installed(&id, digest)?;
                if let Some(c) = self.contributions_for(&p)? {
                    for d in c.optional_dependencies {
                        if (active.contains(&d.plugin_id) || d.plugin_id == target.id)
                            && selected.get(&d.plugin_id).is_some_and(|(m, digest)| {
                                semver::VersionReq::parse(&d.version).is_ok_and(|r| {
                                    semver::Version::parse(&m.version).is_ok_and(|v| r.matches(&v))
                                }) && self.verified(&d.plugin_id, digest).is_ok()
                            })
                        {
                            requires.push(format!("plugin:{}", d.plugin_id));
                        }
                    }
                }
            }
            pending.extend(
                requires
                    .iter()
                    .filter_map(|d| d.strip_prefix("plugin:").map(str::to_owned)),
            );
            graph.insert(id, requires);
        }
        fn visit(
            id: &str,
            graph: &BTreeMap<String, Vec<String>>,
            visiting: &mut Vec<String>,
            done: &mut Vec<String>,
        ) -> Result<()> {
            if visiting.iter().any(|s| s == id) {
                return Err(fail("dependency_cycle"));
            }
            if done.iter().any(|s| s == id) {
                return Ok(());
            }
            let deps = graph.get(id).ok_or(fail("dependency_missing"))?;
            visiting.push(id.into());
            for dep in deps {
                if let Some(dep) = dep.strip_prefix("plugin:") {
                    visit(dep, graph, visiting, done)?;
                }
            }
            visiting.pop();
            done.push(id.into());
            Ok(())
        }
        let mut done = Vec::new();
        visit(&target.id, &graph, &mut Vec::new(), &mut done)?;
        // Optional edges participate in cycle detection, but their absence never blocks.
        let mut pending = target.requires.clone();
        let mut required = std::collections::BTreeSet::new();
        while let Some(dep) = pending.pop() {
            if let Some(id) = dep.strip_prefix("plugin:") {
                if !required.insert(id.to_owned()) {
                    continue;
                }
                if !active.contains(id) {
                    return Err(fail("dependency_missing"));
                }
                pending.extend(
                    selected
                        .get(id)
                        .ok_or(fail("dependency_missing"))?
                        .0
                        .requires
                        .clone(),
                );
            }
        }
        Ok(())
    }
    pub(crate) async fn change(self: &Arc<Self>, request: StateRequest) -> Result<Status> {
        let manager = self.clone();
        let checked = request.clone();
        let installed = super::run_sync(move || {
            manager.ensure_running()?;
            manager.installed(&checked.plugin_id, &checked.digest)
        })
        .await?;
        if !matches!(request.action, StateAction::Enable | StateAction::Rollback) {
            let manager = self.clone();
            return super::run_sync(move || manager.change_synchronous(request, installed)).await;
        }
        let manager = self.clone();
        let id = request.plugin_id.clone();
        let gate = super::run_sync(move || manager.gate(&id)).await?;
        let gate_guard = gate.try_lock_owned().checked("resource_limit")?;
        let manager = self.clone();
        let id = request.plugin_id.clone();
        let selected = super::run_sync(move || manager.selection(&id)).await?;
        if matches!(request.action, StateAction::Enable)
            && selected.enabled
            && selected.digest == request.digest
            && selected.status == "active"
        {
            let manager = self.clone();
            return super::run_sync(move || manager.status(&request.plugin_id, &request.digest))
                .await;
        }
        let activation_guard = self
            .activation
            .clone()
            .try_lock_owned()
            .checked("resource_limit")?;
        let digest = if matches!(request.action, StateAction::Rollback) {
            selected.previous.clone().ok_or(fail("not_found"))?
        } else {
            request.digest.clone()
        };
        let manager = self.clone();
        let id = request.plugin_id.clone();
        let candidate_digest = digest.clone();
        let candidate = super::run_sync(move || {
            let candidate = manager.verified(&id, &candidate_digest)?;
            manager.dependencies(&candidate)?;
            Ok(candidate)
        })
        .await?;
        self.prepare_worker(&candidate, &selected).await?;
        let manager = self.clone();
        super::run_sync(move || {
            let _guards = (gate_guard, activation_guard);
            manager.commit_activation(request, candidate, selected, digest)
        })
        .await
    }
    fn commit_activation(
        &self,
        request: StateRequest,
        candidate: Installed,
        selected: Selection,
        digest: String,
    ) -> Result<Status> {
        self.ensure_running()?;
        let mut db = self.db.lock().checked("unavailable")?;
        let tx = db.transaction().checked("unavailable")?;
        let present: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM packages WHERE digest=?1 AND removed=0)",
                [&digest],
                |r| r.get(0),
            )
            .checked("unavailable")?;
        if !present {
            return Err(fail("grant_revoked"));
        }
        if let Some(key) = &candidate.signing_key {
            let trusted: bool = tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM publishers WHERE id=?1 AND public_key=?2)",
                    params![candidate.manifest.publisher, key],
                    |r| r.get(0),
                )
                .checked("unavailable")?;
            if !trusted {
                return Err(fail("publisher_untrusted"));
            }
        }
        // Recheck dependency selections under the commit lock: a revoke may
        // have arrived while the preflight process was being drained.
        let mut pending = candidate.manifest.requires.clone();
        let mut visited = std::collections::BTreeSet::new();
        while let Some(dependency) = pending.pop() {
            if let Some(id) = dependency.strip_prefix("plugin:") {
                if !visited.insert(id.to_owned()) {
                    continue;
                }
                let manifest:Option<String>=tx.query_row("SELECT p.manifest FROM selections s JOIN packages p ON p.digest=s.digest WHERE s.plugin_id=?1 AND s.enabled=1 AND s.status='active' AND p.removed=0",[id],|r|r.get(0)).optional().checked("unavailable")?;
                let manifest: Manifest =
                    serde_json::from_str(&manifest.ok_or(fail("dependency_missing"))?)
                        .checked("invalid_input")?;
                pending.extend(manifest.requires);
            }
        }
        let previous = if selected.digest != digest {
            Some(selected.digest.clone())
        } else {
            selected.previous
        };
        let changed=tx.execute("UPDATE selections SET digest=?2,previous=?3,enabled=1,status='active',error=NULL,epoch=epoch+1,revision=revision+1,source=NULL,connection=NULL,report=0 WHERE plugin_id=?1 AND digest=?4 AND epoch=?5 AND revision=?6",params![request.plugin_id,digest,previous,selected.digest,i64::try_from(selected.epoch).checked("resource_limit")?,i64::try_from(selected.revision).checked("resource_limit")?]).checked("unavailable")?;
        if changed != 1 {
            return Err(fail("grant_revoked"));
        }
        tx.execute(
            "INSERT INTO audit(plugin_id,digest,action) VALUES(?1,?2,'activated')",
            params![request.plugin_id, digest],
        )
        .checked("unavailable")?;
        tx.commit().checked("unavailable")?;
        self.cache
            .lock()
            .checked("unavailable")?
            .remove(&request.plugin_id);
        drop(db);
        self.cancel_connector(&request.plugin_id)?;
        self.status(&request.plugin_id, &digest)
    }
    pub(super) fn change_synchronous(
        &self,
        request: StateRequest,
        installed: Installed,
    ) -> Result<Status> {
        self.ensure_running()?;
        match request.action {
            StateAction::RevokeGrants => {
                self.revoke(&request.plugin_id, &request.digest, false)?;
            }
            StateAction::Disable | StateAction::Remove | StateAction::RevokePublisher => {
                if matches!(request.action, StateAction::Remove)
                    && self.has_catalog_binding(&request.plugin_id)?
                {
                    return Err(fail("catalog_binding_active"));
                }
                if matches!(request.action, StateAction::Remove)
                    && installed.manifest.is_extension()
                    && self
                        .advanced
                        .lock()
                        .checked("unavailable")?
                        .has_protocol_binding(&request.plugin_id, &request.digest)?
                {
                    return Err(fail("protocol_binding_active"));
                }
                let current = self.selection(&request.plugin_id)?;
                if current.digest != request.digest
                    && !matches!(request.action, StateAction::RevokePublisher)
                {
                    if !matches!(request.action, StateAction::Remove) {
                        return Err(fail("grant_revoked"));
                    }
                    self.db
                        .lock()
                        .checked("unavailable")?
                        .execute(
                            "UPDATE packages SET removed=1 WHERE digest=?1",
                            [&request.digest],
                        )
                        .checked("unavailable")?;
                    self.db.lock().checked("unavailable")?.execute("UPDATE selections SET previous=NULL WHERE plugin_id=?1 AND previous=?2",params![request.plugin_id,request.digest]).checked("unavailable")?;
                    if installed.path.exists() {
                        std::fs::remove_file(&installed.path).checked("unavailable")?;
                    }
                    return Ok(self.removed_status(installed, current));
                }
                let publisher = installed.manifest.publisher.clone();
                let mut affected = vec![request.plugin_id.clone()];
                if matches!(request.action, StateAction::RevokePublisher) {
                    // Invalidating source runtime leases is memory-only and cannot
                    // make this legacy revocation depend on a damaged source database.
                    self.revoke_shared_publisher(&publisher)?;
                    affected.extend(
                        self.list()?
                            .into_iter()
                            .filter(|s| s.manifest.publisher == publisher)
                            .map(|s| s.manifest.id),
                    );
                }
                loop {
                    let mut extra = Vec::new();
                    for item in self.list()?.into_iter().filter(|s| s.enabled) {
                        if !affected.contains(&item.manifest.id)
                            && item.manifest.requires.iter().any(|r| {
                                r.strip_prefix("plugin:")
                                    .is_some_and(|id| affected.iter().any(|a| a == id))
                            })
                        {
                            extra.push(item.manifest.id);
                        }
                    }
                    if extra.is_empty() {
                        break;
                    }
                    affected.extend(extra);
                }
                for id in affected {
                    let selected = self.selection(&id)?;
                    self.revoke(&id, &selected.digest, true)?;
                }
                if matches!(request.action, StateAction::Remove) {
                    let selected = self.selection(&request.plugin_id)?;
                    if selected.enabled {
                        return Err(fail("invalid_input"));
                    }
                    // Only package code is removed; core data and audit records are retained.
                    self.db
                        .lock()
                        .checked("unavailable")?
                        .execute(
                            "UPDATE packages SET removed=1 WHERE digest=?1",
                            [&request.digest],
                        )
                        .checked("unavailable")?;
                    if installed.path.exists() {
                        std::fs::remove_file(&installed.path).checked("unavailable")?;
                    }
                    return Ok(Status {
                        runtime_status: "removed".into(),
                        ..self.removed_status(installed, selected)
                    });
                }
            }
            StateAction::Enable | StateAction::Rollback => return Err(fail("unsupported")),
        }
        self.status(&request.plugin_id, &request.digest)
    }
    fn removed_status(&self, p: Installed, s: Selection) -> Status {
        Status {
            report_authorized: false,
            manifest: p.manifest,
            digest: p.digest,
            signing_key: p.signing_key,
            enabled: false,
            runtime_status: "removed".into(),
            error_code: None,
            activation_epoch: s.epoch.to_string(),
            grant_revision: s.revision.to_string(),
            source_authorized: false,
            connection_authorized: false,
            source_label: None,
            connection_label: None,
            previous_digest: s.previous,
            settings: BTreeMap::new(),
        }
    }
    pub(crate) fn revoke(&self, id: &str, digest: &str, disable: bool) -> Result<()> {
        let s = self.selection(id)?;
        if s.digest != digest {
            return Err(fail("grant_revoked"));
        }
        self.db.lock().checked("unavailable")?.execute("UPDATE selections SET source=NULL,connection=NULL,report=0,revision=revision+1,epoch=epoch+1,enabled=CASE WHEN ?2 THEN 0 ELSE enabled END,status=CASE WHEN ?2 THEN 'disabled' ELSE status END WHERE plugin_id=?1",params![id,disable]).checked("unavailable")?;
        self.cancel_connector(id)?;
        self.db
            .lock()
            .checked("unavailable")?
            .execute(
                "INSERT INTO audit(plugin_id,digest,action) VALUES(?1,?2,?3)",
                params![
                    id,
                    digest,
                    if disable {
                        "disabled"
                    } else {
                        "grants-revoked"
                    }
                ],
            )
            .checked("unavailable")?;
        self.cache.lock().checked("unavailable")?.remove(id);
        Ok(())
    }
    pub(crate) fn grant_source(&self, id: &str, digest: &str, path: &Path) -> Result<Status> {
        use std::os::unix::fs::MetadataExt;
        let installed = self.verified(id, digest)?;
        let s = self.selection(id)?;
        if !s.enabled
            || s.digest != digest
            || !installed
                .manifest
                .permissions
                .iter()
                .any(|p| matches!(p, Permission::SourceContentRead { .. }))
        {
            return Err(fail("permission_denied"));
        }
        if path.extension().is_none_or(|s| s != "jsonl") {
            return Err(fail("unsupported"));
        }
        let before = std::fs::symlink_metadata(path).checked("permission_denied")?;
        if before.file_type().is_symlink()
            || before.nlink() != 1
            || !before.is_file()
            || before.len() > MAX_SOURCE
        {
            return Err(fail("permission_denied"));
        }
        let path = path.canonicalize().checked("permission_denied")?;
        let grant = SourceGrant {
            path,
            device: before.dev(),
            inode: before.ino(),
        };
        self.commit_authorization(id, &s, Authorization::Source(grant))?;
        self.status(id, digest)
    }
    pub(crate) fn grant_connection(
        &self,
        id: &str,
        digest: &str,
        provider: &local_agent_core::Provider,
        allow_private_network: bool,
    ) -> Result<Status> {
        let p = self.verified(id, digest)?;
        let s = self.selection(id)?;
        if !s.enabled
            || s.digest != digest
            || !p
                .manifest
                .permissions
                .iter()
                .any(|p| matches!(p, Permission::ConnectionRequest { .. }))
        {
            return Err(fail("permission_denied"));
        }
        local_agent_core::providers::validate_endpoint(&provider.endpoint)
            .checked("invalid_input")?;
        let grant = ConnectionGrant {
            provider_id: provider.id.clone(),
            endpoint: provider.endpoint.clone(),
            credential_ref: provider.credential_ref.clone(),
            revision: provider.revision,
            allow_private_network,
        };
        self.commit_authorization(id, &s, Authorization::Connection(grant))?;
        self.status(id, digest)
    }
    pub(crate) fn save_private_settings(
        &self,
        id: &str,
        digest: &str,
        values: &BTreeMap<String, String>,
    ) -> Result<Status> {
        let gate = self.gate(id)?;
        let _guard = gate.try_lock().checked("resource_limit")?;
        let installed = self.installed(id, digest)?;
        if values.len() != installed.manifest.settings.len()
            || installed
                .manifest
                .settings
                .iter()
                .any(|s| values.get(&s.key).is_none_or(|v| !s.options.contains(v)))
        {
            return Err(fail("invalid_input"));
        }
        {
            let mut db = self.db.lock().checked("unavailable")?;
            let tx = db.transaction().checked("unavailable")?;
            let changed = tx
                .execute(
                    "UPDATE private_settings SET settings=?2 WHERE digest=?1 AND settings!=?2",
                    params![
                        digest,
                        serde_json::to_string(values).checked("invalid_input")?
                    ],
                )
                .checked("unavailable")?;
            if changed > 0 {
                tx.execute(
                    "UPDATE selections SET revision=revision+1 WHERE plugin_id=?1 AND digest=?2",
                    params![id, digest],
                )
                .checked("unavailable")?;
            }
            tx.commit().checked("unavailable")?;
            if changed > 0 {
                self.cache.lock().checked("unavailable")?.remove(id);
            }
        }
        self.status(id, digest)
    }
    pub(crate) fn authorize_report(&self, id: &str, digest: &str) -> Result<Status> {
        let p = self.verified(id, digest)?;
        let s = self.selection(id)?;
        if s.digest != digest
            || !s.enabled
            || !p
                .manifest
                .permissions
                .iter()
                .any(|p| matches!(p,Permission::DomainQuery{query} if query=="gateway.summary"))
        {
            return Err(fail("permission_denied"));
        }
        self.commit_authorization(id, &s, Authorization::Report)?;
        self.status(id, digest)
    }
    pub(super) fn commit_authorization(
        &self,
        id: &str,
        before: &Selection,
        grant: Authorization,
    ) -> Result<()> {
        self.ensure_running()?;
        if !before.enabled {
            return Err(fail("grant_revoked"));
        }
        let (fields, value) = match grant {
            Authorization::Source(grant) => (
                "source=?1,connection=NULL",
                rusqlite::types::Value::Text(
                    serde_json::to_string(&grant).checked("invalid_input")?,
                ),
            ),
            Authorization::Connection(grant) => (
                "connection=?1,source=NULL",
                rusqlite::types::Value::Text(
                    serde_json::to_string(&grant).checked("invalid_input")?,
                ),
            ),
            Authorization::Report => ("report=?1", rusqlite::types::Value::Integer(1)),
        };
        // The clause is chosen solely from the fixed cases above. Recheck the
        // exact validated lease under the same lock as the authorization write.
        let changed = self.db.lock().checked("unavailable")?.execute(
            &format!("UPDATE selections SET {fields},revision=revision+1 WHERE plugin_id=?2 AND digest=?3 AND epoch=?4 AND revision=?5 AND enabled=1"),
            params![value, id, before.digest, i64::try_from(before.epoch).checked("resource_limit")?, i64::try_from(before.revision).checked("resource_limit")?],
        ).checked("unavailable")?;
        if changed != 1 {
            return Err(fail("grant_revoked"));
        }
        self.cache.lock().checked("unavailable")?.remove(id);
        self.cancel_connector(id)?;
        Ok(())
    }
    pub(crate) fn publish_report(
        &self,
        before: &Status,
        report: GatewayReport,
    ) -> Result<serde_json::Value> {
        let now = self.status(&before.manifest.id, &before.digest)?;
        if !now.enabled
            || !now.report_authorized
            || before.activation_epoch != now.activation_epoch
            || before.grant_revision != now.grant_revision
        {
            return Err(fail("grant_revoked"));
        }
        self.cache.lock().checked("unavailable")?.insert(
            before.manifest.id.clone(),
            super::Cached {
                observed_at: chrono::Utc::now().to_rfc3339(),
                report: Some(report),
                collected_at: std::time::Instant::now(),
                digest: before.digest.clone(),
                epoch: before.activation_epoch.clone(),
                revision: before.grant_revision.clone(),
                snapshot: uuid::Uuid::new_v4().to_string(),
                sessions: vec![],
                source_revision: None,
                quota: None,
            }
            .into(),
        );
        Ok(serde_json::json!({"refreshed":true}))
    }
    pub(crate) fn preset_provider_id(
        &self,
        id: &str,
        digest: &str,
        preset: &str,
    ) -> Result<String> {
        let db = self.db.lock().checked("unavailable")?;
        db.execute("INSERT OR IGNORE INTO provider_presets(plugin_id,digest,preset,provider_id) VALUES(?1,?2,?3,?4)",params![id,digest,preset,format!("custom-{}",uuid::Uuid::new_v4())]).checked("unavailable")?;
        db.query_row("SELECT provider_id FROM provider_presets WHERE plugin_id=?1 AND digest=?2 AND preset=?3",params![id,digest,preset],|r|r.get(0)).checked("unavailable")
    }
}
