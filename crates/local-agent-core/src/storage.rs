use crate::{Provider, Settings, Snapshot, providers};
use parking_lot::{MappedReentrantMutexGuard, ReentrantMutex, ReentrantMutexGuard};
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use std::{
    fs,
    path::Path,
    sync::{Arc, TryLockError, Weak},
    time::Duration,
};

mod control_plane;

#[cfg(test)]
pub(crate) use control_plane::remove_from_fixture as strip_m1_fixture;

pub(crate) fn publish_control_plane(db: &Connection, version: u32) -> Result<(), String> {
    control_plane::initialize(db).map_err(|error| error.to_string())?;
    crate::routing::retention::initialize(db).map_err(|error| error.message)?;
    crate::routing::retention::backfill(db).map_err(|error| error.message)?;
    db.pragma_update(None, "user_version", version)
        .map_err(|error| error.to_string())
}

pub struct Store {
    pub(crate) connection: Connection,
    path: String,
    file_identity: (u64, u64),
    directory: std::sync::Arc<crate::agents::AgentDirectory>,
    writer: Option<Weak<ReentrantMutex<Store>>>,
}

/// The service shares one SQLite writer across its control-plane domains.
/// A scoped operation may consult the same Store again on the same thread.
#[derive(Clone)]
pub struct StoreWriter(Arc<ReentrantMutex<Store>>);

pub type StoreConnection<'a> = MappedReentrantMutexGuard<'a, Connection>;

impl StoreWriter {
    pub fn new(mut store: Store) -> Self {
        Self(Arc::new_cyclic(|writer| {
            store.writer = Some(writer.clone());
            ReentrantMutex::new(store)
        }))
    }

    pub fn lock(&self) -> Result<ReentrantMutexGuard<'_, Store>, TryLockError<()>> {
        let guard = self
            .0
            .try_lock_for(Duration::from_secs(3))
            .ok_or(TryLockError::WouldBlock)?;
        if !guard.database_file_matches() {
            return Err(TryLockError::WouldBlock);
        }
        Ok(guard)
    }

    pub fn try_lock(&self) -> Result<ReentrantMutexGuard<'_, Store>, TryLockError<()>> {
        self.0.try_lock().ok_or(TryLockError::WouldBlock)
    }

    pub fn connection(&self) -> Result<StoreConnection<'_>, TryLockError<()>> {
        let guard = self
            .0
            .try_lock_for(Duration::from_millis(250))
            .ok_or(TryLockError::WouldBlock)?;
        if !guard.database_file_matches() {
            return Err(TryLockError::WouldBlock);
        }
        Ok(ReentrantMutexGuard::map(guard, |store| &store.connection))
    }
}

fn database_file_identity(path: &Path) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    let metadata = fs::symlink_metadata(path).ok()?;
    (metadata.is_file() && metadata.nlink() == 1 && metadata.uid() == unsafe { libc::geteuid() })
        .then(|| (metadata.dev(), metadata.ino()))
}

fn migration_backup(
    connection: &Connection,
    directory: &Path,
    backup: &Path,
    schema_version: u32,
) -> Result<(), String> {
    providers::files::private_dir(directory).map_err(|e| e.message)?;
    providers::files::checked_path(backup).map_err(|e| e.message)?;
    if !backup.exists() {
        connection
            .execute("VACUUM INTO ?1", [backup.to_string_lossy().as_ref()])
            .map_err(|_| "数据库升级前备份失败，原库未迁移")?;
    }
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let metadata = fs::symlink_metadata(backup).map_err(|_| "无法检查升级备份")?;
    if !metadata.is_file() || metadata.nlink() != 1 || metadata.uid() != unsafe { libc::geteuid() }
    {
        return Err("升级备份必须为当前用户拥有的单链接普通文件".into());
    }
    fs::set_permissions(backup, fs::Permissions::from_mode(0o600))
        .map_err(|_| "升级备份权限设置失败")?;
    let snapshot = Connection::open_with_flags(backup, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|_| "升级备份无法读取，原库未迁移")?;
    let valid = snapshot
        .query_row("PRAGMA quick_check", [], |r| r.get::<_, String>(0))
        .is_ok_and(|value| value == "ok")
        && snapshot
            .query_row("PRAGMA user_version", [], |r| r.get::<_, u32>(0))
            .is_ok_and(|value| value == schema_version);
    if !valid {
        return Err("升级备份校验失败，原库未迁移".into());
    }
    Ok(())
}

