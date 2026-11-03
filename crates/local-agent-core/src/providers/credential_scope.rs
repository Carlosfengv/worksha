//! Database-bound credential operations. Legacy records are never relabelled.
use super::{
    ConfigError, CredentialSaved, ProviderManager,
    credential_operations::{OperationStatus, Status},
    fail, files,
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use zeroize::Zeroizing;
mod archive;
pub(crate) mod journal;
pub const MAX_RESPONSE_BYTES: usize = 8192;

pub struct CredentialScope {
    pub(crate) store_id: String,
    pub(crate) directory: PathBuf,
    pub(crate) journal: std::sync::Arc<journal::Journal>,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Protocol {
    pub version: u32,
    pub store_id: String,
    pub operation_limit: usize,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScopedOperation {
    pub version: u32,
    pub store_id: String,
    pub operation: OperationStatus,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Marker {
    version: u32,
    store_id: String,
}
fn unavailable() -> ConfigError {
    ConfigError::new(
        "credential_protocol_unavailable",
        "凭据来源标记不可核查，原记录已保留；未读取或写入钥匙串",
    )
}
pub use journal::{LegacyState, NamespaceState};
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProtocolV3 {
    pub version: u32,
    pub store_id: String,
    pub namespace_id: String,
    pub namespace_state: NamespaceState,
    pub legacy_state: LegacyState,
    pub operation_limit: usize,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScopedOperationV3 {
    pub version: u32,
    pub store_id: String,
    pub namespace_id: String,
    pub operation: OperationStatus,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IdentityV3 {
    pub store_id: String,
    pub namespace_id: String,
    pub operation_id: String,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateV3 {
    pub store_id: String,
    pub namespace_id: String,
    pub operation_id: String,
    pub label: String,
    pub secret: String,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InitializationChoice {
    Restore,
    New,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Initialization {
    pub store_id: String,
    pub namespace_id: String,
    pub choice: InitializationChoice,
}
fn protocol_v3(meta: journal::Metadata) -> ProtocolV3 {
    ProtocolV3 {
        version: 3,
        store_id: meta.store_id,
        namespace_id: meta.namespace_id,
        namespace_state: meta.state,
        legacy_state: meta.legacy,
        operation_limit: 4096,
    }
}
impl ProviderManager {
    pub(super) fn check_credential_scope(
        &self,
        scope: &CredentialScope,
    ) -> Result<(), ConfigError> {
        files::checked_path(&self.directory)?;
        let actual = self
            .directory
            .parent()
            .and_then(|p| p.canonicalize().ok())
            .and_then(|p| self.directory.file_name().map(|name| p.join(name)))
            .ok_or_else(unavailable)?;
        if actual != scope.directory {
            return fail(
                "credential_store_changed",
                "凭据目录不属于原数据库，未执行密钥操作",
            );
        }
        super::credential_operations::legacy_admission::bind(self, &scope.journal)
    }
    pub(super) fn credential_scoped_manager(
        &self,
        scope: &CredentialScope,
        namespace_id: Option<&str>,
    ) -> Result<Self, ConfigError> {
        self.check_credential_scope(scope)?;
        let meta = scope.journal.status()?;
        let domain = if let Some(id) = namespace_id {
            if !super::operations::valid_id(id) || id != meta.namespace_id {
                return fail(
                    "credential_namespace_changed",
                    "凭据恢复空间已变化，请保留原空间和编号核查",
                );
            }
            if !matches!(meta.state, NamespaceState::Active) {
                return Err(journal::unverified());
            }
            3
        } else {
            if matches!(meta.legacy, LegacyState::Unverified) {
                return Err(journal::unverified());
            }
            2
        };
        Ok(Self {
            adapters: self.adapters.clone(),
            home: self.home.clone(),
            directory: self.directory.join(if domain == 3 {
                "credential-operations-v3"
            } else {
                "credential-operations-v2"
            }),
            helper_source: self.helper_source.clone(),
            vault: self.vault.clone(),
            credential_store_id: Some(scope.store_id.clone()),
            credential_namespace_id: namespace_id.map(str::to_owned),
            credential_journal: Some(scope.journal.domain(domain)),
            legacy_credential_fence: self.legacy_credential_fence.clone(),
        })
    }
    pub fn scoped_credential_protocol(
        &self,
        scope: &CredentialScope,
    ) -> Result<Protocol, ConfigError> {
        self.credential_scoped_manager(scope, None)?;
        Ok(Protocol {
            version: 2,
            store_id: scope.store_id.clone(),
            operation_limit: 4096,
        })
    }
    pub fn scoped_credential_protocol_v3(
        &self,
        scope: &CredentialScope,
    ) -> Result<ProtocolV3, ConfigError> {
        self.check_credential_scope(scope)?;
        Ok(protocol_v3(scope.journal.status()?))
    }
    pub fn initialize_credential_namespace(
        &self,
        scope: &CredentialScope,
        namespace_id: &str,
        restore: bool,
    ) -> Result<ProtocolV3, ConfigError> {
        self.check_credential_scope(scope)?;
        if namespace_id != scope.journal.namespace_id() {
            return fail(
                "credential_namespace_changed",
                "原恢复空间已变化，未执行初始化",
            );
        }
        Ok(protocol_v3(scope.journal.initialize_namespace(restore)?))
    }
    pub fn create_scoped_credential(
        &self,
        scope: &CredentialScope,
        id: &str,
        label: &str,
        secret: Zeroizing<String>,
    ) -> Result<ScopedOperation, ConfigError> {
        if !super::operations::valid_id(id) {
            return fail("invalid_credential_operation", "凭据操作编号无效");
        }
        let receipt = self
            .credential_scoped_manager(scope, None)?
            .create_credential_operation(id, label, secret)?;
        Ok(ScopedOperation {
            version: 2,
            store_id: scope.store_id.clone(),
            operation: OperationStatus {
                operation_id: id.into(),
                status: Status::Ready,
                receipt: Some(receipt),
            },
        })
    }
    pub fn scoped_credential_operation(
        &self,
        scope: &CredentialScope,
        id: &str,
        cancel: bool,
    ) -> Result<ScopedOperation, ConfigError> {
        if !super::operations::valid_id(id) {
            return fail("invalid_credential_operation", "凭据操作编号无效");
        }
        let manager = self.credential_scoped_manager(scope, None)?;
        let operation = if cancel {
            manager.cancel_credential_operation(id)?
        } else {
            manager.credential_operation(id)?
        };
        Ok(ScopedOperation {
            version: 2,
            store_id: scope.store_id.clone(),
            operation,
        })
    }
    pub fn create_credential_v3(
        &self,
        scope: &CredentialScope,
        namespace_id: &str,
        id: &str,
        label: &str,
        secret: Zeroizing<String>,
    ) -> Result<ScopedOperationV3, ConfigError> {
        if !super::operations::valid_id(id) {
            return fail("invalid_credential_operation", "凭据操作编号无效");
        }
        let receipt = self
            .credential_scoped_manager(scope, Some(namespace_id))?
            .create_credential_operation(id, label, secret)?;
        Ok(ScopedOperationV3 {
            version: 3,
            store_id: scope.store_id.clone(),
            namespace_id: namespace_id.into(),
            operation: OperationStatus {
                operation_id: id.into(),
                status: Status::Ready,
                receipt: Some(receipt),
            },
        })
    }
    /// Preserve the unnumbered HTTP response while making this new write
    /// discoverable. Each request is independent; callers need v3 with their
    /// own operation ID for idempotent retries. Historical keys are not adopted.
    pub fn compatibility_credential(
        &self,
        scope: &CredentialScope,
        label: &str,
        secret: Zeroizing<String>,
    ) -> Result<CredentialSaved, ConfigError> {
        let protocol = self.scoped_credential_protocol_v3(scope)?;
        let saved = self.create_credential_v3(
            scope,
            &protocol.namespace_id,
            &uuid::Uuid::new_v4().to_string(),
            label,
            secret,
        )?;
        let receipt = saved.operation.receipt.ok_or_else(unavailable)?;
        Ok(CredentialSaved {
            credential_ref: receipt.credential_ref,
            credential_label: receipt.credential_label,
        })
    }
    pub fn credential_operation_v3(
        &self,
        scope: &CredentialScope,
        namespace_id: &str,
        id: &str,
        cancel: bool,
    ) -> Result<ScopedOperationV3, ConfigError> {
        if !super::operations::valid_id(id) {
            return fail("invalid_credential_operation", "凭据操作编号无效");
        }
        let manager = self.credential_scoped_manager(scope, Some(namespace_id))?;
        let operation = if cancel {
            manager.cancel_credential_operation(id)?
        } else {
            manager.credential_operation(id)?
        };
        Ok(ScopedOperationV3 {
            version: 3,
            store_id: scope.store_id.clone(),
            namespace_id: namespace_id.into(),
            operation,
        })
    }
}

#[cfg(test)]
mod tests;
