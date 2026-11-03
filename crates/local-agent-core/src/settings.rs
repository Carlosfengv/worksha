use crate::{Settings, Store};
use rusqlite::{OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};

const MAX_REVISION: u64 = 9_007_199_254_740_991;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SettingsUpdate {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub store_id: Option<String>,
    pub value: Settings,
    pub expected_revision: u64,
    pub operation_id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SettingsState {
    pub value: Settings,
    pub revision: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SettingsSaved {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub store_id: Option<String>,
    pub value: Settings,
    pub revision: u64,
    pub operation_id: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsWriteProtocol {
    pub version: u32,
    pub store_id: String,
    pub receipt_limit: u32,
}

#[derive(Debug, Serialize)]
pub struct SettingsError {
    pub code: &'static str,
    pub message: &'static str,
}

fn unavailable(_: impl std::fmt::Display) -> SettingsError {
    SettingsError {
        code: "unavailable",
        message: "设置存储暂不可用，请核查保存结果后重试",
    }
}

fn invalid_record(_: impl std::fmt::Debug) -> SettingsError {
    SettingsError {
        code: "invalid_record",
        message: "设置或操作记录不可读取，原记录已保留，请核查结果",
    }
}

fn valid_request(update: &SettingsUpdate) -> bool {
    uuid::Uuid::parse_str(&update.operation_id).is_ok()
        && update.expected_revision < MAX_REVISION
        && [7, 30, 90].contains(&update.value.retention_days)
        && update.store_id.as_ref().is_none_or(|id| {
            uuid::Uuid::parse_str(id)
                .is_ok_and(|uuid| uuid.get_version_num() == 4 && uuid.to_string() == *id)
        })
}

pub(crate) fn initialize(connection: &rusqlite::Connection) -> Result<(), String> {
    connection
        .execute_batch(
            "BEGIN IMMEDIATE;
             CREATE TABLE IF NOT EXISTS settings_version (
                id INTEGER PRIMARY KEY CHECK(id=1),
                revision INTEGER NOT NULL CHECK(revision>=0));
             INSERT OR IGNORE INTO settings_version VALUES(1,0);
             CREATE TABLE IF NOT EXISTS settings_operations (
                sequence INTEGER PRIMARY KEY,
                operation_id TEXT NOT NULL UNIQUE,
                request TEXT NOT NULL,
                result TEXT NOT NULL);
             COMMIT;",
        )
        .map_err(|_| "设置版本迁移未完成".to_owned())
}

impl Store {
    pub fn settings_write_protocol(&self) -> Result<SettingsWriteProtocol, SettingsError> {
        Ok(SettingsWriteProtocol {
            version: 1,
            store_id: self.database_id().map_err(invalid_record)?,
            receipt_limit: 256,
        })
    }

    fn require_settings_store(&self, expected: &str) -> Result<(), SettingsError> {
        if self.database_id().map_err(invalid_record)? != expected {
            return Err(SettingsError {
                code: "operation_store_changed",
                message: "设置操作属于另一个本机数据库，请连接原后台后核查；未提交到当前数据库",
            });
        }
        Ok(())
    }

    pub fn settings_state(&self) -> Result<SettingsState, SettingsError> {
        self.connection
            .query_row(
                "SELECT s.value,v.revision FROM settings_version v
                 LEFT JOIN app_settings s ON s.id=v.id WHERE v.id=1",
                [],
                |row| Ok((row.get::<_, Option<String>>(0)?, row.get::<_, i64>(1)?)),
            )
            .map_err(unavailable)
            .and_then(|(value, revision)| {
                Ok(SettingsState {
                    value: value
                        .map(|v| serde_json::from_str(&v))
                        .transpose()
                        .map_err(invalid_record)?
                        .unwrap_or_default(),
                    revision: revision.try_into().map_err(invalid_record)?,
                })
            })
            .and_then(|state| {
                if state.revision > MAX_REVISION
                    || ![7, 30, 90].contains(&state.value.retention_days)
                {
                    return Err(invalid_record("invalid settings state"));
                }
                Ok(state)
            })
    }

    fn load_settings_operation(
        &self,
        id: &str,
    ) -> Result<Option<(String, SettingsSaved)>, SettingsError> {
        let record: Option<(String, String)> = self
            .connection
            .query_row(
                "SELECT request,result FROM settings_operations WHERE operation_id=?1",
                [id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(unavailable)?;
        record
            .map(|(request, result)| {
                let update: SettingsUpdate =
                    serde_json::from_str(&request).map_err(invalid_record)?;
                let saved: SettingsSaved = serde_json::from_str(&result).map_err(invalid_record)?;
                if !valid_request(&update)
                    || update.operation_id != id
                    || saved.operation_id != id
                    || saved.store_id != update.store_id
                    || saved.value != update.value
                    || saved.revision != update.expected_revision + 1
                {
                    return Err(invalid_record("mismatched settings receipt"));
                }
                if let Some(scope) = &saved.store_id {
                    self.require_settings_store(scope)?;
                }
                Ok((request, saved))
            })
            .transpose()
    }

    pub fn settings_operation(&self, id: &str) -> Result<SettingsSaved, SettingsError> {
        uuid::Uuid::parse_str(id).map_err(|_| SettingsError {
            code: "invalid_input",
            message: "设置操作编号无效",
        })?;
        self.load_settings_operation(id)?
            .map(|(_, saved)| saved)
            .ok_or(SettingsError {
                code: "operation_not_found",
                message: "尚未查到此操作的已提交结果；不能据此确认从未执行，请保留编号核查",
            })
    }

    pub fn scoped_settings_operation(
        &self,
        id: &str,
        store_id: &str,
    ) -> Result<SettingsSaved, SettingsError> {
        let transaction =
            rusqlite::Transaction::new_unchecked(&self.connection, TransactionBehavior::Deferred)
                .map_err(unavailable)?;
        self.require_settings_store(store_id)?;
        let saved = self.settings_operation(id)?;
        if saved.store_id.as_deref() != Some(store_id) {
            return Err(SettingsError {
                code: "operation_not_scoped",
                message: "此历史设置回执没有数据库身份，保留原记录，请按旧版结果核对；未重新提交",
            });
        }
        transaction.commit().map_err(unavailable)?;
        Ok(saved)
    }

    pub fn save_settings(&self, update: &SettingsUpdate) -> Result<SettingsSaved, SettingsError> {
        let store_id = update.store_id.as_deref().ok_or(SettingsError {
            code: "write_protocol_unavailable",
            message: "设置保存必须提供原数据库身份，请升级客户端并重新核对；未提交保存",
        })?;
        if !valid_request(update) {
            return Err(SettingsError {
                code: "invalid_input",
                message: "请检查设置操作编号、版本及 7、30 或 90 天保留期",
            });
        }
        // A database transaction protects the revision across independent Store
        // connections as well as the service's in-process mutex.
        let transaction =
            rusqlite::Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)
                .map_err(unavailable)?;
        self.require_settings_store(store_id)?;
        let payload = serde_json::to_string(update).map_err(unavailable)?;
        if let Some((request, result)) = self.load_settings_operation(&update.operation_id)? {
            if request != payload {
                return Err(SettingsError {
                    code: "operation_conflict",
                    message: "操作编号已用于其他设置，请重新核对后保存",
                });
            }
            return Ok(result);
        }
        let current = self.settings_state()?;
        if current.revision != update.expected_revision {
            return Err(SettingsError {
                code: "conflict",
                message: "设置已被修改，草稿已保留；请重新读取并核对后保存",
            });
        }
        let revision = current
            .revision
            .checked_add(1)
            .filter(|r| *r <= MAX_REVISION)
            .ok_or(SettingsError {
                code: "unavailable",
                message: "设置版本已达上限",
            })?;
        let saved = SettingsSaved {
            store_id: update.store_id.clone(),
            value: update.value.clone(),
            revision,
            operation_id: update.operation_id.clone(),
        };
        transaction
            .execute(
                "INSERT INTO app_settings(id,value) VALUES(1,?1)
                 ON CONFLICT(id) DO UPDATE SET value=excluded.value",
                [serde_json::to_string(&saved.value).map_err(unavailable)?],
            )
            .map_err(unavailable)?;
        transaction
            .execute(
                "UPDATE settings_version SET revision=?1 WHERE id=1",
                [revision as i64],
            )
            .map_err(unavailable)?;
        transaction
            .execute(
                "INSERT INTO settings_operations(operation_id,request,result) VALUES(?1,?2,?3)",
                params![
                    update.operation_id,
                    payload,
                    serde_json::to_string(&saved).map_err(unavailable)?,
                ],
            )
            .map_err(unavailable)?;
        transaction
            .execute(
                "DELETE FROM settings_operations WHERE sequence <=
                 (SELECT max(sequence)-256 FROM settings_operations)",
                [],
            )
            .map_err(unavailable)?;
        transaction.commit().map_err(unavailable)?;
        Ok(saved)
    }
}

#[cfg(test)]
mod tests;
