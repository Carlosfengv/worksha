//! Durable intent before Vault writes. Receipts never contain credential bytes.
use super::{ConfigError, ProviderManager, fail, files, vault};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

const MAX_OPERATIONS: usize = 4096;
pub mod discovery;
pub mod legacy;
pub(super) mod legacy_admission;
pub use legacy_admission::LegacyPolicy;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Receipt {
    pub operation_id: String,
    pub credential_ref: String,
    pub credential_label: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Pending,
    Ready,
    Cancelled,
    CancelledPending,
    Removed,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OperationStatus {
    pub operation_id: String,
    pub status: Status,
    pub receipt: Option<Receipt>,
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
enum Operation {
    Migrating,
    Pending {
        reference: String,
        label: String,
        commitment: Vec<u8>,
    },
    Ready {
        reference: String,
        label: String,
        commitment: Vec<u8>,
    },
    CancelledPending {
        reference: String,
        label: String,
        commitment: Vec<u8>,
    },
    Cancelled,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ScopedStored<T> {
    version: u32,
    store_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    namespace_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    legacy_migration: Option<legacy::Migration>,
    operation: T,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ProtectionKey {
    reference: String,
    ready: bool,
    #[serde(default)]
    abandoned: Vec<String>,
}

fn unavailable() -> ConfigError {
    ConfigError::new(
        "credential_result_unknown",
        "凭据保存结果尚未确认，请按原操作编号核查；不要重新创建",
    )
}
fn operation_id(id: &str) -> Result<(), ConfigError> {
    if uuid::Uuid::parse_str(id)
        .is_ok_and(|value| value.get_version_num() == 4 && value.to_string() == id)
    {
        Ok(())
    } else {
        fail("invalid_credential_operation", "凭据操作编号无效")
    }
}
fn digest(label: &str, secret: &[u8]) -> Zeroizing<Vec<u8>> {
    let mut hash = Sha256::new();
    hash.update((label.len() as u64).to_le_bytes());
    hash.update(label.as_bytes());
    hash.update((secret.len() as u64).to_le_bytes());
    hash.update(secret);
    Zeroizing::new(hash.finalize().to_vec())
}
fn context(id: &str, reference: &str, label: &str, phase: &str) -> String {
    // The authenticated context protects the public receipt fields as well.
    serde_json::to_string(&("credential-operation-v1", id, reference, label, phase))
        .expect("string tuple serializes")
}
fn same(left: &[u8], right: &[u8]) -> bool {
    left.len() == right.len() && left.iter().zip(right).fold(0u8, |v, (a, b)| v | (a ^ b)) == 0
}

pub(super) fn validate_archive_value(
    name: &str,
    bytes: &[u8],
    store_id: &str,
) -> Result<bool, ConfigError> {
    if name == "protection-key.json" {
        let value: ProtectionKey = serde_json::from_slice(bytes).map_err(|_| unavailable())?;
        if !super::operations::valid_id(&value.reference)
            || value.abandoned.len() > 32
            || value
                .abandoned
                .iter()
                .any(|id| !super::operations::valid_id(id))
        {
            return Err(unavailable());
        }
        return Ok(false);
    }
    let saved: ScopedStored<Operation> =
        serde_json::from_slice(bytes).map_err(|_| unavailable())?;
    if saved.version != 2
        || saved.store_id != store_id
        || saved.namespace_id.is_some()
        || saved.legacy_migration.is_some()
    {
        return Err(unavailable());
    }
    match saved.operation {
        Operation::Cancelled => Ok(false),
        Operation::Migrating => Err(unavailable()),
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
            if !super::operations::valid_id(&reference)
                || label.trim().is_empty()
                || label.len() > 200
                || label.chars().any(char::is_control)
                || commitment.len() != 64
            {
                return Err(unavailable());
            }
            Ok(true)
        }
    }
}

impl ProviderManager {
    fn read_credential_bytes(
        &self,
        name: &str,
        path: &std::path::Path,
    ) -> Result<Option<Vec<u8>>, ConfigError> {
        if let Some(journal) = &self.credential_journal {
            return journal.read(name);
        }
        let saved = files::read_credential_record(path)?;
        if saved.stamp.mode != 0o600 || saved.bytes.as_ref().is_some_and(|b| b.len() > 4096) {
            return Err(unavailable());
        }
        Ok(saved.bytes)
    }
    fn write_credential_bytes(
        &self,
        name: &str,
        path: &std::path::Path,
        bytes: &[u8],
    ) -> Result<(), ConfigError> {
        if let Some(journal) = &self.credential_journal {
            return journal.write(name, bytes);
        }
        files::atomic_write(path, bytes, 0o600)
    }
    fn credential_operation_context(
        &self,
        id: &str,
        reference: &str,
        label: &str,
        phase: &str,
    ) -> String {
        if let Some(namespace_id) = &self.credential_namespace_id {
            return serde_json::to_string(&(
                "credential-operation-v3",
                &self.credential_store_id,
                namespace_id,
                id,
                reference,
                label,
                phase,
            ))
            .expect("string tuple serializes");
        }
        match &self.credential_store_id {
            Some(store_id) => serde_json::to_string(&(
                "credential-operation-v2",
                store_id,
                id,
                reference,
                label,
                phase,
            ))
            .expect("string tuple serializes"),
            None => context(id, reference, label, phase),
        }
    }
    pub(super) fn credential_operation_key(
        &self,
        create: bool,
    ) -> Result<Zeroizing<Vec<u8>>, ConfigError> {
        let path = self.directory.join(if self.credential_store_id.is_some() {
            "protection-key.json"
        } else {
            "credential-operation-key-v1.json"
        });
        let saved = self.read_credential_bytes("protection-key.json", &path)?;
        let fresh = saved.is_none();
        let mut state: ProtectionKey = if let Some(bytes) = saved {
            serde_json::from_slice(&bytes).map_err(|_| unavailable())?
        } else if create {
            let state = ProtectionKey {
                reference: uuid::Uuid::new_v4().to_string(),
                ready: false,
                abandoned: vec![],
            };
            // Track the auxiliary encryption key before writing Vault too. A
            // lost reply cannot leave an unowned key or allocate another ref.
            self.write_credential_bytes(
                "protection-key.json",
                &path,
                &serde_json::to_vec(&state).map_err(|_| unavailable())?,
            )?;
            state
        } else {
            return Err(unavailable());
        };
        operation_id(&state.reference).map_err(|_| unavailable())?;
        if state.abandoned.len() > 32 || state.abandoned.iter().any(|id| operation_id(id).is_err())
        {
            return Err(unavailable());
        }
        let key = match self.vault.get(&state.reference) {
            Ok(key) => key,
            Err(error) if error.code == "credential_not_found" && create && !state.ready => {
                if !fresh {
                    // Never retry a random key at the old reference: a late
                    // Vault write could replace a newer key and break receipts.
                    // Unconfirmed unused refs remain owned and bounded here.
                    if state.abandoned.len() >= 32 {
                        return Err(unavailable());
                    }
                    state.abandoned.push(state.reference);
                    state.reference = uuid::Uuid::new_v4().to_string();
                    self.write_credential_bytes(
                        "protection-key.json",
                        &path,
                        &serde_json::to_vec(&state).map_err(|_| unavailable())?,
                    )?;
                }
                let key = Zeroizing::new(vault::random_key().to_vec());
                self.vault
                    .set(&state.reference, &key)
                    .map_err(|_| unavailable())?;
                let saved = self
                    .vault
                    .get(&state.reference)
                    .map_err(|_| unavailable())?;
                if !same(&key, &saved) {
                    return Err(unavailable());
                }
                saved
            }
            Err(_) => return Err(unavailable()),
        };
        if key.len() != 32 {
            return Err(unavailable());
        }
        if create && !state.ready {
            state.ready = true;
            self.write_credential_bytes(
                "protection-key.json",
                &path,
                &serde_json::to_vec(&state).map_err(|_| unavailable())?,
            )
            .map_err(|_| unavailable())?;
        }
        Ok(key)
    }
    fn credential_operation_path(&self, id: &str) -> Result<std::path::PathBuf, ConfigError> {
        operation_id(id)?;
        Ok(self
            .directory
            .join(if self.credential_store_id.is_some() {
                "operations"
            } else {
                "credential-operations-v1"
            })
            .join(format!("{id}.json")))
    }
    fn read_credential_operation(&self, id: &str) -> Result<Option<Operation>, ConfigError> {
        let saved = self.read_credential_bytes(
            &format!("operations/{id}.json"),
            &self.credential_operation_path(id)?,
        )?;
        let Some(bytes) = saved else {
            return Ok(None);
        };
        if bytes.len() > 4096 {
            return Err(unavailable());
        }
        let operation: Operation = if let Some(store_id) = &self.credential_store_id {
            let stored: ScopedStored<Operation> =
                serde_json::from_slice(&bytes).map_err(|_| unavailable())?;
            if stored.version
                != if self.credential_namespace_id.is_some() {
                    3
                } else {
                    2
                }
                || &stored.store_id != store_id
                || stored.namespace_id != self.credential_namespace_id
            {
                return Err(unavailable());
            }
            legacy::validate(
                self,
                id,
                &stored.operation,
                stored.legacy_migration.as_ref(),
            )?;
            stored.operation
        } else {
            let operation = serde_json::from_slice(&bytes).map_err(|_| unavailable())?;
            legacy::validate(self, id, &operation, None)?;
            operation
        };
        if let Operation::Pending {
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
        } = &operation
            && (!super::operations::valid_id(reference)
                || label.is_empty()
                || label.len() > 200
                || label.chars().any(char::is_control)
                || commitment.len() != 64)
        {
            return Err(unavailable());
        }
        Ok(Some(operation))
    }
    fn write_credential_operation(
        &self,
        id: &str,
        operation: &Operation,
    ) -> Result<(), ConfigError> {
        let path = self.credential_operation_path(id)?;
        let parent = path.parent().expect("operation path has a parent");
        if self.credential_journal.is_none() {
            files::private_dir(parent)?;
        }
        if self.credential_journal.is_none() && !path.exists() {
            let mut entries = std::fs::read_dir(parent).map_err(|_| unavailable())?;
            for count in 0..MAX_OPERATIONS {
                let Some(entry) = entries.next() else { break };
                entry.map_err(|_| unavailable())?;
                if count + 1 == MAX_OPERATIONS {
                    return fail(
                        "credential_operation_capacity",
                        "凭据操作记录已达上限；保留既有回执和凭据，未创建新凭据",
                    );
                }
            }
        }
        let bytes = if let Some(store_id) = &self.credential_store_id {
            serde_json::to_vec(&ScopedStored {
                version: if self.credential_namespace_id.is_some() {
                    3
                } else {
                    2
                },
                store_id: store_id.clone(),
                namespace_id: self.credential_namespace_id.clone(),
                legacy_migration: legacy::saved_migration(self, id)?,
                operation,
            })
        } else {
            serde_json::to_vec(operation)
        }
        .map_err(|_| unavailable())?;
        self.write_credential_bytes(&format!("operations/{id}.json"), &path, &bytes)
    }
    fn credential_commitment(
        &self,
        id: &str,
        reference: &str,
        label: &str,
        commitment: &[u8],
        phase: &str,
    ) -> Result<Zeroizing<Vec<u8>>, ConfigError> {
        vault::open(
            &self.credential_operation_key(false)?,
            commitment,
            &self.credential_operation_context(id, reference, label, phase),
        )
        .map_err(|_| unavailable())
    }

    fn mark_credential_ready(
        &self,
        id: &str,
        reference: String,
        label: String,
        expected: &[u8],
    ) -> Result<(), ConfigError> {
        let commitment = vault::seal(
            &self.credential_operation_key(false)?,
            expected,
            &self.credential_operation_context(id, &reference, &label, "ready"),
        )
        .map_err(|_| unavailable())?;
        self.write_credential_operation(
            id,
            &Operation::Ready {
                reference,
                label,
                commitment,
            },
        )
        .map_err(|_| unavailable())
    }

    /// Legacy calls remain available for internal config transactions. Current
    /// UI calls use this operation protocol and never retry with another ID.
    pub fn create_credential_operation(
        &self,
        id: &str,
        label: &str,
        secret: Zeroizing<String>,
    ) -> Result<Receipt, ConfigError> {
        operation_id(id)?;
        let label = label.trim();
        if label.is_empty()
            || label.len() > 200
            || label.chars().any(char::is_control)
            || secret.is_empty()
            || secret.len() > 8192
            || !secret.bytes().all(|b| b.is_ascii_graphic())
            || label.contains(secret.as_str())
        {
            return fail(
                "invalid_credential",
                "请输入凭据名称和有效 API Key，名称中不能包含 API Key",
            );
        }
        let _legacy_fence = legacy_admission::guard(self, id)?;
        let _guard = self.lock()?;
        let saved = self.read_credential_operation(id)?;
        legacy_admission::admit(self, id, saved.is_some(), true)?;
        let operation = match saved {
            Some(operation) => operation,
            None => {
                let reference = if let Some(journal) = &self.credential_journal {
                    if !journal.allow_new()? {
                        return fail(
                            "credential_protocol_unavailable",
                            "旧操作尚未查到，未当作全新保存；请使用原恢复空间核查",
                        );
                    }
                    journal.reference(id)
                } else {
                    legacy_admission::new_reference(self, id)
                };
                let commitment = vault::seal(
                    &self.credential_operation_key(true)?,
                    &digest(label, secret.as_bytes()),
                    &self.credential_operation_context(id, &reference, label, "pending"),
                )?;
                let operation = Operation::Pending {
                    reference,
                    label: label.into(),
                    commitment,
                };
                // This is the write-ahead point. No target credential exists if
                // intent cannot be durably recorded.
                self.write_credential_operation(id, &operation)?;
                operation
            }
        };
        let was_ready = matches!(operation, Operation::Ready { .. });
        let (reference, saved_label, commitment) = match operation {
            Operation::Pending {
                reference,
                label,
                commitment,
            }
            | Operation::Ready {
                reference,
                label,
                commitment,
            } => (reference, label, commitment),
            Operation::Cancelled | Operation::CancelledPending { .. } => {
                return fail(
                    "credential_operation_cancelled",
                    "此操作已停止后续保存，未重复写入；已发出的写入结果可继续核查",
                );
            }
            Operation::Migrating => return Err(legacy::pending()),
        };
        let expected = self.credential_commitment(
            id,
            &reference,
            &saved_label,
            &commitment,
            if was_ready { "ready" } else { "pending" },
        )?;
        if !same(&expected, &digest(label, secret.as_bytes())) {
            return fail(
                "operation_conflict",
                "此操作编号已用于其他凭据内容，请核查原操作",
            );
        }
        match self.vault.get(&reference) {
            Ok(value) => {
                if !same(&expected, &digest(&saved_label, &value)) {
                    return Err(unavailable());
                }
            }
            Err(error) if error.code == "credential_not_found" && !was_ready => {
                self.vault
                    .set(&reference, secret.as_bytes())
                    .map_err(|_| unavailable())?;
                // Verify even a successful Vault write before confirming it.
                let saved = self.vault.get(&reference).map_err(|_| unavailable())?;
                if !same(&expected, &digest(&saved_label, &saved)) {
                    return Err(unavailable());
                }
            }
            Err(error) if error.code == "credential_not_found" => {
                return fail(
                    "credential_removed",
                    "已完成操作的凭据已移除，未自动重新创建",
                );
            }
            Err(_) => return Err(unavailable()),
        }
        let receipt = Receipt {
            operation_id: id.into(),
            credential_ref: reference.clone(),
            credential_label: saved_label.clone(),
        };
        if !was_ready {
            self.mark_credential_ready(id, reference, saved_label, &expected)?;
        }
        Ok(receipt)
    }

    pub fn credential_operation(&self, id: &str) -> Result<OperationStatus, ConfigError> {
        operation_id(id)?;
        let _legacy_fence = legacy_admission::guard(self, id)?;
        let _guard = self.lock()?;
        self.check_credential_operation(id, false)
    }
    /// Cancellation writes a tombstone under the same cross-process lock. A
    /// late create cannot resurrect it. Already written credentials are never
    /// deleted here because another config or request may now reference them.
    pub fn cancel_credential_operation(&self, id: &str) -> Result<OperationStatus, ConfigError> {
        operation_id(id)?;
        let _legacy_fence = legacy_admission::guard(self, id)?;
        let _guard = self.lock()?;
        self.check_credential_operation(id, true)
    }
    fn check_credential_operation(
        &self,
        id: &str,
        cancel: bool,
    ) -> Result<OperationStatus, ConfigError> {
        let operation = self.read_credential_operation(id)?;
        legacy_admission::admit(self, id, operation.is_some(), cancel)?;
        if operation.is_none() && !cancel {
            return fail(
                "operation_not_found",
                "尚未查到凭据操作；仍可能在等待执行，请保留编号再核查或取消",
            );
        }
        let Some(operation) = operation else {
            self.write_credential_operation(id, &Operation::Cancelled)?;
            return Ok(OperationStatus {
                operation_id: id.into(),
                status: Status::Cancelled,
                receipt: None,
            });
        };
        let was_ready = matches!(operation, Operation::Ready { .. });
        let was_cancelled = matches!(operation, Operation::CancelledPending { .. });
        let (reference, label, commitment) = match operation {
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
            } => (reference, label, commitment),
            Operation::Cancelled => {
                return Ok(OperationStatus {
                    operation_id: id.into(),
                    status: Status::Cancelled,
                    receipt: None,
                });
            }
            Operation::Migrating => return Err(legacy::pending()),
        };
        let expected = self.credential_commitment(
            id,
            &reference,
            &label,
            &commitment,
            if was_ready {
                "ready"
            } else if was_cancelled {
                "cancelled_pending"
            } else {
                "pending"
            },
        )?;
        match self.vault.get(&reference) {
            Ok(value) => {
                if !same(&expected, &digest(&label, &value)) {
                    return Err(unavailable());
                }
                let receipt = Receipt {
                    operation_id: id.into(),
                    credential_ref: reference.clone(),
                    credential_label: label.clone(),
                };
                if !was_ready {
                    self.mark_credential_ready(id, reference, label, &expected)?;
                }
                Ok(OperationStatus {
                    operation_id: id.into(),
                    status: Status::Ready,
                    receipt: Some(receipt),
                })
            }
            Err(error) if error.code == "credential_not_found" && !was_ready => {
                let status = if was_cancelled {
                    Status::CancelledPending
                } else if cancel {
                    let commitment = vault::seal(
                        &self.credential_operation_key(false)?,
                        &expected,
                        &self.credential_operation_context(
                            id,
                            &reference,
                            &label,
                            "cancelled_pending",
                        ),
                    )
                    .map_err(|_| unavailable())?;
                    self.write_credential_operation(
                        id,
                        &Operation::CancelledPending {
                            reference,
                            label,
                            commitment,
                        },
                    )
                    .map_err(|_| unavailable())?;
                    Status::CancelledPending
                } else {
                    Status::Pending
                };
                Ok(OperationStatus {
                    operation_id: id.into(),
                    status,
                    receipt: None,
                })
            }
            Err(error) if error.code == "credential_not_found" => Ok(OperationStatus {
                operation_id: id.into(),
                status: Status::Removed,
                receipt: None,
            }),
            Err(_) => Err(unavailable()),
        }
    }
}

#[cfg(test)]
mod tests;
