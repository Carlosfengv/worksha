use super::{ConfigError, fail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
};

pub const LIMIT: u64 = 1024 * 1024;
const EXECUTABLE_LIMIT: u64 = 128 * LIMIT;
pub fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

// Fixed roots only; don't follow user-controlled symlinks or write through hard links.
pub fn checked_path(path: &Path) -> Result<(), ConfigError> {
    check_path(path, false)
}

fn check_path(path: &Path, bundle_source: bool) -> Result<(), ConfigError> {
    if !path.is_absolute() {
        return fail("unsafe_path", "配置位置必须是绝对路径");
    }
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(m) if m.file_type().is_symlink() => {
                return fail(
                    "unsafe_path",
                    "配置路径包含符号链接，请先在 Agent 中核查实际位置",
                );
            }
            Ok(m)
                if m.mode() & 0o022 != 0
                    && current != Path::new("/tmp")
                    && current != Path::new("/private/tmp")
                    && !(bundle_source
                        && m.is_dir()
                        && is_macos_applications(&current, m.mode(), m.uid(), m.gid())) =>
            {
                return fail("unsafe_path", "配置路径可被其他用户写入，拒绝覆盖");
            }
            Ok(_) => (),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(_) => return fail("unavailable", "无法检查配置目录权限"),
        }
    }
    Ok(())
}

fn is_macos_applications(path: &Path, mode: u32, uid: u32, gid: u32) -> bool {
    // macOS installs shared apps under root:admin (GID 80), often mode 0775.
    cfg!(target_os = "macos")
        && path == Path::new("/Applications")
        && uid == 0
        && gid == 80
        && mode & 0o002 == 0
}

pub fn private_dir(path: &Path) -> Result<(), ConfigError> {
    checked_path(path)?;
    fs::create_dir_all(path).map_err(|_| ConfigError::new("unavailable", "无法创建受保护目录"))?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .map_err(|_| ConfigError::new("unavailable", "无法保护配置目录"))
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Stamp {
    pub digest: Option<String>,
    pub inode: u64,
    pub device: u64,
    pub mode: u32,
    pub modified_sec: i64,
    pub modified_nsec: i64,
}
pub struct FileSnapshot {
    pub bytes: Option<Vec<u8>>,
    pub stamp: Stamp,
}
pub fn read(path: &Path) -> Result<FileSnapshot, ConfigError> {
    read_limit(path, LIMIT)
}
pub fn read_protected(path: &Path) -> Result<FileSnapshot, ConfigError> {
    read_limit(path, 16 * LIMIT)
}
pub(super) fn read_credential_record(path: &Path) -> Result<FileSnapshot, ConfigError> {
    read_limit(path, 4096)
}
pub fn read_executable(path: &Path) -> Result<FileSnapshot, ConfigError> {
    read_limit(path, EXECUTABLE_LIMIT)
}

/// Read the running service for the initial copy into the user's private vault.
/// The installed bundle is only a read source. The pinned worker uses the
/// ordinary private-file rules when opened for execution.
pub(super) fn read_worker_source(path: &Path) -> Result<FileSnapshot, ConfigError> {
    check_path(path, true)?;
    read_checked(path, EXECUTABLE_LIMIT, true)
}
fn read_limit(path: &Path, limit: u64) -> Result<FileSnapshot, ConfigError> {
    checked_path(path)?;
    read_checked(path, limit, false)
}

fn read_checked(path: &Path, limit: u64, bundle_source: bool) -> Result<FileSnapshot, ConfigError> {
    let mut file = match fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
    {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(FileSnapshot {
                bytes: None,
                stamp: Stamp {
                    digest: None,
                    inode: 0,
                    device: 0,
                    mode: 0o600,
                    modified_sec: 0,
                    modified_nsec: 0,
                },
            });
        }
        Err(_) => return fail("unavailable", "无法读取配置文件，请检查系统目录权限"),
    };
    let m = file
        .metadata()
        .map_err(|_| ConfigError::new("unavailable", "无法检查配置文件"))?;
    let owned = m.uid() == unsafe { libc::geteuid() } || (bundle_source && m.uid() == 0);
    if !m.is_file()
        || m.nlink() != 1
        || !owned
        || m.len() > limit
        || (bundle_source && (m.mode() & 0o022 != 0 || m.mode() & 0o111 == 0))
    {
        if bundle_source {
            return fail(
                "unsafe_path",
                "应用包内服务的所有者、权限或文件类型不符合要求",
            );
        }
        return fail(
            "unsafe_path",
            "配置必须是当前用户拥有的单链接普通文件，且小于 1 MiB",
        );
    }
    let mut bytes = vec![];
    (&mut file)
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| ConfigError::new("unavailable", "配置读取中断"))?;
    let after = file
        .metadata()
        .map_err(|_| ConfigError::new("unavailable", "无法复核配置文件"))?;
    if bytes.len() as u64 > limit
        || m.len() != after.len()
        || m.mtime() != after.mtime()
        || m.mtime_nsec() != after.mtime_nsec()
    {
        return fail("conflict", "配置正在被其他程序修改，请重新读取");
    }
    let stamp = Stamp {
        digest: Some(hash(&bytes)),
        inode: m.ino(),
        device: m.dev(),
        mode: m.mode() & 0o777,
        modified_sec: m.mtime(),
        modified_nsec: m.mtime_nsec(),
    };
    Ok(FileSnapshot {
        bytes: Some(bytes),
        stamp,
    })
}

