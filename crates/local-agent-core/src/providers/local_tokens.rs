//! Purpose-bound ownership of generated loopback tokens. Never an upstream API key.
use super::{
    ConfigError, ConfigTransactionGuard, ProviderManager,
    credential_operations::{OperationStatus, Receipt},
    credential_scope::{CredentialScope, IdentityV3, NamespaceState},
    fail, files,
    operations::valid_id,
    vault,
};
use crate::{Provider, Store};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

mod cancellations;
mod storage;
#[cfg(test)]
mod tests;
pub(crate) use storage::{crypto_metadata, initialize, save_binding_owner, upgrade_previous};

pub const LIMIT: usize = 4096;
pub const PENDING_LIMIT: usize = 4;
pub const DOMAIN: u32 = 4;
pub const MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;
pub const CANCELLATION_LIMIT: usize = 4096;
const RECORD_LIMIT: usize = 2 * 1024 * 1024;
const BYTE_LIMIT: usize = 512 * 1024 * 1024;
const PENDING_RESERVE: usize = 4096;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Purpose {
    FixedRoute,
    DynamicBinding,
    ModelService,
}
impl Purpose {
    fn label(self) -> &'static str {
        match self {
            Self::FixedRoute => "Local Agent 本机路由",
            Self::DynamicBinding => "固定动态入口",
            Self::ModelService => "模型服务本机入口",
        }
    }
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ServiceIdentity {
    pub id: String,
    pub version_id: String,
    pub revision: u64,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Request {
    pub identity: IdentityV3,
    pub purpose: Purpose,
    pub agent_id: String,
    pub configuration_path: String,
    pub configuration_path_hash: String,
    pub expected_revision: String,
    pub provider: Provider,
    pub service: Option<ServiceIdentity>,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Pending,
    Committed,
    Cancelled,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Operation {
    pub version: u32,
    pub status: Status,
    pub request: Request,
    pub owner_id: String,
    pub credential_ref: String,
    pub owner_saved: bool,
    pub preview_id: Option<String>,
}
/// Proves only that this identity cannot create an entry in the future.
/// It does not assert that historical external effects never occurred.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Cancellation {
    pub version: u32,
    pub identity: IdentityV3,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Stored {
    operation: Operation,
    protected_token: Option<Vec<u8>>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Protocol {
    pub version: u32,
    pub schema_version: u32,
    pub store_id: String,
    pub namespace_id: String,
    pub namespace_state: NamespaceState,
    pub operation_limit: usize,
    pub pending_limit: usize,
    pub cancellation_limit: usize,
    pub pending: Vec<Operation>,
}

fn unknown() -> ConfigError {
    ConfigError::new(
        "local_token_result_unknown",
        "本机入口结果尚未确认，请按原用途和编号核查；未重新生成令牌",
    )
}
fn corrupt() -> ConfigError {
    ConfigError::new(
        "local_token_record_unavailable",
        "本机令牌归属记录不可核查，原记录与凭据已保留",
    )
}
fn encoded<T: Serialize>(value: &T) -> Result<String, ConfigError> {
    serde_json::to_string(value).map_err(|_| corrupt())
}
fn validate(request: &Request) -> Result<(), ConfigError> {
    let identity = &request.identity;
    let revision = &request.expected_revision;
    if !valid_id(&identity.store_id)
        || !valid_id(&identity.namespace_id)
        || !valid_id(&identity.operation_id)
        || !["claude", "codex", "opencode", "claude-desktop"].contains(&request.agent_id.as_str())
        || request.purpose != Purpose::FixedRoute
            && !["claude", "opencode"].contains(&request.agent_id.as_str())
        || request.configuration_path.is_empty()
        || request.configuration_path.len() > 16384
        || request.configuration_path.chars().any(char::is_control)
        || request.configuration_path_hash.len() != 64
        || !request
            .configuration_path_hash
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        || revision != "absent"
            && (revision.len() != 64
                || !revision
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()))
        || (request.purpose == Purpose::ModelService) != request.service.is_some()
        || request
            .service
            .as_ref()
            .is_some_and(|s| !valid_id(&s.id) || !valid_id(&s.version_id))
        || encoded(request)?.len() > RECORD_LIMIT - PENDING_RESERVE
    {
        return fail("invalid_local_token", "本机令牌用途、原来源或请求身份无效");
    }
    super::validate_source(&request.provider)
}

/// Holds the operation lock until its SQL/file owner and preview are recorded.
/// Recovery locking is acquired only inside credential calls, after this lock.
pub struct Session<'a> {
    manager: ProviderManager,
    store: &'a Store,
    stored: Stored,
    _guard: ConfigTransactionGuard,
}
impl Session<'_> {
    pub fn operation(&self) -> &Operation {
        &self.stored.operation
    }
    pub fn check_credential(&self) -> Result<Option<OperationStatus>, ConfigError> {
        if self.stored.protected_token.is_none() {
            return Ok(None);
        }
        match self
            .manager
            .credential_operation(&self.stored.operation.request.identity.operation_id)
        {
            Ok(value) => Ok(Some(value)),
            Err(error) if error.code == "operation_not_found" => Ok(None),
            Err(_) => Err(unknown()),
        }
    }
    pub fn cancel(&mut self) -> Result<Operation, ConfigError> {
        if self.stored.operation.status == Status::Pending {
            self.stored.operation.status = Status::Cancelled;
            self.save()?;
        }
        Ok(self.stored.operation.clone())
    }
    fn context(&self) -> Result<String, ConfigError> {
        encoded(&(
            "local-token-material-v1",
            &self.stored.operation.request,
            &self.stored.operation.owner_id,
            &self.stored.operation.credential_ref,
        ))
    }
    fn save(&mut self) -> Result<(), ConfigError> {
        self.store.replace_local_token(&self.stored)
    }
    pub fn ensure_credential(&mut self) -> Result<Receipt, ConfigError> {
        if self.stored.operation.status == Status::Cancelled {
            return fail(
                "local_token_cancelled",
                "原本机令牌操作已停止，未再次创建令牌",
            );
        }
        self.validate_source()?;
        let secret = {
            let _guard = self.manager.provider_edit_guard()?;
            let key = self
                .manager
                .credential_operation_key(true)
                .map_err(|_| unknown())?;
            let context = self.context()?;
            if self.stored.protected_token.is_none() {
                let secret = Zeroizing::new(uuid::Uuid::new_v4().simple().to_string());
                self.stored.protected_token = Some(vault::seal(&key, secret.as_bytes(), &context)?);
                // Persist the exact generated value, encrypted, before the
                // credential operation can issue its first target Vault write.
                self.save().map_err(|_| unknown())?;
            }
            let plain = vault::open(
                &key,
                self.stored.protected_token.as_deref().ok_or_else(unknown)?,
                &context,
            )
            .map_err(|_| unknown())?;
            let text = std::str::from_utf8(&plain).map_err(|_| unknown())?;
            if text.len() != 32 || !text.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err(unknown());
            }
            Zeroizing::new(text.to_owned())
        };
        let receipt = self
            .manager
            .create_credential_operation(
                &self.stored.operation.request.identity.operation_id,
                self.stored.operation.request.purpose.label(),
                secret,
            )
            .map_err(|_| unknown())?;
        if receipt.credential_ref != self.stored.operation.credential_ref {
            return Err(unknown());
        }
        self.validate_source()?;
        Ok(receipt)
    }
    pub fn validate_source(&self) -> Result<(), ConfigError> {
        self.manager
            .validate_local_token_source(self.store, &self.stored.operation.request)
    }
    pub fn mark_owner_saved(&mut self) -> Result<(), ConfigError> {
        if self.stored.operation.status == Status::Cancelled {
            return fail("local_token_cancelled", "原本机令牌操作已停止");
        }
        self.validate_source()?;
        self.stored.operation.owner_saved = true;
        self.save().map_err(|_| unknown())
    }
    pub fn finish_preview(&mut self, preview_id: &str) -> Result<Operation, ConfigError> {
        if preview_id != self.stored.operation.request.identity.operation_id
            || !self.stored.operation.owner_saved
        {
            return Err(unknown());
        }
        if self.stored.operation.status == Status::Cancelled {
            return fail("local_token_cancelled", "原本机令牌操作已停止");
        }
        if self
            .stored
            .operation
            .preview_id
            .as_ref()
            .is_some_and(|id| id != preview_id)
        {
            return fail("operation_conflict", "原本机令牌已归属另一份预览");
        }
        self.stored.operation.status = Status::Committed;
        self.stored.operation.preview_id = Some(preview_id.into());
        self.save().map_err(|_| unknown())?;
        Ok(self.stored.operation.clone())
    }
}

