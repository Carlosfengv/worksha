//! Exclusive editor grants have durable host ownership; borrowing never owns deletion.
use super::{Checked, Manager, Result, a, fail, package, records};
use local_agent_core::{
    domain::{
        provider_connection::Selection,
        provider_draft_grant::{
            DraftGrantOwner, DraftGrantReceipt, DraftGrantScope, DraftGrantState,
        },
        provider_source_operation::SourceRequest,
    },
    providers::files,
};
use serde::{Deserialize, Serialize};
use std::{os::unix::fs::MetadataExt, path::PathBuf};

const LIMIT: usize = 128;
const MAX_RECORD: usize = 32768;

mod journal;
mod recovery;
pub(crate) use recovery::RecoverySnapshot;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Pin {
    pub operation_id: String,
    pub request_hash: String,
    pub source_revision: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct OwnerRecord {
    version: u32,
    owner: DraftGrantOwner,
    scope: DraftGrantScope,
    opened_at: i64,
    instance: String,
    grant: Option<records::Record>,
    published: bool,
    managed: bool,
    operation: Option<Pin>,
}

pub(crate) fn request_hash(
    input: &local_agent_core::providers::operations::Request,
) -> Result<String> {
    Ok(package::hash(
        &serde_json::to_vec(input).checked("invalid_input")?,
    ))
}
fn target(scope: &DraftGrantScope) -> a::Target {
    a::Target::Draft {
        label: scope.label.clone(),
        endpoint: scope.draft.endpoint.clone(),
        protocol: scope.draft.protocol.clone(),
        vendor: scope.draft.vendor.clone(),
        credential_ref: scope.draft.credential_ref.clone(),
    }
}
fn selection(record: &records::Record) -> Selection {
    Selection {
        plugin_id: record.plugin_id.clone(),
        digest: record.digest.clone(),
        binding_id: record.id.clone(),
        target: record.target.clone(),
    }
}
fn receipt(record: &OwnerRecord, state: DraftGrantState) -> DraftGrantReceipt {
    DraftGrantReceipt {
        owner: record.owner.clone(),
        state,
        selection: record.grant.as_ref().map(selection),
    }
}

impl Manager {
    fn owner_directory(&self, create: bool) -> Result<PathBuf> {
        let root = self.root.join("draft-catalog-owners");
        if create
            && std::fs::symlink_metadata(&root)
                .is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound)
        {
            files::private_dir(&root).checked("unavailable")?;
        }
        if root.exists() {
            let m = std::fs::symlink_metadata(&root).checked("unavailable")?;
            if !m.is_dir()
                || m.file_type().is_symlink()
                || m.uid() != unsafe { libc::geteuid() }
                || m.mode() & 0o077 != 0
            {
                return Err(fail("permission_denied"));
            }
        }
        Ok(root)
    }
    fn owner_path(&self, owner: &DraftGrantOwner, create: bool) -> Result<PathBuf> {
        if !local_agent_core::providers::operations::valid_id(&owner.store_id)
            || !local_agent_core::providers::operations::valid_id(&owner.owner_id)
        {
            return Err(fail("invalid_input"));
        }
        Ok(self
            .owner_directory(create)?
            .join(format!("{}-{}.json", owner.store_id, owner.owner_id)))
    }
    fn read_owner(&self, owner: &DraftGrantOwner) -> Result<Option<OwnerRecord>> {
        self.read_owner_record(owner, true)
    }
    fn read_owner_record(
        &self,
        owner: &DraftGrantOwner,
        recover_request: bool,
    ) -> Result<Option<OwnerRecord>> {
        let s = files::read(&self.owner_path(owner, false)?).checked("invalid_record")?;
        let Some(bytes) = s.bytes else {
            return Ok(None);
        };
        if bytes.len() > MAX_RECORD || s.stamp.mode & 0o077 != 0 {
            return Err(fail("invalid_record"));
        }
        let mut record: OwnerRecord = serde_json::from_slice(&bytes).checked("invalid_record")?;
        if record.version != 1
            || record.owner != *owner
            || (record.published && record.grant.is_none())
            || (record.operation.is_some() && !record.published)
        {
            return Err(fail("invalid_record"));
        }
        let (input, _) = records::resolve(&target(&record.scope), &[])?;
        if let Some(g) = &record.grant
            && (g.plugin_id != record.scope.catalog.plugin_id
                || g.digest != record.scope.catalog.digest
                || g.input != input
                || g.allow_private != record.scope.allow_private_network
                || !matches!(g.target, a::Target::Draft { .. }))
        {
            return Err(fail("invalid_record"));
        }
        // A crash after the request copy, before the small pin, must not make
        // an attached operation appear idle and eligible for grant deletion.
        if recover_request
            && record.operation.is_none()
            && let Some(request) = self.read_draft_request(owner)?
        {
            Self::check_draft_request(&record, &request)?;
            record.operation = Some(journal::pin(&request)?);
        }
        Ok(Some(record))
    }
    fn write_owner(&self, record: &OwnerRecord) -> Result<()> {
        let bytes = serde_json::to_vec(record).checked("invalid_input")?;
        if bytes.len() > MAX_RECORD {
            return Err(fail("resource_limit"));
        }
        files::atomic_write(&self.owner_path(&record.owner, true)?, &bytes, 0o600)
            .checked("unavailable")
    }
    fn remove_owner(&self, owner: &DraftGrantOwner) -> Result<()> {
        let path = self.owner_path(owner, false)?;
        std::fs::remove_file(&path).checked("unavailable")?;
        std::fs::File::open(path.parent().ok_or(fail("unavailable"))?)
            .checked("unavailable")?
            .sync_all()
            .checked("unavailable")
    }
    fn owners(&self) -> Result<Vec<OwnerRecord>> {
        self.owner_records(true)
    }
    fn owner_records(&self, recover_request: bool) -> Result<Vec<OwnerRecord>> {
        let root = self.owner_directory(false)?;
        if !root.exists() {
            return Ok(vec![]);
        }
        let entries = std::fs::read_dir(&root)
            .checked("unavailable")?
            .take(LIMIT + 1)
            .collect::<std::result::Result<Vec<_>, _>>()
            .checked("unavailable")?;
        if entries.len() > LIMIT {
            return Err(fail("resource_limit"));
        }
        entries
            .into_iter()
            .map(|e| {
                let n = e.file_name();
                let name = n.to_str().ok_or(fail("invalid_record"))?;
                // Both IDs are canonical UUIDs; do not split at UUID hyphens.
                if !name.is_ascii()
                    || name.len() != 78
                    || &name[36..37] != "-"
                    || !name.ends_with(".json")
                {
                    return Err(fail("invalid_record"));
                }
                let owner = DraftGrantOwner {
                    store_id: name[..36].into(),
                    owner_id: name[37..73].into(),
                };
                self.read_owner_record(&owner, recover_request)?
                    .ok_or(fail("invalid_record"))
            })
            .collect()
    }
    /// Caller holds the plugin metadata mutex, as does legacy Bind's reuse path.
    pub(super) fn share_owned_catalog(&self, grant: &records::Record) -> Result<()> {
        for mut owner in self.owners()? {
            if owner.grant.as_ref().is_some_and(|g| g.id == grant.id) && owner.managed {
                owner.managed = false;
                self.write_owner(&owner)?;
            }
        }
        Ok(())
    }
    pub(crate) fn open_draft_owner(
        &self,
        store_id: String,
        scope: DraftGrantScope,
    ) -> Result<DraftGrantReceipt> {
        // Validate the scope without creating authority. A lost Open reply can
        // leave only an empty intent, never an unowned network grant.
        self.catalog_proposal(
            &scope.catalog,
            target(&scope),
            scope.allow_private_network,
            &[],
        )?;
        let _guard = self.db.lock().checked("unavailable")?;
        let mut count = 0;
        for old in self.owners()? {
            if old.grant.is_none()
                && (old.instance != self.catalog_instance
                    || chrono::Utc::now().timestamp().saturating_sub(old.opened_at) > 300)
            {
                self.remove_owner(&old.owner)?;
            } else {
                count += 1;
            }
        }
        if count >= LIMIT {
            return Err(fail("resource_limit"));
        }
        let record = OwnerRecord {
            version: 1,
            owner: DraftGrantOwner {
                store_id,
                owner_id: uuid::Uuid::new_v4().to_string(),
            },
            scope,
            opened_at: chrono::Utc::now().timestamp(),
            instance: self.catalog_instance.clone(),
            grant: None,
            published: false,
            managed: false,
            operation: None,
        };
        self.write_owner(&record)?;
        Ok(receipt(&record, DraftGrantState::Prepared))
    }
    pub(crate) fn acquire_draft_owner(&self, owner: &DraftGrantOwner) -> Result<DraftGrantReceipt> {
        let initial = self.read_owner(owner)?.ok_or(fail("not_found"))?;
        let proposal = self.catalog_proposal(
            &initial.scope.catalog,
            target(&initial.scope),
            initial.scope.allow_private_network,
            &[],
        )?;
        let _guard = self.db.lock().checked("unavailable")?;
        let mut record = self.read_owner(owner)?.ok_or(fail("not_found"))?;
        if record.instance != self.catalog_instance {
            return Err(fail("grant_revoked"));
        }
        if let Some(g) = &record.grant {
            let current = self.catalog_at(&g.stem());
            if record.published {
                if current? != *g
                    || g.epoch != proposal.epoch
                    || g.revision != proposal.revision
                    || g.instance != proposal.instance
                {
                    return Err(fail("grant_revoked"));
                }
                return Ok(receipt(
                    &record,
                    if record.managed {
                        DraftGrantState::Acquired
                    } else {
                        DraftGrantState::Borrowed
                    },
                ));
            }
            if let Ok(current) = current {
                if current != *g {
                    return Err(fail("stale_snapshot"));
                }
            } else if current.err().is_some_and(|e| e.code != "grant_revoked") {
                return Err(fail("invalid_record"));
            }
        } else {
            let current = self.catalog_at(&proposal.stem());
            match current {
                Ok(old)
                    if old.epoch == proposal.epoch
                        && old.revision == proposal.revision
                        && old.instance == proposal.instance
                        && old.digest == proposal.digest =>
                {
                    if old.input != proposal.input
                        || old.policy != proposal.policy
                        || old.allow_private != proposal.allow_private
                    {
                        return Err(fail("permission_denied"));
                    }
                    // Managed editors retain a reference to the same exact
                    // host-created grant. Legacy grants never acquire ownership.
                    record.managed = self
                        .owners()?
                        .iter()
                        .any(|r| r.managed && r.grant.as_ref() == Some(&old));
                    record.grant = Some(old);
                    record.published = true;
                    self.write_owner(&record)?;
                    return Ok(receipt(
                        &record,
                        if record.managed {
                            DraftGrantState::Acquired
                        } else {
                            DraftGrantState::Borrowed
                        },
                    ));
                }
                Ok(_) => {}
                Err(e) if e.code == "grant_revoked" => {}
                Err(e) => return Err(e),
            }
            let root = self.catalog_directory(true)?.ok_or(fail("unavailable"))?;
            if std::fs::read_dir(&root)
                .checked("unavailable")?
                .take(records::MAX_BINDINGS)
                .count()
                >= records::MAX_BINDINGS
            {
                return Err(fail("resource_limit"));
            }
            record.grant = Some(proposal);
            record.managed = true;
            // Intent is durable BEFORE the grant. Retry has the same nonce;
            // releasing first makes a late Acquire fail, never recreate it.
            self.write_owner(&record)?;
        }
        let g = record.grant.as_ref().ok_or(fail("invalid_record"))?;
        let root = self.catalog_directory(true)?.ok_or(fail("unavailable"))?;
        files::atomic_write(
            &root.join(format!("{}.json", g.stem())),
            &serde_json::to_vec(g).checked("invalid_input")?,
            0o600,
        )
        .checked("unavailable")?;
        record.published = true;
        self.write_owner(&record)?;
        Ok(receipt(&record, DraftGrantState::Acquired))
    }
    pub(crate) fn attach_draft_owner(
        &self,
        owner: &DraftGrantOwner,
        request: &SourceRequest,
    ) -> Result<DraftGrantReceipt> {
        let _guard = self.db.lock().checked("unavailable")?;
        let mut record = self.read_owner(owner)?.ok_or(fail("not_found"))?;
        let g = record.grant.as_ref().ok_or(fail("permission_denied"))?;
        let p = &request.operation.provider;
        if !record.published
            || request.operation.store_id != owner.store_id
            || request.source.catalog() != Some(&selection(g))
            || p.endpoint != g.input.endpoint
            || p.protocol != g.input.protocol
            || p.vendor != g.input.vendor
            || p.credential_ref != g.input.credential_ref
        {
            return Err(fail("permission_denied"));
        }
        let pin = Pin {
            operation_id: request.operation.operation_id.clone(),
            request_hash: request_hash(&request.operation)?,
            source_revision: request.expected_source_revision.clone(),
        };
        if let Some(old) = &record.operation
            && (old.operation_id != pin.operation_id
                || old.request_hash != pin.request_hash
                || old.source_revision != pin.source_revision)
        {
            return Err(fail("stale_snapshot"));
        }
        // Unlike the old hash-only records this protected copy permits an
        // independent recovery view to cancel an unadmitted original request.
        // Preserve the original small owner record format and write ordering.
        self.write_draft_request(&record, request)?;
        if record.operation.is_none() {
            record.operation = Some(pin);
            self.write_owner(&record)?;
        }
        Ok(receipt(&record, DraftGrantState::Pending))
    }
    pub(crate) fn release_draft_owner(
        &self,
        owner: &DraftGrantOwner,
        safe: impl FnOnce(Option<&Pin>, Option<&Selection>, bool) -> Result<bool>,
    ) -> Result<DraftGrantReceipt> {
        let _guard = self.db.lock().checked("unavailable")?;
        let Some(record) = self.read_owner(owner)? else {
            return Ok(DraftGrantReceipt {
                owner: owner.clone(),
                state: DraftGrantState::Released,
                selection: None,
            });
        };
        let request = self.read_draft_request(owner)?;
        if let Some(request) = &request {
            Self::check_draft_request(&record, request)?;
        }
        let request_hash = request.as_ref().map(|r| journal::hash(r)).transpose()?;
        let selected = record.grant.as_ref().map(selection);
        let last_managed = record.managed
            && !self.owners()?.iter().any(|other| {
                other.owner != record.owner && other.grant.is_some() && other.grant == record.grant
            });
        if !safe(record.operation.as_ref(), selected.as_ref(), last_managed)? {
            return Ok(receipt(&record, DraftGrantState::Pending));
        }
        // Publication is guarded against a restored/changed private snapshot
        // during the SQL proof, before removing any exact grant or request copy.
        self.check_draft_request_hash(owner, request_hash.as_deref())?;
        let current = self.read_owner(owner)?.ok_or(fail("stale_snapshot"))?;
        if serde_json::to_vec(&current).checked("invalid_record")?
            != serde_json::to_vec(&record).checked("invalid_record")?
        {
            return Err(fail("stale_snapshot"));
        }
        if last_managed && let Some(g) = &record.grant {
            match self.catalog_at(&g.stem()) {
                Ok(current) if current == *g => {
                    let root = self.catalog_directory(false)?.ok_or(fail("unavailable"))?;
                    std::fs::remove_file(root.join(format!("{}.json", g.stem())))
                        .checked("unavailable")?;
                    std::fs::File::open(root)
                        .checked("unavailable")?
                        .sync_all()
                        .checked("unavailable")?;
                }
                Ok(_) => {} // Replacement belongs to someone else.
                Err(e) if e.code == "grant_revoked" => {}
                Err(e) => return Err(e),
            }
        }
        self.remove_draft_request(owner, request_hash.as_deref())?;
        self.remove_owner(owner)?;
        Ok(DraftGrantReceipt {
            owner: owner.clone(),
            state: DraftGrantState::Released,
            selection: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use local_agent_core::{
        domain::provider_draft_catalog::DraftEndpoint, providers::vault::MemoryVault,
    };
    use std::{os::unix::fs::PermissionsExt, sync::Arc};

    fn fixture() -> (tempfile::TempDir, Manager, OwnerRecord) {
        let temp = tempfile::tempdir().unwrap();
        let manager = Manager::open(
            &temp.path().canonicalize().unwrap(),
            Arc::new(MemoryVault::default()),
        )
        .unwrap();
        let scope = DraftGrantScope {
            catalog: a::Query {
                plugin_id: "org.example.owner".into(),
                digest: "a".repeat(64),
            },
            draft: DraftEndpoint {
                endpoint: "http://127.0.0.1:11434/common".into(),
                protocol: "responses".into(),
                vendor: "custom".into(),
                credential_ref: None,
            },
            label: "Owned draft".into(),
            allow_private_network: true,
        };
        let (input, label) = records::resolve(&target(&scope), &[]).unwrap();
        let mut g = records::Record {
            id: String::new(),
            plugin_id: scope.catalog.plugin_id.clone(),
            digest: scope.catalog.digest.clone(),
            epoch: 1,
            revision: 1,
            instance: manager.catalog_instance.clone(),
            target: target(&scope),
            input,
            label,
            allow_private: true,
            policy: a::Policy {
                schema_version: 1,
                base_protocol: "responses".into(),
                path: "/catalog".into(),
            },
        };
        g.id = format!("{}:{}", g.stem(), uuid::Uuid::new_v4());
        let record = OwnerRecord {
            version: 1,
            owner: DraftGrantOwner {
                store_id: uuid::Uuid::new_v4().to_string(),
                owner_id: uuid::Uuid::new_v4().to_string(),
            },
            scope,
            opened_at: chrono::Utc::now().timestamp(),
            instance: manager.catalog_instance.clone(),
            grant: Some(g),
            published: true,
            managed: true,
            operation: None,
        };
        manager.write_owner(&record).unwrap();
        publish(&manager, record.grant.as_ref().unwrap());
        (temp, manager, record)
    }
    fn grant_path(manager: &Manager, grant: &records::Record) -> PathBuf {
        manager
            .catalog_directory(true)
            .unwrap()
            .unwrap()
            .join(format!("{}.json", grant.stem()))
    }
    fn publish(manager: &Manager, grant: &records::Record) {
        files::atomic_write(
            &grant_path(manager, grant),
            &serde_json::to_vec(grant).unwrap(),
            0o600,
        )
        .unwrap();
    }
    #[test]
    fn exclusive_release_is_idempotent_and_late_acquire_cannot_recreate_authority() {
        let (_tmp, manager, r) = fixture();
        let path = grant_path(&manager, r.grant.as_ref().unwrap());
        manager
            .release_draft_owner(&r.owner, |_, _, _| Ok(true))
            .unwrap();
        assert!(!path.exists());
        assert!(matches!(
            manager
                .release_draft_owner(&r.owner, |_, _, _| panic!("already released"))
                .unwrap()
                .state,
            DraftGrantState::Released
        ));
        assert_eq!(
            manager.acquire_draft_owner(&r.owner).err().unwrap().code,
            "not_found"
        );
        assert!(!path.exists());
    }
    #[test]
    fn shared_and_borrowed_grants_are_never_deleted_by_editor_finalization() {
        let (_tmp, manager, r) = fixture();
        let g = r.grant.as_ref().unwrap();
        let path = grant_path(&manager, g);
        let before = std::fs::read(&path).unwrap();
        manager.share_owned_catalog(g).unwrap();
        assert!(!manager.read_owner(&r.owner).unwrap().unwrap().managed);
        manager
            .release_draft_owner(&r.owner, |_, _, _| Ok(true))
            .unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }
    #[test]
    fn managed_editors_release_their_reference_and_only_the_last_can_remove_the_grant() {
        let (_tmp, manager, a) = fixture();
        let mut b = a.clone();
        b.owner.owner_id = uuid::Uuid::new_v4().to_string();
        manager.write_owner(&b).unwrap();
        let path = grant_path(&manager, a.grant.as_ref().unwrap());
        manager
            .release_draft_owner(&a.owner, |_, _, last| {
                assert!(!last);
                Ok(true)
            })
            .unwrap();
        assert!(path.exists());
        manager
            .release_draft_owner(&b.owner, |_, _, last| {
                assert!(last);
                Ok(true)
            })
            .unwrap();
        assert!(!path.exists());
    }
    #[test]
    fn pending_work_preserves_both_records_and_replacements_are_not_revoked() {
        let (_tmp, manager, mut r) = fixture();
        r.operation = Some(Pin {
            operation_id: uuid::Uuid::new_v4().to_string(),
            request_hash: "a".repeat(64),
            source_revision: None,
        });
        manager.write_owner(&r).unwrap();
        let before = std::fs::read(manager.owner_path(&r.owner, false).unwrap()).unwrap();
        let reply = manager
            .release_draft_owner(&r.owner, |pin, selected, _| {
                assert!(pin.is_some() && selected.is_some());
                Ok(false)
            })
            .unwrap();
        assert!(matches!(reply.state, DraftGrantState::Pending));
        assert_eq!(
            std::fs::read(manager.owner_path(&r.owner, false).unwrap()).unwrap(),
            before
        );
        let mut replacement = r.grant.as_ref().unwrap().clone();
        replacement.id = format!("{}:{}", replacement.stem(), uuid::Uuid::new_v4());
        publish(&manager, &replacement);
        let path = grant_path(&manager, &replacement);
        let bytes = std::fs::read(&path).unwrap();
        manager
            .release_draft_owner(&r.owner, |_, _, _| Ok(true))
            .unwrap();
        assert_eq!(std::fs::read(path).unwrap(), bytes);
    }
    #[test]
    fn crash_between_intent_grant_and_acknowledgement_keeps_exact_cleanup_proof() {
        for written in [false, true] {
            let (_tmp, manager, mut r) = fixture();
            let path = grant_path(&manager, r.grant.as_ref().unwrap());
            r.published = false;
            manager.write_owner(&r).unwrap();
            if !written {
                std::fs::remove_file(&path).unwrap();
            }
            manager
                .release_draft_owner(&r.owner, |_, _, _| Ok(true))
                .unwrap();
            assert!(!path.exists());
        }
    }
    #[test]
    fn restart_can_finalize_old_exclusive_grants_without_restoring_permission() {
        let (temp, manager, r) = fixture();
        let restarted = Manager::open(
            &temp.path().canonicalize().unwrap(),
            Arc::new(MemoryVault::default()),
        )
        .unwrap();
        assert_ne!(r.instance, restarted.catalog_instance);
        assert_eq!(
            restarted
                .catalog_lease(r.grant.as_ref().unwrap())
                .err()
                .unwrap()
                .code,
            "grant_revoked"
        );
        restarted
            .release_draft_owner(&r.owner, |_, _, _| Ok(true))
            .unwrap();
        assert!(!grant_path(&manager, r.grant.as_ref().unwrap()).exists());
    }
    #[test]
    fn unsafe_owner_metadata_is_preserved_without_repair_or_panics() {
        let (_tmp, manager, r) = fixture();
        let root = manager.owner_directory(false).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(
            manager.owner_directory(true).err().unwrap().code,
            "permission_denied"
        );
        assert_eq!(std::fs::metadata(&root).unwrap().mode() & 0o777, 0o755);
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        let bad = root.join(format!("{}.json", "é".repeat(36)));
        std::fs::write(&bad, b"corrupt").unwrap();
        assert!(manager.owners().is_err());
        assert!(manager.read_owner(&r.owner).unwrap().is_some());
    }
    fn recovery_request(record: &OwnerRecord) -> SourceRequest {
        let g = record.grant.as_ref().unwrap();
        serde_json::from_value(serde_json::json!({
            "operation": {
                "storeId": record.owner.store_id,
                "operationId": uuid::Uuid::new_v4().to_string(),
                "expectedSequence": 0,
                "kind": "save",
                "provider": {
                    "id": format!("custom-{}", uuid::Uuid::new_v4()),
                    "name": "Original recovery request",
                    "endpoint": g.input.endpoint,
                    "protocol": g.input.protocol,
                    "vendor": g.input.vendor,
                    "credentialRef": g.input.credential_ref,
                    "credentialLabel": null,
                    "model": "original-model",
                    "models": ["original-model"],
                    "disabledModels": [],
                    "modelCatalog": null,
                    "configured": false,
                    "revision": 0,
                    "category": "local"
                }
            },
            "source": {"kind":"external", "catalog": selection(g)},
            "expectedSourceRevision": null
        }))
        .unwrap()
    }
    #[test]
    fn request_copy_is_private_immutable_and_a_crash_before_the_small_pin_is_not_idle() {
        let (_tmp, manager, record) = fixture();
        let request = recovery_request(&record);
        let owner_path = manager.owner_path(&record.owner, false).unwrap();
        let before = std::fs::read(&owner_path).unwrap();
        manager.write_draft_request(&record, &request).unwrap();
        assert_eq!(std::fs::read(&owner_path).unwrap(), before);
        let root = manager.root.join("draft-catalog-requests");
        assert_eq!(std::fs::metadata(&root).unwrap().mode() & 0o777, 0o700);
        let file = root.join(format!(
            "{}-{}.json",
            record.owner.store_id, record.owner.owner_id
        ));
        assert_eq!(std::fs::metadata(&file).unwrap().mode() & 0o777, 0o600);
        let loaded = manager.read_owner(&record.owner).unwrap().unwrap();
        assert_eq!(
            loaded.operation.as_ref().unwrap().operation_id,
            request.operation.operation_id
        );
        assert_eq!(
            loaded.operation.as_ref().unwrap().request_hash,
            request_hash(&request.operation).unwrap()
        );
        let unchanged = std::fs::read(&file).unwrap();
        manager.write_draft_request(&record, &request).unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), unchanged);
        let mut other = request.clone();
        other.operation.operation_id = uuid::Uuid::new_v4().to_string();
        assert_eq!(
            manager
                .write_draft_request(&record, &other)
                .err()
                .unwrap()
                .code,
            "stale_snapshot"
        );
        assert_eq!(std::fs::read(file).unwrap(), unchanged);
        assert!(matches!(
            manager
                .release_draft_owner(&record.owner, |pin, _, _| {
                    assert!(pin.is_some());
                    Ok(false)
                })
                .unwrap()
                .state,
            DraftGrantState::Pending
        ));
        assert!(grant_path(&manager, record.grant.as_ref().unwrap()).exists());
    }
    #[test]
    fn inventory_reads_small_metadata_without_decoding_even_a_damaged_request_body() {
        let (_tmp, manager, record) = fixture();
        let request = recovery_request(&record);
        manager.write_draft_request(&record, &request).unwrap();
        let path = manager.root.join("draft-catalog-requests").join(format!(
            "{}-{}.json",
            record.owner.store_id, record.owner.owner_id
        ));
        // Copy-before-pin crash state is not reported as idle, and neither an
        // oversized nor malformed body is decoded by the metadata inventory.
        for bytes in [b"{malformed".to_vec(), vec![b'x'; 2 * 1024 * 1024 + 1]] {
            files::atomic_write(&path, &bytes, 0o600).unwrap();
            let rows = manager
                .draft_recovery_entries(&record.owner.store_id)
                .unwrap();
            assert_eq!(rows.len(), 1);
            assert!(matches!(
                rows[0].state,
                local_agent_core::domain::provider_draft_recovery::DraftRecoveryState::Unchecked
            ));
            assert!(!rows[0].can_finish);
            assert!(
                manager
                    .draft_recovery_snapshot(&record.owner, true)
                    .is_err()
            );
            assert_eq!(std::fs::read(&path).unwrap(), bytes);
            assert!(grant_path(&manager, record.grant.as_ref().unwrap()).exists());
        }
    }
    #[test]
    fn an_owner_restored_during_orphan_sql_proof_preserves_the_request_and_grant() {
        let (_tmp, manager, record) = fixture();
        let request = recovery_request(&record);
        manager.write_draft_request(&record, &request).unwrap();
        manager.remove_owner(&record.owner).unwrap();
        let path = manager.root.join("draft-catalog-requests").join(format!(
            "{}-{}.json",
            record.owner.store_id, record.owner.owner_id
        ));
        let before = std::fs::read(&path).unwrap();
        let error = manager
            .finish_orphan_draft_request(&record.owner, |_| {
                manager.write_owner(&record)?;
                Ok(true)
            })
            .err()
            .unwrap();
        assert_eq!(error.code, "stale_snapshot");
        assert_eq!(std::fs::read(path).unwrap(), before);
        assert!(manager.read_owner(&record.owner).unwrap().is_some());
        assert!(grant_path(&manager, record.grant.as_ref().unwrap()).exists());
    }
    #[test]
    fn orphan_request_can_be_discovered_without_claiming_the_missing_grant_ownership() {
        let (_tmp, manager, record) = fixture();
        let request = recovery_request(&record);
        manager.write_draft_request(&record, &request).unwrap();
        manager.remove_owner(&record.owner).unwrap();
        let ids = manager
            .draft_recovery_entries(&record.owner.store_id)
            .unwrap();
        assert!(ids.len() == 1 && ids[0].owner == record.owner);
        assert!(
            manager
                .draft_recovery_entries(&uuid::Uuid::new_v4().to_string())
                .unwrap()
                .is_empty()
        );
        let snapshot = manager
            .draft_recovery_snapshot(&record.owner, true)
            .unwrap();
        assert!(matches!(
            snapshot.entry.ownership,
            local_agent_core::domain::provider_draft_recovery::DraftRecoveryOwnership::Missing
        ));
        assert!(!snapshot.last_managed && snapshot.selection.is_none());
        assert_eq!(
            snapshot.request.unwrap().operation.operation_id,
            request.operation.operation_id
        );
        let path = grant_path(&manager, record.grant.as_ref().unwrap());
        let grant = std::fs::read(&path).unwrap();
        assert!(
            !manager
                .finish_orphan_draft_request(&record.owner, |_| Ok(false))
                .unwrap()
        );
        assert!(manager.read_draft_request(&record.owner).unwrap().is_some());
        assert!(
            manager
                .finish_orphan_draft_request(&record.owner, |_| Ok(true))
                .unwrap()
        );
        assert!(
            manager
                .draft_recovery_entries(&record.owner.store_id)
                .unwrap()
                .is_empty()
        );
        assert_eq!(std::fs::read(path).unwrap(), grant);
    }
    #[test]
    fn damaged_future_or_changed_request_copies_never_become_idle_or_get_erased() {
        let (_tmp, manager, record) = fixture();
        let request = recovery_request(&record);
        manager.write_draft_request(&record, &request).unwrap();
        let root = manager.root.join("draft-catalog-requests");
        let path = root.join(format!(
            "{}-{}.json",
            record.owner.store_id, record.owner.owner_id
        ));
        let valid = std::fs::read(&path).unwrap();
        for version in [2, 99] {
            let mut data: serde_json::Value = serde_json::from_slice(&valid).unwrap();
            data["version"] = version.into();
            let bytes = serde_json::to_vec(&data).unwrap();
            files::atomic_write(&path, &bytes, 0o600).unwrap();
            assert!(manager.read_owner(&record.owner).is_err());
            assert!(
                manager
                    .release_draft_owner(&record.owner, |_, _, _| panic!("unverified copy"))
                    .is_err()
            );
            assert_eq!(std::fs::read(&path).unwrap(), bytes);
            assert!(grant_path(&manager, record.grant.as_ref().unwrap()).exists());
        }
        files::atomic_write(&path, &valid, 0o600).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(manager.read_draft_request(&record.owner).is_err());
        assert_eq!(std::fs::metadata(&root).unwrap().mode() & 0o777, 0o755);
    }
    #[test]
    fn legacy_hash_only_owners_still_read_and_finalize_without_creating_a_request_copy() {
        let (_tmp, manager, mut record) = fixture();
        let request = recovery_request(&record);
        record.operation = Some(journal::pin(&request).unwrap());
        manager.write_owner(&record).unwrap();
        let before = std::fs::read(manager.owner_path(&record.owner, false).unwrap()).unwrap();
        let snapshot = manager
            .draft_recovery_snapshot(&record.owner, true)
            .unwrap();
        assert!(snapshot.request.is_none());
        assert_eq!(
            snapshot.pin.unwrap().operation_id,
            request.operation.operation_id
        );
        assert_eq!(
            std::fs::read(manager.owner_path(&record.owner, false).unwrap()).unwrap(),
            before
        );
        manager
            .release_draft_owner(&record.owner, |_, _, _| Ok(true))
            .unwrap();
        assert!(!manager.root.join("draft-catalog-requests").exists());
    }
    #[test]
    fn request_copy_does_not_turn_explicit_shared_adoption_into_grant_deletion() {
        let (_tmp, manager, record) = fixture();
        manager
            .write_draft_request(&record, &recovery_request(&record))
            .unwrap();
        let grant = record.grant.as_ref().unwrap();
        manager.share_owned_catalog(grant).unwrap();
        manager
            .release_draft_owner(&record.owner, |_, _, last| {
                assert!(!last);
                Ok(true)
            })
            .unwrap();
        assert!(grant_path(&manager, grant).exists());
        assert!(manager.read_draft_request(&record.owner).unwrap().is_none());
        assert!(manager.read_owner(&record.owner).unwrap().is_none());
    }
    #[test]
    fn a_request_change_during_sql_proof_is_rejected_before_revoking_the_grant() {
        let (_tmp, manager, record) = fixture();
        let request = recovery_request(&record);
        manager.write_draft_request(&record, &request).unwrap();
        let path = manager.root.join("draft-catalog-requests").join(format!(
            "{}-{}.json",
            record.owner.store_id, record.owner.owner_id
        ));
        let mut replacement: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        replacement["request"]["operation"]["operationId"] =
            uuid::Uuid::new_v4().to_string().into();
        let bytes = serde_json::to_vec(&replacement).unwrap();
        let grant_path = grant_path(&manager, record.grant.as_ref().unwrap());
        let grant = std::fs::read(&grant_path).unwrap();
        assert_eq!(
            manager
                .release_draft_owner(&record.owner, |_, _, _| {
                    files::atomic_write(&path, &bytes, 0o600).unwrap();
                    Ok(true)
                })
                .err()
                .unwrap()
                .code,
            "stale_snapshot"
        );
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        assert_eq!(std::fs::read(grant_path).unwrap(), grant);
        assert!(manager.read_owner(&record.owner).unwrap().is_some());
    }
    #[test]
    fn oversized_request_copies_fail_before_creating_a_directory_or_mutating_the_owner() {
        let (_tmp, manager, record) = fixture();
        let mut request = recovery_request(&record);
        request.operation.provider.name = "x".repeat(2 * 1024 * 1024);
        assert_eq!(
            manager
                .write_draft_request(&record, &request)
                .err()
                .unwrap()
                .code,
            "resource_limit"
        );
        assert!(!manager.root.join("draft-catalog-requests").exists());
        assert!(
            manager
                .read_owner(&record.owner)
                .unwrap()
                .unwrap()
                .operation
                .is_none()
        );
        assert!(grant_path(&manager, record.grant.as_ref().unwrap()).exists());
    }
}
