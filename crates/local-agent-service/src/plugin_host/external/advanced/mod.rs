//! M5 is opt-in and additive. No native file, process or connection comes from a guest.
pub(crate) mod api;
mod config;
mod connector;
pub(super) use connector::Live;
pub(crate) mod protocol;
#[cfg(test)]
mod record_tests;
#[cfg(test)]
mod recovery_tests;
mod runtime;
#[cfg(test)]
mod tests;

use super::{Checked, Manager, Result, fail, package};
use local_agent_core::plugin::{
    advanced::*,
    external::{Manifest, Permission, Runtime, Status, ViewKind},
};
use rusqlite::{Connection as Db, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

const CAPS: [&str; 3] = [
    "gateway.protocol@1",
    "config.plan-provider@1",
    "integration.connector@1",
];
pub(crate) fn validate_manifest(m: &Manifest) -> Result<()> {
    package::validate_header(m)?;
    if m.source.is_some() || m.kind.is_some() {
        return Err(fail("unsupported"));
    }
    if m.capabilities.len() != 2
        || !m.capabilities.iter().any(|c| c == "ui.contribution@1")
        || m.capabilities
            .iter()
            .filter(|c| CAPS.contains(&c.as_str()))
            .count()
            != 1
        || m.permissions != [Permission::UiContribute]
        || m.requires.iter().any(|r| {
            ![
                local_agent_core::plugin::contributions::OPTIONAL,
                local_agent_core::plugin::contributions::DETAILS,
            ]
            .contains(&r.as_str())
        })
        || m.requires.len() > 2
        || m.requires
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            != m.requires.len()
        || m.agent.is_some()
        || !m.providers.is_empty()
        || !m.settings.is_empty()
        || m.views.len() != 1
        || m.views[0].kind != ViewKind::Settings
        || !package::local_id(&m.views[0].id)
        || m.views[0].title.is_empty()
        || m.views[0].title.len() > 100
        || m.views[0].description.len() > 1000
    {
        return Err(fail("unsupported"));
    }
    match &m.runtime {
        Runtime::WasmComponent { entry, abi }
            if entry == "plugin.wasm" && abi == "local-agent:plugin@1.0.0" => {}
        Runtime::Declarative if m.capabilities.iter().any(|c| c == CAPS[2]) => {}
        _ => return Err(fail("unsupported")),
    }
    Ok(())
}
pub(crate) fn validate_policy(m: &Manifest, files: &BTreeMap<String, Vec<u8>>) -> Result<Policy> {
    let bytes = files.get("extensions.json").ok_or(fail("invalid_input"))?;
    if bytes.len() > 16384 {
        return Err(fail("resource_limit"));
    }
    let p: Policy = serde_json::from_slice(bytes).checked("invalid_input")?;
    if p.schema_version != 1
        || [
            p.protocol.is_some(),
            p.config.is_some(),
            p.connector.is_some(),
        ]
        .into_iter()
        .filter(|x| *x)
        .count()
            != 1
        || p.protocol.is_some() != m.capabilities.iter().any(|c| c == CAPS[0])
        || p.config.is_some() != m.capabilities.iter().any(|c| c == CAPS[1])
        || p.connector.is_some() != m.capabilities.iter().any(|c| c == CAPS[2])
    {
        return Err(fail("permission_denied"));
    }
    if let Some(v) = &p.protocol
        && (!["messages", "responses", "chat-completions"].contains(&v.base_protocol.as_str())
            || !safe_route(&v.path))
    {
        return Err(fail("unsupported"));
    }
    if let Some(v) = &p.config {
        let mut fields = std::collections::BTreeSet::new();
        if v.fields.is_empty()
            || v.fields.len() > 8
            || v.fields
                .iter()
                .any(|f| !fields.insert(&f.pointer) || !safe_pointer(&f.pointer))
        {
            return Err(fail("invalid_input"));
        }
    }
    Ok(p)
}
fn safe_route(s: &str) -> bool {
    s.starts_with('/')
        && s.len() <= 128
        && !s.contains("..")
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"/-_".contains(&b))
}
fn safe_pointer(s: &str) -> bool {
    local_agent_core::domain::config::valid_pointer(s)
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub(super) struct Lease {
    pub id: String,
    pub digest: String,
    pub epoch: String,
    pub revision: String,
}
impl Lease {
    fn from(s: &Status) -> Self {
        Self {
            id: s.manifest.id.clone(),
            digest: s.digest.clone(),
            epoch: s.activation_epoch.clone(),
            revision: s.grant_revision.clone(),
        }
    }
    pub(super) fn check(&self, m: &Manager) -> Result<()> {
        m.ensure_running()?;
        let s = m.status(&self.id, &self.digest)?;
        if !s.enabled
            || s.runtime_status != "active"
            || s.activation_epoch != self.epoch
            || s.grant_revision != self.revision
        {
            return Err(fail("grant_revoked"));
        }
        Ok(())
    }
    pub(super) async fn check_async(&self, manager: &std::sync::Arc<Manager>) -> Result<()> {
        manager.ensure_running()?;
        let copy = manager.clone();
        let lease = self.clone();
        super::run_sync(move || lease.check(&copy)).await?;
        manager.ensure_running()
    }
}
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub(super) struct FileGrant {
    pub lease: Lease,
    pub path: PathBuf,
    pub device: u64,
    pub inode: u64,
    pub hash: String,
    pub executable: Option<String>,
    #[serde(default)]
    pub target: Option<String>,
}
pub(crate) struct Store {
    db: std::sync::OnceLock<Result<Db>>,
    data: PathBuf,
    bindings_path: PathBuf,
}
impl Store {
    pub(super) fn has_protocol_binding(&self, id: &str, digest: &str) -> Result<bool> {
        Ok(self
            .bindings()?
            .values()
            .any(|b| b.lease.id == id && b.lease.digest == digest))
    }
    pub(super) fn open(data: &Path) -> Result<Self> {
        // No extension database work on the original startup / request path.
        // A failed first use retains the damaged file and fails only extensions.
        Ok(Self {
            db: std::sync::OnceLock::new(),
            data: data.to_owned(),
            bindings_path: data.join("plugin-protocol-bindings.json"),
        })
    }
    fn ready(&self) -> Result<()> {
        self.db().map(|_| ())
    }
    fn db(&self) -> Result<&Db> {
        self.db
            .get_or_init(|| Self::open_db(&self.data))
            .as_ref()
            .map_err(|e| fail(e.code))
    }
    fn bindings(&self) -> Result<BTreeMap<String, protocol::Binding>> {
        let snapshot =
            local_agent_core::providers::files::read(&self.bindings_path).checked("unavailable")?;
        match snapshot.bytes {
            None => Ok(BTreeMap::new()),
            Some(bytes) => serde_json::from_slice(&bytes).checked("unavailable"),
        }
    }
    fn save_bindings(&self, bindings: &BTreeMap<String, protocol::Binding>) -> Result<()> {
        local_agent_core::providers::files::atomic_write(
            &self.bindings_path,
            &serde_json::to_vec(bindings).checked("invalid_input")?,
            0o600,
        )
        .checked("unavailable")
    }
    fn open_db(data: &Path) -> Result<Db> {
        use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
        let path = data.join("plugin-extensions.sqlite3");
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&path)
            .checked("permission_denied")?;
        if !file.metadata().checked("permission_denied")?.is_file()
            || file.metadata().checked("permission_denied")?.nlink() != 1
        {
            return Err(fail("permission_denied"));
        }
        let db = Db::open(path).checked("unavailable")?;
        super::store::validate_version(&db, "extension_version")?;
        db.execute_batch("PRAGMA journal_mode=WAL;CREATE TABLE IF NOT EXISTS extension_version(version INTEGER NOT NULL);INSERT INTO extension_version SELECT 1 WHERE NOT EXISTS(SELECT 1 FROM extension_version);CREATE TABLE IF NOT EXISTS resources(plugin_id TEXT PRIMARY KEY,record TEXT NOT NULL);CREATE TABLE IF NOT EXISTS protocols(provider_id TEXT PRIMARY KEY,plugin_id TEXT NOT NULL,record TEXT NOT NULL);CREATE TABLE IF NOT EXISTS plans(id TEXT PRIMARY KEY,plugin_id TEXT NOT NULL,record TEXT NOT NULL);CREATE TABLE IF NOT EXISTS connector_calls(id INTEGER PRIMARY KEY,plugin_id TEXT NOT NULL,identity TEXT NOT NULL,record TEXT NOT NULL,UNIQUE(plugin_id,identity));CREATE TRIGGER IF NOT EXISTS call_retention AFTER INSERT ON connector_calls BEGIN DELETE FROM connector_calls WHERE id<=NEW.id-10000;END;").checked("unavailable")?;
        let version: u32 = db
            .query_row("SELECT version FROM extension_version", [], |r| r.get(0))
            .checked("unavailable")?;
        if version != 1 {
            return Err(fail("incompatible_api"));
        }
        Ok(db)
    }
}
impl Manager {
    pub(crate) fn retention_tool_impact(
        &self,
        days: u16,
        now: chrono::DateTime<chrono::Utc>,
    ) -> std::result::Result<
        local_agent_core::observation::retention::impact::ToolImpact,
        local_agent_core::observation::retention::impact::Error,
    > {
        use local_agent_core::observation::retention::impact;
        self.ensure_running()
            .map_err(|_| impact::Error::Unavailable)?;
        // Clone only the trusted path; do not call db()/ready() or hold the
        // extension mutex while scanning an independent read-only snapshot.
        let directory = self
            .advanced
            .lock()
            .map_err(|_| impact::Error::Unavailable)?
            .data
            .clone();
        impact::connector(&directory, days, now)
    }
    pub(super) fn extension(&self, q: &Query) -> Result<(super::store::Installed, Policy, Lease)> {
        self.advanced.lock().checked("unavailable")?.ready()?;
        let p = self.verified(&q.plugin_id, &q.digest)?;
        if !p.manifest.is_extension() {
            return Err(fail("unsupported"));
        }
        let bytes = package::read_file(&p.path, package::MAX_ARCHIVE)?;
        let checked = package::inspect(&bytes, self.development)?;
        let policy = validate_policy(&p.manifest, &checked.files)?;
        let lease = Lease::from(&self.status(&q.plugin_id, &q.digest)?);
        lease.check(self)?;
        Ok((p, policy, lease))
    }
    pub(crate) fn extension_status(&self, q: &Query) -> Result<Value> {
        let p = self.installed(&q.plugin_id, &q.digest)?;
        if !p.manifest.is_extension() {
            return Err(fail("unsupported"));
        }
        let checked = package::inspect(
            &package::read_file(&p.path, package::MAX_ARCHIVE)?,
            self.development,
        )?;
        let policy = validate_policy(&p.manifest, &checked.files)?;
        let status = self.status(&q.plugin_id, &q.digest)?;
        let store = self.advanced.lock().checked("unavailable")?;
        store.ready()?;
        let grant: Option<String> = store
            .db()?
            .query_row(
                "SELECT record FROM resources WHERE plugin_id=?1",
                [&q.plugin_id],
                |r| r.get(0),
            )
            .optional()
            .checked("unavailable")?;
        let grant = grant
            .map(|s| serde_json::from_str::<FileGrant>(&s))
            .transpose()
            .checked("invalid_record")?
            .filter(|g| g.lease.digest == q.digest);
        let label = grant.as_ref().map(|g| g.path.display().to_string());
        let authorized = grant.as_ref().is_some_and(|g| {
            status.enabled
                && status.activation_epoch == g.lease.epoch
                && status.grant_revision == g.lease.revision
        });
        let bindings: Vec<String> = store
            .bindings()?
            .into_iter()
            .filter(|(_, b)| b.lease.id == q.plugin_id)
            .map(|(id, _)| id)
            .collect();
        let mut statement = store
            .db()?
            .prepare("SELECT record FROM plans WHERE plugin_id=?1 ORDER BY rowid DESC LIMIT 32")
            .checked("unavailable")?;
        let plans: Vec<Value> = statement
            .query_map([&q.plugin_id], |r| r.get::<_, String>(0))
            .checked("unavailable")?
            .collect::<std::result::Result<Vec<_>, _>>()
            .checked("unavailable")?
            .iter()
            .map(|r| {
                serde_json::from_str::<config::Plan>(r)
                    .checked("invalid_record")
                    .map(|p| p.public())
            })
            .collect::<Result<_>>()?;
        let mut statement = store
            .db()?
            .prepare(
                "SELECT record FROM connector_calls WHERE plugin_id=?1 ORDER BY id DESC LIMIT 100",
            )
            .checked("unavailable")?;
        let calls: Vec<Value> = statement
            .query_map([&q.plugin_id], |r| r.get::<_, String>(0))
            .checked("unavailable")?
            .collect::<std::result::Result<Vec<_>, _>>()
            .checked("unavailable")?
            .into_iter()
            .map(|r| serde_json::from_str(&r).checked("invalid_record"))
            .collect::<Result<_>>()?;
        Ok(
            json!({"apiVersion":API_VERSION,"policy":policy,"resourceLabel":label,"resourceTarget":grant.as_ref().and_then(|g|g.target.as_ref()),"resourceAuthorized":authorized,"protocolBindings":bindings,"plans":plans,"planCapacity":config::capacity(store.db()?)?,"connector":self.connectors.lock().checked("unavailable")?.get(&q.plugin_id).map(|s|s.public()),"calls":calls,"cleanup":"unsupported","redemption":"unsupported"}),
        )
    }
    pub(crate) fn grant_extension_file(&self, q: &Query, path: &Path) -> Result<Value> {
        use std::os::unix::fs::MetadataExt;
        let (_, policy, lease) = self.extension(q)?;
        if policy.config.is_none() && policy.connector.is_none() {
            return Err(fail("permission_denied"));
        }
        let bytes = package::read_file(path, MAX_DOCUMENT)?;
        if path.extension().and_then(|p| p.to_str()) != Some("json") {
            return Err(fail("invalid_input"));
        }
        let _: Value = serde_json::from_slice(&bytes).checked("invalid_input")?;
        let mut executable = None;
        let mut target = None;
        if let Some(c) = policy.connector {
            let connection: Connection = serde_json::from_slice(&bytes).checked("invalid_input")?;
            connector::validate_connection(&c, &connection)?;
            target = Some(match &connection {
                Connection::Stdio { command, .. } => format!("stdio: {command}"),
                Connection::Http {
                    url,
                    allow_private_network,
                    provider_id,
                } => format!(
                    "HTTP: {url} · 本机 / 局域网：{} · 凭据连接：{}",
                    if *allow_private_network {
                        "允许"
                    } else {
                        "不允许"
                    },
                    provider_id.as_deref().unwrap_or("无")
                ),
                Connection::Dsh { .. } => return Err(fail("unverified_connector")),
            });
            if let Connection::Stdio { command, .. } = connection {
                executable = Some(connector::executable_fingerprint(std::path::Path::new(
                    &command,
                ))?);
            }
        }
        let meta = std::fs::symlink_metadata(path).checked("permission_denied")?;
        lease.check(self)?;
        let changed=self.db.lock().checked("unavailable")?.execute("UPDATE selections SET epoch=epoch+1,revision=revision+1 WHERE plugin_id=?1 AND digest=?2 AND enabled=1 AND epoch=?3 AND revision=?4",params![lease.id,lease.digest,lease.epoch,lease.revision]).checked("unavailable")?;
        if changed != 1 {
            return Err(fail("grant_revoked"));
        }
        let lease = Lease::from(&self.status(&q.plugin_id, &q.digest)?);
        let grant = FileGrant {
            lease,
            path: path.to_owned(),
            device: meta.dev(),
            inode: meta.ino(),
            hash: package::hash(&bytes),
            executable,
            target,
        };
        grant.lease.check(self)?;
        self.advanced
            .lock()
            .checked("unavailable")?
            .db()?
            .execute(
                "INSERT OR REPLACE INTO resources VALUES(?1,?2)",
                params![
                    q.plugin_id,
                    serde_json::to_string(&grant).checked("invalid_input")?
                ],
            )
            .checked("unavailable")?;
        Ok(json!({"authorized":true,"label":path.display().to_string()}))
    }
    pub(super) fn extension_grant(&self, q: &Query) -> Result<FileGrant> {
        let store = self.advanced.lock().checked("unavailable")?;
        store.ready()?;
        let text: Option<String> = store
            .db()?
            .query_row(
                "SELECT record FROM resources WHERE plugin_id=?1",
                [&q.plugin_id],
                |r| r.get(0),
            )
            .optional()
            .checked("unavailable")?;
        let g: FileGrant = serde_json::from_str(&text.ok_or(fail("permission_denied"))?)
            .checked("invalid_input")?;
        drop(store);
        if g.lease.digest != q.digest {
            return Err(fail("grant_revoked"));
        }
        g.lease.check(self)?;
        Ok(g)
    }
}
pub(super) fn checked_bytes(grant: &FileGrant) -> Result<Vec<u8>> {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::symlink_metadata(&grant.path).checked("permission_denied")?;
    if meta.dev() != grant.device || meta.ino() != grant.inode {
        return Err(fail("grant_revoked"));
    }
    let bytes = package::read_file(&grant.path, MAX_DOCUMENT)?;
    if package::hash(&bytes) != grant.hash {
        return Err(fail("stale_snapshot"));
    }
    Ok(bytes)
}
