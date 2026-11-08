//! Private discovery records shared by the desktop owner and its local service.
use crate::providers::{ConfigError, files};
use serde::{Deserialize, Serialize};
use std::{
    fs::File,
    path::{Path, PathBuf},
};

pub const PROTOCOL: u32 = 1;
pub const IAM_PROTOCOL_HEADER: &str = "x-local-agent-iam-protocol";
pub const IAM_PROTOCOL: &str = "1";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Identity {
    pub protocol: u32,
    pub version: String,
    pub instance_id: String,
    pub pid: u32,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub persistent: bool,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ServiceRecord {
    pub identity: Identity,
    pub port: u16,
    pub token: String,
}

pub fn directory(data: &Path) -> PathBuf {
    data.join("runtime")
}

pub fn desktop_data_dir(identifier: &str) -> Result<PathBuf, String> {
    if let Some(path) = std::env::var_os("LOCAL_AGENT_DATA_DIR") {
        let path = PathBuf::from(path);
        files::checked_path(&path).map_err(|e| e.message)?;
        return Ok(path);
    }
    dirs::data_dir()
        .map(|path| path.join(identifier))
        .ok_or_else(|| "无法确定应用数据目录".into())
}

pub fn lock(data: &Path, name: &str) -> Result<File, ConfigError> {
    let directory = directory(data);
    files::private_dir(&directory)?;
    files::exclusive_lock(&directory.join(name))
}

pub fn read_service(data: &Path) -> Result<Option<ServiceRecord>, String> {
    let file = files::read_protected(&directory(data).join("service.json"))
        .map_err(|error| error.message)?;
    let Some(bytes) = file.bytes else {
        return Ok(None);
    };
    if file.stamp.mode != 0o600 {
        return Err("本地服务连接文件权限异常，请检查应用数据目录".into());
    }
    let record: ServiceRecord = serde_json::from_slice(&bytes)
        .map_err(|_| "本地服务连接文件无法读取，请检查应用数据目录")?;
    if record.port == 0
        || record.identity.pid == 0
        || uuid::Uuid::parse_str(&record.identity.instance_id).is_err()
        || record.token.len() < 32
        || record.token.len() > 256
        || !record.token.bytes().all(|byte| byte.is_ascii_graphic())
    {
        return Err("本地服务连接文件内容无效".into());
    }
    Ok(Some(record))
}

pub struct ServiceGuard {
    _lock: File,
    data: PathBuf,
    instance_id: String,
}

impl ServiceGuard {
    pub fn acquire(data: &Path) -> Result<Self, ConfigError> {
        Ok(Self {
            _lock: lock(data, "service.lock")?,
            data: data.to_path_buf(),
            instance_id: uuid::Uuid::new_v4().to_string(),
        })
    }

    pub fn publish(&self, port: u16, token: String, version: &str) -> Result<Identity, String> {
        self.publish_mode(port, token, version, false)
    }

    pub fn publish_mode(
        &self,
        port: u16,
        token: String,
        version: &str,
        persistent: bool,
    ) -> Result<Identity, String> {
        let identity = Identity {
            protocol: PROTOCOL,
            version: version.into(),
            instance_id: self.instance_id.clone(),
            pid: std::process::id(),
            persistent,
        };
        let record = ServiceRecord {
            identity: identity.clone(),
            port,
            token,
        };
        let bytes = serde_json::to_vec(&record).map_err(|_| "无法保存本地服务连接信息")?;
        files::atomic_write(&directory(&self.data).join("service.json"), &bytes, 0o600)
            .map_err(|error| error.message)?;
        Ok(identity)
    }
}

impl Drop for ServiceGuard {
    fn drop(&mut self) {
        // Never remove another owner's publication (including a replaced record).
        if read_service(&self.data)
            .ok()
            .flatten()
            .is_some_and(|record| record.identity.instance_id == self.instance_id)
        {
            let _ = std::fs::remove_file(directory(&self.data).join("service.json"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};

    fn temp() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    #[test]
    fn service_and_desktop_have_independent_exclusive_lifetimes() {
        let temp = temp();
        let data = temp.path().canonicalize().unwrap();
        let desktop = lock(&data, "desktop.lock").unwrap();
        assert_eq!(lock(&data, "desktop.lock").unwrap_err().code, "busy");
        let guard = ServiceGuard::acquire(&data).unwrap();
        assert_eq!(ServiceGuard::acquire(&data).err().unwrap().code, "busy");
        let identity = guard
            .publish(12345, uuid::Uuid::new_v4().to_string(), "test")
            .unwrap();
        assert_eq!(read_service(&data).unwrap().unwrap().identity, identity);
        drop(desktop);
        assert!(lock(&data, "desktop.lock").is_ok());
        drop(guard);
        assert!(read_service(&data).unwrap().is_none());
        assert!(ServiceGuard::acquire(&data).is_ok());
    }

    #[test]
    fn discovery_rejects_exposed_secrets_symlinks_and_invalid_records() {
        let temp = temp();
        let data = temp.path().canonicalize().unwrap();
        let guard = ServiceGuard::acquire(&data).unwrap();
        guard
            .publish(12345, uuid::Uuid::new_v4().to_string(), "test")
            .unwrap();
        let path = directory(&data).join("service.json");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(read_service(&data).is_err());
        std::fs::remove_file(&path).unwrap();
        let target = data.join("outside.json");
        std::fs::write(&target, "{}").unwrap();
        symlink(&target, &path).unwrap();
        assert!(read_service(&data).is_err());
        std::fs::remove_file(&path).unwrap();
        files::atomic_write(&path, b"{}", 0o600).unwrap();
        assert!(read_service(&data).is_err());
    }

    #[test]
    fn stale_owner_does_not_remove_a_replaced_publication() {
        let temp = temp();
        let data = temp.path().canonicalize().unwrap();
        let guard = ServiceGuard::acquire(&data).unwrap();
        guard
            .publish(12345, uuid::Uuid::new_v4().to_string(), "test")
            .unwrap();
        let mut record = read_service(&data).unwrap().unwrap();
        record.identity.instance_id = uuid::Uuid::new_v4().to_string();
        files::atomic_write(
            &directory(&data).join("service.json"),
            &serde_json::to_vec(&record).unwrap(),
            0o600,
        )
        .unwrap();
        drop(guard);
        assert_eq!(
            read_service(&data).unwrap().unwrap().identity,
            record.identity
        );
    }
}
