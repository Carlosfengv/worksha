//! A pinned helper keeps its Keychain code identity across desktop rebuilds.
use super::{SystemVault, Vault};
use crate::providers::{ConfigError, files};
use serde::{Deserialize, Serialize};
use std::{
    io::{Read, Write},
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::Mutex,
    time::{Duration, Instant},
};
use zeroize::Zeroizing;

#[derive(Serialize, Deserialize)]
struct Worker {
    digest: String,
}
#[derive(PartialEq, Eq)]
struct WorkerFile {
    device: u64,
    inode: u64,
    len: u64,
    modified: (i64, i64),
    changed: (i64, i64),
}
static VERIFIED_WORKER: Mutex<Option<(PathBuf, WorkerFile)>> = Mutex::new(None);

fn worker_file(path: &Path) -> Result<WorkerFile, ConfigError> {
    files::checked_path(path)?;
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|_| ConfigError::new("unavailable", "钥匙串助手缺失或不可读取"))?;
    if !metadata.is_file()
        || metadata.nlink() != 1
        || metadata.uid() != super::super::current_uid()
        || metadata.mode() & 0o022 != 0
        || metadata.mode() & 0o111 == 0
    {
        return Err(ConfigError::new(
            "unsafe_path",
            "钥匙串助手权限或文件类型无效",
        ));
    }
    Ok(WorkerFile {
        device: metadata.dev(),
        inode: metadata.ino(),
        len: metadata.len(),
        modified: (metadata.mtime(), metadata.mtime_nsec()),
        changed: (metadata.ctime(), metadata.ctime_nsec()),
    })
}
#[derive(Serialize, Deserialize)]
struct Request {
    operation: String,
    account: String,
    bytes: Vec<u8>,
}
#[derive(Serialize, Deserialize)]
struct Reply {
    bytes: Option<Vec<u8>>,
    code: Option<String>,
    message: Option<String>,
}
fn directory() -> Option<PathBuf> {
    Some(
        dirs::home_dir()?
            .join("Library/Application Support/app.localagent.desktop/provider-configs"),
    )
}
fn executable(directory: &Path, digest: &str) -> Result<PathBuf, ConfigError> {
    if digest.len() != 64 || !digest.bytes().all(|c| c.is_ascii_hexdigit()) {
        return Err(ConfigError::new("unsafe_path", "钥匙串助手摘要无效"));
    }
    let path = directory.join("bin").join(format!("vault-{digest}"));
    let mut verified = VERIFIED_WORKER
        .lock()
        .map_err(|_| ConfigError::new("unavailable", "钥匙串助手核对状态不可用"))?;
    let identity = worker_file(&path)?;
    // Cache the executable check only. Each operation still reads the live
    // Keychain. ctime also changes when an edit restores the original mtime.
    if verified
        .as_ref()
        .is_some_and(|(previous, file)| previous == &path && file == &identity)
    {
        return Ok(path);
    }
    let actual = files::read_executable(&path)?
        .stamp
        .digest
        .ok_or_else(|| ConfigError::new("unavailable", "钥匙串助手缺失"))?;
    if actual != digest || worker_file(&path)? != identity {
        return Err(ConfigError::new(
            "unsafe_path",
            "钥匙串助手已被修改，拒绝执行",
        ));
    }
    *verified = Some((path.clone(), identity));
    Ok(path)
}

#[cfg(test)]
#[test]
fn verified_worker_rechecks_modified_executable() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
    let bin = tmp.path().join("bin");
    files::private_dir(&bin).unwrap();
    let digest = files::hash(b"worker");
    let path = bin.join(format!("vault-{digest}"));
    files::atomic_write(&path, b"worker", 0o700).unwrap();
    assert_eq!(executable(tmp.path(), &digest).unwrap(), path);
    assert_eq!(executable(tmp.path(), &digest).unwrap(), path);
    let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
    let file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
    (&file).write_all(b"edited").unwrap();
    file.set_modified(modified).unwrap();
    assert_eq!(
        executable(tmp.path(), &digest).unwrap_err().code,
        "unsafe_path"
    );
    files::atomic_write(&path, b"worker", 0o700).unwrap();
    assert_eq!(executable(tmp.path(), &digest).unwrap(), path);
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(
        executable(tmp.path(), &digest).unwrap_err().code,
        "unsafe_path"
    );
}