impl ProviderManager {
    /// Used by fixed-route reuse, which intentionally has no new SQL owner.
    pub fn local_token_admission_guard(
        &self,
        store: &Store,
        identity: &IdentityV3,
    ) -> Result<ConfigTransactionGuard, ConfigError> {
        let scope = self.local_scope(store, identity)?;
        let guard = scope.journal.local_owner_lock()?;
        if store.local_token_cancellation(identity)?.is_some() {
            return fail(
                "local_token_cancelled",
                "原编号已阻止后续创建，已有本机入口材料保留",
            );
        }
        Ok(guard)
    }
    pub fn cancel_unowned_local_token(
        &self,
        store: &Store,
        identity: &IdentityV3,
    ) -> Result<Cancellation, ConfigError> {
        let scope = self.local_scope(store, identity)?;
        let _guard = scope.journal.local_owner_lock()?;
        if self
            .status()?
            .transactions
            .iter()
            .any(|t| t.id == identity.operation_id)
        {
            return Err(unknown());
        }
        store.cancel_unowned_local_token(identity)
    }
    fn validate_local_token_source(
        &self,
        store: &Store,
        request: &Request,
    ) -> Result<(), ConfigError> {
        let path = self.adapters.resolve(&request.agent_id)?.path(&self.home)?;
        let current = files::read(&path)?;
        if path.to_string_lossy() != request.configuration_path
            || files::hash(path.as_os_str().as_encoded_bytes()) != request.configuration_path_hash
            || current.stamp.digest.as_deref().unwrap_or("absent") != request.expected_revision
        {
            return fail(
                "conflict",
                "原本机配置路径或版本已变化，请保留原操作核查，未改变本机入口目标",
            );
        }
        let provider = store
            .providers()
            .map_err(|_| corrupt())?
            .into_iter()
            .find(|p| p.id == request.provider.id)
            .ok_or_else(|| {
                ConfigError::new("conflict", "原 Provider 已移除，请保留本机令牌操作核查")
            })?;
        if super::fingerprint(&super::select_model(
            &provider,
            Some(&request.provider.model),
        )?) != super::fingerprint(&request.provider)
        {
            return fail(
                "conflict",
                "原 Provider 或模型版本已改变，未继续创建本机入口",
            );
        }
        if let Some(service) = &request.service {
            let current = store.model_service(&service.id)?;
            if current.version_id != service.version_id
                || current.revision != service.revision
                || !current.models.iter().any(|m| {
                    m.id == current.default_model_id
                        && super::fingerprint(&m.provider) == super::fingerprint(&request.provider)
                })
            {
                return fail(
                    "conflict",
                    "原模型服务版本或默认模型已变化，未继续创建本机入口",
                );
            }
        }
        Ok(())
    }
    fn local_manager(&self, scope: &CredentialScope) -> Result<ProviderManager, ConfigError> {
        let mut manager =
            self.credential_scoped_manager(scope, Some(scope.journal.namespace_id()))?;
        manager.directory = self.directory.join("local-token-operations");
        manager.credential_journal = Some(scope.journal.domain(DOMAIN));
        Ok(manager)
    }
    fn local_scope(
        &self,
        store: &Store,
        identity: &IdentityV3,
    ) -> Result<CredentialScope, ConfigError> {
        if !valid_id(&identity.operation_id) {
            return fail("invalid_local_token", "本机令牌操作编号无效");
        }
        let scope = store.credential_scope(&identity.store_id)?;
        self.credential_scoped_manager(&scope, Some(&identity.namespace_id))?;
        Ok(scope)
    }
    pub fn new_local_token_request(
        &self,
        store: &Store,
        draft: &super::BindingDraft,
        purpose: Purpose,
        provider: &Provider,
        service: Option<ServiceIdentity>,
    ) -> Result<Request, ConfigError> {
        if draft.provider_id.as_deref() != Some(provider.id.as_str())
            || draft
                .expected_provider_revision
                .is_some_and(|r| r != provider.revision)
            || draft.model.as_deref().is_some_and(|m| m != provider.model)
        {
            return fail("conflict", "本机入口的原 Provider 草稿不一致");
        }
        let protocol = self.local_token_protocol(store)?;
        let path = self.adapters.resolve(&draft.agent_id)?.path(&self.home)?;
        let identity = draft.operation.clone().unwrap_or_else(|| IdentityV3 {
            store_id: protocol.store_id,
            namespace_id: protocol.namespace_id,
            operation_id: uuid::Uuid::new_v4().to_string(),
        });
        self.local_scope(store, &identity)?;
        let request = Request {
            identity,
            purpose,
            agent_id: draft.agent_id.clone(),
            configuration_path: path.to_string_lossy().into(),
            configuration_path_hash: files::hash(path.as_os_str().as_encoded_bytes()),
            expected_revision: draft.expected_revision.clone(),
            provider: provider.clone(),
            service,
        };
        validate(&request)?;
        self.validate_local_token_source(store, &request)?;
        Ok(request)
    }
    pub fn local_token_protocol(&self, store: &Store) -> Result<Protocol, ConfigError> {
        let scope = store.credential_scope(&store.database_id()?)?;
        let source = self.scoped_credential_protocol_v3(&scope)?;
        let pending = store.pending_local_tokens(&source.store_id, &source.namespace_id)?;
        Ok(Protocol {
            version: 2,
            schema_version: Store::SCHEMA_VERSION,
            store_id: source.store_id,
            namespace_id: source.namespace_id,
            namespace_state: source.namespace_state,
            operation_limit: LIMIT,
            pending_limit: PENDING_LIMIT,
            cancellation_limit: CANCELLATION_LIMIT,
            pending,
        })
    }
    pub fn begin_local_token<'a>(
        &self,
        store: &'a Store,
        request: Request,
    ) -> Result<Session<'a>, ConfigError> {
        validate(&request)?;
        let scope = self.local_scope(store, &request.identity)?;
        let guard = scope.journal.local_owner_lock()?;
        if store.local_token_cancellation(&request.identity)?.is_some() {
            return fail(
                "local_token_cancelled",
                "原编号已阻止后续创建，已有本机入口材料保留",
            );
        }
        if let Some(existing) = store.read_local_token(&request.identity)?
            && encoded(&existing.operation.request)? != encoded(&request)?
        {
            return fail(
                "operation_conflict",
                "原本机令牌编号已用于其他输入，未创建新令牌",
            );
        }
        self.validate_local_token_source(store, &request)?;
        let manager = self.local_manager(&scope)?;
        let reference = scope
            .journal
            .domain(DOMAIN)
            .reference(&request.identity.operation_id);
        let stored = store.begin_local_token(request, reference)?;
        Ok(Session {
            manager,
            store,
            stored,
            _guard: guard,
        })
    }
    pub fn resume_local_token<'a>(
        &self,
        store: &'a Store,
        identity: &IdentityV3,
    ) -> Result<Session<'a>, ConfigError> {
        let scope = self.local_scope(store, identity)?;
        let guard = scope.journal.local_owner_lock()?;
        let stored = store.read_local_token(identity)?.ok_or_else(unknown)?;
        let manager = self.local_manager(&scope)?;
        if stored.operation.credential_ref
            != scope
                .journal
                .domain(DOMAIN)
                .reference(&identity.operation_id)
        {
            return Err(corrupt());
        }
        Ok(Session {
            manager,
            store,
            stored,
            _guard: guard,
        })
    }
    pub fn local_token_operation(
        &self,
        store: &Store,
        identity: &IdentityV3,
    ) -> Result<Operation, ConfigError> {
        Ok(self.resume_local_token(store, identity)?.stored.operation)
    }
    pub fn check_local_token_credential(
        &self,
        store: &Store,
        identity: &IdentityV3,
    ) -> Result<Option<OperationStatus>, ConfigError> {
        let session = self.resume_local_token(store, identity)?;
        session.check_credential()
    }
    pub fn cancel_local_token(
        &self,
        store: &Store,
        identity: &IdentityV3,
    ) -> Result<Operation, ConfigError> {
        let mut session = self.resume_local_token(store, identity)?;
        // Never delete/overwrite a token or a route/binding: a caller may have
        // observed the owner before its response was lost.
        session.cancel()
    }
}
