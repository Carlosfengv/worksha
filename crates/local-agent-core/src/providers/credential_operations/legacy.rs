//! Explicit ownership claims for unscoped v1 operations. No target Vault writes.
use super::{Operation, ProtectionKey, ScopedStored, Status, context};
use crate::providers::{
    ConfigError, ProviderManager, credential_scope::CredentialScope, fail, files,
    operations::valid_id, vault,
};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Evidence {
    operation: String,
    operation_stamp: files::Stamp,
    protection_key: Option<String>,
    key_stamp: Option<files::Stamp>,
}
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    Unclaimed,
    Prepared,
    Fenced,
    Committed,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Migration {
    version: u32,
    source_digest: String,
    stage: Stage,
    evidence: Evidence,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Preview {
    pub version: u32,
    pub store_id: String,
    pub namespace_id: String,
    pub operation_id: String,
    pub source_digest: String,
    pub stage: Stage,
    pub phase: Status,
    pub label: Option<String>,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Confirmation {
    pub store_id: String,
    pub namespace_id: String,
    pub operation_id: String,
    pub source_digest: String,
    pub confirmed: bool,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Fence {
    state: String,
    version: u32,
    store_id: String,
    namespace_id: String,
    operation_id: String,
    source_digest: String,
}
fn invalid() -> ConfigError {
    ConfigError::new(
        "credential_legacy_unverified",
        "旧凭据恢复材料无法核查，原材料已保留；未创建目标凭据",
    )
}
fn changed() -> ConfigError {
    ConfigError::new(
        "credential_legacy_source_changed",
        "旧记录或恢复来源已变化，请重新核查；未覆盖其他记录",
    )
}
pub(super) fn pending() -> ConfigError {
    ConfigError::new(
        "credential_migration_pending",
        "旧凭据迁移尚未完成，请按原来源和确认摘要继续恢复；未创建另一份凭据",
    )
}
fn original(e: &Evidence) -> Result<Operation, ConfigError> {
    if e.operation.len() > 4096
        || e.operation_stamp.mode != 0o600
        || e.operation_stamp.digest.as_deref() != Some(files::hash(e.operation.as_bytes()).as_str())
    {
        return Err(invalid());
    }
    let value: Operation = serde_json::from_str(&e.operation).map_err(|_| invalid())?;
    match &value {
        Operation::Cancelled => {
            if e.protection_key.is_some() || e.key_stamp.is_some() {
                return Err(invalid());
            }
        }
        Operation::Migrating => return Err(invalid()),
        Operation::Pending {
            reference,
            label,
            commitment,
        }
        | Operation::Ready {
            reference,
            label,
            commitment,
        }
        | Operation::CancelledPending {
            reference,
            label,
            commitment,
        } => {
            if !valid_id(reference)
                || label.trim().is_empty()
                || label.len() > 200
                || label.chars().any(char::is_control)
                || commitment.len() != 64
            {
                return Err(invalid());
            }
            let raw = e.protection_key.as_ref().ok_or_else(invalid)?;
            let stamp = e.key_stamp.as_ref().ok_or_else(invalid)?;
            if raw.len() > 4096
                || stamp.mode != 0o600
                || stamp.digest.as_deref() != Some(files::hash(raw.as_bytes()).as_str())
            {
                return Err(invalid());
            }
            let key: ProtectionKey = serde_json::from_str(raw).map_err(|_| invalid())?;
            if !valid_id(&key.reference)
                || key.abandoned.len() > 32
                || key.abandoned.iter().any(|id| !valid_id(id))
            {
                return Err(invalid());
            }
        }
    }
    Ok(value)
}
fn digest(manager: &ProviderManager, id: &str, evidence: &Evidence) -> Result<String, ConfigError> {
    Ok(files::hash(
        &serde_json::to_vec(&(
            "legacy-credential-claim-v1",
            &manager.credential_store_id,
            &manager.credential_namespace_id,
            id,
            evidence,
        ))
        .map_err(|_| invalid())?,
    ))
}
fn identity(manager: &ProviderManager, id: &str) -> Result<(), ConfigError> {
    if manager.credential_journal.is_none()
        || manager
            .credential_store_id
            .as_ref()
            .is_none_or(|id| !valid_id(id))
        || manager
            .credential_namespace_id
            .as_ref()
            .is_none_or(|id| !valid_id(id))
        || !valid_id(id)
    {
        return Err(invalid());
    }
    Ok(())
}
// Called for every regular operation read as well as explicit recovery. A
// partially migrated record cannot accidentally enter create/query/cancel.
pub(super) fn validate(
    manager: &ProviderManager,
    id: &str,
    current: &Operation,
    migration: Option<&Migration>,
) -> Result<(), ConfigError> {
    let Some(m) = migration else {
        return if matches!(current, Operation::Migrating) {
            Err(invalid())
        } else {
            Ok(())
        };
    };
    identity(manager, id)?;
    if m.version != 1 || m.source_digest != digest(manager, id, &m.evidence)? {
        return Err(invalid());
    }
    let old = original(&m.evidence)?;
    if m.stage == Stage::Unclaimed
        || (m.stage == Stage::Committed) == matches!(current, Operation::Migrating)
    {
        return Err(invalid());
    }
    if m.stage != Stage::Committed {
        return Ok(());
    }
    let fields = |op: &Operation| match op {
        Operation::Pending {
            reference, label, ..
        }
        | Operation::Ready {
            reference, label, ..
        }
        | Operation::CancelledPending {
            reference, label, ..
        } => Some((reference.clone(), label.clone())),
        _ => None,
    };
    if fields(&old) != fields(current) {
        return Err(invalid());
    }
    let allowed = match old {
        Operation::Cancelled => matches!(current, Operation::Cancelled),
        Operation::Ready { .. } => matches!(current, Operation::Ready { .. }),
        Operation::CancelledPending { .. } => matches!(
            current,
            Operation::CancelledPending { .. } | Operation::Ready { .. }
        ),
        Operation::Pending { .. } => {
            !matches!(current, Operation::Cancelled | Operation::Migrating)
        }
        Operation::Migrating => false,
    };
    if !allowed {
        return Err(invalid());
    }
    Ok(())
}
fn envelope(
    manager: &ProviderManager,
    id: &str,
    bytes: &[u8],
) -> Result<ScopedStored<Operation>, ConfigError> {
    let saved: ScopedStored<Operation> = serde_json::from_slice(bytes).map_err(|_| invalid())?;
    if saved.version != 3
        || Some(&saved.store_id) != manager.credential_store_id.as_ref()
        || saved.namespace_id != manager.credential_namespace_id
    {
        return Err(invalid());
    }
    validate(
        manager,
        id,
        &saved.operation,
        saved.legacy_migration.as_ref(),
    )?;
    Ok(saved)
}
pub(super) fn saved_migration(
    manager: &ProviderManager,
    id: &str,
) -> Result<Option<Migration>, ConfigError> {
    if manager.credential_namespace_id.is_none() {
        return Ok(None);
    }
    let path = manager.credential_operation_path(id)?;
    let Some(bytes) = manager.read_credential_bytes(&format!("operations/{id}.json"), &path)?
    else {
        return Ok(None);
    };
    Ok(envelope(manager, id, &bytes)?.legacy_migration)
}
pub(super) fn has_migration(manager: &ProviderManager, id: &str) -> Result<bool, ConfigError> {
    let path = manager.credential_operation_path(id)?;
    let Some(bytes) = manager.read_credential_bytes(&format!("operations/{id}.json"), &path)?
    else {
        return Ok(false);
    };
    Ok(envelope(manager, id, &bytes)?.legacy_migration.is_some())
}
fn load(manager: &ProviderManager, id: &str) -> Result<Option<Migration>, ConfigError> {
    let path = manager.credential_operation_path(id)?;
    let Some(bytes) = manager.read_credential_bytes(&format!("operations/{id}.json"), &path)?
    else {
        return Ok(None);
    };
    envelope(manager, id, &bytes)?
        .legacy_migration
        .map(Some)
        .ok_or_else(|| {
            ConfigError::new(
                "operation_conflict",
                "当前恢复空间已有同编号操作，未覆盖或认领旧记录",
            )
        })
}
fn save(
    manager: &ProviderManager,
    id: &str,
    operation: &Operation,
    migration: &Migration,
) -> Result<(), ConfigError> {
    validate(manager, id, operation, Some(migration))?;
    let value = ScopedStored {
        version: 3,
        store_id: manager.credential_store_id.clone().ok_or_else(invalid)?,
        namespace_id: manager.credential_namespace_id.clone(),
        legacy_migration: Some(migration.clone()),
        operation,
    };
    let bytes = serde_json::to_vec(&value).map_err(|_| invalid())?;
    if bytes.len() > 4096 {
        return fail(
            "credential_migration_capacity",
            "旧恢复材料超过本次迁移的持久记录上限，已保留原文件；未开始迁移",
        );
    }
    manager.write_credential_bytes(
        &format!("operations/{id}.json"),
        &manager.credential_operation_path(id)?,
        &bytes,
    )
}
fn check_capacity(manager: &ProviderManager, migration: &Migration) -> Result<(), ConfigError> {
    let old = original(&migration.evidence)?;
    let worst = match old {
        Operation::Cancelled => Operation::Cancelled,
        Operation::Pending {
            reference, label, ..
        }
        | Operation::Ready {
            reference, label, ..
        }
        | Operation::CancelledPending {
            reference, label, ..
        } => Operation::CancelledPending {
            reference,
            label,
            // Decimal JSON encoding must also fit after resealing or a later
            // phase change; the source nonce's current spelling is irrelevant.
            commitment: vec![255; 64],
        },
        Operation::Migrating => return Err(invalid()),
    };
    let mut committed = migration.clone();
    committed.stage = Stage::Committed;
    let bytes = serde_json::to_vec(&ScopedStored {
        version: 3,
        store_id: manager.credential_store_id.clone().ok_or_else(invalid)?,
        namespace_id: manager.credential_namespace_id.clone(),
        legacy_migration: Some(committed),
        operation: worst,
    })
    .map_err(|_| invalid())?;
    if bytes.len() > 4096 {
        return fail(
            "credential_migration_capacity",
            "旧恢复材料超过迁移记录上限，原文件已保留；未开始迁移",
        );
    }
    Ok(())
}
fn read_evidence(manager: &ProviderManager, id: &str) -> Result<Evidence, ConfigError> {
    let source = files::read_credential_record(&manager.credential_operation_path(id)?)?;
    let operation = String::from_utf8(source.bytes.ok_or_else(|| {
        ConfigError::new(
            "credential_legacy_not_found",
            "尚未找到原旧版记录，请连接原后台或恢复原材料",
        )
    })?)
    .map_err(|_| invalid())?;
    let op: Operation = serde_json::from_str(&operation).map_err(|_| invalid())?;
    let (protection_key, key_stamp) = if matches!(op, Operation::Cancelled) {
        (None, None)
    } else {
        let key = files::read_credential_record(
            &manager.directory.join("credential-operation-key-v1.json"),
        )?;
        (
            Some(String::from_utf8(key.bytes.ok_or_else(invalid)?).map_err(|_| invalid())?),
            Some(key.stamp),
        )
    };
    let evidence = Evidence {
        operation,
        operation_stamp: source.stamp,
        protection_key,
        key_stamp,
    };
    original(&evidence)?;
    Ok(evidence)
}
fn preview(manager: &ProviderManager, id: &str, m: &Migration) -> Result<Preview, ConfigError> {
    let old = original(&m.evidence)?;
    let (phase, label) = match old {
        Operation::Cancelled => (Status::Cancelled, None),
        Operation::Pending { label, .. } => (Status::Pending, Some(label)),
        Operation::Ready { label, .. } => (Status::Ready, Some(label)),
        Operation::CancelledPending { label, .. } => (Status::CancelledPending, Some(label)),
        Operation::Migrating => return Err(invalid()),
    };
    Ok(Preview {
        version: 2,
        store_id: manager.credential_store_id.clone().ok_or_else(invalid)?,
        namespace_id: manager
            .credential_namespace_id
            .clone()
            .ok_or_else(invalid)?,
        operation_id: id.into(),
        source_digest: m.source_digest.clone(),
        stage: m.stage,
        phase,
        label,
    })
}
pub(super) fn discovery_preview(
    legacy: &ProviderManager,
    target: &ProviderManager,
    id: &str,
    unclaimed: bool,
) -> Result<Option<Preview>, ConfigError> {
    let migration = if unclaimed {
        load(target, id)?
    } else {
        saved_migration(target, id)?
    };
    if let Some(m) = migration {
        return if unclaimed {
            Ok(None)
        } else {
            preview(target, id, &m).map(Some)
        };
    }
    if !unclaimed {
        return Ok(None);
    }
    let evidence = read_evidence(legacy, id)?;
    let migration = Migration {
        version: 1,
        source_digest: digest(target, id, &evidence)?,
        stage: Stage::Unclaimed,
        evidence,
    };
    check_capacity(target, &migration)?;
    preview(target, id, &migration).map(Some)
}
fn fence_bytes(manager: &ProviderManager, id: &str, m: &Migration) -> Result<Vec<u8>, ConfigError> {
    serde_json::to_vec(&Fence {
        state: "migrated".into(),
        version: 1,
        store_id: manager.credential_store_id.clone().ok_or_else(invalid)?,
        namespace_id: manager
            .credential_namespace_id
            .clone()
            .ok_or_else(invalid)?,
        operation_id: id.into(),
        source_digest: m.source_digest.clone(),
    })
    .map_err(|_| invalid())
}
fn fence_legacy(
    legacy: &ProviderManager,
    target: &ProviderManager,
    id: &str,
    m: &Migration,
) -> Result<Vec<u8>, ConfigError> {
    let path = legacy.credential_operation_path(id)?;
    let fence = fence_bytes(target, id, m)?;
    let source = files::read_credential_record(&path)?;
    if source.stamp.mode != 0o600 {
        return Err(invalid());
    }
    if let Some(bytes) = source.bytes {
        if bytes == fence {
            return Ok(fence);
        }
        if bytes != m.evidence.operation.as_bytes() {
            return Err(changed());
        }
    }
    // A confirmed durable intent may recreate only its fence if its original
    // directory disappeared. It never reconstructs an unfenced v1 operation.
    if let Some(expected) = &m.evidence.protection_key {
        let key = files::read_credential_record(
            &legacy.directory.join("credential-operation-key-v1.json"),
        )?;
        if key.bytes.is_some_and(|bytes| bytes != expected.as_bytes()) {
            return Err(changed());
        }
    }
    files::private_dir(path.parent().ok_or_else(invalid)?)?;
    files::atomic_write(&path, &fence, 0o600)?;
    if files::read_credential_record(&path)?.bytes.as_deref() != Some(fence.as_slice()) {
        return Err(changed());
    }
    Ok(fence)
}
fn convert(target: &ProviderManager, id: &str, m: &Migration) -> Result<Operation, ConfigError> {
    let old = original(&m.evidence)?;
    let (reference, label, commitment, phase) = match old {
        Operation::Cancelled => return Ok(Operation::Cancelled),
        Operation::Pending {
            reference,
            label,
            commitment,
        } => (reference, label, commitment, "pending"),
        Operation::Ready {
            reference,
            label,
            commitment,
        } => (reference, label, commitment, "ready"),
        Operation::CancelledPending {
            reference,
            label,
            commitment,
        } => (reference, label, commitment, "cancelled_pending"),
        Operation::Migrating => return Err(invalid()),
    };
    let key: ProtectionKey =
        serde_json::from_str(m.evidence.protection_key.as_ref().ok_or_else(invalid)?)
            .map_err(|_| invalid())?;
    let old_key = target.vault.get(&key.reference).map_err(|_| pending())?;
    let expected: Zeroizing<Vec<u8>> = vault::open(
        &old_key,
        &commitment,
        &context(id, &reference, &label, phase),
    )
    .map_err(|_| invalid())?;
    if expected.len() != 32 {
        return Err(invalid());
    }
    let new_key = target.credential_operation_key(true)?;
    let commitment = vault::seal(
        &new_key,
        &expected,
        &target.credential_operation_context(id, &reference, &label, phase),
    )
    .map_err(|_| pending())?;
    Ok(match phase {
        "pending" => Operation::Pending {
            reference,
            label,
            commitment,
        },
        "ready" => Operation::Ready {
            reference,
            label,
            commitment,
        },
        _ => Operation::CancelledPending {
            reference,
            label,
            commitment,
        },
    })
}
impl ProviderManager {
    /// Files only: directory presence and a UUID do not prove historic ownership.
    pub fn preview_legacy_credential(
        &self,
        scope: &CredentialScope,
        namespace: &str,
        id: &str,
    ) -> Result<Preview, ConfigError> {
        if !valid_id(id) {
            return Err(invalid());
        }
        let target = self.credential_scoped_manager(scope, Some(namespace))?;
        let _guard = target.lock()?;
        if let Some(m) = load(&target, id)? {
            return preview(&target, id, &m);
        }
        let _legacy_guard = self.lock()?;
        let evidence = read_evidence(self, id)?;
        let m = Migration {
            version: 1,
            source_digest: digest(&target, id, &evidence)?,
            stage: Stage::Unclaimed,
            evidence,
        };
        check_capacity(&target, &m)?;
        preview(&target, id, &m)
    }
    /// Explicitly claimed source, original reference, resumable fence. Migration
    /// reads only the old auxiliary key and may create a tracked new one; it
    /// never writes/deletes the target credential or calls a model.
    pub fn confirm_legacy_credential(
        &self,
        scope: &CredentialScope,
        input: &Confirmation,
    ) -> Result<Preview, ConfigError> {
        if !input.confirmed
            || !valid_id(&input.operation_id)
            || input.store_id != scope.store_id
            || input.source_digest.len() != 64
            || !input
                .source_digest
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        {
            return Err(invalid());
        }
        let target = self.credential_scoped_manager(scope, Some(&input.namespace_id))?;
        let _guard = target.lock()?;
        let mut existing = load(&target, &input.operation_id)?;
        if let Some(m) = &existing {
            if m.source_digest != input.source_digest {
                return Err(changed());
            }
            if m.stage == Stage::Committed {
                return preview(&target, &input.operation_id, m);
            }
            files::private_dir(&self.directory)?;
        }
        let legacy_guard = self.lock()?;
        let mut m = if let Some(m) = existing.take() {
            m
        } else {
            let evidence = read_evidence(self, &input.operation_id)?;
            let source_digest = digest(&target, &input.operation_id, &evidence)?;
            if source_digest != input.source_digest {
                return Err(changed());
            }
            let m = Migration {
                version: 1,
                source_digest,
                stage: Stage::Prepared,
                evidence,
            };
            check_capacity(&target, &m)?;
            save(&target, &input.operation_id, &Operation::Migrating, &m)?;
            m
        };
        let fence = fence_legacy(self, &target, &input.operation_id, &m)?;
        if m.stage != Stage::Fenced {
            m.stage = Stage::Fenced;
            save(&target, &input.operation_id, &Operation::Migrating, &m)?;
        }
        drop(legacy_guard);
        let operation = convert(&target, &input.operation_id, &m)?;
        if files::read_credential_record(&self.credential_operation_path(&input.operation_id)?)?
            .bytes
            .as_deref()
            != Some(fence.as_slice())
        {
            return Err(changed());
        }
        m.stage = Stage::Committed;
        save(&target, &input.operation_id, &operation, &m)?;
        preview(&target, &input.operation_id, &m)
    }
}
#[cfg(test)]
mod tests;
