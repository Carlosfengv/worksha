use super::{ConfigError, fail};
use aes_gcm::{
    Aes256Gcm, KeyInit, Nonce,
    aead::{Aead, OsRng, Payload, rand_core::RngCore},
};
use zeroize::Zeroizing;

pub const SERVICE: &str = "app.localagent.desktop.provider";
pub trait Vault: Send + Sync {
    fn get(&self, account: &str) -> Result<Zeroizing<Vec<u8>>, ConfigError>;
    fn set(&self, account: &str, bytes: &[u8]) -> Result<(), ConfigError>;
    fn delete(&self, account: &str) -> Result<(), ConfigError>;
}
pub struct SystemVault;
mod worker;
pub use worker::{install_worker, run_worker};
impl Vault for SystemVault {
    fn delete(&self, account: &str) -> Result<(), ConfigError> {
        #[cfg(target_os = "macos")]
        {
            if let Some(result) = worker::request("delete", account, &[]) {
                return result.map(|_| ());
            }
            let _guard = worker::silent()?;
            security_framework::passwords::delete_generic_password(SERVICE, account)
                .map_err(|_| ConfigError::new("credential_unavailable", "临时凭据清理未完成"))
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = account;
            fail("unsupported", "当前版本仅支持 macOS 钥匙串")
        }
    }
    fn get(&self, account: &str) -> Result<Zeroizing<Vec<u8>>, ConfigError> {
        #[cfg(target_os = "macos")]
        {
            if let Some(result) = worker::request("get", account, &[]) {
                return result;
            }
            let _guard = worker::silent()?;
            security_framework::passwords::get_generic_password(SERVICE, account)
                .map(Zeroizing::new)
                .map_err(|e| {
                    ConfigError::new(
                        if e.code() == -25300 {
                            "credential_not_found"
                        } else {
                            "credential_unavailable"
                        },
                        "钥匙串需要授权或当前已锁定；后台未等待系统弹窗。请解锁钥匙串并重新授权此应用后重试",
                    )
                })
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = account;
            fail("unsupported", "当前版本仅支持 macOS 钥匙串")
        }
    }
    fn set(&self, account: &str, bytes: &[u8]) -> Result<(), ConfigError> {
        #[cfg(target_os = "macos")]
        {
            if let Some(result) = worker::request("set", account, bytes) {
                return result.map(|_| ());
            }
            let _guard = worker::silent()?;
            security_framework::passwords::set_generic_password(SERVICE, account, bytes).map_err(
                |_| {
                    ConfigError::new(
                        "credential_unavailable",
                        "未能保存至 macOS 钥匙串，配置未切换",
                    )
                },
            )
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (account, bytes);
            fail("unsupported", "当前版本仅支持 macOS 钥匙串")
        }
    }
}
pub fn random_key() -> [u8; 32] {
    let mut key = [0; 32];
    OsRng.fill_bytes(&mut key);
    key
}
pub fn seal(key: &[u8], plain: &[u8], context: &str) -> Result<Vec<u8>, ConfigError> {
    let cipher = Aes256Gcm::new_from_slice(key)
        .map_err(|_| ConfigError::new("backup_failed", "备份密钥格式无效"))?;
    let mut nonce = [0; 12];
    OsRng.fill_bytes(&mut nonce);
    let encrypted = cipher
        .encrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: plain,
                aad: context.as_bytes(),
            },
        )
        .map_err(|_| ConfigError::new("backup_failed", "无法加密配置备份"))?;
    Ok([b"LAC1".as_slice(), nonce.as_slice(), &encrypted].concat())
}
pub fn open(key: &[u8], bytes: &[u8], context: &str) -> Result<Zeroizing<Vec<u8>>, ConfigError> {
    if bytes.len() < 32 || &bytes[..4] != b"LAC1" {
        return fail("backup_failed", "配置备份格式无效");
    }
    Aes256Gcm::new_from_slice(key)
        .map_err(|_| ConfigError::new("backup_failed", "备份密钥格式无效"))?
        .decrypt(
            Nonce::from_slice(&bytes[4..16]),
            Payload {
                msg: &bytes[16..],
                aad: context.as_bytes(),
            },
        )
        .map(Zeroizing::new)
        .map_err(|_| ConfigError::new("backup_failed", "备份损坏或密钥不匹配，当前配置未覆盖"))
}

#[cfg(any(test, debug_assertions))]
#[derive(Default)]
pub struct MemoryVault(pub std::sync::Mutex<std::collections::HashMap<String, Vec<u8>>>);
#[cfg(any(test, debug_assertions))]
impl Vault for MemoryVault {
    fn delete(&self, account: &str) -> Result<(), ConfigError> {
        self.0.lock().unwrap().remove(account);
        Ok(())
    }
    fn get(&self, account: &str) -> Result<Zeroizing<Vec<u8>>, ConfigError> {
        self.0
            .lock()
            .unwrap()
            .get(account)
            .cloned()
            .map(Zeroizing::new)
            .ok_or_else(|| ConfigError::new("credential_not_found", "测试凭据不存在"))
    }
    fn set(&self, account: &str, bytes: &[u8]) -> Result<(), ConfigError> {
        self.0
            .lock()
            .unwrap()
            .insert(account.into(), bytes.to_vec());
        Ok(())
    }
}