impl Store {
    pub const SCHEMA_VERSION: u32 = 18;

    pub(crate) fn database_file_matches(&self) -> bool {
        database_file_identity(Path::new(&self.path)) == Some(self.file_identity)
    }

    pub fn open_reader(&self) -> Result<Self, String> {
        if !self.database_file_matches() {
            return Err("本地数据库文件已变化".to_owned());
        }
        let connection = Connection::open_with_flags(
            &self.path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )
        .map_err(|error| error.to_string())?;
        connection
            .busy_timeout(std::time::Duration::from_millis(250))
            .map_err(|error| error.to_string())?;
        if !self.database_file_matches() {
            return Err("本地数据库文件已变化".to_owned());
        }
        Ok(Self {
            connection,
            path: self.path.clone(),
            file_identity: self.file_identity,
            directory: self.directory.clone(),
            writer: None,
        })
    }

    pub fn open(directory: &Path) -> Result<Self, String> {
        Self::open_with_directory(
            directory,
            std::sync::Arc::new(crate::agents::builtin_directory().clone()),
        )
    }
    pub fn open_with_directory(
        directory: &Path,
        agents: std::sync::Arc<crate::agents::AgentDirectory>,
    ) -> Result<Self, String> {
        fs::create_dir_all(directory).map_err(|_| "无法创建应用数据目录".to_string())?;
        let directory =
            &fs::canonicalize(directory).map_err(|_| "无法解析应用数据目录".to_string())?;
        let path = directory.join("local-agent.sqlite3");
        let fresh = !path.exists();
        if fresh {
            use std::os::unix::fs::OpenOptionsExt;
            fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&path)
                .map_err(|_| "无法创建本地数据库".to_string())?;
        }
        let file_identity =
            database_file_identity(&path).ok_or("本地数据库必须为当前用户拥有的单链接普通文件")?;
        let connection = Connection::open_with_flags(
            &path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )
        .map_err(|_| "无法打开本地数据库".to_string())?;
        if database_file_identity(&path) != Some(file_identity) {
            return Err("本地数据库文件已变化".to_owned());
        }
        let schema_version: u32 = connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .map_err(|e| e.to_string())?;
        if schema_version > crate::iam::PROTECTED_SCHEMA_VERSION {
            return Err("数据库来自更新版本，请升级应用后重试".into());
        }
        if (2..Self::SCHEMA_VERSION).contains(&schema_version) {
            let backup = directory.join(if schema_version == 2 {
                "local-agent.pre-dynamic-v2.sqlite3"
            } else if schema_version == 3 {
                "local-agent.pre-model-services-v3.sqlite3"
            } else if schema_version == 4 {
                "local-agent.pre-routing-policy-v4.sqlite3"
            } else if schema_version == 5 {
                "local-agent.pre-route-notices-v5.sqlite3"
            } else if schema_version == 6 {
                "local-agent.pre-settings-v6.sqlite3"
            } else if schema_version == 7 {
                "local-agent.pre-model-service-operations-v7.sqlite3"
            } else if schema_version == 8 {
                "local-agent.pre-provider-operations-v8.sqlite3"
            } else if schema_version == 9 {
                "local-agent.pre-credential-recovery-v9.sqlite3"
            } else if schema_version == 10 {
                "local-agent.pre-legacy-admission-v10.sqlite3"
            } else if schema_version == 11 {
                "local-agent.pre-legacy-write-v11.sqlite3"
            } else if schema_version == 12 {
                "local-agent.pre-native-import-v12.sqlite3"
            } else if schema_version == 13 {
                "local-agent.pre-local-token-v13.sqlite3"
            } else if schema_version == 14 {
                "local-agent.pre-local-token-admission-v14.sqlite3"
            } else if schema_version == 15 {
                "local-agent.pre-plugin-lifecycle-v15.sqlite3"
            } else if schema_version == 16 {
                "local-agent.pre-single-control-v16.sqlite3"
            } else {
                "local-agent.pre-single-control-v17.sqlite3"
            });
            migration_backup(&connection, directory, &backup, schema_version)?;
        }
        connection
            .busy_timeout(std::time::Duration::from_secs(3))
            .map_err(|e| e.to_string())?;
        connection
            .execute_batch(
                "PRAGMA journal_mode=WAL;
             PRAGMA foreign_keys=ON;
             BEGIN IMMEDIATE;
             CREATE TABLE IF NOT EXISTS app_settings (
                id INTEGER PRIMARY KEY CHECK(id=1), value TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS providers (
                id TEXT PRIMARY KEY, value TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS native_sources (
                agent_id TEXT PRIMARY KEY, enabled INTEGER NOT NULL CHECK(enabled IN (0,1)));
             CREATE TABLE IF NOT EXISTS hidden_projects (
                id TEXT PRIMARY KEY, value TEXT NOT NULL);
             COMMIT;",
            )
            .map_err(|e| e.to_string())?;
        crate::routing::initialize(&connection).map_err(|e| e.message)?;
        crate::model_services::initialize(&connection).map_err(|e| e.message)?;
        crate::routing::chat::initialize(&connection).map_err(|e| e.message)?;
        crate::routing::budget::initialize(&connection).map_err(|e| e.message)?;
        crate::routing::loop_guard::initialize(&connection).map_err(|e| e.message)?;
        crate::model_services::policy::initialize(&connection).map_err(|e| e.message)?;
        crate::providers::upstream_accounts::initialize(&connection).map_err(|e| e.message)?;
        crate::model_services::notices::initialize(&connection).map_err(|e| e.message)?;
        crate::settings::initialize(&connection)?;
        crate::model_services::operations::initialize(&connection).map_err(|e| e.message)?;
        crate::providers::operations::initialize(&connection).map_err(|e| e.message)?;
        crate::providers::credential_scope::journal::initialize(
            &connection,
            &path,
            schema_version,
            fresh,
        )
        .map_err(|e| e.message)?;
        control_plane::validate(&connection).map_err(|_| "控制面数据库结构无效".to_owned())?;
        Ok(Self {
            connection,
            path: path.to_string_lossy().into_owned(),
            file_identity,
            directory: agents,
            writer: None,
        })
    }

    pub(crate) fn backup_iam_activation(&self) -> Result<(), crate::plugin::PluginError> {
        let directory = Path::new(&self.path).parent().ok_or_else(|| {
            crate::plugin::PluginError::new("iam_unavailable", "账号管理数据目录不可用")
        })?;
        migration_backup(
            &self.connection,
            directory,
            &directory.join("local-agent.pre-iam-v18.sqlite3"),
            Self::SCHEMA_VERSION,
        )
        .map_err(|message| crate::plugin::PluginError::new("iam_unavailable", message))
    }

    pub fn settings(&self) -> Result<Settings, String> {
        let value: Option<String> = self
            .connection
            .query_row("SELECT value FROM app_settings WHERE id=1", [], |row| {
                row.get(0)
            })
            .optional()
            .map_err(|e| e.to_string())?;
        match value {
            Some(value) => serde_json::from_str(&value).map_err(|_| "设置数据无法读取".to_string()),
            None => Ok(Settings::default()),
        }
    }

    pub fn credential_scope(
        &self,
        expected_store_id: &str,
    ) -> Result<providers::credential_scope::CredentialScope, providers::ConfigError> {
        if !matches!(self.connection.is_readonly("main"), Ok(false)) {
            return providers::fail("credential_protocol_unavailable", "凭据恢复需要主库写连接");
        }
        let store_id = self.database_id()?;
        if !providers::operations::valid_id(expected_store_id) || store_id != expected_store_id {
            return providers::fail(
                "credential_store_changed",
                "凭据操作属于另一个数据库，请连接原后台核查",
            );
        }
        let directory = Path::new(&self.path)
            .parent()
            .and_then(|path| path.canonicalize().ok())
            .map(|path| path.join("provider-configs"))
            .ok_or_else(|| {
                providers::ConfigError::new(
                    "credential_protocol_unavailable",
                    "凭据来源目录暂不可核查",
                )
            })?;
        let journal = providers::credential_scope::journal::Journal::from_store(
            &self.connection,
            Path::new(&self.path),
            &store_id,
            self.writer
                .as_ref()
                .map(|writer| {
                    writer.upgrade().map(StoreWriter).ok_or_else(|| {
                        providers::ConfigError::new(
                            "credential_recovery_unavailable",
                            "凭据恢复账本暂不可用",
                        )
                    })
                })
                .transpose()?,
        )?;
        Ok(providers::credential_scope::CredentialScope {
            store_id,
            directory,
            journal,
        })
    }

    pub fn database_id(&self) -> Result<String, crate::providers::ConfigError> {
        use crate::providers::ConfigError;
        let id: String = self
            .connection
            .query_row(
                "SELECT store_id FROM model_service_operation_scope WHERE id=1",
                [],
                |row| row.get(0),
            )
            .map_err(|_| ConfigError::new("service_unavailable", "数据库身份暂不可读取"))?;
        let parsed = uuid::Uuid::parse_str(&id)
            .map_err(|_| ConfigError::new("invalid_record", "数据库身份不可读取，原记录已保留"))?;
        if parsed.get_version_num() != 4 || parsed.to_string() != id {
            return Err(ConfigError::new(
                "invalid_record",
                "数据库身份不可读取，原记录已保留",
            ));
        }
        Ok(id)
    }

    pub fn native_sources(&self) -> Result<Vec<String>, String> {
        let mut statement = self
            .connection
            .prepare("SELECT agent_id FROM native_sources WHERE enabled=1 ORDER BY agent_id")
            .map_err(|_| "无法读取会话来源授权".to_string())?;
        let rows = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|_| "无法读取会话来源授权".to_string())?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|_| "无法读取会话来源授权".to_string())
    }

    pub fn save_native_source(&self, agent_id: &str, enabled: bool) -> Result<(), String> {
        if self.directory.source(agent_id).is_none() {
            return Err("暂不支持此 Agent 的原生会话读取".into());
        }
        self.connection
            .execute(
                "INSERT INTO native_sources(agent_id,enabled) VALUES(?1,?2)
             ON CONFLICT(agent_id) DO UPDATE SET enabled=excluded.enabled",
                params![agent_id, enabled],
            )
            .map_err(|_| "无法保存会话来源授权".to_string())?;
        Ok(())
    }

    pub fn providers(&self) -> Result<Vec<Provider>, String> {
        let mut providers = default_providers();
        let mut statement = self
            .connection
            .prepare("SELECT value FROM providers ORDER BY id")
            .map_err(|e| e.to_string())?;
        let rows = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|e| e.to_string())?;
        for row in rows {
            let provider: Provider = serde_json::from_str(&row.map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?;
            if let Some(existing) = providers.iter_mut().find(|p| p.id == provider.id) {
                *existing = provider;
            } else {
                providers.push(provider);
            }
        }
        Ok(providers)
    }
    /// Object-scoped reads do not parse every other connection's model catalog.
    pub fn provider_by_id(&self, id: &str) -> Result<Option<Provider>, String> {
        use rusqlite::OptionalExtension;
        let raw: Option<String> = self
            .connection
            .query_row(
                "SELECT CAST(substr(CAST(value AS BLOB),1,?2) AS TEXT) FROM providers WHERE id=?1",
                params![
                    id,
                    (crate::providers::operations::MAX_REQUEST_BYTES + 1) as i64
                ],
                |r| r.get(0),
            )
            .optional()
            .map_err(|_| "连接元数据暂不可读取")?;
        if let Some(raw) = raw {
            if raw.len() > crate::providers::operations::MAX_REQUEST_BYTES {
                return Err("连接元数据超出读取预算".into());
            }
            let provider: Provider =
                serde_json::from_str(&raw).map_err(|_| "连接元数据无法读取")?;
            if provider.id != id {
                return Err("连接元数据身份不一致".into());
            }
            Ok(Some(provider))
        } else {
            Ok(default_providers().into_iter().find(|p| p.id == id))
        }
    }

    pub fn hidden_projects(
        &self,
    ) -> Result<Vec<crate::projects::visibility::HiddenProject>, String> {
        let mut statement = self
            .connection
            .prepare("SELECT value FROM hidden_projects ORDER BY id")
            .map_err(|_| "无法读取已隐藏项目")?;
        let rows = statement
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(|_| "无法读取已隐藏项目")?;
        rows.map(|row| {
            serde_json::from_str(&row.map_err(|_| "无法读取已隐藏项目")?)
                .map_err(|_| "已隐藏项目记录无法读取".to_string())
        })
        .collect()
    }

    pub fn hide_project(
        &self,
        project: &crate::projects::visibility::HiddenProject,
    ) -> Result<(), String> {
        if !crate::projects::visibility::valid_id(&project.id)
            || !Path::new(&project.path).is_absolute()
        {
            return Err("项目标识或目录无效".into());
        }
        let value = serde_json::to_string(project).map_err(|_| "无法保存项目显示设置")?;
        self.connection
            .execute(
                "INSERT INTO hidden_projects(id,value) VALUES(?1,?2) ON CONFLICT(id) DO NOTHING",
                params![project.id, value],
            )
            .map_err(|_| "无法保存项目显示设置")?;
        Ok(())
    }

    pub fn restore_project(&self, id: &str) -> Result<(), String> {
        if !crate::projects::visibility::valid_id(id) {
            return Err("项目标识无效".into());
        }
        self.connection
            .execute("DELETE FROM hidden_projects WHERE id=?1", [id])
            .map_err(|_| "无法恢复项目显示")?;
        Ok(())
    }

    pub fn snapshot(&self) -> Result<Snapshot, String> {
        let settings = self.settings_state().map_err(|e| e.message.to_owned())?;
        Ok(Snapshot {
            hidden_projects: self.hidden_projects()?,
            version: env!("CARGO_PKG_VERSION"),
            database_path: self.path.clone(),
            gateway_status: "not_configured",
            agents: crate::discovery::discover_with_directory(&self.directory),
            providers: self.providers()?,
            provider_write_protocol: self.provider_write_protocol().map_err(|e| e.message)?,
            settings: settings.value,
            settings_revision: settings.revision,
            settings_write_protocol: self
                .settings_write_protocol()
                .map_err(|e| e.message.to_owned())?,
        })
    }
}

