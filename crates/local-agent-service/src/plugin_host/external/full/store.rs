//! Private source storage; opening the legacy service does not touch this store.
use super::super::{Checked, Result, fail, package as legacy};
use local_agent_core::{domain as d, plugin::full as f};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, OnceLock},
};

const MAX_RECORD: usize = 11 * 1024 * 1024;
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Installed {
    pub manifest: f::Manifest,
    pub digest: String,
    pub signing_key: String,
    pub compressed_bytes: u64,
    pub expanded_bytes: u64,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Selection {
    pub digest: String,
    pub previous: Option<String>,
    pub enabled: bool,
    pub epoch: u64,
    pub revision: u64,
    pub persist_metadata: bool,
}
impl Selection {
    pub fn installed(digest: &str) -> Self {
        Self {
            digest: digest.into(),
            previous: None,
            enabled: false,
            epoch: 1,
            revision: 1,
            persist_metadata: false,
        }
    }
    pub fn advance(&mut self) -> Result<()> {
        self.epoch = self.epoch.checked_add(1).ok_or(fail("resource_limit"))?;
        self.revision = self.revision.checked_add(1).ok_or(fail("resource_limit"))?;
        Ok(())
    }
    pub(super) fn validate(&self) -> Result<()> {
        if legacy::unhex::<32>(&self.digest).is_err()
            || self.epoch == 0
            || self.revision == 0
            || self
                .previous
                .as_ref()
                .is_some_and(|p| legacy::unhex::<32>(p).is_err())
        {
            return Err(fail("invalid_record"));
        }
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Lease {
    pub id: String,
    pub digest: String,
    pub publisher: String,
    pub epoch: u64,
    pub revision: u64,
    pub publisher_revision: u64,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Grant {
    pub lease: Lease,
    pub path: PathBuf,
    pub device: u64,
    pub inode: u64,
    pub unified: bool,
    pub connection: Option<String>,
}
#[derive(Clone)]
pub(super) struct ConsentReview {
    pub reply: d::SourceConsentReview,
    pub grant: Grant,
    pub persist: bool,
    pub created: std::time::Instant,
    pub applied: Option<(Lease, bool, bool)>,
}
#[derive(Clone)]
pub(super) struct Projection {
    pub lease: Lease,
    pub facts: Arc<d::facts::FactSnapshot>,
    pub collected: std::time::Instant,
    pub encoded_bytes: usize,
    pub summary: d::facts::FactSummary,
    pub source_stamp: super::runtime::FileStamp,
    pub captured_source: Option<super::runtime::SourceData>,
}
impl Projection {
    pub(super) fn resident_bytes(&self) -> usize {
        self.encoded_bytes.saturating_add(
            self.captured_source
                .as_ref()
                .map_or(0, |source| source.text.len()),
        )
    }
}
pub(crate) struct Host {
    pub(super) root: PathBuf,
    db_path: PathBuf,
    db: OnceLock<Connection>,
    pub(super) active: BTreeMap<String, Lease>,
    pub(super) grants: BTreeMap<String, Grant>,
    pub(super) projections: BTreeMap<String, Projection>,
    pub(super) failures: BTreeMap<String, (Lease, &'static str)>,
    pub(super) consents: BTreeMap<String, ConsentReview>,
}
pub(in crate::plugin_host::external) fn directory(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    match std::fs::symlink_metadata(path) {
        Ok(m) if m.is_dir() && !m.file_type().is_symlink() => Ok(()),
        Ok(_) => Err(fail("permission_denied")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            std::fs::create_dir(path).checked("unavailable")?;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
                .checked("unavailable")
        }
        Err(_) => Err(fail("permission_denied")),
    }
}
impl Host {
    pub(in crate::plugin_host::external) fn cache_root(&self) -> &Path {
        &self.root
    }
    pub(in crate::plugin_host::external) fn write_private(
        &self,
        path: &Path,
        bytes: &[u8],
    ) -> Result<()> {
        use std::{
            io::Write,
            os::unix::fs::{MetadataExt, OpenOptionsExt},
        };
        let relative = path.strip_prefix(&self.root).checked("permission_denied")?;
        if relative
            .components()
            .any(|c| !matches!(c, std::path::Component::Normal(_)))
        {
            return Err(fail("permission_denied"));
        }
        let parent = path.parent().ok_or(fail("permission_denied"))?;
        let mut checked = self.root.clone();
        for component in parent
            .strip_prefix(&self.root)
            .checked("permission_denied")?
            .components()
        {
            directory(&checked)?;
            checked.push(component);
        }
        directory(&checked)?;
        if let Ok(m) = std::fs::symlink_metadata(path)
            && (!m.is_file() || m.file_type().is_symlink() || m.nlink() != 1)
        {
            return Err(fail("permission_denied"));
        }
        let temporary = parent.join(format!(".{}.tmp", uuid::Uuid::new_v4()));
        let result = (|| -> std::io::Result<()> {
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW)
                .open(&temporary)?;
            file.write_all(bytes)?;
            file.sync_all()?;
            std::fs::rename(&temporary, path)?;
            std::fs::File::open(parent)?.sync_all()
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(temporary);
            return Err(fail("unavailable"));
        }
        Ok(())
    }
    pub fn new(data: &Path) -> Self {
        Self {
            root: data.join("plugin-sources"),
            db_path: data.join("plugin-source-host.sqlite3"),
            db: OnceLock::new(),
            active: BTreeMap::new(),
            grants: BTreeMap::new(),
            projections: BTreeMap::new(),
            failures: BTreeMap::new(),
            consents: BTreeMap::new(),
        }
    }
    pub fn db(&self) -> Result<&Connection> {
        if let Some(db) = self.db.get() {
            return Ok(db);
        }
        // Host is protected by the Manager mutex. A transient open/lock error must
        // remain retryable instead of disabling this optional store until restart.
        let db = self.open()?;
        let _ = self.db.set(db);
        self.db.get().ok_or(fail("unavailable"))
    }
    fn open(&self) -> Result<Connection> {
        use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
        directory(&self.root)?;
        for name in ["packages", "staging", "selected"] {
            directory(&self.root.join(name))?;
        }
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&self.db_path)
            .checked("permission_denied")?;
        let m = file.metadata().checked("permission_denied")?;
        if !m.is_file() || m.nlink() != 1 {
            return Err(fail("permission_denied"));
        }
        let db = Connection::open(&self.db_path).checked("unavailable")?;
        db.busy_timeout(std::time::Duration::from_millis(250))
            .checked("unavailable")?;
        db.pragma_update(None, "foreign_keys", true)
            .checked("unavailable")?;
        super::super::store::initialize_database(
            &db,
            "full_version",
            "version",
            &[
                "full_packages",
                "full_selections",
                "full_reviews",
                "full_projections",
                "full_hidden_projects",
            ],
            "CREATE TABLE IF NOT EXISTS full_version(version INTEGER NOT NULL);INSERT INTO full_version SELECT 1 WHERE NOT EXISTS(SELECT 1 FROM full_version);CREATE TABLE IF NOT EXISTS full_packages(digest TEXT PRIMARY KEY,plugin_id TEXT NOT NULL,record TEXT NOT NULL,removed INTEGER NOT NULL DEFAULT 0);CREATE TABLE IF NOT EXISTS full_selections(plugin_id TEXT PRIMARY KEY,record TEXT NOT NULL);CREATE TABLE IF NOT EXISTS full_reviews(id TEXT PRIMARY KEY,record TEXT NOT NULL,expires INTEGER NOT NULL);CREATE TABLE IF NOT EXISTS full_projections(plugin_id TEXT PRIMARY KEY,record TEXT NOT NULL);CREATE TABLE IF NOT EXISTS full_hidden_projects(id TEXT PRIMARY KEY);",
        )?;
        super::recovery::reconcile(self, &db)?;
        Ok(db)
    }
    pub(super) fn package_path(&self, p: &Installed) -> PathBuf {
        self.root
            .join("packages")
            .join(&p.manifest.id)
            .join(&p.manifest.version)
            .join(&p.digest)
            .join("package.laplug")
    }
    pub(super) fn installed(&self, q: &f::PackageQuery) -> Result<Installed> {
        if !legacy::valid_id(&q.plugin_id) || legacy::unhex::<32>(&q.digest).is_err() {
            return Err(fail("invalid_input"));
        }
        let text: Option<String>=self.db()?.query_row("SELECT CASE WHEN length(record)<=65536 THEN record ELSE NULL END FROM full_packages WHERE plugin_id=?1 AND digest=?2 AND removed=0",params![q.plugin_id,q.digest],|r|r.get(0)).optional().checked("unavailable")?.flatten();
        let p: Installed = decode(&text.ok_or(fail("not_found"))?)?;
        validate_installed(q, &p)?;
        Ok(p)
    }
    pub(super) fn packages(&self) -> Result<Vec<Installed>> {
        let mut statement=self.db()?.prepare("SELECT plugin_id,digest FROM full_packages WHERE removed=0 ORDER BY plugin_id,digest LIMIT 65").checked("unavailable")?;
        let ids = statement
            .query_map([], |r| {
                Ok(f::PackageQuery {
                    plugin_id: r.get(0)?,
                    digest: r.get(1)?,
                })
            })
            .checked("unavailable")?
            .collect::<std::result::Result<Vec<_>, _>>()
            .checked("unavailable")?;
        if ids.len() > 64 {
            return Err(fail("resource_limit"));
        }
        ids.iter().map(|q| self.installed(q)).collect()
    }
    pub(in crate::plugin_host::external) fn installation_inventory(
        &self,
    ) -> Result<Vec<super::super::installation::Entry>> {
        // Do not create optional source files when an old-format package is installed.
        match std::fs::symlink_metadata(&self.db_path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
            Err(_) => return Err(fail("unavailable")),
            Ok(_) => {}
        }
        Ok(self
            .packages()?
            .into_iter()
            .map(|p| super::super::installation::Entry {
                host_kind: super::super::installation::HostKind::History,
                id: p.manifest.id,
                version: p.manifest.version,
                digest: p.digest,
                publisher: p.manifest.publisher,
                key: Some(p.signing_key),
                bytes: p.compressed_bytes,
            })
            .collect())
    }
    pub(super) fn selection(&self, id: &str) -> Result<Selection> {
        let text: String = self
            .db()?
            .query_row(
                "SELECT record FROM full_selections WHERE plugin_id=?1",
                [id],
                |r| r.get(0),
            )
            .checked("not_found")?;
        let s: Selection = decode(&text)?;
        s.validate()?;
        Ok(s)
    }
    #[cfg(test)]
    pub(super) fn save_selection(&self, id: &str, selection: &Selection) -> Result<()> {
        self.db()?
            .execute(
                "INSERT OR REPLACE INTO full_selections VALUES(?1,?2)",
                params![id, encode(selection)?],
            )
            .checked("unavailable")?;
        Ok(())
    }
    pub(super) fn previous_for_activation(
        &self,
        id: &str,
        selection: &Selection,
        next_digest: &str,
    ) -> Result<Option<String>> {
        for digest in std::iter::once(&selection.digest).chain(selection.previous.iter()) {
            if digest == next_digest {
                continue;
            }
            match self.installed(&f::PackageQuery {
                plugin_id: id.into(),
                digest: digest.clone(),
            }) {
                Ok(_) => return Ok(Some(digest.clone())),
                Err(error) if error.code == "not_found" => {}
                Err(error) => return Err(error),
            }
        }
        Ok(None)
    }
    pub(super) fn save_selection_and_clear_projection(
        &self,
        id: &str,
        selection: &Selection,
    ) -> Result<()> {
        self.commit_selection(id, selection, None)
    }
    pub(super) fn remove_selected_package(
        &self,
        id: &str,
        digest: &str,
        selection: &Selection,
    ) -> Result<()> {
        if selection.enabled || selection.digest != digest {
            return Err(fail("invalid_input"));
        }
        self.commit_selection(id, selection, Some(digest))
    }
    fn commit_selection(
        &self,
        id: &str,
        selection: &Selection,
        remove: Option<&str>,
    ) -> Result<()> {
        if !legacy::valid_id(id) {
            return Err(fail("invalid_input"));
        }
        selection.validate()?;
        let marker = self.root.join("selected").join(id);
        let previous = match std::fs::symlink_metadata(&marker) {
            Ok(_) => Some(legacy::read_file(&marker, 64)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(_) => return Err(fail("unavailable")),
        };
        let mut marker_attempted = false;
        let result = (|| {
            let tx = self.db()?.unchecked_transaction().checked("unavailable")?;
            tx.execute(
                "INSERT OR REPLACE INTO full_selections VALUES(?1,?2)",
                params![id, encode(selection)?],
            )
            .checked("unavailable")?;
            tx.execute("DELETE FROM full_projections WHERE plugin_id=?1", [id])
                .checked("unavailable")?;
            if let Some(digest) = remove {
                tx.execute(
                    "UPDATE full_packages SET removed=1 WHERE plugin_id=?1 AND digest=?2",
                    params![id, digest],
                )
                .checked("unavailable")?;
            }
            marker_attempted = true;
            self.selected_marker(id, selection.enabled.then_some(selection.digest.as_str()))?;
            tx.commit().checked("unavailable")
        })();
        if result.is_err() && marker_attempted {
            // A failed SQLite commit must not invalidate a previously working version.
            // If the filesystem itself is unavailable, status/lease checks remain closed.
            match previous {
                Some(bytes) => self.write_private(&marker, &bytes)?,
                None => self.selected_marker(id, None)?,
            }
        }
        result
    }
    pub(super) fn remove_unselected_package(
        &self,
        id: &str,
        digest: &str,
        selection: &Selection,
    ) -> Result<()> {
        if !legacy::valid_id(id)
            || legacy::unhex::<32>(digest).is_err()
            || selection.digest == digest
        {
            return Err(fail("invalid_input"));
        }
        selection.validate()?;
        let tx = self.db()?.unchecked_transaction().checked("unavailable")?;
        tx.execute(
            "UPDATE full_packages SET removed=1 WHERE plugin_id=?1 AND digest=?2",
            params![id, digest],
        )
        .checked("unavailable")?;
        tx.execute(
            "INSERT OR REPLACE INTO full_selections VALUES(?1,?2)",
            params![id, encode(selection)?],
        )
        .checked("unavailable")?;
        tx.commit().checked("unavailable")
    }
    pub fn selected_marker(&self, id: &str, digest: Option<&str>) -> Result<()> {
        if !legacy::valid_id(id) {
            return Err(fail("invalid_input"));
        }
        let path = self.root.join("selected").join(id);
        if let Some(digest) = digest {
            legacy::unhex::<32>(digest)?;
            self.write_private(&path, digest.as_bytes())
        } else {
            match std::fs::remove_file(path) {
                Ok(()) => std::fs::File::open(self.root.join("selected"))
                    .checked("unavailable")?
                    .sync_all()
                    .checked("unavailable"),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(_) => Err(fail("unavailable")),
            }
        }
    }
}
pub(super) fn validate_installed(q: &f::PackageQuery, p: &Installed) -> Result<()> {
    super::package::validate(&p.manifest).map_err(|_| fail("invalid_record"))?;
    if p.manifest.id != q.plugin_id
        || p.digest != q.digest
        || legacy::unhex::<32>(&p.digest).is_err()
        || legacy::unhex::<32>(&p.signing_key).is_err()
        || p.compressed_bytes == 0
        || p.compressed_bytes > legacy::MAX_ARCHIVE
        || p.expanded_bytes > 128 * 1_048_576
    {
        return Err(fail("invalid_record"));
    }
    Ok(())
}
pub(super) fn encode<T: Serialize>(record: &T) -> Result<String> {
    let text = serde_json::to_string(record).checked("invalid_record")?;
    if text.len() > MAX_RECORD {
        return Err(fail("resource_limit"));
    }
    Ok(text)
}
pub(super) fn decode<T: serde::de::DeserializeOwned>(text: &str) -> Result<T> {
    if text.len() > MAX_RECORD {
        return Err(fail("resource_limit"));
    }
    serde_json::from_str(text).checked("invalid_record")
}
