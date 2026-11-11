pub mod catalog;
pub mod config;
pub mod credential_operations;
pub mod credential_scope;
mod desktop_clients;
mod errors;
pub use errors::{ErrorPolicy, InitialProviderWrite, error_policy};
mod catalog_operations;
mod catalog_sources;
pub use catalog_sources::CatalogSourceSnapshot;
pub mod files;
pub mod imports;
pub mod local_tokens;
mod metadata;
pub mod model_observations;
mod opencode_config;
pub mod operations;
pub mod subscription;
#[cfg(test)]
mod tests;
pub mod upstream_accounts;
pub mod vault;

use crate::Provider;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};
use zeroize::Zeroizing;

#[derive(Debug, Clone, Serialize)]
pub struct ConfigError {
    pub code: String,
    pub message: String,
}
impl ConfigError {
    pub fn new(code: &str, message: &str) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}
pub fn fail<T>(code: &str, message: &str) -> Result<T, ConfigError> {
    Err(ConfigError::new(code, message))
}
pub fn fingerprint(provider: &Provider) -> String {
    files::hash(&serde_json::to_vec(provider).unwrap_or_default())
}
/// Claude Desktop filters gateway IDs. Keep a stable, unique compatibility ID
/// per connection/model; labels and observations always identify the real model.
pub fn claude_desktop_model_id(provider_id: &str, model: &str) -> String {
    config::builtin()
        .resolve("claude-desktop")
        .expect("builtin model alias")
        .model_id(provider_id, model)
}
pub fn select_model(provider: &Provider, model: Option<&str>) -> Result<Provider, ConfigError> {
    let mut selected = provider.clone();
    let model = model.unwrap_or(&provider.model);
    if !provider.model_enabled(model) {
        return fail(
            "invalid_model",
            "请选择该 Provider 已启用的模型；可在供应商详情管理模型启用状态",
        );
    }
    if model != provider.model && !selected.models.contains(&provider.model) {
        selected.models.push(provider.model.clone());
    }
    selected.model = model.into();
    Ok(selected)
}
pub fn current_uid() -> u32 {
    unsafe { libc::geteuid() }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BindingDraft {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation: Option<credential_scope::IdentityV3>,
    pub agent_id: String,
    pub expected_revision: String,
    pub provider_id: Option<String>,
    #[serde(default)]
    pub expected_provider_revision: Option<u64>,
    #[serde(default)]
    pub model: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Transaction {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_token_operation: Option<credential_scope::IdentityV3>,
    pub id: String,
    pub agent_id: String,
    pub state: String,
    pub created_at: String,
    pub applied_at: Option<String>,
    pub before: config::NativeConfig,
    pub target_name: String,
    pub provider: Option<Provider>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub backup_path: String,
    pub restore_of: Option<String>,
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub routing: Option<Routing>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activation: Option<Activation>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Activation {
    pub state: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Routing {
    pub id: String,
    pub client_protocol: String,
    pub upstream_protocol: String,
    pub local_endpoint: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<String>,
}
#[derive(Serialize, Deserialize)]
struct Plan {
    before: Option<Vec<u8>>,
    after: Option<Vec<u8>>,
    before_stamp: files::Stamp,
    after_mode: u32,
}
#[derive(Default, Serialize, Deserialize)]
struct Journal {
    transactions: Vec<Transaction>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderStatus {
    pub configs: Vec<config::NativeConfig>,
    pub transactions: Vec<Transaction>,
    pub backup_protection: &'static str,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CredentialSaved {
    pub credential_ref: String,
    pub credential_label: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Verification {
    pub provider_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_revision: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub fingerprint: String,
    pub state: String,
    pub checked_at: String,
    pub message: String,
}

/// Release the transaction lock at scope exit, even if a child between fork and
/// exec temporarily retains a copy of the same open file description.
pub struct ConfigTransactionGuard {
    file: fs::File,
}
impl Drop for ConfigTransactionGuard {
    fn drop(&mut self) {
        let _ = fs2::FileExt::unlock(&self.file);
    }
}

pub struct ProviderManager {
    adapters: Arc<config::ConfigAdapters>,
    home: PathBuf,
    directory: PathBuf,
    helper_source: PathBuf,
    vault: Arc<dyn vault::Vault>,
    credential_store_id: Option<String>,
    credential_namespace_id: Option<String>,
    credential_journal: Option<Arc<credential_scope::journal::Journal>>,
    legacy_credential_fence: Arc<std::sync::OnceLock<Arc<credential_scope::journal::Journal>>>,
}
impl ProviderManager {
    pub fn new(
        home: PathBuf,
        data: &Path,
        helper_source: PathBuf,
        vault: Arc<dyn vault::Vault>,
    ) -> Result<Self, ConfigError> {
        let adapters = Arc::new(
            config::ConfigAdapters::builtin().map_err(|e| ConfigError::new(e.code, &e.message))?,
        );
        Self::with_adapters(home, data, helper_source, vault, adapters)
    }
    pub fn with_adapters(
        home: PathBuf,
        data: &Path,
        helper_source: PathBuf,
        vault: Arc<dyn vault::Vault>,
        adapters: Arc<config::ConfigAdapters>,
    ) -> Result<Self, ConfigError> {
        let directory = data.join("provider-configs");
        files::private_dir(&directory)?;
        Ok(Self {
            adapters,
            home,
            directory,
            helper_source,
            vault,
            credential_store_id: None,
            credential_namespace_id: None,
            credential_journal: None,
            legacy_credential_fence: Arc::new(std::sync::OnceLock::new()),
        })
    }
    pub fn provider_edit_guard(&self) -> Result<ConfigTransactionGuard, ConfigError> {
        self.lock()
    }
    fn lock(&self) -> Result<ConfigTransactionGuard, ConfigError> {
        if let Some(journal) = &self.credential_journal {
            return journal.lock();
        }
        use std::os::unix::fs::OpenOptionsExt;
        let path = self.directory.join("transaction.lock");
        files::checked_path(&path)?;
        let f = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(path)
            .map_err(|_| ConfigError::new("unavailable", "配置事务锁不可用"))?;
        fs2::FileExt::try_lock_exclusive(&f)
            .map_err(|_| ConfigError::new("busy", "另一个配置事务正在处理，请稍后重试"))?;
        Ok(ConfigTransactionGuard { file: f })
    }
    fn journal(&self) -> Result<Journal, ConfigError> {
        let saved = files::read_protected(&self.directory.join("journal.json"))?;
        match saved.bytes {
            Some(b) => serde_json::from_slice(&b)
                .map_err(|_| ConfigError::new("unavailable", "配置事务记录损坏，停止写入")),
            None => Ok(Journal::default()),
        }
    }
    fn save(&self, journal: &Journal) -> Result<(), ConfigError> {
        let bytes = serde_json::to_vec_pretty(journal)
            .map_err(|_| ConfigError::new("write_failed", "无法保存配置事务记录"))?;
        files::atomic_write(&self.directory.join("journal.json"), &bytes, 0o600)
    }
    fn key(&self, create: bool) -> Result<Zeroizing<Vec<u8>>, ConfigError> {
        if create {
            let path = self.directory.join("backup-key-v2.json");
            if let Some(bytes) = files::read(&path)?.bytes {
                let reference: String = serde_json::from_slice(&bytes)
                    .map_err(|_| ConfigError::new("backup_failed", "备份密钥引用损坏"))?;
                uuid::Uuid::parse_str(&reference)
                    .map_err(|_| ConfigError::new("backup_failed", "备份密钥引用无效"))?;
                return self.vault.get(&reference);
            }
            let reference = uuid::Uuid::new_v4().to_string();
            let key = Zeroizing::new(vault::random_key().to_vec());
            self.vault.set(&reference, &key)?;
            files::atomic_write(
                &path,
                &serde_json::to_vec(&reference)
                    .map_err(|_| ConfigError::new("backup_failed", "备份引用无效"))?,
                0o600,
            )?;
            return Ok(key);
        }
        let account = format!(
            "backup-{}",
            files::hash(self.directory.to_string_lossy().as_bytes())
        );
        match self.vault.get(&account) {
            Ok(k) => Ok(k),
            Err(e)
                if create
                    && e.code == "credential_not_found"
                    && !fs::read_dir(&self.directory)
                        .map_err(|_| ConfigError::new("backup_failed", "备份目录不可读取"))?
                        .flatten()
                        .any(|e| e.path().extension().is_some_and(|x| x == "sealed")) =>
            {
                let k = Zeroizing::new(vault::random_key().to_vec());
                self.vault.set(&account, &k)?;
                Ok(k)
            }
            Err(e) => Err(e),
        }
    }
    fn plan_path(&self, id: &str) -> Result<PathBuf, ConfigError> {
        uuid::Uuid::parse_str(id)
            .map_err(|_| ConfigError::new("invalid_id", "配置事务 ID 无效"))?;
        Ok(self.directory.join(format!("{id}.sealed")))
    }
    fn plan(&self, id: &str) -> Result<Plan, ConfigError> {
        let encrypted = files::read_protected(&self.plan_path(id)?)?
            .bytes
            .ok_or_else(|| ConfigError::new("backup_failed", "找不到配置备份"))?;
        let plain = if encrypted.starts_with(b"LAC2") {
            if encrypted.len() < 40 {
                return fail("backup_failed", "配置备份格式无效");
            }
            let reference = std::str::from_utf8(&encrypted[4..40])
                .map_err(|_| ConfigError::new("backup_failed", "备份引用无效"))?;
            uuid::Uuid::parse_str(reference)
                .map_err(|_| ConfigError::new("backup_failed", "备份引用无效"))?;
            vault::open(&self.vault.get(reference)?, &encrypted[40..], id)?
        } else if encrypted.starts_with(b"LAC1") {
            vault::open(&self.key(false)?, &encrypted, id)?
        } else {
            return fail("backup_failed", "配置备份格式无效");
        };
        serde_json::from_slice(&plain)
            .map_err(|_| ConfigError::new("backup_failed", "无法读取加密配置备份"))
    }
    fn transaction_path(&self, transaction: &Transaction) -> Result<PathBuf, ConfigError> {
        let path = self
            .adapters
            .resolve(&transaction.agent_id)?
            .path(&self.home)?;
        if path.to_string_lossy() != transaction.before.path {
            return fail(
                "conflict",
                "配置适配器的目标路径已变化；原事务与备份保留，请恢复原适配器后核查",
            );
        }
        Ok(path)
    }
    fn inspect_target(&self, agent: &str, path: &Path) -> config::NativeConfig {
        let mut snapshot = config::inspect_with(&self.adapters, &self.home, agent);
        snapshot.path = path.to_string_lossy().into_owned();
        snapshot
    }
    fn recover(&self, journal: &mut Journal) -> Result<(), ConfigError> {
        let mut changed = false;
        for t in journal
            .transactions
            .iter_mut()
            .filter(|t| t.state == "applying")
        {
            let p = self.plan(&t.id)?;
            let current = files::read(&self.transaction_path(t)?)?;
            let after = p.after.as_deref().map(files::hash);
            if current.stamp.digest == after {
                t.state = "applied".into();
                t.applied_at = Some(chrono::Utc::now().to_rfc3339());
            } else if current.stamp == p.before_stamp {
                t.state = "pending".into();
            } else {
                t.state = "conflict".into();
                t.error = Some("中断后发现外部配置变更，备份保留，未再次覆盖".into());
            }
            changed = true;
        }
        if changed {
            self.save(journal)?;
        }
        Ok(())
    }
    /// Read routing metadata without opening credentials or recovering transactions.
    pub fn inspect_configs(&self) -> Vec<config::NativeConfig> {
        self.adapters
            .ids()
            .map(|agent| config::inspect_with(&self.adapters, &self.home, agent))
            .collect()
    }
    pub fn status(&self) -> Result<ProviderStatus, ConfigError> {
        let _lock = self.lock()?;
        let mut j = self.journal()?;
        self.recover(&mut j)?;
        Ok(ProviderStatus {
            configs: self.inspect_configs(),
            transactions: j.transactions.into_iter().rev().collect(),
            backup_protection: "AES-256-GCM · 密钥保存在 macOS 钥匙串",
        })
    }
    pub fn credential(
        &self,
        label: &str,
        secret: Zeroizing<String>,
    ) -> Result<CredentialSaved, ConfigError> {
        let label = label.trim();
        if label.is_empty()
            || label.len() > 200
            || label.chars().any(char::is_control)
            || secret.is_empty()
            || secret.len() > 8192
            || !secret.bytes().all(|b| b.is_ascii_graphic())
        {
            return fail(
                "invalid_credential",
                "请输入凭据名称和不含空白 / 控制字符的 API Key（最多 8192 字节）",
            );
        }
        let reference = uuid::Uuid::new_v4().to_string();
        self.vault.set(&reference, secret.as_bytes())?;
        Ok(CredentialSaved {
            credential_ref: reference,
            credential_label: label.into(),
        })
    }
    pub fn secret(&self, reference: &str) -> Result<Zeroizing<Vec<u8>>, ConfigError> {
        uuid::Uuid::parse_str(reference)
            .map_err(|_| ConfigError::new("invalid_credential", "凭据引用无效"))?;
        self.vault.get(reference)
    }
    pub fn verifications(&self) -> Result<Vec<Verification>, ConfigError> {
        let b = files::read_protected(&self.directory.join("verifications.json"))?.bytes;
        b.map(|b| {
            serde_json::from_slice(&b)
                .map_err(|_| ConfigError::new("unavailable", "验证记录不可读取"))
        })
        .unwrap_or_else(|| Ok(vec![]))
    }
    pub fn save_verification(&self, v: Verification) -> Result<(), ConfigError> {
        let _lock = self.lock()?;
        self.save_verification_locked(v)
    }
    fn save_verification_locked(&self, v: Verification) -> Result<(), ConfigError> {
        let mut list = self.verifications()?;
        list.retain(|old| {
            old.provider_id != v.provider_id || (old.model.is_some() && old.model != v.model)
        });
        list.push(v);
        files::atomic_write(
            &self.directory.join("verifications.json"),
            &serde_json::to_vec(&list)
                .map_err(|_| ConfigError::new("write_failed", "无法保存验证结果"))?,
            0o600,
        )
    }
    /// Commit a successfully probed connection only if its verification can also be recorded.
    pub fn save_connected_provider(
        &self,
        store: &crate::Store,
        provider: Provider,
        checked_at: String,
    ) -> Result<(Provider, Verification), ConfigError> {
        let _lock = self.lock()?;
        let transaction = store
            .connection
            .unchecked_transaction()
            .map_err(|_| ConfigError::new("write_failed", "无法开始连接保存事务"))?;
        let id = provider.id.clone();
        store
            .save_provider(provider)
            .map_err(|message| ConfigError::new("conflict", &message))?;
        let saved = store
            .providers()
            .map_err(|_| ConfigError::new("unavailable", "无法读取已保存连接"))?
            .into_iter()
            .find(|p| p.id == id)
            .ok_or_else(|| ConfigError::new("unavailable", "连接保存未完成"))?;
        let verification = Verification {
            provider_id: id,
            account_revision: None,
            model: Some(saved.model.clone()),
            fingerprint: fingerprint(&saved),
            state: "verified".into(),
            checked_at,
            message: "已自动识别并完成短请求验证；工具、结构化输出及完整容量仍以各自验证为准"
                .into(),
        };
        self.save_verification_locked(verification.clone())?;
        transaction
            .commit()
            .map_err(|_| ConfigError::new("write_failed", "连接保存未提交，请重新读取"))?;
        Ok((saved, verification))
    }
    pub fn finish_provider_operation(
        &self,
        store: &crate::Store,
        request: &operations::Request,
        detected: Provider,
        checked_at: String,
    ) -> Result<operations::Operation, ConfigError> {
        let _lock = self.lock()?;
        store.finish_provider_connection(request, detected, checked_at, |verification| {
            self.save_verification_locked(verification.clone())
        })
    }
    pub fn require_verification(&self, p: &Provider) -> Result<(), ConfigError> {
        self.require_model_verifications(p, vec![p.model.clone()])
    }
    pub fn require_model_verifications(
        &self,
        provider: &Provider,
        models: Vec<String>,
    ) -> Result<(), ConfigError> {
        self.require_target_verifications(provider, models, None)
    }
    pub fn require_upstream_verification(
        &self,
        provider: &Provider,
        account: &upstream_accounts::AccountRef,
    ) -> Result<(), ConfigError> {
        self.require_target_verifications(provider, vec![provider.model.clone()], Some(account))
    }
    fn require_target_verifications(
        &self,
        provider: &Provider,
        models: Vec<String>,
        account: Option<&upstream_accounts::AccountRef>,
    ) -> Result<(), ConfigError> {
        let now = chrono::Utc::now();
        let (id, revision) = account.map_or((provider.id.as_str(), None), |account| {
            (account.id.as_str(), Some(account.revision))
        });
        let verified: std::collections::HashSet<_> = self
            .verifications()?
            .into_iter()
            .filter(|v| {
                v.provider_id == id
                    && v.account_revision == revision
                    && v.state == "verified"
                    && chrono::DateTime::parse_from_rfc3339(&v.checked_at).is_ok_and(|date| {
                        let age = now.signed_duration_since(date);
                        age >= chrono::Duration::zero() && age < chrono::Duration::minutes(15)
                    })
            })
            .map(|v| v.fingerprint)
            .collect();
        let mut missing = Vec::new();
        for model in models {
            if !verified.contains(&fingerprint(&select_model(provider, Some(&model))?)) {
                missing.push(model);
            }
        }
        if missing.is_empty() {
            return Ok(());
        }
        let display = missing
            .iter()
            .take(8)
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join("、");
        let extra = if missing.len() > 8 {
            format!("等 {} 个模型", missing.len())
        } else {
            String::new()
        };
        Err(ConfigError::new(
            "verification_required",
            &format!(
                "需要验证 {display}{extra}。请使用当前端点和凭据验证，或在供应商详情调整要发布的模型。验证结果保留 15 分钟。"
            ),
        ))
    }
    fn helper(&self) -> Result<PathBuf, ConfigError> {
        let bytes = fs::read(&self.helper_source)
            .map_err(|_| ConfigError::new("unavailable", "无法准备已打包的凭据助手"))?;
        if bytes.len() > 128 * 1024 * 1024 {
            return fail("unavailable", "凭据助手文件超限");
        }
        let bin = self.directory.join("bin");
        files::private_dir(&bin)?;
        let target = bin.join(format!("credential-helper-{}", &files::hash(&bytes)[..24]));
        if target.exists() {
            files::checked_path(&target)?;
            if fs::read(&target).is_ok_and(|b| b == bytes) {
                return Ok(target);
            }
            return fail("conflict", "已有凭据助手被修改，拒绝覆盖");
        }
        files::atomic_write(&target, &bytes, 0o700)?;
        Ok(target)
    }
    fn claude_desktop_helper(&self, reference: &str) -> Result<PathBuf, ConfigError> {
        uuid::Uuid::parse_str(reference)
            .map_err(|_| ConfigError::new("invalid_credential", "桌面连接凭据引用无效"))?;
        let helper = self.helper()?;
        let path = helper
            .to_str()
            .ok_or_else(|| ConfigError::new("invalid_config", "凭据助手路径无法表示"))?
            .replace('\'', "'\\''");
        // Older Desktop releases launch the helper without configuration arguments.
        let script = format!("#!/bin/sh\nexec '{path}' --credential '{reference}'\n");
        let target = self.directory.join("bin").join(format!(
            "claude-desktop-{}",
            &files::hash(script.as_bytes())[..24]
        ));
        let before = files::read(&target)?;
        match before.bytes {
            Some(bytes) if bytes != script.as_bytes() => {
                return fail("conflict", "已有 Claude 桌面凭据助手被修改，拒绝覆盖");
            }
            None => files::replace(&target, &before.stamp, Some(script.as_bytes()), 0o700)?,
            Some(_) => {}
        }
        Ok(target)
    }
    fn add(
        &self,
        j: &mut Journal,
        mut t: Transaction,
        p: Plan,
    ) -> Result<Transaction, ConfigError> {
        if j.transactions.len() >= 500 {
            return fail("capacity", "已保留 500 条配置事务，请先核查备份留存");
        }
        let plain = Zeroizing::new(
            serde_json::to_vec(&p)
                .map_err(|_| ConfigError::new("backup_failed", "无法生成配置备份"))?,
        );
        let encrypted = vault::seal(&self.key(true)?, &plain, &t.id)?;
        let reference: String = serde_json::from_slice(
            &files::read(&self.directory.join("backup-key-v2.json"))?
                .bytes
                .ok_or_else(|| ConfigError::new("backup_failed", "备份引用缺失"))?,
        )
        .map_err(|_| ConfigError::new("backup_failed", "备份引用损坏"))?;
        let encrypted = [b"LAC2".as_slice(), reference.as_bytes(), &encrypted].concat();
        let file = self.plan_path(&t.id)?;
        files::atomic_write(&file, &encrypted, 0o600)?;
        // Read and authenticate the durable backup before exposing an applicable transaction.
        let check = self.plan(&t.id)?;
        if check.before != p.before || check.after != p.after {
            return fail("backup_failed", "配置备份复核失败，原文件未修改");
        }
        t.backup_path = file.to_string_lossy().into();
        j.transactions.push(t.clone());
        self.save(j)?;
        Ok(t)
    }
    pub fn prepare(
        &self,
        draft: BindingDraft,
        provider: Option<&Provider>,
    ) -> Result<Transaction, ConfigError> {
        self.prepare_routed(draft, provider, None)
    }
    pub fn prepare_routed(
        &self,
        draft: BindingDraft,
        provider: Option<&Provider>,
        routed: Option<(&Provider, Routing)>,
    ) -> Result<Transaction, ConfigError> {
        self.prepare_catalog(draft, provider, routed, None, None)
    }
    pub fn prepare_fixed_route(
        &self,
        draft: BindingDraft,
        provider: &Provider,
        rendered: &Provider,
        route: Routing,
        codex_catalog: Option<&[u8]>,
    ) -> Result<Transaction, ConfigError> {
        self.prepare_catalog(
            draft,
            Some(provider),
            Some((rendered, route)),
            None,
            codex_catalog,
        )
    }
    pub fn prepare_model_service(
        &self,
        draft: BindingDraft,
        provider: &Provider,
        rendered: &Provider,
        route: Routing,
        catalog: &crate::model_services::ServiceAttachment,
        codex_catalog: Option<&[u8]>,
    ) -> Result<Transaction, ConfigError> {
        self.prepare_catalog(
            draft,
            Some(provider),
            Some((rendered, route)),
            Some(catalog),
            codex_catalog,
        )
    }
    fn codex_catalog_path(&self, bytes: &[u8]) -> PathBuf {
        self.directory
            .join("codex-catalogs")
            .join(format!("{}.json", files::hash(bytes)))
    }
    fn owned_codex_catalog(&self, doc: &toml_edit::DocumentMut) -> Option<(PathBuf, String)> {
        let path = PathBuf::from(doc.get("model_catalog_json")?.as_str()?);
        let digest = path.file_stem()?.to_str()?;
        (path.parent()? == self.directory.join("codex-catalogs")
            && path.extension()?.to_str()? == "json"
            && digest.len() == 64
            && digest
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
        .then(|| (path.clone(), digest.to_owned()))
    }
    pub fn codex_catalog_current(&self, bytes: &[u8]) -> bool {
        let expected = self.codex_catalog_path(bytes);
        let Ok(path) = self
            .adapters
            .resolve("codex")
            .and_then(|a| a.path(&self.home))
        else {
            return false;
        };
        files::read(&path)
            .and_then(|f| config::parse_toml(f.bytes.as_deref()))
            .ok()
            .and_then(|doc| self.owned_codex_catalog(&doc))
            .is_some_and(|(path, _)| {
                path == expected
                    && files::read(&path).is_ok_and(|f| f.bytes.as_deref() == Some(bytes))
            })
    }
    fn check_codex_catalog(&self, after: Option<&[u8]>) -> Result<(), ConfigError> {
        if let Some(after) = after
            && let Ok(doc) = config::parse_toml(Some(after))
            && let Some((path, digest)) = self.owned_codex_catalog(&doc)
            && files::read(&path)?.stamp.digest.as_deref() != Some(&digest)
        {
            return fail(
                "conflict",
                "Codex 模型目录缺失或已变化，未写入配置；请重新同步目录",
            );
        }
        Ok(())
    }
    fn prepare_catalog(
        &self,
        draft: BindingDraft,
        provider: Option<&Provider>,
        routed: Option<(&Provider, Routing)>,
        catalog: Option<&crate::model_services::ServiceAttachment>,
        codex_catalog: Option<&[u8]>,
    ) -> Result<Transaction, ConfigError> {
        let _lock = self.lock()?;
        let mut j = self.journal()?;
        self.recover(&mut j)?;
        let preview_id = if let Some(identity) = &draft.operation {
            if !operations::valid_id(&identity.operation_id) {
                return fail("invalid_local_token", "原本机入口编号无效");
            }
            if j.transactions.iter().any(|t| t.id == identity.operation_id)
                || files::read_protected(&self.plan_path(&identity.operation_id)?)?
                    .bytes
                    .is_some()
            {
                return fail(
                    "local_token_result_unknown",
                    "原预览记录或备份已存在，请按原编号核查；未覆盖原材料",
                );
            }
            identity.operation_id.clone()
        } else {
            uuid::Uuid::new_v4().to_string()
        };
        let path = self.adapters.resolve(&draft.agent_id)?.path(&self.home)?;
        let before = files::read(&path)?;
        if before.stamp.digest.as_deref().unwrap_or("absent") != draft.expected_revision {
            return fail("conflict", "配置版本已改变，请重新读取后预览");
        }
        if j.transactions
            .iter()
            .any(|t| t.agent_id == draft.agent_id && t.state == "pending")
        {
            return fail("conflict", "该 Agent 已有待应用事务，请先应用或撤销");
        }
        if provider.is_none() && draft.model.is_some() {
            return fail("invalid_model", "官方原生登录的模型由 Agent 管理");
        }
        let selected = provider
            .map(|p| select_model(p, draft.model.as_deref()))
            .transpose()?;
        let rendered = routed.as_ref().map(|(p, _)| *p).or(selected.as_ref());
        if draft
            .expected_provider_revision
            .is_some_and(|revision| provider.is_none_or(|p| p.revision != revision))
        {
            return fail("conflict", "供应商版本已变化，请重新选择后预览");
        }
        if let Some(p) = provider {
            validate_source(p)?;
            if subscription::minimax_plan(p)
                && let Some(reference) = &p.credential_ref
            {
                let key = self.secret(reference)?;
                subscription::validate_key(p, Some(&key))?;
            }
        }
        if let Some(p) = rendered {
            validate_with(&self.adapters, &draft.agent_id, p)?;
            if let Some(r) = &p.credential_ref {
                let _ = self.secret(r)?;
            }
        }
        if draft.provider_id.as_deref() != provider.map(|p| p.id.as_str()) {
            return fail("conflict", "目标服务商与草稿不一致");
        }
        let credential_ref = rendered.and_then(|p| p.credential_ref.as_deref());
        let helper = match credential_ref {
            Some(reference) if draft.agent_id == "claude-desktop" => {
                self.claude_desktop_helper(reference)?
            }
            Some(_) => self.helper()?,
            None => self.helper_source.clone(),
        };
        let adapter = self.adapters.resolve(&draft.agent_id)?;
        let mut render_input = None;
        if draft.agent_id == "codex" && before.bytes.is_some() {
            let mut doc = config::parse_toml(before.bytes.as_deref())?;
            if self.owned_codex_catalog(&doc).is_some() {
                doc.remove("model_catalog_json");
                render_input = Some(doc.to_string().into_bytes());
            }
        }
        let (mut after, planning_lease) = self.adapters.plan_native(
            &draft.agent_id,
            render_input.as_deref().or(before.bytes.as_deref()),
            rendered,
            &helper,
        )?;
        if let Some((p, routing)) = &routed
            && !routing.models.is_empty()
        {
            let source =
                provider.ok_or_else(|| ConfigError::new("invalid_provider", "缺少供应商快照"))?;
            if !adapter.supports_model_catalog()
                || routing.models != source.enabled_model_ids()
                || !routing.models.contains(&p.model)
            {
                return fail(
                    "invalid_model",
                    "模型分组必须匹配此 Provider 全部已启用模型",
                );
            }
            if !adapter.requires_inline_credential() {
                adapter.models(&mut after, p, &routing.models, None)?;
            }
        }
        if adapter.requires_inline_credential() {
            let p = rendered
                .ok_or_else(|| ConfigError::new("invalid_provider", "OpenCode 需要供应商连接"))?;
            let key = self.secret(
                p.credential_ref
                    .as_deref()
                    .ok_or_else(|| ConfigError::new("invalid_credential", "本机路由凭据缺失"))?,
            )?;
            adapter.models(
                &mut after,
                p,
                routed
                    .as_ref()
                    .map(|(_, route)| route.models.as_slice())
                    .unwrap_or_default(),
                Some(
                    std::str::from_utf8(&key).map_err(|_| {
                        ConfigError::new("invalid_credential", "本机路由凭据格式无效")
                    })?,
                ),
            )?;
        }
        if let Some(catalog) = catalog {
            adapter.service_models(
                &mut after,
                rendered
                    .ok_or_else(|| ConfigError::new("invalid_provider", "模型服务缺少本机入口"))?,
                catalog,
            )?;
            config::preserve_service_selection(
                &draft.agent_id,
                before.bytes.as_deref(),
                &mut after,
                catalog,
            )?;
        }
        if draft.agent_id == "opencode" {
            opencode_config::preserve_route_allowlist(&path, before.bytes.as_deref(), &mut after)?;
        }
        if draft.agent_id == "codex" && (codex_catalog.is_some() || catalog.is_some()) {
            let bytes = codex_catalog
                .filter(|b| !b.is_empty() && b.len() as u64 <= files::LIMIT)
                .ok_or_else(|| ConfigError::new("invalid_config", "Codex 模型目录无法生成"))?;
            let path = self.codex_catalog_path(bytes);
            files::private_dir(&self.directory.join("codex-catalogs"))?;
            let snapshot = files::read(&path)?;
            if snapshot.bytes.as_deref() != Some(bytes) {
                files::replace(&path, &snapshot.stamp, Some(bytes), 0o600)?;
            }
            let mut doc = config::parse_toml(Some(&after))?;
            doc["model_catalog_json"] =
                toml_edit::value(path.to_str().ok_or_else(|| {
                    ConfigError::new("invalid_config", "Codex 模型目录路径无法表示")
                })?);
            after = doc.to_string().into_bytes();
        }
        if after.len() as u64 > files::LIMIT {
            return fail("invalid_config", "生成的配置超过 1 MiB，原文件未修改");
        }
        let t = Transaction {
            local_token_operation: draft.operation,
            id: preview_id,
            agent_id: draft.agent_id.clone(),
            state: "pending".into(),
            created_at: chrono::Utc::now().to_rfc3339(),
            applied_at: None,
            before: self.inspect_target(&draft.agent_id, &path),
            target_name: catalog
                .map(|c| format!("模型服务 · {}", c.installed.name))
                .or_else(|| {
                    selected
                        .as_ref()
                        .map(|p| format!("{} · {}", p.name, p.model))
                })
                .unwrap_or_else(|| "官方原生登录（由 Agent 管理登录与模型）".into()),
            provider: provider.cloned(),
            model: draft.model,
            backup_path: String::new(),
            restore_of: None,
            error: None,
            routing: routed.map(|(_, route)| route),
            activation: None,
        };
        planning_lease
            .ensure_active()
            .map_err(|_| config::stopped())?;
        self.add(
            &mut j,
            t,
            Plan {
                before: before.bytes,
                after: Some(after),
                before_stamp: before.stamp,
                after_mode: 0o600,
            },
        )
    }
    pub fn prepare_restore(&self, id: &str) -> Result<Transaction, ConfigError> {
        let _lock = self.lock()?;
        let mut j = self.journal()?;
        self.recover(&mut j)?;
        let original = j
            .transactions
            .iter()
            .find(|t| t.id == id && t.state == "applied")
            .cloned()
            .ok_or_else(|| ConfigError::new("not_found", "找不到已应用的备份事务"))?;
        if j.transactions
            .iter()
            .any(|t| t.agent_id == original.agent_id && t.state == "pending")
        {
            return fail("conflict", "请先处理该 Agent 的待应用事务");
        }
        let backup = self.plan(id)?;
        let path = self.transaction_path(&original)?;
        let before = files::read(&path)?;
        if before.stamp.digest != backup.after.as_deref().map(files::hash) {
            return fail(
                "conflict",
                "当前文件已不同于该次切换结果；为保留外部修改，未自动恢复",
            );
        }
        let t = Transaction {
            local_token_operation: None,
            id: uuid::Uuid::new_v4().to_string(),
            agent_id: original.agent_id.clone(),
            state: "pending".into(),
            created_at: chrono::Utc::now().to_rfc3339(),
            applied_at: None,
            before: self.inspect_target(&original.agent_id, &path),
            target_name: format!("恢复 {} 的原配置", original.created_at),
            provider: None,
            model: None,
            backup_path: String::new(),
            restore_of: Some(id.into()),
            error: None,
            routing: None,
            activation: None,
        };
        self.add(
            &mut j,
            t,
            Plan {
                before: before.bytes,
                after: backup.before,
                before_stamp: before.stamp,
                after_mode: backup.before_stamp.mode,
            },
        )
    }
    pub fn cancel(&self, id: &str) -> Result<Transaction, ConfigError> {
        let _lock = self.lock()?;
        let mut j = self.journal()?;
        self.recover(&mut j)?;
        let t = j
            .transactions
            .iter_mut()
            .find(|t| t.id == id && t.state == "pending")
            .ok_or_else(|| ConfigError::new("not_found", "找不到待应用事务"))?;
        t.state = "cancelled".into();
        let result = t.clone();
        self.save(&j)?;
        Ok(result)
    }
    pub fn apply(
        &self,
        id: &str,
        current_provider: Option<&Provider>,
        idle: bool,
    ) -> Result<Transaction, ConfigError> {
        self.apply_with_provider(id, || Ok(current_provider.cloned()), idle)
    }
    /// Records a user's deferral or a native launch request, never proof of model traffic.
    pub fn activation(&self, id: &str, state: &str) -> Result<Transaction, ConfigError> {
        let _lock = self.lock()?;
        let mut j = self.journal()?;
        self.recover(&mut j)?;
        let t = j
            .transactions
            .iter_mut()
            .find(|t| t.id == id)
            .ok_or_else(|| ConfigError::new("not_found", "找不到配置事务"))?;
        if !matches!(
            (t.state.as_str(), state),
            ("pending", "deferred") | ("applied", "launch_requested")
        ) {
            return fail("conflict", "配置事务状态已变化，请重新读取");
        }
        if state == "launch_requested" {
            self.check_target(t)?;
        }
        t.activation = Some(Activation {
            state: state.into(),
            updated_at: chrono::Utc::now().to_rfc3339(),
        });
        let result = t.clone();
        self.save(&j)?;
        Ok(result)
    }
    fn check_target(&self, t: &Transaction) -> Result<(), ConfigError> {
        let plan = self.plan(&t.id)?;
        let current = files::read(&self.transaction_path(t)?)?;
        if current.stamp.digest != plan.after.as_deref().map(files::hash) {
            return fail("conflict", "当前配置已变化，请重新读取；未重启 Agent");
        }
        Ok(())
    }
    pub fn check_switch(
        &self,
        id: &str,
        provider: Option<&Provider>,
    ) -> Result<Transaction, ConfigError> {
        let _lock = self.lock()?;
        let mut j = self.journal()?;
        self.recover(&mut j)?;
        let t = j
            .transactions
            .iter()
            .find(|t| t.id == id)
            .ok_or_else(|| ConfigError::new("not_found", "找不到配置事务"))?;
        match t.state.as_str() {
            "pending" => {
                if t.restore_of.is_none()
                    && t.provider.as_ref().map(fingerprint) != provider.map(fingerprint)
                {
                    return fail("conflict", "服务商配置或凭据版本已改变，请撤销后重新预览");
                }
                let path = self.transaction_path(t)?;
                let plan = self.plan(id)?;
                if files::read(&path)?.stamp != plan.before_stamp {
                    return fail(
                        "conflict",
                        "配置被其他程序修改，请撤销后重新预览；未重启 Agent",
                    );
                }
                Self::check_opencode_route(t, &plan, &path)?;
            }
            "applied" => self.check_target(t)?,
            _ => return fail("conflict", "此事务已结束，请重新读取"),
        }
        Ok(t.clone())
    }
    fn check_opencode_route(t: &Transaction, plan: &Plan, path: &Path) -> Result<(), ConfigError> {
        if t.agent_id == "opencode"
            && t.restore_of.is_none()
            && let Some(after) = plan.after.as_deref()
        {
            opencode_config::check_route_allowlist(path, plan.before.as_deref(), after)?;
        }
        Ok(())
    }
    pub fn apply_with_provider(
        &self,
        id: &str,
        read_provider: impl FnOnce() -> Result<Option<Provider>, ConfigError>,
        idle: bool,
    ) -> Result<Transaction, ConfigError> {
        self.apply_checked(id, read_provider, || {
            if idle {
                Ok(())
            } else {
                fail(
                    "agent_busy",
                    "相关 Agent 仍在运行或进程状态无法确认，尚未写入；请关闭相关 Agent 后重试，或撤销待应用事务",
                )
            }
        })
    }
    pub fn apply_checked(
        &self,
        id: &str,
        read_provider: impl FnOnce() -> Result<Option<Provider>, ConfigError>,
        check_idle: impl FnOnce() -> Result<(), ConfigError>,
    ) -> Result<Transaction, ConfigError> {
        let _lock = self.lock()?;
        let mut j = self.journal()?;
        self.recover(&mut j)?;
        let index = j
            .transactions
            .iter()
            .position(|t| t.id == id)
            .ok_or_else(|| ConfigError::new("not_found", "找不到配置事务"))?;
        let t = &j.transactions[index];
        if t.state == "applied" {
            return Ok(t.clone());
        }
        if t.state != "pending" {
            return fail("conflict", "此事务已结束，请重新读取");
        }
        if t.restore_of.is_none() {
            let current_provider = read_provider()?;
            match (&t.provider, current_provider.as_ref()) {
                (Some(a), Some(b)) if fingerprint(a) == fingerprint(b) => (),
                (None, None) => (),
                _ => return fail("conflict", "服务商配置或凭据版本已改变，请撤销后重新预览"),
            }
        }
        let p = self.plan(id)?;
        let path = self.transaction_path(t)?;
        if files::read(&path)?.stamp != p.before_stamp {
            return fail("conflict", "配置被其他程序修改，未覆盖；请撤销后重新预览");
        }
        if t.agent_id == "codex" {
            self.check_codex_catalog(p.after.as_deref())?;
        }
        check_idle()?;
        Self::check_opencode_route(t, &p, &path)?;
        j.transactions[index].state = "applying".into();
        j.transactions[index].activation = Some(Activation {
            state: "awaiting_restart".into(),
            updated_at: chrono::Utc::now().to_rfc3339(),
        });
        self.save(&j)?;
        if let Err(e) = files::replace(&path, &p.before_stamp, p.after.as_deref(), p.after_mode) {
            self.recover(&mut j)?;
            return Err(e);
        }
        self.recover(&mut j)?;
        Ok(j.transactions[index].clone())
    }
}

pub fn is_loopback_url(url: &url::Url) -> bool {
    match url.host() {
        Some(url::Host::Domain("localhost")) => true,
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        _ => false,
    }
}

pub fn validate(agent: &str, p: &Provider) -> Result<(), ConfigError> {
    validate_with(config::builtin(), agent, p)
}
fn validate_with(
    adapters: &config::ConfigAdapters,
    agent: &str,
    p: &Provider,
) -> Result<(), ConfigError> {
    if adapters.resolve(agent).ok().and_then(|a| a.protocol()) != Some(p.protocol.as_str()) {
        return fail(
            "incompatible",
            "Claude Code 需要 Messages，Codex 需要 Responses；不会自动转换 Chat Completions",
        );
    }
    validate_source(p)
}

pub fn validate_endpoint(endpoint: &str) -> Result<url::Url, ConfigError> {
    let url =
        url::Url::parse(endpoint).map_err(|_| ConfigError::new("invalid_provider", "端点无效"))?;
    let local = is_loopback_url(&url);
    if !(url.scheme() == "https" || url.scheme() == "http" && local)
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return fail(
            "invalid_provider",
            "远程连接需要 HTTPS；HTTP 仅用于本机回环端点，禁止 URL 凭据或参数",
        );
    }
    Ok(url)
}
pub fn validate_source(p: &Provider) -> Result<(), ConfigError> {
    if !["messages", "responses", "chat-completions"].contains(&p.protocol.as_str()) {
        return fail("incompatible", "供应商接口协议不受支持");
    }
    let url = validate_endpoint(&p.endpoint)?;
    subscription::validate_endpoint(p)?;
    if !p.configured
        || p.model.trim().is_empty()
        || (!is_loopback_url(&url) && p.credential_ref.is_none())
    {
        return fail(
            "invalid_provider",
            "请先保存模型和钥匙串凭据；本地无鉴权连接可不设置密钥",
        );
    }
    Ok(())
}