pub fn install_worker() -> Result<(), ConfigError> {
    let dir = directory().ok_or_else(|| ConfigError::new("unavailable", "用户目录不可用"))?;
    files::private_dir(&dir)?;
    let _lock = files::exclusive_lock(&dir.join("vault-install.lock"))?;
    let record = dir.join("vault-helper.json");
    if let Some(bytes) = files::read(&record)?.bytes {
        let worker: Worker = serde_json::from_slice(&bytes)
            .map_err(|_| ConfigError::new("unavailable", "钥匙串助手记录损坏"))?;
        executable(&dir, &worker.digest)?;
        return Ok(());
    }
    let source = std::env::current_exe()
        .map_err(|_| ConfigError::new("unavailable", "无法确定应用包内服务的位置"))?;
    let snapshot = files::read_worker_source(&source)?;
    let (Some(bytes), Some(digest)) = (snapshot.bytes, snapshot.stamp.digest) else {
        return Err(ConfigError::new("unavailable", "打包助手缺失"));
    };
    files::private_dir(&dir.join("bin"))?;
    files::atomic_write(
        &dir.join("bin").join(format!("vault-{digest}")),
        &bytes,
        0o700,
    )?;
    files::atomic_write(
        &record,
        &serde_json::to_vec(&Worker { digest })
            .map_err(|_| ConfigError::new("unavailable", "助手记录无效"))?,
        0o600,
    )
}
pub(super) fn request(
    operation: &str,
    account: &str,
    bytes: &[u8],
) -> Option<Result<Zeroizing<Vec<u8>>, ConfigError>> {
    if std::env::var_os("LOCAL_AGENT_VAULT_WORKER").is_some() {
        return None;
    }
    let dir = directory()?;
    if !dir.join("vault-helper.json").exists() {
        return None;
    }
    Some((|| {
        let record = files::read(&dir.join("vault-helper.json"))?
            .bytes
            .ok_or_else(|| ConfigError::new("unavailable", "助手记录缺失"))?;
        let worker: Worker = serde_json::from_slice(&record)
            .map_err(|_| ConfigError::new("unavailable", "助手记录损坏"))?;
        let path = executable(&dir, &worker.digest)?;
        let mut child = Command::new(path)
            .arg("--vault-worker")
            .env("LOCAL_AGENT_VAULT_WORKER", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| ConfigError::new("credential_unavailable", "无法启动钥匙串助手"))?;
        let input = Zeroizing::new(
            serde_json::to_vec(&Request {
                operation: operation.into(),
                account: account.into(),
                bytes: bytes.into(),
            })
            .map_err(|_| ConfigError::new("invalid_credential", "凭据请求无效"))?,
        );
        let written = child
            .stdin
            .take()
            .ok_or_else(|| ConfigError::new("credential_unavailable", "助手输入不可用"))?
            .write_all(&input);
        if written.is_err() {
            let _ = child.kill();
            let _ = child.wait();
            return Err(ConfigError::new("credential_unavailable", "助手输入失败"));
        }
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| ConfigError::new("credential_unavailable", "助手输出不可用"))?;
        let reader = std::thread::spawn(move || {
            let mut output = Zeroizing::new(vec![]);
            stdout.take(65536).read_to_end(&mut output).map(|_| output)
        });
        let start = Instant::now();
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    let output = reader
                        .join()
                        .map_err(|_| ConfigError::new("credential_unavailable", "助手读取失败"))?
                        .map_err(|_| ConfigError::new("credential_unavailable", "助手读取失败"))?;
                    if !status.success() {
                        return Err(ConfigError::new(
                            "credential_unavailable",
                            "钥匙串助手未完成操作",
                        ));
                    }
                    let mut reply: Reply = serde_json::from_slice(&output).map_err(|_| {
                        ConfigError::new("credential_unavailable", "助手返回格式无效")
                    })?;
                    return if let Some(code) = reply.code {
                        Err(ConfigError::new(
                            &code,
                            reply.message.as_deref().unwrap_or("钥匙串不可用"),
                        ))
                    } else {
                        Ok(Zeroizing::new(reply.bytes.take().unwrap_or_default()))
                    };
                }
                Err(_) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(ConfigError::new(
                        "credential_unavailable",
                        "钥匙串助手状态不可用",
                    ));
                }
                Ok(None) => (),
            }
            if start.elapsed() > Duration::from_secs(5) {
                let _ = child.kill();
                let _ = child.wait();
                return Err(ConfigError::new(
                    "credential_timeout",
                    "钥匙串读取超过 5 秒，已停止等待；请检查系统钥匙串权限后重试",
                ));
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    })())
}
pub fn run_worker() -> Result<(), String> {
    let mut input = Zeroizing::new(vec![]);
    std::io::stdin()
        .take(65537)
        .read_to_end(&mut input)
        .map_err(|_| "助手输入失败")?;
    if input.len() > 65536 {
        return Err("助手请求超限".into());
    }
    let mut request: Request = serde_json::from_slice(&input).map_err(|_| "助手请求无效")?;
    if request.account.is_empty()
        || request.account.len() > 128
        || request.account.chars().any(char::is_control)
        || request.bytes.len() > 8192
    {
        return Err("助手参数无效".into());
    }
    let bytes = Zeroizing::new(std::mem::take(&mut request.bytes));
    let result = match request.operation.as_str() {
        "get" => SystemVault.get(&request.account),
        "set" => SystemVault
            .set(&request.account, &bytes)
            .map(|_| Zeroizing::new(vec![])),
        "delete" => SystemVault
            .delete(&request.account)
            .map(|_| Zeroizing::new(vec![])),
        _ => return Err("助手操作无效".into()),
    };
    let reply = match result {
        Ok(mut b) => Reply {
            bytes: Some(std::mem::take(&mut *b)),
            code: None,
            message: None,
        },
        Err(e) => Reply {
            bytes: None,
            code: Some(e.code),
            message: Some(e.message),
        },
    };
    let mut output = Zeroizing::new(serde_json::to_vec(&reply).map_err(|_| "助手响应失败")?);
    let result = std::io::stdout()
        .write_all(&output)
        .map_err(|_| "助手写入失败".to_string());
    use zeroize::Zeroize;
    if let Some(mut b) = reply.bytes {
        b.zeroize();
    }
    output.zeroize();
    result
}
#[cfg(target_os = "macos")]
pub(super) fn silent()
-> Result<security_framework::os::macos::keychain::KeychainUserInteractionLock, ConfigError> {
    security_framework::os::macos::keychain::SecKeychain::disable_user_interaction()
        .map_err(|_| ConfigError::new("credential_unavailable", "无法设置后台钥匙串读取模式"))
}
