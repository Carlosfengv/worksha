use super::*;
use crate::provider_models::ModelsRequest;
use local_agent_core::{
    Provider,
    domain::provider_access::Target,
    providers::{files, validate_endpoint},
};
use serde::{Deserialize, Serialize};
use std::{
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::PathBuf,
};

pub(super) const MAX_BINDINGS: usize = 128;
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Record {
    pub id: String,
    pub plugin_id: String,
    pub digest: String,
    pub epoch: u64,
    pub revision: u64,
    #[serde(default)]
    pub instance: String,
    pub target: Target,
    pub input: ModelsRequest,
    pub label: String,
    pub allow_private: bool,
    pub policy: a::Policy,
}
pub(super) fn resolve(target: &Target, providers: &[Provider]) -> Result<(ModelsRequest, String)> {
    let (input, label) = match target {
        Target::Saved {
            provider_id,
            expected_revision,
        } => {
            let p = providers
                .iter()
                .find(|p| &p.id == provider_id)
                .ok_or(fail("stale_snapshot"))?;
            if p.revision.to_string() != *expected_revision {
                return Err(fail("stale_snapshot"));
            }
            (
                ModelsRequest {
                    endpoint: p.endpoint.clone(),
                    protocol: p.protocol.clone(),
                    vendor: p.vendor.clone(),
                    credential_ref: p.credential_ref.clone(),
                },
                p.name.clone(),
            )
        }
        Target::Draft {
            label,
            endpoint,
            protocol,
            vendor,
            credential_ref,
        } => (
            ModelsRequest {
                endpoint: endpoint.clone(),
                protocol: protocol.clone(),
                vendor: vendor.clone(),
                credential_ref: credential_ref.clone(),
            },
            label.clone(),
        ),
    };
    if !crate::provider_models::safe_label(&label, 120, None)
        || input.endpoint.len() > 2048
        || input.vendor.len() > 50
        || !["messages", "responses", "chat-completions"].contains(&input.protocol.as_str())
        || input
            .credential_ref
            .as_ref()
            .is_some_and(|r| uuid::Uuid::parse_str(r).is_err())
    {
        return Err(fail("invalid_input"));
    }
    validate_endpoint(&input.endpoint).checked("invalid_input")?;
    Ok((input, label))
}
impl Record {
    fn key(&self) -> String {
        let scope = match &self.target {
            Target::Saved { provider_id, .. } => format!("saved:{provider_id}"),
            Target::Draft { .. } => format!(
                "draft:{}",
                package::hash(&serde_json::to_vec(&self.input).expect("catalog context"))
            ),
        };
        package::hash(
            serde_json::json!([self.plugin_id, scope])
                .to_string()
                .as_bytes(),
        )
    }
    pub(super) fn stem(&self) -> String {
        format!(
            "{}-{}",
            package::hash(self.plugin_id.as_bytes()),
            self.key()
        )
    }
    fn public(&self, error: Option<&str>) -> a::Binding {
        let (provider_id, expected_revision) = match &self.target {
            Target::Saved {
                provider_id,
                expected_revision,
            } => (Some(provider_id.clone()), Some(expected_revision.clone())),
            Target::Draft { .. } => (None, None),
        };
        a::Binding {
            binding_id: self.id.clone(),
            digest: self.digest.clone(),
            label: self.label.clone(),
            provider_id,
            expected_revision,
            endpoint: self.input.endpoint.clone(),
            protocol: self.input.protocol.clone(),
            allow_private_network: self.allow_private,
            authorized: error.is_none(),
            error_code: error.map(str::to_owned),
        }
    }
}
impl Manager {
    pub(super) fn catalog_directory(&self, create: bool) -> Result<Option<PathBuf>> {
        let path = self.root.join("provider-catalog");
        let m = match std::fs::symlink_metadata(&path) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && !create => return Ok(None),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                std::fs::create_dir(&path).checked("unavailable")?;
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))
                    .checked("unavailable")?;
                std::fs::symlink_metadata(&path).checked("unavailable")?
            }
            Err(_) => return Err(fail("unavailable")),
        };
        if !m.is_dir()
            || m.file_type().is_symlink()
            || m.uid() != unsafe { libc::geteuid() }
            || m.mode() & 0o077 != 0
        {
            return Err(fail("permission_denied"));
        }
        Ok(Some(path))
    }
    pub(super) fn catalog_records(&self, id: &str) -> Result<Vec<Record>> {
        let Some(root) = self.catalog_directory(false)? else {
            return Ok(vec![]);
        };
        let mut records = vec![];
        let mut count = 0;
        for entry in std::fs::read_dir(root).checked("unavailable")? {
            let path = entry.checked("unavailable")?.path();
            if !path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with(&format!("{}-", package::hash(id.as_bytes()))))
            {
                continue;
            }
            count += 1;
            if count > MAX_BINDINGS {
                return Err(fail("resource_limit"));
            }
            let stem = path
                .file_stem()
                .and_then(|n| n.to_str())
                .ok_or(fail("invalid_record"))?;
            let record = self.catalog_at(stem)?;
            if record.plugin_id != id || path.extension().and_then(|n| n.to_str()) != Some("json") {
                return Err(fail("invalid_record"));
            }
            records.push(record);
        }
        Ok(records)
    }
    pub(super) fn catalog_at(&self, stem: &str) -> Result<Record> {
        let root = self
            .catalog_directory(false)?
            .ok_or(fail("grant_revoked"))?;
        let snapshot = files::read(&root.join(format!("{stem}.json"))).checked("invalid_record")?;
        if snapshot.stamp.mode & 0o077 != 0 {
            return Err(fail("permission_denied"));
        }
        let bytes = snapshot.bytes.ok_or(fail("grant_revoked"))?;
        if bytes.len() > 16384 {
            return Err(fail("resource_limit"));
        }
        let record: Record = serde_json::from_slice(&bytes).checked("invalid_record")?;
        if record.stem() != stem || binding_stem(&record.plugin_id, &record.id)? != stem {
            return Err(fail("invalid_record"));
        }
        Ok(record)
    }
    pub(super) fn catalog_lease(&self, record: &Record) -> Result<()> {
        self.ensure_running()?;
        if record.instance != self.catalog_instance {
            return Err(fail("grant_revoked"));
        }
        let s = self.selection(&record.plugin_id)?;
        if !s.enabled {
            return Err(fail("plugin_disabled"));
        }
        if s.digest != record.digest || s.epoch != record.epoch || s.revision != record.revision {
            return Err(fail("grant_revoked"));
        }
        let current = self.catalog_at(&record.stem())?;
        if current != *record {
            return Err(fail("stale_snapshot"));
        }
        Ok(())
    }
    pub(super) fn catalog_check(&self, record: &Record, providers: &[Provider]) -> Result<()> {
        self.catalog_lease(record)?;
        if resolve(&record.target, providers)?.0 != record.input {
            return Err(fail("stale_snapshot"));
        }
        Ok(())
    }
    pub(crate) fn has_catalog_binding(&self, id: &str) -> Result<bool> {
        // An unavailable catalog namespace must not disable uninstall of an
        // unrelated legacy plugin. Retain this gate across non-catalog upgrades.
        let declared = {
            let db = self.db.lock().checked("unavailable")?;
            let mut stmt = db
                .prepare("SELECT manifest FROM packages WHERE plugin_id=?1")
                .checked("unavailable")?;
            let manifests = stmt
                .query_map([id], |r| r.get::<_, String>(0))
                .checked("unavailable")?;
            let mut declared = false;
            for manifest in manifests {
                let manifest: local_agent_core::plugin::external::Manifest =
                    serde_json::from_str(&manifest.checked("unavailable")?)
                        .checked("invalid_record")?;
                declared |= super::opted_in(&manifest);
            }
            declared
        };
        if !declared {
            return Ok(false);
        }
        let Some(root) = self.catalog_directory(false)? else {
            return Ok(false);
        };
        let prefix = format!("{}-", package::hash(id.as_bytes()));
        for entry in std::fs::read_dir(root).checked("unavailable")? {
            if entry
                .checked("unavailable")?
                .file_name()
                .to_str()
                .is_some_and(|n| n.starts_with(&prefix))
            {
                return Ok(true);
            }
        }
        Ok(false)
    }
    pub(crate) fn catalog_state(&self, q: &a::Query, providers: &[Provider]) -> Result<a::State> {
        let admission = self.catalog_plugin(q);
        let (policy, error) = match admission {
            Ok((_, p)) => (Some(p), None),
            Err(e) => (None, Some(e.code.to_owned())),
        };
        let bindings = self
            .catalog_records(&q.plugin_id)?
            .into_iter()
            .filter(|r| r.plugin_id == q.plugin_id)
            .map(|r| {
                let check = if r.digest != q.digest {
                    Err(fail("grant_revoked"))
                } else if error.is_some() {
                    Err(fail("permission_denied"))
                } else if policy.as_ref() != Some(&r.policy) {
                    Err(fail("grant_revoked"))
                } else {
                    self.catalog_check(&r, providers)
                };
                let error = check.err();
                r.public(error.as_ref().map(|e| e.code))
            })
            .collect();
        Ok(a::State {
            can_bind: policy.is_some(),
            policy,
            bindings,
            error_code: error,
        })
    }
    pub(crate) fn catalog_bind(
        &self,
        q: &a::Query,
        target: Target,
        allow_private: bool,
        providers: &[Provider],
    ) -> Result<a::State> {
        let mut record = self.catalog_proposal(q, target, allow_private, providers)?;
        let _guard = self.db.lock().checked("unavailable")?;
        let records = self.catalog_records(&q.plugin_id)?;
        if let Some(old) = records.iter().find(|r| r.key() == record.key()) {
            record.id = old.id.clone();
            if old == &record {
                // A legacy caller explicitly reusing an editor grant makes it
                // shared. An editor's eventual cleanup must retain that grant.
                self.share_owned_catalog(old)?;
                drop(_guard);
                return self.catalog_state(q, providers);
            }
        }
        let root = self.catalog_directory(true)?.ok_or(fail("unavailable"))?;
        if !records.iter().any(|r| r.key() == record.key())
            && std::fs::read_dir(&root)
                .checked("unavailable")?
                .take(MAX_BINDINGS)
                .count()
                >= MAX_BINDINGS
        {
            return Err(fail("resource_limit"));
        }
        record.id = format!("{}:{}", record.stem(), uuid::Uuid::new_v4());
        files::atomic_write(
            &root.join(format!("{}.json", record.stem())),
            &serde_json::to_vec(&record).checked("invalid_input")?,
            0o600,
        )
        .checked("unavailable")?;
        drop(_guard);
        self.catalog_state(q, providers)
    }
    pub(super) fn catalog_proposal(
        &self,
        q: &a::Query,
        target: Target,
        allow_private: bool,
        providers: &[Provider],
    ) -> Result<Record> {
        let (_, policy) = self.catalog_plugin(q)?;
        let s = self.selection(&q.plugin_id)?;
        let (input, label) = resolve(&target, providers)?;
        if input.protocol != policy.base_protocol {
            return Err(fail("incompatible_api"));
        }
        let mut record = Record {
            id: uuid::Uuid::new_v4().to_string(),
            plugin_id: q.plugin_id.clone(),
            digest: q.digest.clone(),
            epoch: s.epoch,
            revision: s.revision,
            instance: self.catalog_instance.clone(),
            target,
            input,
            label,
            allow_private,
            policy,
        };
        record.id = format!("{}:{}", record.stem(), uuid::Uuid::new_v4());
        Ok(record)
    }
    pub(crate) fn catalog_unbind(
        &self,
        q: &a::Query,
        id: &str,
        providers: &[Provider],
    ) -> Result<a::State> {
        self.ensure_running()?;
        let stem = binding_stem(&q.plugin_id, id)?;
        let _guard = self.db.lock().checked("unavailable")?;
        let record = self.catalog_at(stem)?;
        if record.id == id && record.plugin_id == q.plugin_id {
            let root = self.catalog_directory(false)?.ok_or(fail("unavailable"))?;
            std::fs::remove_file(root.join(format!("{}.json", record.stem())))
                .checked("unavailable")?;
            std::fs::File::open(root)
                .checked("unavailable")?
                .sync_all()
                .checked("unavailable")?;
        } else {
            return Err(fail("not_found"));
        }
        drop(_guard);
        self.catalog_state(q, providers)
    }
    pub(super) fn catalog_record(
        &self,
        q: &a::Query,
        id: &str,
        target: &Target,
        providers: &[Provider],
    ) -> Result<Record> {
        let record = self.catalog_at(binding_stem(&q.plugin_id, id)?)?;
        if record.plugin_id != q.plugin_id || record.id != id || record.digest != q.digest {
            return Err(fail("permission_denied"));
        }
        if &record.target != target {
            return Err(fail("stale_snapshot"));
        }
        self.catalog_check(&record, providers)?;
        let (_, policy) = self.catalog_plugin(q)?;
        if policy != record.policy {
            return Err(fail("grant_revoked"));
        }
        Ok(record)
    }
}
fn binding_stem<'a>(plugin: &str, id: &'a str) -> Result<&'a str> {
    let (stem, nonce) = id.split_once(':').ok_or(fail("invalid_input"))?;
    if stem.len() != 129
        || !stem.starts_with(&format!("{}-", package::hash(plugin.as_bytes())))
        || !stem[65..].bytes().all(|b| b.is_ascii_hexdigit())
        || uuid::Uuid::parse_str(nonce).is_err()
    {
        return Err(fail("invalid_input"));
    }
    Ok(stem)
}