pub(crate) fn default_providers() -> Vec<Provider> {
    providers::catalog::builtin()
        .presets()
        .iter()
        .filter_map(|preset| {
            preset.endpoints.get(&preset.protocol).map(|endpoint| {
                (
                    preset.id.as_str(),
                    preset.name.as_str(),
                    endpoint.as_str(),
                    preset.protocol.as_str(),
                )
            })
        })
        .map(|(id, name, endpoint, protocol)| Provider {
            id: id.into(),
            name: name.into(),
            vendor: String::new(),
            endpoint: endpoint.into(),
            protocol: protocol.into(),
            model: String::new(),
            configured: false,
            credential_ref: None,
            credential_label: None,
            models: vec![],
            disabled_models: vec![],
            model_catalog: None,
            revision: 0,
            category: if ["ollama", "vllm"].contains(&id) {
                "local"
            } else {
                "cloud"
            }
            .into(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ModelCatalogSource;

    #[test]
    fn provider_object_lookup_matches_the_legacy_catalog_without_parsing_unrelated_rows() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(directory.path()).unwrap();
        let mut saved = default_providers().remove(0);
        saved.model = "chosen-model".into();
        store.save_provider(saved).unwrap();
        for provider in store.providers().unwrap() {
            assert_eq!(
                serde_json::to_value(store.provider_by_id(&provider.id).unwrap().unwrap()).unwrap(),
                serde_json::to_value(provider).unwrap()
            );
        }
        assert!(store.provider_by_id("absent").unwrap().is_none());
        store
            .connection
            .execute(
                "INSERT INTO providers(id,value) VALUES('broken','not JSON')",
                [],
            )
            .unwrap();
        assert!(store.providers().is_err());
        assert_eq!(
            store.provider_by_id("openai").unwrap().unwrap().model,
            "chosen-model"
        );
        assert!(store.provider_by_id("broken").is_err());
    }

    #[test]
    fn provider_object_lookup_rejects_mismatched_identity_and_oversized_payload() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(directory.path()).unwrap();
        let value = serde_json::to_string(&default_providers().remove(0)).unwrap();
        store
            .connection
            .execute(
                "INSERT INTO providers(id,value) VALUES('mismatch',?1)",
                [&value],
            )
            .unwrap();
        assert!(store.provider_by_id("mismatch").is_err());
        store
            .connection
            .execute(
                "INSERT INTO providers(id,value) VALUES('large',?1)",
                ["x".repeat(crate::providers::operations::MAX_REQUEST_BYTES + 1)],
            )
            .unwrap();
        assert_eq!(
            store.provider_by_id("large").unwrap_err(),
            "连接元数据超出读取预算"
        );
        assert!(store.provider_by_id("anthropic").unwrap().is_some());
    }

    #[test]
    fn settings_survive_reopening_and_invalid_values_do_not_replace_them() {
        let directory = tempfile::tempdir().unwrap();
        let settings = Settings {
            capture_content: true,
            retention_days: 7,
        };
        {
            let store = Store::open(directory.path()).unwrap();
            assert_eq!(store.settings().unwrap(), Settings::default());
            store
                .save_settings(&crate::settings::SettingsUpdate {
                    store_id: Some(store.database_id().unwrap()),
                    value: settings.clone(),
                    expected_revision: 0,
                    operation_id: uuid::Uuid::new_v4().to_string(),
                })
                .unwrap();
            assert!(
                store
                    .save_settings(&crate::settings::SettingsUpdate {
                        store_id: Some(store.database_id().unwrap()),
                        value: Settings {
                            retention_days: 5,
                            ..settings.clone()
                        },
                        expected_revision: 1,
                        operation_id: uuid::Uuid::new_v4().to_string(),
                    })
                    .is_err()
            );
        }
        assert_eq!(
            Store::open(directory.path()).unwrap().settings().unwrap(),
            settings
        );
    }

    #[test]
    fn provider_save_preserves_catalog_and_rejects_embedded_credentials() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(directory.path()).unwrap();
        let mut provider = default_providers().remove(0);
        provider.model = "chosen-model".into();
        store.save_provider(provider.clone()).unwrap();
        let providers = store.providers().unwrap();
        assert_eq!(providers.len(), default_providers().len());
        assert_eq!(providers[0].model, "chosen-model");
        assert!(providers[0].configured);
        provider.endpoint = "https://user:secret@example.com".into();
        assert!(store.save_provider(provider.clone()).is_err());
        provider.endpoint = "file:///etc/passwd".into();
        assert!(store.save_provider(provider).is_err());
    }

    #[test]
    fn unified_vendors_preserve_legacy_connections_and_custom_endpoints() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(directory.path()).unwrap();
        let defaults = store.providers().unwrap();
        assert_eq!(
            defaults.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
            [
                "OpenAI",
                "Anthropic",
                "OpenRouter",
                "DeepSeek",
                "Kimi",
                "MiniMax M Plan",
                "Ollama",
                "vLLM"
            ]
        );
        assert!(defaults.iter().all(|p| !p.configured));
        // Existing persisted connections remain readable without the new optional identity field.
        let mut legacy = serde_json::to_value(&defaults[0]).unwrap();
        legacy["endpoint"] = "https://existing-gateway.invalid/v1".into();
        store
            .connection
            .execute(
                "INSERT INTO providers(id,value) VALUES('openai',?1)",
                [legacy.to_string()],
            )
            .unwrap();
        let mut old = store.providers().unwrap().remove(0);
        assert_eq!(old.endpoint, "https://existing-gateway.invalid/v1");
        old.model = "original-model".into();
        store.save_provider(old).unwrap();
        let old = store.providers().unwrap().remove(0);
        assert_eq!(old.vendor, "openai");
        assert_eq!(old.endpoint, "https://existing-gateway.invalid/v1");

        let mut p = defaults.iter().find(|p| p.id == "ollama").unwrap().clone();
        p.id = format!("custom-{}", uuid::Uuid::new_v4());
        p.vendor = "ollama".into();
        p.name = "工作站推理".into();
        p.model = "installed-model".into();
        p.endpoint = "http://[::1]:11435/v1".into();
        store.save_provider(p.clone()).unwrap();
        let saved = store
            .providers()
            .unwrap()
            .into_iter()
            .find(|v| v.id == p.id)
            .unwrap();
        assert_eq!(saved.vendor, "ollama");
        assert_eq!(saved.endpoint, p.endpoint);
        assert_eq!(saved.models, ["installed-model"]);
        assert!(providers::validate("codex", &saved).is_ok());
        p.revision = saved.revision;
        p.endpoint = "http://remote.example/v1".into();
        assert!(store.save_provider(p.clone()).is_err());
        p.endpoint = "https://remote.example/v1".into();
        store.save_provider(p.clone()).unwrap();
        let saved = store
            .providers()
            .unwrap()
            .into_iter()
            .find(|v| v.id == p.id)
            .unwrap();
        assert_eq!(saved.vendor, "ollama");
        assert!(
            providers::validate("codex", &saved).is_err(),
            "remote local-runtime vendor still requires a credential"
        );
        let mut spoofed = defaults[0].clone();
        spoofed.vendor = "ollama".into();
        assert!(store.save_provider(spoofed).is_err());
    }

    #[test]
    fn model_availability_survives_reopen_and_catalog_refresh() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(directory.path()).unwrap();
        let mut p = default_providers().remove(0);
        p.model = "default-model".into();
        p.models = vec!["enabled-model".into(), "disabled-model".into()];
        // Legacy metadata enables every registered model, including a default outside models.
        let legacy = serde_json::to_value(&p).unwrap();
        assert!(legacy.get("disabledModels").is_none());
        let legacy: Provider = serde_json::from_value(legacy).unwrap();
        assert!(legacy.model_enabled("default-model"));
        assert!(legacy.model_enabled("disabled-model"));
        assert!(!legacy.model_enabled("unknown"));
        p.disabled_models = vec!["disabled-model".into(), "disabled-model".into()];
        store.save_provider(p).unwrap();
        drop(store);
        let store = Store::open(directory.path()).unwrap();
        let mut saved = store.providers().unwrap().remove(0);
        assert_eq!(saved.disabled_models, ["disabled-model"]);
        assert!(!saved.model_enabled("disabled-model"));
        saved.models = vec!["new-fetched-model".into()];
        store.save_provider(saved).unwrap();
        let mut saved = store.providers().unwrap().remove(0);
        assert!(saved.models.contains(&"disabled-model".into()));
        assert!(!saved.model_enabled("disabled-model"));
        assert!(saved.model_enabled("new-fetched-model"));
        saved.disabled_models.push("not-in-catalog".into());
        assert!(store.save_provider(saved).is_err());
        let mut saved = store.providers().unwrap().remove(0);
        saved.disabled_models = saved.models.clone();
        store.save_provider(saved).unwrap();
        let mut saved = store.providers().unwrap().remove(0);
        assert!(saved.models.iter().all(|m| !saved.model_enabled(m)));
        saved.disabled_models.clear();
        store.save_provider(saved).unwrap();
        assert!(
            store
                .providers()
                .unwrap()
                .remove(0)
                .model_enabled("disabled-model")
        );
    }

    #[test]
    fn fetched_catalog_survives_reopen_and_source_change_clears_provenance() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(directory.path()).unwrap();
        let mut p = default_providers().remove(0);
        p.models = (0..300).map(|i| format!("model-{i}")).collect();
        p.model = "manual-default".into();
        p.model_catalog = Some(ModelCatalogSource {
            fetched_at: chrono::Utc::now().to_rfc3339(),
            source_url: format!("{}/models", p.endpoint),
            endpoint: p.endpoint.clone(),
            protocol: p.protocol.clone(),
            credential_ref: None,
        });
        store.save_provider(p).unwrap();
        drop(store);
        let store = Store::open(directory.path()).unwrap();
        let mut saved = store.providers().unwrap().remove(0);
        assert_eq!(saved.models.len(), 301);
        assert!(saved.model_catalog.is_some());
        saved.model_catalog.as_mut().unwrap().source_url = "https://other.invalid/models".into();
        assert!(store.save_provider(saved.clone()).is_err());
        saved.model_catalog.as_mut().unwrap().source_url = format!("{}/models", saved.endpoint);
        saved.endpoint = "https://other.invalid/v1".into();
        store.save_provider(saved).unwrap();
        let updated = store.providers().unwrap().remove(0);
        assert!(updated.model_catalog.is_none());
        assert!(updated.models.contains(&"manual-default".into()));
    }

    #[test]
    fn future_schema_is_rejected_without_downgrading_it() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("local-agent.sqlite3");
        let connection = Connection::open(&path).unwrap();
        connection
            .pragma_update(
                None,
                "user_version",
                crate::iam::PROTECTED_SCHEMA_VERSION + 1,
            )
            .unwrap();
        drop(connection);
        assert!(Store::open(directory.path()).is_err());
        let connection = Connection::open(path).unwrap();
        assert_eq!(
            connection
                .query_row("PRAGMA user_version", [], |row| row.get::<_, u32>(0))
                .unwrap(),
            crate::iam::PROTECTED_SCHEMA_VERSION + 1
        );
    }

    #[test]
    fn visibility_migrates_v1_and_survives_restart_without_altering_settings_or_sources() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let settings = Settings {
            capture_content: true,
            retention_days: 90,
        };
        store
            .save_settings(&crate::settings::SettingsUpdate {
                store_id: Some(store.database_id().unwrap()),
                value: settings.clone(),
                expected_revision: 0,
                operation_id: uuid::Uuid::new_v4().to_string(),
            })
            .unwrap();
        store.save_native_source("codex", true).unwrap();
        strip_m1_fixture(&store.connection);
        let providers = serde_json::to_value(store.providers().unwrap()).unwrap();
        store
            .connection
            .execute_batch("DROP TABLE local_token_cancellations;DROP TABLE local_token_owners;DROP TABLE local_token_values;DROP TABLE local_token_scope;DROP TABLE native_import_operations;DROP TABLE native_import_scope;DROP TABLE credential_legacy_operations;DROP TABLE credential_legacy_scope;DROP TABLE credential_recovery_values;DROP TABLE credential_recovery_scope;DROP TABLE hidden_projects; PRAGMA user_version=1;")
            .unwrap();
        drop(store);
        let store = Store::open(dir.path()).unwrap();
        assert!(store.hidden_projects().unwrap().is_empty());
        assert_eq!(
            store
                .connection
                .query_row("PRAGMA foreign_keys", [], |r| r.get::<_, u8>(0))
                .unwrap(),
            1
        );
        assert_eq!(store.settings().unwrap(), settings);
        assert_eq!(store.native_sources().unwrap(), vec!["codex"]);
        assert_eq!(
            serde_json::to_value(store.providers().unwrap()).unwrap(),
            providers
        );
        let project: crate::projects::visibility::HiddenProject =
            crate::projects::identity("/Users/fixture/project")
                .unwrap()
                .into();
        store.hide_project(&project).unwrap();
        let mut repeated = project.clone();
        repeated.hidden_at = "different".into();
        store.hide_project(&repeated).unwrap();
        drop(store);
        let reopened = Store::open(dir.path()).unwrap();
        assert_eq!(
            reopened.snapshot().unwrap().hidden_projects,
            vec![project.clone()]
        );
        assert!(reopened.restore_project("project:invalid").is_err());
        reopened.restore_project(&project.id).unwrap();
        reopened.restore_project(&project.id).unwrap();
        assert!(reopened.hidden_projects().unwrap().is_empty());
        assert_eq!(reopened.settings().unwrap(), settings);
        assert_eq!(reopened.native_sources().unwrap(), vec!["codex"]);
    }

    #[test]
    fn provider_name_limit_counts_chinese_characters() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(directory.path()).unwrap();
        let mut provider = default_providers().remove(0);
        provider.name = "测".repeat(100);
        store.save_provider(provider.clone()).unwrap();
        provider.name.push('试');
        assert!(store.save_provider(provider).is_err());
    }
}
