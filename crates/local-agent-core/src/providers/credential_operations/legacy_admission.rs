//! Admit unscoped compatibility operations without assigning historical ownership.
use super::super::{
    ConfigError, ConfigTransactionGuard, ProviderManager,
    credential_scope::{CredentialScope, NamespaceState, journal::Journal},
    fail,
};
use crate::Store;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LegacyPolicy {
    pub version: u32,
    pub store_id: String,
    pub schema_version: u32,
}
impl Store {
    pub fn legacy_credential_policy(&self) -> Result<LegacyPolicy, ConfigError> {
        let id = self.database_id().map_err(|_| {
            ConfigError::new("credential_recovery_unavailable", "凭据主库身份无法核查")
        })?;
        let scope = self.credential_scope(&id)?;
        scope.journal.validate_legacy_admission()?;
        Ok(LegacyPolicy {
            version: 2,
            store_id: id,
            schema_version: Self::SCHEMA_VERSION,
        })
    }
}
impl ProviderManager {
    /// Bind compatibility admission before the managed service accepts requests.
    /// The source remains fixed even if no scoped operation has run yet.
    pub fn bind_legacy_credential_source(
        &self,
        scope: &CredentialScope,
    ) -> Result<(), ConfigError> {
        self.check_credential_scope(scope)?;
        scope.journal.validate_legacy_admission()
    }
}
pub(super) fn admit(
    manager: &ProviderManager,
    id: &str,
    material_exists: bool,
    create_or_cancel: bool,
) -> Result<(), ConfigError> {
    if manager.credential_store_id.is_none()
        && let Some(journal) = manager.legacy_credential_fence.get()
    {
        journal.admit_legacy_operation(id, material_exists, create_or_cancel)?;
    }
    Ok(())
}
pub(super) fn new_reference(manager: &ProviderManager, id: &str) -> String {
    manager.legacy_credential_fence.get().map_or_else(
        || uuid::Uuid::new_v4().to_string(),
        |journal| journal.domain(1).reference(id),
    )
}
pub(crate) fn bind(manager: &ProviderManager, journal: &Arc<Journal>) -> Result<(), ConfigError> {
    let _ = manager.legacy_credential_fence.set(journal.clone());
    let held = manager
        .legacy_credential_fence
        .get()
        .expect("binding has been set");
    if !held.same_source(journal)? {
        return fail(
            "credential_store_changed",
            "旧凭据入口的原数据库已变化，未重新绑定或执行密钥操作",
        );
    }
    Ok(())
}
pub(super) fn guard(
    manager: &ProviderManager,
    id: &str,
) -> Result<Option<ConfigTransactionGuard>, ConfigError> {
    if manager.credential_store_id.is_some() {
        return Ok(None);
    }
    if manager.legacy_credential_fence.get().is_none() {
        let data = manager.directory.parent().ok_or_else(|| {
            ConfigError::new("credential_recovery_unavailable", "凭据主库位置无法核查")
        })?;
        if let Some(journal) = Journal::existing_for_legacy(data)? {
            bind(manager, &journal)?;
        }
    }
    let Some(journal) = manager.legacy_credential_fence.get() else {
        return Ok(None);
    };
    let held = journal.lock()?;
    let meta = journal.status()?;
    // The metadata validator also proves there are no domain-3 entries in an
    // unverified namespace. This is not an ownership claim on legacy files.
    if matches!(meta.state, NamespaceState::Unverified) {
        return Ok(Some(held));
    }
    let scope = CredentialScope {
        store_id: meta.store_id,
        directory: manager.directory.clone(),
        journal: journal.clone(),
    };
    let target = manager.credential_scoped_manager(&scope, Some(&meta.namespace_id))?;
    if super::legacy::has_migration(&target, id)? {
        return fail(
            "credential_legacy_migrated",
            "此旧编号已进入来源绑定恢复，请使用原恢复空间核查；未读取或写入密钥",
        );
    }
    Ok(Some(held))
}
