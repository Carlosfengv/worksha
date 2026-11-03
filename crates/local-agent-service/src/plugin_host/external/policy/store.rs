use super::super::{Checked, Result, fail, installation, package};
use local_agent_core::plugin::policy as p;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, OnceLock, RwLock},
};
mod evidence;
pub(super) mod recovery;
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Installed {
    pub manifest: p::Manifest,
    pub digest: String,
    pub signing_key: String,
    pub bytes: u64,
    pub expanded: u64,
    pub removed: bool,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Selection {
    pub digest: String,
    pub previous: Option<String>,
    pub enabled: bool,
    pub epoch: u64,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Control {
    pub store_id: String,
    pub service_id: String,
    pub revision: String,
    pub authority: String,
    pub version: Option<String>,
    pub head: Option<p::BindingStatus>,
    pub pending: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_request_identity: Option<String>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Binding {
    pub control_authority: String,
    pub status: p::BindingStatus,
    pub instance: String,
    pub epoch: u64,
    pub publisher_revision: u64,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Authorization {
    pub instance: String,
    pub epoch: u64,
    pub publisher_revision: u64,
}
impl Authorization {
    pub fn from_context(c: &super::runtime::Context, instance: &str) -> Self {
        Self {
            instance: instance.into(),
            epoch: c.epoch,
            publisher_revision: c.publisher_revision,
        }
    }
    pub fn matches(&self, b: &Binding) -> bool {
        self.instance == b.instance
            && self.epoch == b.epoch
            && self.publisher_revision == b.publisher_revision
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Intent {
    pub status: p::PublishStatus,
    pub package: p::PackageQuery,
    pub failure: p::FailureMode,
    pub expected: Option<String>,
    pub before_revision: u64,
    pub control_revision: String,
    #[serde(default)]
    pub authorization: Option<Authorization>,
    #[serde(default)]
    pub cancellation_revision: Option<String>,
}
#[derive(Default)]
pub(crate) struct Presence {
    pub(super) scopes: BTreeMap<(String, String), Control>,
    pub(super) unknown: std::collections::BTreeSet<String>,
    // Preserve existing UUID spellings privately. The legacy public unknown
    // list stays normalized, but repairing one raw scope cannot clear another.
    uncertain_scopes: std::collections::BTreeSet<(String, String)>,
    unknown_markers: BTreeMap<String, String>,
    pub(super) unavailable: bool,
}
impl Presence {
    fn mark_unknown(&mut self, store: &str, service: &str) {
        if let Ok(id) = uuid::Uuid::parse_str(service) {
            self.unknown.insert(id.to_string());
            self.uncertain_scopes.insert((store.into(), service.into()));
        } else {
            self.unavailable = true;
        }
    }
    fn mark_known(&mut self, c: &Control) -> Result<()> {
        let key = (c.store_id.clone(), c.service_id.clone());
        let id = uuid::Uuid::parse_str(&c.service_id).checked("invalid_input")?;
        self.scopes.insert(key.clone(), c.clone());
        self.uncertain_scopes.remove(&key);
        self.unknown_markers
            .remove(&marker_name(&c.store_id, &c.service_id)?);
        let normalized = id.to_string();
        if !self
            .uncertain_scopes
            .iter()
            .any(|(_, service)| uuid::Uuid::parse_str(service).ok() == Some(id))
            && !self
                .unknown_markers
                .values()
                .any(|service| service == &normalized)
        {
            self.unknown.remove(&normalized);
        }
        Ok(())
    }
    fn scope_count(&self) -> usize {
        let mut names: std::collections::BTreeSet<_> = self
            .scopes
            .keys()
            .chain(&self.uncertain_scopes)
            .filter_map(|(store, service)| marker_name(store, service).ok())
            .collect();
        names.extend(self.unknown_markers.keys().cloned());
        let located: std::collections::BTreeSet<_> = self
            .uncertain_scopes
            .iter()
            .filter_map(|(_, service)| uuid::Uuid::parse_str(service).ok().map(|id| id.to_string()))
            .chain(self.unknown_markers.values().cloned())
            .collect();
        names.len() + self.unknown.difference(&located).count()
    }
}
pub(crate) struct Host {
    pub root: PathBuf,
    pub cache: PathBuf,
    db_path: PathBuf,
    db: OnceLock<Connection>,
    db_identity: OnceLock<(u64, u64)>,
    pub(super) recovery_references: BTreeMap<String, recovery::Reference>,
    pub presence: Arc<RwLock<Presence>>,
    pub active: BTreeMap<String, u64>,
}
pub(super) fn encode<T: Serialize>(t: &T) -> Result<String> {
    let s = serde_json::to_string(t).checked("invalid_record")?;
    if s.len() > 2 * 1024 * 1024 {
        return Err(fail("resource_limit"));
    }
    Ok(s)
}
pub(super) fn decode<T: serde::de::DeserializeOwned>(s: &str) -> Result<T> {
    if s.len() > 2 * 1024 * 1024 {
        return Err(fail("invalid_record"));
    }
    serde_json::from_str(s).checked("invalid_record")
}
pub(super) fn uuid(s: &str) -> Result<()> {
    if uuid::Uuid::parse_str(s).is_err() {
        return Err(fail("invalid_input"));
    }
    Ok(())
}
pub(super) fn operation_identity(
    request: &local_agent_core::model_services::operations::OperationRequest,
) -> Result<String> {
    Ok(package::hash(encode(request)?.as_bytes()))
}
pub(super) struct BindRequest<'a> {
    pub store: &'a str,
    pub service: &'a str,
    pub version: &'a str,
    pub expected: &'a Option<String>,
    pub package: &'a p::PackageQuery,
    pub failure: p::FailureMode,
    pub pending: Option<&'a str>,
}
impl Host {
    pub(crate) fn new(data: &Path) -> Self {
        let root = data.join("routing-policy-required");
        let mut presence = Presence::default();
        match std::fs::read_dir(&root) {
            Ok(rows) => {
                for (i, row) in rows.enumerate() {
                    if i >= 256 {
                        presence.unavailable = true;
                        break;
                    }
                    match row {
                        Ok(row) => {
                            let name = row.file_name().to_string_lossy().into_owned();
                            let Some(stem) = name.strip_suffix(".json") else {
                                continue;
                            };
                            let parts: Vec<_> = stem.split('-').collect();
                            if parts.len() != 3
                                || parts[0].len() != 32
                                || parts[1].len() != 32
                                || parts[2].len() != 64
                            {
                                continue;
                            }
                            let Ok(id) = uuid::Uuid::parse_str(parts[1]) else {
                                continue;
                            };
                            let id = id.to_string();
                            let value = package::read_file(&row.path(), 4096).and_then(|v| {
                                serde_json::from_slice::<Control>(&v).checked("invalid_record")
                            });
                            match value {
                                Ok(c)
                                    if marker_name(&c.store_id, &c.service_id).ok().as_deref()
                                        == Some(name.as_str())
                                        && uuid(&c.revision).is_ok()
                                        && uuid(&c.authority).is_ok() =>
                                {
                                    presence
                                        .scopes
                                        .insert((c.store_id.clone(), c.service_id.clone()), c);
                                }
                                _ => {
                                    presence.unknown.insert(id.clone());
                                    presence.unknown_markers.insert(name, id);
                                }
                            }
                        }
                        Err(_) => presence.unavailable = true,
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => presence.unavailable = true,
        };
        reconcile_startup(data, &mut presence);
        Self {
            root,
            cache: data.join("plugin-sources"),
            db_path: data.join("routing-policy.sqlite3"),
            db: OnceLock::new(),
            db_identity: OnceLock::new(),
            recovery_references: BTreeMap::new(),
            presence: Arc::new(RwLock::new(presence)),
            active: BTreeMap::new(),
        }
    }
    pub(super) fn db(&self) -> Result<&Connection> {
        if let Some(db) = self.db.get() {
            return Ok(db);
        }
        let exists = self.db_path.exists();
        if let Ok(m) = std::fs::symlink_metadata(&self.db_path) {
            use std::os::unix::fs::MetadataExt;
            if !m.is_file() || m.file_type().is_symlink() || m.nlink() != 1 {
                return Err(fail("permission_denied"));
            }
        }
        let db = Connection::open(&self.db_path).checked("unavailable")?;
        db.busy_timeout(std::time::Duration::from_millis(25))
            .checked("unavailable")?;
        if !exists {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&self.db_path, std::fs::Permissions::from_mode(0o600))
                .checked("unavailable")?;
            db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; CREATE TABLE policy_meta(format INTEGER NOT NULL); INSERT INTO policy_meta VALUES(1); CREATE TABLE policy_packages(digest TEXT PRIMARY KEY,id TEXT NOT NULL,record TEXT NOT NULL); CREATE TABLE policy_selections(id TEXT PRIMARY KEY,record TEXT NOT NULL); CREATE TABLE policy_reviews(id TEXT PRIMARY KEY,record TEXT NOT NULL); CREATE TABLE policy_controls(scope TEXT PRIMARY KEY,record TEXT NOT NULL); CREATE TABLE policy_bindings(scope TEXT NOT NULL,version TEXT NOT NULL,record TEXT NOT NULL,PRIMARY KEY(scope,version)); CREATE TABLE policy_intents(id TEXT PRIMARY KEY,record TEXT NOT NULL);").checked("unavailable")?;
        }
        let v: i64 = db
            .query_row("SELECT format FROM policy_meta", [], |r| r.get(0))
            .checked("invalid_record")?;
        if v != 1 {
            return Err(fail("invalid_record"));
        }
        use std::os::unix::fs::MetadataExt;
        let metadata = std::fs::symlink_metadata(&self.db_path).checked("unavailable")?;
        let _ = self.db_identity.set((metadata.dev(), metadata.ino()));
        let _ = self.db.set(db);
        self.db.get().ok_or(fail("unavailable"))
    }
    pub(super) fn packages(&self) -> Result<Vec<Installed>> {
        if !self.db_path.exists() {
            return Ok(vec![]);
        }
        let mut st = self
            .db()?
            .prepare("SELECT record FROM policy_packages LIMIT 65")
            .checked("unavailable")?;
        let rows = st
            .query_map([], |r| r.get::<_, String>(0))
            .checked("unavailable")?
            .collect::<std::result::Result<Vec<_>, _>>()
            .checked("unavailable")?;
        if rows.len() > 64 {
            return Err(fail("resource_limit"));
        }
        rows.iter().map(|s| decode(s)).collect()
    }
    pub(super) fn installed(&self, q: &p::PackageQuery) -> Result<Installed> {
        if !package::valid_id(&q.plugin_id) || package::unhex::<32>(&q.digest).is_err() {
            return Err(fail("invalid_input"));
        }
        let text: String = self
            .db()?
            .query_row(
                "SELECT record FROM policy_packages WHERE digest=?1",
                [&q.digest],
                |r| r.get(0),
            )
            .checked("not_found")?;
        let p: Installed = decode(&text)?;
        super::package::validate(&p.manifest)?;
        if p.removed || p.digest != q.digest || p.manifest.id != q.plugin_id {
            return Err(fail("not_found"));
        }
        Ok(p)
    }
    pub(super) fn path(&self, p: &Installed) -> PathBuf {
        self.cache
            .join("packages")
            .join(&p.manifest.id)
            .join(&p.manifest.version)
            .join(&p.digest)
            .join("package.laplug")
    }
    pub(super) fn selection(&self, id: &str) -> Result<Option<Selection>> {
        let s: Option<String> = self
            .db()?
            .query_row(
                "SELECT record FROM policy_selections WHERE id=?1",
                [id],
                |r| r.get(0),
            )
            .optional()
            .checked("unavailable")?;
        s.map(|v| decode(&v)).transpose()
    }
    pub(super) fn save_selection(&self, id: &str, s: &Selection) -> Result<()> {
        self.db()?
            .execute(
                "INSERT OR REPLACE INTO policy_selections VALUES(?1,?2)",
                params![id, encode(s)?],
            )
            .checked("unavailable")?;
        Ok(())
    }
    pub(super) fn inventory(&self) -> Result<Vec<installation::Entry>> {
        Ok(self
            .packages()?
            .into_iter()
            .filter(|p| !p.removed)
            .map(|p| installation::Entry {
                host_kind: installation::HostKind::Routing,
                id: p.manifest.id,
                version: p.manifest.version,
                digest: p.digest,
                publisher: p.manifest.publisher,
                key: Some(p.signing_key),
                bytes: p.bytes,
            })
            .collect())
    }
    pub(super) fn scope(store: &str, service: &str) -> String {
        format!("{store}:{service}")
    }
    pub(super) fn control(&self, store: &str, service: &str) -> Result<Option<Control>> {
        let row: Option<String> = self
            .db()?
            .query_row(
                "SELECT record FROM policy_controls WHERE scope=?1",
                [Self::scope(store, service)],
                |r| r.get(0),
            )
            .optional()
            .checked("unavailable")?;
        row.map(|s| {
            let c: Control = decode(&s)?;
            if c.store_id != store
                || c.service_id != service
                || uuid(&c.revision).is_err()
                || uuid(&c.authority).is_err()
            {
                return Err(fail("invalid_record"));
            }
            Ok(c)
        })
        .transpose()
    }
    pub(super) fn marker(&self, c: &Control) -> Result<()> {
        {
            let cache = self.presence.read().map_err(|_| fail("unavailable"))?;
            if cache.unavailable {
                return Err(fail("invalid_record"));
            }
            if !cache
                .scopes
                .contains_key(&(c.store_id.clone(), c.service_id.clone()))
                && cache.scope_count() >= 256
            {
                return Err(fail("resource_limit"));
            }
        }
        super::super::full::store::directory(&self.root)?;
        let path = self.root.join(marker_name(&c.store_id, &c.service_id)?);
        write_private(&path, encode(c)?.as_bytes())?;
        let mut cache = self.presence.write().map_err(|_| fail("unavailable"))?;
        cache.mark_known(c)
    }
    pub(super) fn cached_control(&self, store: &str, service: &str) -> Result<Option<Control>> {
        let cache = self.presence.read().map_err(|_| fail("unavailable"))?;
        if cache.unavailable {
            return Err(fail("invalid_record"));
        }
        Ok(cache.scopes.get(&(store.into(), service.into())).cloned())
    }
    /// Repair only the original publication's durable pending barrier. No
    /// permission, package selection or original model operation is replayed.
    pub(super) fn sync_pending(
        &self,
        c: &Control,
        operation: &str,
        identity: &str,
        before: &str,
    ) -> Result<()> {
        if c.pending.as_deref() != Some(operation)
            || c.pending_request_identity
                .as_deref()
                .is_some_and(|v| v != identity)
        {
            return Err(fail("stale_snapshot"));
        }
        if let Some(marker) = self.cached_control(&c.store_id, &c.service_id)?
            && marker != *c
            && !(marker.authority == c.authority
                && marker.pending.as_deref() == Some(operation)
                && marker
                    .pending_request_identity
                    .as_deref()
                    .is_none_or(|v| v == identity)
                && marker.revision == before)
        {
            return Err(fail("invalid_record"));
        }
        self.marker(c)
    }
    pub(super) fn clear_publish_pending(
        &self,
        binding: &p::BindingStatus,
        operation: &str,
        identity: &str,
    ) -> Result<()> {
        let Some(mut c) = self.control(&binding.store_id, &binding.service_id)? else {
            return Err(fail("invalid_record"));
        };
        // A later choice belongs to that choice, not to this historical receipt.
        if c.revision != binding.revision
            || c.head.as_ref() != Some(binding)
            || c.pending.as_deref().is_some_and(|p| p != operation)
        {
            return Ok(());
        }
        if c.pending_request_identity
            .as_deref()
            .is_some_and(|v| v != identity)
        {
            return Err(fail("invalid_record"));
        }
        let marker = self.cached_control(&c.store_id, &c.service_id)?;
        if let Some(marker) = &marker {
            if marker.pending.as_deref().is_some_and(|p| p != operation) {
                return Ok(());
            }
            let mut expected = c.clone();
            expected.pending = marker.pending.clone();
            expected.pending_request_identity = marker.pending_request_identity.clone();
            if marker
                .pending_request_identity
                .as_deref()
                .is_some_and(|v| v != identity)
            {
                return Err(fail("invalid_record"));
            }
            if marker != &expected {
                return Err(fail("invalid_record"));
            }
        }
        if c.pending.is_none()
            && marker.as_ref() == Some(&c)
            && !self
                .presence
                .read()
                .map_err(|_| fail("unavailable"))?
                .unknown
                .contains(
                    &uuid::Uuid::parse_str(&c.service_id)
                        .checked("invalid_record")?
                        .to_string(),
                )
        {
            return Ok(());
        }
        if marker.is_none() && c.pending.is_some() {
            self.marker(&c)?;
        }
        c.pending = None;
        c.pending_request_identity = None;
        // Until the final marker is durable, its old pending value still blocks
        // fresh dispatch and other publication even if this write succeeded.
        self.db()?
            .execute(
                "INSERT OR REPLACE INTO policy_controls VALUES(?1,?2)",
                params![Self::scope(&c.store_id, &c.service_id), encode(&c)?],
            )
            .checked("unavailable")?;
        self.marker(&c)
    }
    pub(super) fn save_control(&self, c: &Control) -> Result<()> {
        self.marker(c)?;
        self.db()?
            .execute(
                "INSERT OR REPLACE INTO policy_controls VALUES(?1,?2)",
                params![Self::scope(&c.store_id, &c.service_id), encode(c)?],
            )
            .checked("unavailable")?;
        Ok(())
    }
    pub(super) fn binding(
        &self,
        store: &str,
        service: &str,
        version: &str,
    ) -> Result<Option<Binding>> {
        let row: Option<String> = self
            .db()?
            .query_row(
                "SELECT record FROM policy_bindings WHERE scope=?1 AND version=?2",
                params![Self::scope(store, service), version],
                |r| r.get(0),
            )
            .optional()
            .checked("unavailable")?;
        row.map(|s| {
            let b: Binding = decode(&s)?;
            if b.status.store_id != store
                || b.status.service_id != service
                || b.status.service_version_id != version
                || uuid(&b.status.revision).is_err()
                || uuid(&b.control_authority).is_err()
            {
                return Err(fail("invalid_record"));
            }
            Ok(b)
        })
        .transpose()
    }
    pub(super) fn consistent_control(&self, store: &str, service: &str) -> Result<Option<Control>> {
        let row = self.control(store, service)?;
        let cache = self.presence.read().map_err(|_| fail("unavailable"))?;
        if cache.unavailable
            || cache.unknown.contains(
                &uuid::Uuid::parse_str(service)
                    .checked("invalid_input")?
                    .to_string(),
            )
            || row.as_ref() != cache.scopes.get(&(store.into(), service.into()))
        {
            return Err(fail("invalid_record"));
        }
        Ok(row)
    }
    pub(super) fn controls(&self, store: &str) -> Result<Vec<Control>> {
        let cache = self.presence.read().map_err(|_| fail("unavailable"))?;
        if cache.unavailable {
            return Err(fail("invalid_record"));
        }
        Ok(cache
            .scopes
            .values()
            .filter(|c| c.store_id == store)
            .cloned()
            .collect())
    }
    pub(super) fn unbind(&self, store: &str, service: &str, expected: &str) -> Result<String> {
        if self
            .control(store, service)?
            .is_some_and(|c| c.pending.is_some())
        {
            return Err(fail("result_pending"));
        }
        let marker = {
            let cache = self.presence.read().map_err(|_| fail("unavailable"))?;
            if cache.unavailable {
                return Err(fail("invalid_record"));
            }
            cache
                .scopes
                .get(&(store.into(), service.into()))
                .cloned()
                .ok_or(fail("not_found"))?
        };
        // An explicit CAS can repair a marker-first failure, but never discard an unresolved publication.
        if marker.revision != expected || marker.pending.is_some() {
            return Err(fail("stale_snapshot"));
        }
        let c = Control {
            store_id: store.into(),
            service_id: service.into(),
            revision: uuid::Uuid::new_v4().to_string(),
            authority: uuid::Uuid::new_v4().to_string(),
            version: None,
            head: None,
            pending: None,
            pending_request_identity: None,
        };
        self.save_control(&c)?;
        Ok(c.revision)
    }
    pub(super) fn intent_optional(&self, id: &str) -> Result<Option<Intent>> {
        let row: Option<String> = self
            .db()?
            .query_row("SELECT record FROM policy_intents WHERE id=?1", [id], |r| {
                r.get(0)
            })
            .optional()
            .checked("unavailable")?;
        row.map(|r| decode(&r)).transpose()
    }
    pub(super) fn intent(&self, id: &str) -> Result<Intent> {
        self.intent_optional(id)?.ok_or(fail("not_found"))
    }
    pub(super) fn intents(&self, store: &str) -> Result<Vec<Intent>> {
        if !self.db_path.exists() {
            return Ok(vec![]);
        }
        let mut s = self
            .db()?
            .prepare("SELECT record FROM policy_intents LIMIT 257")
            .checked("unavailable")?;
        let rows = s
            .query_map([], |r| r.get::<_, String>(0))
            .checked("unavailable")?
            .collect::<std::result::Result<Vec<_>, _>>()
            .checked("unavailable")?;
        if rows.len() > 256 {
            return Err(fail("resource_limit"));
        }
        let rows = rows
            .iter()
            .map(|r| decode::<Intent>(r))
            .collect::<Result<Vec<_>>>()?;
        Ok(rows
            .into_iter()
            .filter(|i| i.status.store_id == store)
            .collect())
    }
    pub(super) fn save_intent(&self, i: &Intent) -> Result<()> {
        self.db()?
            .execute(
                "INSERT OR REPLACE INTO policy_intents VALUES(?1,?2)",
                params![i.status.operation_id, encode(i)?],
            )
            .checked("unavailable")?;
        Ok(())
    }
    pub(super) fn prepare_intent(&self, control: &Control, intent: &Intent) -> Result<()> {
        self.marker(control)?;
        let tx = self.db()?.unchecked_transaction().checked("unavailable")?;
        tx.execute(
            "INSERT OR REPLACE INTO policy_controls VALUES(?1,?2)",
            params![
                Self::scope(&control.store_id, &control.service_id),
                encode(control)?
            ],
        )
        .checked("unavailable")?;
        tx.execute(
            "INSERT OR REPLACE INTO policy_intents VALUES(?1,?2)",
            params![intent.status.operation_id, encode(intent)?],
        )
        .checked("unavailable")?;
        tx.commit().checked("unavailable")
    }
    pub(super) fn cancel_prepared(&self, mut i: Intent) -> Result<Intent> {
        if i.status.state != "prepared" {
            return Err(fail("result_pending"));
        }
        let mut c = self
            .consistent_control(&i.status.store_id, &i.status.service_id)?
            .ok_or(fail("invalid_record"))?;
        let identity = operation_identity(&i.status.request)?;
        if c.pending.as_deref() != Some(&i.status.operation_id)
            || c.pending_request_identity.as_deref() != Some(&identity)
        {
            return Err(fail("stale_snapshot"));
        }
        c.pending = None;
        c.pending_request_identity = None;
        c.revision = uuid::Uuid::new_v4().to_string();
        i.status.state = "cancelled".into();
        i.status.error_code = None;
        i.cancellation_revision = Some(c.revision.clone());
        // Keep the durable pending marker until both cancellation records commit.
        let tx = self.db()?.unchecked_transaction().checked("unavailable")?;
        tx.execute(
            "INSERT OR REPLACE INTO policy_controls VALUES(?1,?2)",
            params![Self::scope(&c.store_id, &c.service_id), encode(&c)?],
        )
        .checked("unavailable")?;
        tx.execute(
            "INSERT OR REPLACE INTO policy_intents VALUES(?1,?2)",
            params![i.status.operation_id, encode(&i)?],
        )
        .checked("unavailable")?;
        tx.commit().checked("unavailable")?;
        self.marker(&c)?;
        Ok(i)
    }
    pub(super) fn repair_cancelled(&self, i: &Intent) -> Result<()> {
        let Some(revision) = &i.cancellation_revision else {
            return Ok(());
        };
        let Some(c) = self.control(&i.status.store_id, &i.status.service_id)? else {
            return Err(fail("invalid_record"));
        };
        if &c.revision != revision || c.pending.is_some() || c.pending_request_identity.is_some() {
            return Ok(());
        }
        if let Some(marker) = self.cached_control(&c.store_id, &c.service_id)? {
            if marker == c {
                return self.marker(&c);
            }
            let mut expected = c.clone();
            expected.revision = i.control_revision.clone();
            expected.pending = Some(i.status.operation_id.clone());
            expected.pending_request_identity = Some(operation_identity(&i.status.request)?);
            if marker != expected {
                return Err(fail("invalid_record"));
            }
        }
        self.marker(&c)
    }
    pub(super) fn bind(
        &self,
        request: BindRequest<'_>,
        authorization: &Authorization,
    ) -> Result<Binding> {
        let BindRequest {
            store,
            service,
            version,
            expected,
            package: q,
            failure,
            pending,
        } = request;
        let old = self.consistent_control(store, service)?;
        if old.as_ref().map(|c| &c.revision) != expected.as_ref()
            || old.as_ref().and_then(|c| c.pending.as_deref()) != pending
        {
            return Err(fail("stale_snapshot"));
        }
        let authority = old
            .as_ref()
            .map(|c| c.authority.clone())
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let revision = uuid::Uuid::new_v4().to_string();
        let b = Binding {
            control_authority: authority.clone(),
            status: p::BindingStatus {
                store_id: store.into(),
                service_id: service.into(),
                service_version_id: version.into(),
                revision: revision.clone(),
                package: q.clone(),
                failure_mode: failure,
                authorized: true,
                error_code: None,
            },
            instance: authorization.instance.clone(),
            epoch: authorization.epoch,
            publisher_revision: authorization.publisher_revision,
        };
        let c = Control {
            store_id: store.into(),
            service_id: service.into(),
            revision,
            authority,
            version: Some(version.into()),
            head: Some(b.status.clone()),
            pending: pending.map(str::to_owned),
            pending_request_identity: pending.and_then(|_| {
                old.as_ref()
                    .and_then(|c| c.pending_request_identity.clone())
            }),
        };
        // Publication already has a durable pending marker. Keep it until the
        // binding and the completion receipt are both durable; direct binding
        // still installs its required marker before the first database write.
        if pending.is_none() {
            self.marker(&c)?;
        }
        let tx = self.db()?.unchecked_transaction().checked("unavailable")?;
        tx.execute(
            "INSERT OR REPLACE INTO policy_bindings VALUES(?1,?2,?3)",
            params![Self::scope(store, service), version, encode(&b)?],
        )
        .checked("unavailable")?;
        tx.execute(
            "INSERT OR REPLACE INTO policy_controls VALUES(?1,?2)",
            params![Self::scope(store, service), encode(&c)?],
        )
        .checked("unavailable")?;
        tx.commit().checked("unavailable")?;
        if pending.is_some() {
            self.marker(&c)?;
        }
        Ok(b)
    }
}

// Read once at startup, without creating or migrating this optional repository.
// A lost marker must not make an existing required binding look unbound.
fn reconcile_startup(data: &Path, presence: &mut Presence) {
    let records = (|| -> Result<Vec<(String, String)>> {
        let db = evidence::database(data)?;
        let mut query = db.prepare("SELECT CASE WHEN length(scope)<=128 THEN scope ELSE '' END, CASE WHEN length(record)<=4096 THEN record ELSE '' END FROM policy_controls LIMIT 257")
            .checked("invalid_record")?;
        query
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .checked("invalid_record")?
            .collect::<std::result::Result<Vec<_>, _>>()
            .checked("invalid_record")
    })();
    let known = records
        .as_ref()
        .into_iter()
        .flatten()
        .map(|(scope, _)| scope.as_str())
        .collect();
    evidence::missing_controls(data, &known, presence);
    let records = match records {
        Ok(rows) => rows,
        Err(_) => {
            // Existing markers still identify the scopes that require policy.
            // With no readable controls/markers, no lost binding is asserted.
            for (store, service) in presence.scopes.keys().cloned().collect::<Vec<_>>() {
                presence.mark_unknown(&store, &service);
            }
            return;
        }
    };
    if records.len() > 256 {
        presence.unavailable = true;
        return;
    }
    for (scope, record) in records {
        let Some((store, service)) = scope.split_once(':') else {
            presence.unavailable = true;
            continue;
        };
        let (Ok(_), Ok(_)) = (uuid::Uuid::parse_str(store), uuid::Uuid::parse_str(service)) else {
            presence.unavailable = true;
            continue;
        };
        let control = decode::<Control>(&record).ok().filter(|c| {
            c.store_id == store
                && c.service_id == service
                && uuid(&c.revision).is_ok()
                && uuid(&c.authority).is_ok()
                && c.pending.as_deref().is_none_or(|s| uuid(s).is_ok())
                && c.pending_request_identity
                    .as_deref()
                    .is_none_or(|s| package::unhex::<32>(s).is_ok())
                && (c.pending.is_some() || c.pending_request_identity.is_none())
                && match (&c.version, &c.head) {
                    (None, None) => true,
                    (Some(v), Some(b)) => {
                        uuid(v).is_ok()
                            && b.store_id == store
                            && b.service_id == service
                            && &b.service_version_id == v
                            && uuid(&b.revision).is_ok()
                    }
                    _ => false,
                }
        });
        let Some(c) = control else {
            presence.mark_unknown(store, service);
            continue;
        };
        let key = (store.into(), service.into());
        if let Some(marker) = presence.scopes.get(&key) {
            if marker != &c {
                presence.mark_unknown(store, service);
            }
        } else {
            if c.head.is_some() || c.version.is_some() || c.pending.is_some() {
                presence.mark_unknown(store, service);
            }
            presence.scopes.insert(key, c);
        }
    }
}
pub(super) fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::{
        io::Write,
        os::unix::fs::{MetadataExt, OpenOptionsExt},
    };
    if let Ok(m) = std::fs::symlink_metadata(path)
        && (!m.is_file() || m.file_type().is_symlink() || m.nlink() != 1)
    {
        return Err(fail("permission_denied"));
    }
    let parent = path.parent().ok_or(fail("invalid_input"))?;
    let tmp = parent.join(format!(".{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| -> std::io::Result<()> {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        std::fs::rename(&tmp, path)?;
        std::fs::File::open(parent)?.sync_all()
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(tmp);
    }
    result.checked("unavailable")
}

fn marker_name(store: &str, service: &str) -> Result<String> {
    let a = uuid::Uuid::parse_str(store).checked("invalid_input")?;
    let b = uuid::Uuid::parse_str(service).checked("invalid_input")?;
    Ok(format!(
        "{}-{}-{}.json",
        a.simple(),
        b.simple(),
        package::hash(Host::scope(store, service).as_bytes())
    ))
}