#[cfg(all(test, target_os = "macos"))]
#[test]
fn bundle_source_allows_only_the_admin_managed_applications_parent() {
    let apps = Path::new("/Applications");
    assert!(is_macos_applications(apps, 0o775, 0, 80));
    assert!(!is_macos_applications(apps, 0o777, 0, 80));
    assert!(!is_macos_applications(apps, 0o775, 501, 80));
    assert!(!is_macos_applications(apps, 0o775, 0, 20));
    assert!(!is_macos_applications(
        Path::new("/tmp/Applications"),
        0o775,
        0,
        80
    ));
}

pub fn atomic_write(path: &Path, bytes: &[u8], mode: u32) -> Result<(), ConfigError> {
    checked_path(path)?;
    let parent = path
        .parent()
        .ok_or_else(|| ConfigError::new("unsafe_path", "配置没有父目录"))?;
    if !parent.exists() {
        private_dir(parent)?;
    }
    let tmp = parent.join(format!(".local-agent-{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        f.set_permissions(fs::Permissions::from_mode(mode))?;
        fs::rename(&tmp, path)?;
        fs::File::open(parent)?.sync_all()
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
        return fail("write_failed", "配置写入未完成，请重新读取事务结果");
    }
    Ok(())
}

pub fn exclusive_lock(path: &Path) -> Result<fs::File, ConfigError> {
    checked_path(path)?;
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(|_| ConfigError::new("unavailable", "本地路由锁不可读取"))?;
    fs2::FileExt::try_lock_exclusive(&file)
        .map_err(|_| ConfigError::new("busy", "此应用数据已有本地路由服务运行"))?;
    Ok(file)
}

pub fn replace(
    path: &Path,
    expected: &Stamp,
    bytes: Option<&[u8]>,
    mode: u32,
) -> Result<(), ConfigError> {
    if read(path)?.stamp != *expected {
        return fail("conflict", "配置已被其他程序修改，原文件未覆盖；请重新读取");
    }
    match bytes {
        Some(bytes) => atomic_write(path, bytes, mode),
        None => {
            if expected.digest.is_some() {
                fs::remove_file(path).map_err(|_| {
                    ConfigError::new("write_failed", "未能恢复到原先无配置文件的状态")
                })?;
                fs::File::open(path.parent().unwrap())
                    .and_then(|f| f.sync_all())
                    .map_err(|_| ConfigError::new("write_failed", "恢复结果尚未确认写入磁盘"))?;
            }
            Ok(())
        }
    }
}
