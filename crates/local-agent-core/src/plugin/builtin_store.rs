//! Configuration, consent revocation and receipts commit in the existing main
//! database transaction. No source file, credential or plugin-private data is written.
use super::{PluginError, builtin::*};
use crate::Store;
use rusqlite::{OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};

#[cfg(test)]
mod database_tests;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SavedComposition {
    pub schema_version: u16,
    pub revision: crate::domain::Decimal,
    pub composition: Composition,
}
fn unavailable(_: impl std::fmt::Display) -> PluginError {
    PluginError::new("unavailable", "组合保存状态暂不可确认，请保留操作编号核查")
}
fn invalid_record(_: impl std::fmt::Debug) -> PluginError {
    PluginError::new("invalid_record", "内置组合或回执不可读取，原记录已保留")
}
fn valid_id(value: &str) -> bool {
    uuid::Uuid::parse_str(value)
        .is_ok_and(|id| id.get_version_num() == 4 && id.to_string() == value)
}
fn valid_composition(composition: &Composition) -> bool {
    [
        &composition.disabled_plugin_ids,
        &composition.removed_plugin_ids,
    ]
    .into_iter()
    .all(|ids| {
        ids.len() <= 256
            && ids.windows(2).all(|ids| ids[0] < ids[1])
            && ids.iter().all(|id| {
                !id.is_empty()
                    && id.len() <= 100
                    && id
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b".-_".contains(&b))
            })
    })
}
pub(crate) fn initialize(
    connection: &rusqlite::Connection,
    original_version: u32,
) -> Result<(), String> {
    let count: i64 = connection.query_row("SELECT count(*) FROM sqlite_master WHERE type='table' AND name IN ('builtin_plugin_composition','builtin_plugin_operations')", [], |row| row.get(0))
        .map_err(|_| "无法核查内置插件存储")?;
    if count == 2 {
        connection.prepare("SELECT id,value FROM builtin_plugin_composition LIMIT 0")
            .and_then(|_| connection.prepare("SELECT sequence,operation_id,request,receipt FROM builtin_plugin_operations LIMIT 0"))
            .map_err(|_| "内置插件存储不完整，原记录已保留")?;
        let saved: Option<String> = connection
            .query_row(
                "SELECT value FROM builtin_plugin_composition WHERE id=1",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(|_| "无法核查内置插件状态")?;
        if let Some(value) = saved {
            let state: SavedComposition =
                serde_json::from_str(&value).map_err(|_| "内置插件状态不可读取，原记录已保留")?;
            if state.schema_version != 1
                || state.revision.value() > 9_007_199_254_740_991
                || !valid_composition(&state.composition)
            {
                return Err("内置插件状态不可读取，原记录已保留".into());
            }
        } else {
            let changed: bool = connection
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM builtin_plugin_operations)",
                    [],
                    |row| row.get(0),
                )
                .map_err(|_| "无法核查内置插件回执")?;
            if original_version >= 16 || changed {
                return Err("内置插件状态缺失，原记录已保留；未恢复默认启用状态".into());
            }
            connection.execute("INSERT INTO builtin_plugin_composition VALUES(1,?1)", [r#"{"schemaVersion":1,"revision":"0","composition":{"profile":"desktop","disabledPluginIds":[]}}"#])
                .map_err(|_| "内置插件初始状态保存失败")?;
        }
        return Ok(());
    }
    if count != 0 || original_version >= 16 {
        return Err("内置插件存储不完整，原记录已保留；未恢复默认启用状态".into());
    }
    // Caller owns the migration transaction, including the reader-version barrier.
    connection.execute_batch("
        CREATE TABLE IF NOT EXISTS builtin_plugin_composition (id INTEGER PRIMARY KEY CHECK(id=1), value TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS builtin_plugin_operations (sequence INTEGER PRIMARY KEY, operation_id TEXT NOT NULL UNIQUE, request TEXT NOT NULL, receipt TEXT NOT NULL);
        ")
        .map_err(|_| "内置插件组合存储初始化失败".to_owned())?;
    initialize(connection, original_version)
}
impl Store {
    pub fn builtin_composition(
        &self,
        default_profile: Profile,
    ) -> Result<SavedComposition, PluginError> {
        let saved: Option<String> = self
            .connection
            .query_row(
                "SELECT value FROM builtin_plugin_composition WHERE id=1",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(unavailable)?;
        let mut state: SavedComposition = match saved {
            Some(value) => serde_json::from_str(&value).map_err(invalid_record)?,
            None => return Err(invalid_record("configuration missing")),
        };
        if state.schema_version != 1
            || state.revision.value() > 9_007_199_254_740_991
            || !valid_composition(&state.composition)
        {
            return Err(invalid_record("invalid composition"));
        }
        // An untouched store follows the desktop/background entry point.
        // A committed change always keeps its explicitly saved profile.
        if state.revision.value() == 0 {
            if state.composition != Composition::default() {
                return Err(invalid_record("invalid initial composition"));
            }
            state.composition.profile = default_profile;
        }
        Ok(state)
    }
    fn require_builtin_store(&self, store_id: &str) -> Result<(), PluginError> {
        if !valid_id(store_id) || self.database_id().map_err(invalid_record)? != store_id {
            return Err(PluginError::new(
                "operation_store_changed",
                "组合操作属于另一个数据库，请连接原后台核查",
            ));
        }
        Ok(())
    }
    fn builtin_operation_record(
        &self,
        query: &CompositionOperation,
    ) -> Result<Option<(CompositionChange, CompositionReceipt)>, PluginError> {
        self.require_builtin_store(&query.store_id)?;
        if !valid_id(&query.operation_id) {
            return Err(PluginError::new("invalid_input", "组合操作编号无效"));
        }
        let record: Option<(String, String)> = self
            .connection
            .query_row(
                "SELECT request,receipt FROM builtin_plugin_operations WHERE operation_id=?1",
                [&query.operation_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(unavailable)?;
        record
            .map(|(request, receipt)| {
                let request: CompositionChange =
                    serde_json::from_str(&request).map_err(invalid_record)?;
                let receipt: CompositionReceipt =
                    serde_json::from_str(&receipt).map_err(invalid_record)?;
                if request.store_id != query.store_id
                    || !valid_id(&request.expected_generation)
                    || receipt.revision.value() > 9_007_199_254_740_991
                    || request.operation_id != query.operation_id
                    || receipt.store_id != request.store_id
                    || receipt.operation_id != request.operation_id
                    || receipt.composition != request.composition
                    || !valid_composition(&receipt.composition)
                    || request.expected_revision.value().checked_add(1)
                        != Some(receipt.revision.value())
                {
                    return Err(invalid_record("mismatched receipt"));
                }
                Ok((request, receipt))
            })
            .transpose()
    }
    pub fn builtin_composition_operation(
        &self,
        query: &CompositionOperation,
    ) -> Result<Option<CompositionReceipt>, PluginError> {
        self.builtin_operation_record(query)
            .map(|record| record.map(|(_, receipt)| receipt))
    }
    pub fn check_builtin_composition_replay(
        &self,
        request: &CompositionChange,
    ) -> Result<Option<CompositionReceipt>, PluginError> {
        let record = self.builtin_operation_record(&CompositionOperation {
            store_id: request.store_id.clone(),
            operation_id: request.operation_id.clone(),
        })?;
        match record {
            Some((previous, receipt)) if previous == *request => Ok(Some(receipt)),
            Some(_) => Err(PluginError::new(
                "operation_conflict",
                "同一操作编号已用于不同的组合参数",
            )),
            None => Ok(None),
        }
    }
    pub fn save_builtin_composition(
        &self,
        request: &CompositionChange,
        default_profile: Profile,
        revoked_sources: &[String],
    ) -> Result<CompositionReceipt, PluginError> {
        self.require_builtin_store(&request.store_id)?;
        if !valid_id(&request.operation_id)
            || !valid_id(&request.expected_generation)
            || !valid_composition(&request.composition)
        {
            return Err(PluginError::new(
                "invalid_input",
                "组合参数、版本或操作编号无效",
            ));
        }
        let transaction =
            rusqlite::Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)
                .map_err(unavailable)?;
        if let Some(receipt) = self.check_builtin_composition_replay(request)? {
            return Ok(receipt);
        }
        let state = self.builtin_composition(default_profile)?;
        if state.revision != request.expected_revision {
            return Err(PluginError::new(
                "stale_snapshot",
                "组合版本已变化，请重新读取并核对",
            ));
        }
        let revision = state
            .revision
            .value()
            .checked_add(1)
            .filter(|v| *v <= 9_007_199_254_740_991)
            .ok_or_else(|| PluginError::new("resource_limit", "组合版本已达上限"))?;
        let next = SavedComposition {
            schema_version: 1,
            revision: revision.into(),
            composition: request.composition.clone(),
        };
        let receipt = CompositionReceipt {
            operation_id: request.operation_id.clone(),
            store_id: request.store_id.clone(),
            revision: revision.into(),
            composition: request.composition.clone(),
        };
        transaction.execute("INSERT INTO builtin_plugin_composition(id,value) VALUES(1,?1) ON CONFLICT(id) DO UPDATE SET value=excluded.value",[serde_json::to_string(&next).map_err(unavailable)?]).map_err(unavailable)?;
        for source in revoked_sources {
            self.save_native_source(source, false)
                .map_err(unavailable)?;
        }
        transaction.execute("INSERT INTO builtin_plugin_operations(operation_id,request,receipt) VALUES(?1,?2,?3)",params![request.operation_id,serde_json::to_string(request).map_err(unavailable)?,serde_json::to_string(&receipt).map_err(unavailable)?]).map_err(unavailable)?;
        transaction.execute("DELETE FROM builtin_plugin_operations WHERE sequence NOT IN (SELECT sequence FROM builtin_plugin_operations ORDER BY sequence DESC LIMIT 256)",[]).map_err(unavailable)?;
        transaction.commit().map_err(unavailable)?;
        Ok(receipt)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request(store: &Store) -> CompositionChange {
        CompositionChange {
            expected_impact: None,
            operation_id: uuid::Uuid::new_v4().to_string(),
            store_id: store.database_id().unwrap(),
            expected_revision: 0u64.into(),
            expected_generation: uuid::Uuid::new_v4().to_string(),
            composition: Composition {
                profile: Profile::Desktop,
                disabled_plugin_ids: vec!["io.localagent.builtin.source.codex".into()],
                removed_plugin_ids: vec![],
            },
        }
    }
    #[test]
    fn configuration_and_consent_revocation_commit_once_and_survive_restart() {
        let temp = tempfile::tempdir().unwrap();
        let store = Store::open(temp.path()).unwrap();
        store.save_native_source("codex", true).unwrap();
        let change = request(&store);
        let receipt = store
            .save_builtin_composition(&change, Profile::Desktop, &["codex".into()])
            .unwrap();
        assert_eq!(receipt.revision.value(), 1);
        assert!(store.native_sources().unwrap().is_empty());
        // A repeated already committed request must not revoke a new explicit grant.
        store.save_native_source("codex", true).unwrap();
        for _ in 0..100 {
            assert_eq!(
                store
                    .save_builtin_composition(&change, Profile::Desktop, &["codex".into()])
                    .unwrap(),
                receipt
            );
        }
        assert_eq!(store.native_sources().unwrap(), ["codex"]);
        store.save_native_source("codex", false).unwrap();
        drop(store);
        let store = Store::open(temp.path()).unwrap();
        assert_eq!(
            store
                .builtin_composition(Profile::Background)
                .unwrap()
                .composition,
            change.composition
        );
        let mut enabled = change.clone();
        enabled.operation_id = uuid::Uuid::new_v4().to_string();
        enabled.expected_revision = 1u64.into();
        enabled.composition.disabled_plugin_ids.clear();
        store
            .save_builtin_composition(&enabled, Profile::Desktop, &[])
            .unwrap();
        assert!(store.native_sources().unwrap().is_empty());
        assert_eq!(
            store
                .builtin_composition_operation(&CompositionOperation {
                    store_id: change.store_id.clone(),
                    operation_id: change.operation_id.clone()
                })
                .unwrap(),
            Some(receipt)
        );
    }
    #[test]
    fn a_failed_revocation_rolls_back_both_configuration_and_permission() {
        let temp = tempfile::tempdir().unwrap();
        let store = Store::open(temp.path()).unwrap();
        store.save_native_source("codex", true).unwrap();
        let change = request(&store);
        assert!(
            store
                .save_builtin_composition(
                    &change,
                    Profile::Desktop,
                    &["codex".into(), "unknown-source".into()]
                )
                .is_err()
        );
        assert_eq!(
            store
                .builtin_composition(Profile::Desktop)
                .unwrap()
                .revision
                .value(),
            0
        );
        assert_eq!(store.native_sources().unwrap(), ["codex"]);
        assert!(
            store
                .builtin_composition_operation(&CompositionOperation {
                    store_id: change.store_id,
                    operation_id: change.operation_id
                })
                .unwrap()
                .is_none()
        );
    }
    #[test]
    fn identities_revisions_and_reused_operation_parameters_are_checked_before_mutation() {
        let temp = tempfile::tempdir().unwrap();
        let first = Store::open(temp.path()).unwrap();
        let second = Store::open(temp.path()).unwrap();
        let change = request(&first);
        let receipt = first
            .save_builtin_composition(&change, Profile::Desktop, &[])
            .unwrap();
        let mut other = change.clone();
        other.composition.profile = Profile::Recovery;
        assert_eq!(
            second
                .save_builtin_composition(&other, Profile::Desktop, &[])
                .unwrap_err()
                .code,
            "operation_conflict"
        );
        other.operation_id = uuid::Uuid::new_v4().to_string();
        assert_eq!(
            second
                .save_builtin_composition(&other, Profile::Desktop, &[])
                .unwrap_err()
                .code,
            "stale_snapshot"
        );
        other.store_id = uuid::Uuid::new_v4().to_string();
        assert_eq!(
            second
                .save_builtin_composition(&other, Profile::Desktop, &[])
                .unwrap_err()
                .code,
            "operation_store_changed"
        );
        assert_eq!(
            second
                .builtin_composition(Profile::Desktop)
                .unwrap()
                .revision,
            receipt.revision
        );
    }
    #[test]
    fn corrupted_configuration_and_receipts_are_rejected_without_overwriting_them() {
        let temp = tempfile::tempdir().unwrap();
        let store = Store::open(temp.path()).unwrap();
        let change = request(&store);
        store
            .save_builtin_composition(&change, Profile::Desktop, &[])
            .unwrap();
        store
            .connection
            .execute("UPDATE builtin_plugin_operations SET receipt='{}'", [])
            .unwrap();
        assert_eq!(
            store
                .builtin_composition_operation(&CompositionOperation {
                    store_id: change.store_id,
                    operation_id: change.operation_id
                })
                .unwrap_err()
                .code,
            "invalid_record"
        );
        store
            .connection
            .execute(
                "UPDATE builtin_plugin_composition SET value='{\"schemaVersion\":2}'",
                [],
            )
            .unwrap();
        assert_eq!(
            store
                .builtin_composition(Profile::Desktop)
                .unwrap_err()
                .code,
            "invalid_record"
        );
        assert_eq!(
            store
                .connection
                .query_row("SELECT value FROM builtin_plugin_composition", [], |row| {
                    row.get::<_, String>(0)
                })
                .unwrap(),
            "{\"schemaVersion\":2}"
        );
    }
}