#[cfg(test)]
mod tests {
    use super::*;
    use local_agent_core::providers::vault::MemoryVault;
    use std::sync::Arc;

    fn record(manager: &Manager) -> Record {
        let target = Target::Draft {
            label: "Owned catalog".into(),
            endpoint: "https://example.com/common/responses".into(),
            protocol: "responses".into(),
            vendor: "custom".into(),
            credential_ref: Some(uuid::Uuid::new_v4().to_string()),
        };
        let (input, label) = resolve(&target, &[]).unwrap();
        let mut record = Record {
            id: String::new(),
            plugin_id: "org.example.catalog".into(),
            digest: "a".repeat(64),
            epoch: 1,
            revision: 1,
            instance: manager.catalog_instance.clone(),
            target,
            input,
            label,
            allow_private: false,
            policy: a::Policy {
                schema_version: 1,
                base_protocol: "responses".into(),
                path: "/catalog".into(),
            },
        };
        record.id = format!("{}:{}", record.stem(), uuid::Uuid::new_v4());
        record
    }
    fn persist(manager: &Manager, record: &Record) -> PathBuf {
        let root = manager.catalog_directory(true).unwrap().unwrap();
        let path = root.join(format!("{}.json", record.stem()));
        files::atomic_write(&path, &serde_json::to_vec(record).unwrap(), 0o600).unwrap();
        path
    }
    #[test]
    fn catalog_leases_expire_on_restart_and_replacement_without_deleting_records() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let vault = Arc::new(MemoryVault::default());
        let manager = Manager::open(&root, vault.clone()).unwrap();
        let original = record(&manager);
        manager
            .db
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO selections(plugin_id,digest,enabled,status) VALUES(?1,?2,1,'active')",
                rusqlite::params![original.plugin_id, original.digest],
            )
            .unwrap();
        let path = persist(&manager, &original);
        manager.catalog_check(&original, &[]).unwrap();
        let public = serde_json::to_string(&original.public(None)).unwrap();
        assert!(!public.contains(original.input.credential_ref.as_ref().unwrap()));
        assert!(binding_stem("org.example.foreign", &original.id).is_err());
        let before = std::fs::read(&path).unwrap();
        let restarted = Manager::open(&root, vault).unwrap();
        assert_eq!(
            restarted.catalog_lease(&original).unwrap_err().code,
            "grant_revoked"
        );
        assert_eq!(std::fs::read(&path).unwrap(), before);
        let mut replaced = original.clone();
        replaced.id = format!("{}:{}", replaced.stem(), uuid::Uuid::new_v4());
        persist(&manager, &replaced);
        assert_eq!(
            manager.catalog_lease(&original).unwrap_err().code,
            "stale_snapshot"
        );
        manager.catalog_lease(&replaced).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(
            manager.catalog_lease(&replaced).unwrap_err().code,
            "permission_denied"
        );
        assert_eq!(
            manager
                .catalog_records(&replaced.plugin_id)
                .err()
                .unwrap()
                .code,
            "permission_denied"
        );
    }
    #[test]
    fn unrelated_catalog_records_and_storage_failure_do_not_break_legacy_uninstall() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let manager = Manager::open(&root, Arc::new(MemoryVault::default())).unwrap();
        let original = record(&manager);
        let path = persist(&manager, &original);
        std::fs::write(&path, b"invalid-owned-record").unwrap();
        assert!(manager.catalog_records(&original.plugin_id).is_err());
        assert!(
            manager
                .catalog_records("org.example.unrelated")
                .unwrap()
                .is_empty()
        );
        let root = manager.root.join("provider-catalog");
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(!manager.has_catalog_binding("org.example.legacy").unwrap());
    }
    #[test]
    fn saved_catalog_targets_require_the_exact_provider_revision_and_credential_scope() {
        let provider: Provider = serde_json::from_value(serde_json::json!({
            "id":"owned", "name":"Owned", "endpoint":"https://example.com/common",
            "protocol":"responses", "model":"model", "configured":true,
            "credentialRef":uuid::Uuid::new_v4().to_string(), "revision":7
        }))
        .unwrap();
        let target = Target::Saved {
            provider_id: provider.id.clone(),
            expected_revision: "7".into(),
        };
        assert_eq!(
            resolve(&target, std::slice::from_ref(&provider))
                .unwrap()
                .0
                .credential_ref,
            provider.credential_ref
        );
        for expected_revision in ["6", "07", "8", "18446744073709551616"] {
            let target = Target::Saved {
                provider_id: provider.id.clone(),
                expected_revision: expected_revision.into(),
            };
            assert_eq!(
                resolve(&target, std::slice::from_ref(&provider))
                    .err()
                    .unwrap()
                    .code,
                "stale_snapshot"
            );
        }
        assert_eq!(resolve(&target, &[]).err().unwrap().code, "stale_snapshot");
    }
}
