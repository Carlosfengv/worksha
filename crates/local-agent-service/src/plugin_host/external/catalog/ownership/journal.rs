//! A bounded original request copy; never a new execution or grant authority.
use super::*;
use std::path::Path;

const REQUEST_BYTES: usize = 2 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RequestRecord {
    version: u32,
    owner: DraftGrantOwner,
    request: Box<SourceRequest>,
}

pub(super) fn pin(request: &SourceRequest) -> Result<Pin> {
    Ok(Pin {
        operation_id: request.operation.operation_id.clone(),
        request_hash: request_hash(&request.operation)?,
        source_revision: request.expected_source_revision.clone(),
    })
}
pub(super) fn hash(request: &SourceRequest) -> Result<String> {
    Ok(package::hash(
        &serde_json::to_vec(request).checked("invalid_record")?,
    ))
}

impl Manager {
    fn request_directory(&self, create: bool) -> Result<PathBuf> {
        let root = self.root.join("draft-catalog-requests");
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
    fn request_path(&self, owner: &DraftGrantOwner, create: bool) -> Result<PathBuf> {
        // Reuse the original canonical identity validation, not caller paths.
        self.owner_path(owner, false)?;
        Ok(self
            .request_directory(create)?
            .join(format!("{}-{}.json", owner.store_id, owner.owner_id)))
    }
    pub(super) fn request_owners(&self) -> Result<Vec<DraftGrantOwner>> {
        let root = self.request_directory(false)?;
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
                let n = n.to_str().ok_or(fail("invalid_record"))?;
                if !n.is_ascii() || n.len() != 78 || &n[36..37] != "-" || !n.ends_with(".json") {
                    return Err(fail("invalid_record"));
                }
                let owner = DraftGrantOwner {
                    store_id: n[..36].into(),
                    owner_id: n[37..73].into(),
                };
                self.owner_path(&owner, false)?;
                Ok(owner)
            })
            .collect()
    }
    pub(super) fn read_draft_request(
        &self,
        owner: &DraftGrantOwner,
    ) -> Result<Option<Box<SourceRequest>>> {
        let path = self.request_path(owner, false)?;
        match std::fs::symlink_metadata(&path) {
            Ok(m) if m.len() > REQUEST_BYTES as u64 => return Err(fail("resource_limit")),
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(fail("unavailable")),
        }
        let snapshot = files::read_protected(&path).checked("invalid_record")?;
        let Some(bytes) = snapshot.bytes else {
            return Ok(None);
        };
        if bytes.len() > REQUEST_BYTES || snapshot.stamp.mode & 0o077 != 0 {
            return Err(fail("invalid_record"));
        }
        let journal: RequestRecord = serde_json::from_slice(&bytes).checked("invalid_record")?;
        if journal.version != 1
            || journal.owner != *owner
            || journal.request.operation.store_id != owner.store_id
            || !local_agent_core::providers::operations::valid_id(
                &journal.request.operation.operation_id,
            )
        {
            return Err(fail("invalid_record"));
        }
        let selected = journal
            .request
            .source
            .catalog()
            .ok_or(fail("invalid_record"))?;
        let digest = |s: &str| {
            s.len() == 64
                && s.bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        };
        let binding_valid = selected
            .binding_id
            .split_once(':')
            .is_some_and(|(stem, nonce)| {
                stem.split_once('-').is_some_and(|(publisher, scope)| {
                    publisher == package::hash(selected.plugin_id.as_bytes()) && digest(scope)
                }) && local_agent_core::providers::operations::valid_id(nonce)
            });
        if selected.plugin_id.is_empty()
            || selected.plugin_id.len() > 128
            || !selected
                .plugin_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || [b'.', b'-', b'_'].contains(&b))
            || !digest(&selected.digest)
            || !binding_valid
        {
            return Err(fail("invalid_record"));
        }
        let a::Target::Draft { .. } = &selected.target else {
            return Err(fail("invalid_record"));
        };
        let (input, _) = records::resolve(&selected.target, &[])?;
        let p = &journal.request.operation.provider;
        if (&p.endpoint, &p.protocol, &p.vendor, &p.credential_ref)
            != (
                &input.endpoint,
                &input.protocol,
                &input.vendor,
                &input.credential_ref,
            )
        {
            return Err(fail("invalid_record"));
        }
        Ok(Some(journal.request))
    }
    pub(super) fn check_draft_request(owner: &OwnerRecord, request: &SourceRequest) -> Result<()> {
        let grant = owner.grant.as_ref().ok_or(fail("invalid_record"))?;
        let pin = pin(request)?;
        if !owner.published
            || request.operation.store_id != owner.owner.store_id
            || request.source.catalog() != Some(&selection(grant))
            || owner.operation.as_ref().is_some_and(|old| {
                old.operation_id != pin.operation_id
                    || old.request_hash != pin.request_hash
                    || old.source_revision != pin.source_revision
            })
        {
            return Err(fail("invalid_record"));
        }
        Ok(())
    }
    pub(super) fn write_draft_request(
        &self,
        owner: &OwnerRecord,
        request: &SourceRequest,
    ) -> Result<()> {
        Self::check_draft_request(owner, request)?;
        if let Some(old) = self.read_draft_request(&owner.owner)? {
            if hash(&old)? != hash(request)? {
                return Err(fail("stale_snapshot"));
            }
            return Ok(());
        }
        let bytes = serde_json::to_vec(&RequestRecord {
            version: 1,
            owner: owner.owner.clone(),
            request: Box::new(request.clone()),
        })
        .checked("invalid_input")?;
        if bytes.len() > REQUEST_BYTES || self.request_owners()?.len() >= LIMIT {
            return Err(fail("resource_limit"));
        }
        files::atomic_write(&self.request_path(&owner.owner, true)?, &bytes, 0o600)
            .checked("unavailable")
    }
    pub(super) fn remove_draft_request(
        &self,
        owner: &DraftGrantOwner,
        expected: Option<&str>,
    ) -> Result<()> {
        let current = self.read_draft_request(owner)?;
        let actual = current.as_ref().map(|r| hash(r)).transpose()?;
        if actual.as_deref() != expected {
            return Err(fail("stale_snapshot"));
        }
        if current.is_some() {
            let p = self.request_path(owner, false)?;
            std::fs::remove_file(&p).checked("unavailable")?;
            sync_parent(&p)?;
        }
        Ok(())
    }

    pub(super) fn check_draft_request_hash(
        &self,
        owner: &DraftGrantOwner,
        expected: Option<&str>,
    ) -> Result<()> {
        let actual = self
            .read_draft_request(owner)?
            .as_ref()
            .map(|r| hash(r))
            .transpose()?;
        if actual.as_deref() != expected {
            return Err(fail("stale_snapshot"));
        }
        Ok(())
    }
}

fn sync_parent(path: &Path) -> Result<()> {
    std::fs::File::open(path.parent().ok_or(fail("unavailable"))?)
        .checked("unavailable")?
        .sync_all()
        .checked("unavailable")
}
