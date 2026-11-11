//! Durable results share the transaction that publishes or deletes a service.
use super::{ModelService, ServiceDraft};
use crate::{
    Store,
    providers::{ConfigError, fail},
};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const MAX_REVISION: u64 = 9_007_199_254_740_991;
pub const RECEIPT_LIMIT: usize = 256;
pub const MAX_REQUEST_BYTES: usize = 65_536;
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VerifyMemberRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream_account: Option<crate::providers::upstream_accounts::AccountRef>,
    pub store_id: String,
    pub provider_id: String,
    pub expected_revision: u64,
    pub model: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum PreviewEntry {
    Default,
    Specified { model_id: String },
    Rule { rule_id: String },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PreviewRequest {
    pub store_id: String,
    pub draft: ServiceDraft,
    pub entry: PreviewEntry,
}
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OperationRequest {
    pub store_id: String,
    pub operation_id: String,
    pub action: OperationAction,
}
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum OperationAction {
    Save {
        draft: Box<ServiceDraft>,
    },
    Delete {
        service_id: String,
        expected_revision: u64,
    },
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum OperationKind {
    Save,
    Delete,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OperationReceipt {
    pub store_id: String,
    pub api_version: u32,
    pub operation_id: String,
    pub action: OperationKind,
    pub service_id: String,
    pub committed_revision: u64,
    pub service: Option<ModelService>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StoredOutcome {
    store_id: String,
    api_version: u32,
    operation_id: String,
    action: OperationKind,
    service_id: String,
    committed_revision: u64,
}
impl OperationAction {
    pub fn identity(&self) -> (OperationKind, &str, u64) {
        match self {
            Self::Save { draft } => (OperationKind::Save, &draft.id, draft.revision),
            Self::Delete {
                service_id,
                expected_revision,
            } => (OperationKind::Delete, service_id, *expected_revision),
        }
    }
}
fn unavailable(_: impl std::fmt::Display) -> ConfigError {
    ConfigError::new(
        "service_unavailable",
        "模型服务操作结果暂不可读取，请保留原编号核查",
    )
}
fn invalid_record(_: impl std::fmt::Display) -> ConfigError {
    ConfigError::new(
        "invalid_record",
        "模型服务操作记录不可读取，原记录已保留，请核查结果",
    )
}
fn validate(request: &OperationRequest) -> Result<(), ConfigError> {
    let id = uuid::Uuid::parse_str(&request.operation_id).ok();
    let (_, service, revision) = request.action.identity();
    if !id.is_some_and(|id| id.get_version_num() == 4 && id.to_string() == request.operation_id)
        || uuid::Uuid::parse_str(&request.store_id).is_err()
        || uuid::Uuid::parse_str(service).is_err()
        || revision >= MAX_REVISION
        || serde_json::to_vec(request).map_err(unavailable)?.len() > MAX_REQUEST_BYTES
    {
        return fail("invalid_input", "模型服务操作编号、服务身份或版本无效");
    }
    Ok(())
}
// D5 additive request validation; existing operation behavior stays below.
impl OperationRequest {
    pub fn validate(&self) -> Result<(), ConfigError> {
        validate(self)
    }
}
// End D5 additive request validation.
pub(crate) fn initialize(db: &Connection) -> Result<(), ConfigError> {
    let tx = rusqlite::Transaction::new_unchecked(db, TransactionBehavior::Immediate)
        .map_err(unavailable)?;
    let schema: u32 = tx
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(unavailable)?;
    tx.execute_batch("CREATE TABLE IF NOT EXISTS model_service_operations(sequence INTEGER PRIMARY KEY,operation_id TEXT NOT NULL UNIQUE,request TEXT NOT NULL CHECK(json_valid(request)),result TEXT NOT NULL CHECK(json_valid(result)),service_version TEXT REFERENCES model_service_versions(id)); CREATE TABLE IF NOT EXISTS model_service_operation_scope(id INTEGER PRIMARY KEY CHECK(id=1),store_id TEXT NOT NULL UNIQUE);").map_err(unavailable)?;
    if schema < 8 {
        tx.execute(
            "INSERT OR IGNORE INTO model_service_operation_scope VALUES(1,?1)",
            [uuid::Uuid::new_v4().to_string()],
        )
        .map_err(unavailable)?;
    }
    // Once migrated, missing identity is corruption, never a new database.
    let identity: String = tx
        .query_row(
            "SELECT store_id FROM model_service_operation_scope WHERE id=1",
            [],
            |row| row.get(0),
        )
        .map_err(invalid_record)?;
    let id = uuid::Uuid::parse_str(&identity).map_err(invalid_record)?;
    if id.get_version_num() != 4 || id.to_string() != identity {
        return Err(invalid_record("invalid store identity"));
    }
    tx.commit().map_err(unavailable)
}

fn load(
    db: &Connection,
    id: &str,
    store_id: &str,
) -> Result<Option<(OperationRequest, OperationReceipt)>, ConfigError> {
    let row: Option<(String, String, Option<String>)> = db
        .query_row(
            "SELECT request,result,service_version FROM model_service_operations WHERE operation_id=?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()
        .map_err(unavailable)?;
    row.map(|(request, result, version)| {
        let request: OperationRequest = serde_json::from_str(&request).map_err(invalid_record)?;
        validate(&request).map_err(|e| invalid_record(e.message))?;
        let outcome: StoredOutcome = serde_json::from_str(&result).map_err(invalid_record)?;
        let service = match (outcome.action, version) {
            (OperationKind::Save, Some(version)) => {
                Some(super::version(db, &version).map_err(|e| invalid_record(e.message))?)
            }
            (OperationKind::Delete, None) => None,
            _ => return Err(invalid_record("version")),
        };
        let receipt = OperationReceipt {
            store_id: outcome.store_id,
            api_version: outcome.api_version,
            operation_id: outcome.operation_id,
            action: outcome.action,
            service_id: outcome.service_id,
            committed_revision: outcome.committed_revision,
            service,
        };
        let (kind, service_id, revision) = request.action.identity();
        if request.store_id != store_id
            || receipt.store_id != store_id
            || request.operation_id != id
            || receipt.api_version != 1
            || receipt.operation_id != id
            || receipt.action != kind
            || receipt.service_id != service_id
            || receipt.committed_revision != revision + 1
            || match (&receipt.service, kind) {
                (Some(service), OperationKind::Save) => {
                    service.id != service_id || service.revision != receipt.committed_revision
                }
                (None, OperationKind::Delete) => false,
                _ => true,
            }
        {
            return Err(invalid_record("identity"));
        }
        Ok((request, receipt))
    })
    .transpose()
}
impl Store {
    pub fn model_service_operation_store_id(&self) -> Result<String, ConfigError> {
        self.database_id()
    }
    pub fn require_model_service_store(&self, expected: &str) -> Result<(), ConfigError> {
        if self.model_service_operation_store_id()? != expected {
            return fail(
                "operation_store_changed",
                "操作属于另一个本机数据库，请连接原后台后核查；未提交到当前数据库",
            );
        }
        Ok(())
    }
    pub fn model_service_operation(
        &self,
        id: &str,
        store_id: &str,
    ) -> Result<OperationReceipt, ConfigError> {
        self.require_model_service_store(store_id)?;
        uuid::Uuid::parse_str(id)
            .map_err(|_| ConfigError::new("invalid_input", "模型服务操作编号无效"))?;
        load(&self.connection, id, store_id)?
            .map(|(_, r)| r)
            .ok_or_else(|| {
                ConfigError::new(
                    "operation_not_found",
                    "尚未查到已提交回执；不能据此确认操作未执行，请保留编号继续核查",
                )
            })
    }
    // D5 additive immutable receipt matcher; never publishes or validates models.
    pub fn model_service_operation_for(
        &self,
        request: &OperationRequest,
    ) -> Result<OperationReceipt, ConfigError> {
        validate(request)?;
        self.require_model_service_store(&request.store_id)?;
        let (prior, receipt) = load(&self.connection, &request.operation_id, &request.store_id)?
            .ok_or_else(|| {
                ConfigError::new(
                    "operation_not_found",
                    "尚未查到已提交回执；不能据此确认操作未执行，请保留编号继续核查",
                )
            })?;
        if serde_json::to_string(&prior).map_err(unavailable)?
            != serde_json::to_string(request).map_err(unavailable)?
        {
            return fail("operation_conflict", "此操作编号对应其他内容，请核查原结果");
        }
        Ok(receipt)
    }
    // End D5 additive immutable receipt matcher.
    pub fn commit_model_service_operation(
        &self,
        request: &OperationRequest,
        validate_change: impl FnOnce(&OperationAction) -> Result<Vec<String>, ConfigError>,
    ) -> Result<OperationReceipt, ConfigError> {
        self.commit_model_service_operation_as(request, None, validate_change)
    }

    pub fn commit_model_service_operation_as(
        &self,
        request: &OperationRequest,
        creator: Option<&str>,
        validate_change: impl FnOnce(&OperationAction) -> Result<Vec<String>, ConfigError>,
    ) -> Result<OperationReceipt, ConfigError> {
        validate(request)?;
        let tx =
            rusqlite::Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)
                .map_err(unavailable)?;
        self.require_model_service_store(&request.store_id)?;
        if let Some((prior, receipt)) = load(&tx, &request.operation_id, &request.store_id)? {
            if serde_json::to_string(&prior).map_err(unavailable)?
                != serde_json::to_string(request).map_err(unavailable)?
            {
                return fail(
                    "operation_conflict",
                    "此操作编号已用于其他内容，请核查原结果",
                );
            }
            return Ok(receipt);
        }
        let active = validate_change(&request.action)?;
        let service = match &request.action {
            OperationAction::Save { draft } => {
                Some(self.write_model_service(&tx, draft.as_ref().clone(), creator)?)
            }
            OperationAction::Delete {
                service_id,
                expected_revision,
            } => {
                self.remove_model_service(&tx, service_id, *expected_revision, &active)?;
                None
            }
        };
        let (action, service_id, expected_revision) = request.action.identity();
        let receipt = OperationReceipt {
            store_id: request.store_id.clone(),
            api_version: 1,
            operation_id: request.operation_id.clone(),
            action,
            service_id: service_id.into(),
            committed_revision: expected_revision + 1,
            service,
        };
        // Reference the immutable version rather than duplicating Provider
        // catalogs in every receipt. The FK also protects future reclamation.
        let stored = StoredOutcome {
            store_id: receipt.store_id.clone(),
            api_version: 1,
            operation_id: receipt.operation_id.clone(),
            action: receipt.action,
            service_id: receipt.service_id.clone(),
            committed_revision: receipt.committed_revision,
        };
        tx.execute(
            "INSERT INTO model_service_operations(operation_id,request,result,service_version) VALUES(?1,?2,?3,?4)",
            params![
                request.operation_id,
                serde_json::to_string(request).map_err(unavailable)?,
                serde_json::to_string(&stored).map_err(unavailable)?,
                receipt.service.as_ref().map(|s| &s.version_id)
            ],
        )
        .map_err(unavailable)?;
        tx.execute("DELETE FROM model_service_operations WHERE sequence <= (SELECT max(sequence)-?1 FROM model_service_operations)", [RECEIPT_LIMIT as i64]).map_err(unavailable)?;
        tx.commit().map_err(unavailable)?;
        Ok(receipt)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    fn request(store: &Store) -> OperationRequest {
        OperationRequest {
            store_id: store.model_service_operation_store_id().unwrap(),
            operation_id: uuid::Uuid::new_v4().to_string(),
            action: OperationAction::Save {
                draft: Box::new(super::super::tests::draft(store)),
            },
        }
    }
    // D5 additive immutable receipt matcher test.
    #[test]
    fn immutable_receipt_matching_rejects_changed_content_without_publication() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(root.path()).unwrap();
        let original = request(&store);
        assert_eq!(
            store
                .model_service_operation_for(&original)
                .unwrap_err()
                .code,
            "operation_not_found"
        );
        let first = store
            .commit_model_service_operation(&original, |_| Ok(vec![]))
            .unwrap();
        let matched = store.model_service_operation_for(&original).unwrap();
        assert_eq!(
            matched.service.unwrap().version_id,
            first.service.unwrap().version_id
        );
        let mut changed = original.clone();
        if let OperationAction::Save { draft } = &mut changed.action {
            draft.name = "Changed request".into();
        }
        assert_eq!(
            store
                .model_service_operation_for(&changed)
                .unwrap_err()
                .code,
            "operation_conflict"
        );
        changed.store_id = uuid::Uuid::new_v4().to_string();
        assert_eq!(
            store
                .model_service_operation_for(&changed)
                .unwrap_err()
                .code,
            "operation_store_changed"
        );
        assert_eq!(store.model_service(&first.service_id).unwrap().revision, 1);
        let count: i64 = store
            .connection
            .query_row("SELECT count(*) FROM model_service_operations", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(count, 1);
    }
    // End D5 additive immutable receipt matcher test.
    #[test]
    fn replay_survives_restart_and_later_publication_without_running_validation() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(root.path()).unwrap();
        let request = request(&store);
        let first = store
            .commit_model_service_operation(&request, |_| Ok(vec![]))
            .unwrap();
        let mut later = request.clone();
        later.operation_id = uuid::Uuid::new_v4().to_string();
        if let OperationAction::Save { draft } = &mut later.action {
            draft.revision = 1;
            draft.name = "Later".into();
        }
        store
            .commit_model_service_operation(&later, |_| Ok(vec![]))
            .unwrap();
        drop(store);
        let store = Store::open(root.path()).unwrap();
        let replay = store
            .commit_model_service_operation(&request, |_| {
                panic!("replay must not validate or call a model")
            })
            .unwrap();
        assert_eq!(
            replay.service.unwrap().version_id,
            first.service.unwrap().version_id
        );
        assert_eq!(store.model_service(&first.service_id).unwrap().revision, 2);
        assert_eq!(
            store
                .model_service_operation(
                    &request.operation_id,
                    &store.model_service_operation_store_id().unwrap()
                )
                .unwrap()
                .committed_revision,
            1
        );
    }
    #[test]
    fn a_reused_id_cannot_change_payload_or_action() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(root.path()).unwrap();
        let request = request(&store);
        store
            .commit_model_service_operation(&request, |_| Ok(vec![]))
            .unwrap();
        let mut changed = request.clone();
        if let OperationAction::Save { draft } = &mut changed.action {
            draft.name = "other".into();
        }
        assert_eq!(
            store
                .commit_model_service_operation(&changed, |_| panic!())
                .unwrap_err()
                .code,
            "operation_conflict"
        );
        changed.action = OperationAction::Delete {
            service_id: request.action.identity().1.into(),
            expected_revision: 1,
        };
        assert_eq!(
            store
                .commit_model_service_operation(&changed, |_| panic!())
                .unwrap_err()
                .code,
            "operation_conflict"
        );
        assert_eq!(
            store
                .model_service(request.action.identity().1)
                .unwrap()
                .revision,
            1
        );
    }
    #[test]
    fn publication_and_receipt_roll_back_together_on_disk_write_failure() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(root.path()).unwrap();
        let request = request(&store);
        store.connection.execute_batch("CREATE TRIGGER reject_receipt BEFORE INSERT ON model_service_operations BEGIN SELECT RAISE(ABORT,'fixture'); END;").unwrap();
        assert_eq!(
            store
                .commit_model_service_operation(&request, |_| Ok(vec![]))
                .unwrap_err()
                .code,
            "service_unavailable"
        );
        for table in [
            "model_services",
            "model_service_versions",
            "model_service_operations",
        ] {
            assert_eq!(
                store
                    .connection
                    .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r
                        .get::<_, i64>(0))
                    .unwrap(),
                0
            );
        }
        store
            .connection
            .execute_batch("DROP TRIGGER reject_receipt")
            .unwrap();
        assert_eq!(
            store
                .commit_model_service_operation(&request, |_| Ok(vec![]))
                .unwrap()
                .committed_revision,
            1
        );
    }
    #[test]
    fn deletion_and_receipt_roll_back_and_replay_without_needing_a_current_service() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(root.path()).unwrap();
        let save = request(&store);
        let saved = store
            .commit_model_service_operation(&save, |_| Ok(vec![]))
            .unwrap();
        let delete = OperationRequest {
            store_id: store.model_service_operation_store_id().unwrap(),
            operation_id: uuid::Uuid::new_v4().to_string(),
            action: OperationAction::Delete {
                service_id: saved.service_id.clone(),
                expected_revision: 1,
            },
        };
        store.connection.execute_batch("CREATE TRIGGER reject_receipt BEFORE INSERT ON model_service_operations BEGIN SELECT RAISE(ABORT,'fixture'); END;").unwrap();
        assert!(
            store
                .commit_model_service_operation(&delete, |_| Ok(vec![]))
                .is_err()
        );
        assert_eq!(store.model_service(&saved.service_id).unwrap().revision, 1);
        store
            .connection
            .execute_batch("DROP TRIGGER reject_receipt")
            .unwrap();
        let receipt = store
            .commit_model_service_operation(&delete, |_| Ok(vec![]))
            .unwrap();
        assert_eq!(receipt.committed_revision, 2);
        assert!(receipt.service.is_none());
        assert_eq!(
            store.model_service(&saved.service_id).unwrap_err().code,
            "not_found"
        );
        assert!(
            store
                .commit_model_service_operation(&delete, |_| panic!())
                .unwrap()
                .service
                .is_none()
        );
    }
    #[test]
    fn protected_bindings_reject_deletion_without_creating_a_success_receipt() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(root.path()).unwrap();
        let save = request(&store);
        let service = store
            .commit_model_service_operation(&save, |_| Ok(vec![]))
            .unwrap()
            .service
            .unwrap();
        let binding = super::super::tests::attach(&store, &service);
        let delete = OperationRequest {
            store_id: store.model_service_operation_store_id().unwrap(),
            operation_id: uuid::Uuid::new_v4().to_string(),
            action: OperationAction::Delete {
                service_id: service.id.clone(),
                expected_revision: service.revision,
            },
        };
        assert_eq!(
            store
                .commit_model_service_operation(&delete, |_| Ok(vec![binding.binding_id]))
                .unwrap_err()
                .code,
            "in_use"
        );
        assert_eq!(
            store
                .model_service_operation(
                    &delete.operation_id,
                    &store.model_service_operation_store_id().unwrap()
                )
                .unwrap_err()
                .code,
            "operation_not_found"
        );
        assert!(store.model_service(&service.id).is_ok());
    }
    #[test]
    fn corrupt_receipts_are_not_hidden_or_overwritten() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(root.path()).unwrap();
        let request = request(&store);
        store
            .commit_model_service_operation(&request, |_| Ok(vec![]))
            .unwrap();
        store
            .connection
            .execute("UPDATE model_service_operations SET result='{}'", [])
            .unwrap();
        assert_eq!(
            store
                .model_service_operation(
                    &request.operation_id,
                    &store.model_service_operation_store_id().unwrap()
                )
                .unwrap_err()
                .code,
            "invalid_record"
        );
        assert_eq!(
            store
                .commit_model_service_operation(&request, |_| panic!())
                .unwrap_err()
                .code,
            "invalid_record"
        );
        assert_eq!(
            store
                .connection
                .query_row("SELECT result FROM model_service_operations", [], |r| {
                    r.get::<_, String>(0)
                })
                .unwrap(),
            "{}"
        );
    }
    #[test]
    fn independent_connections_commit_the_same_request_once() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(root.path()).unwrap();
        let request = request(&store);
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let checked = Arc::new(AtomicUsize::new(0));
        let connections = [
            Store::open(root.path()).unwrap(),
            Store::open(root.path()).unwrap(),
        ];
        let workers = connections.map(|store| {
            let barrier = barrier.clone();
            let checked = checked.clone();
            let request = request.clone();
            std::thread::spawn(move || {
                barrier.wait();
                store
                    .commit_model_service_operation(&request, |_| {
                        checked.fetch_add(1, Ordering::SeqCst);
                        Ok(vec![])
                    })
                    .unwrap()
                    .service
                    .unwrap()
                    .version_id
            })
        });
        let [a, b] = workers.map(|w| w.join().unwrap());
        assert_eq!(a, b);
        assert_eq!(checked.load(Ordering::SeqCst), 1);
    }
    #[test]
    fn bounded_receipts_do_not_allow_an_evicted_write_to_republish() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(root.path()).unwrap();
        let original = request(&store);
        store
            .commit_model_service_operation(&original, |_| Ok(vec![]))
            .unwrap();
        let mut next = original.clone();
        for revision in 1..=RECEIPT_LIMIT as u64 {
            next.operation_id = uuid::Uuid::new_v4().to_string();
            if let OperationAction::Save { draft } = &mut next.action {
                draft.revision = revision;
            }
            store
                .commit_model_service_operation(&next, |_| Ok(vec![]))
                .unwrap();
        }
        assert_eq!(
            store
                .connection
                .query_row("SELECT count(*) FROM model_service_operations", [], |r| {
                    r.get::<_, i64>(0)
                })
                .unwrap(),
            RECEIPT_LIMIT as i64
        );
        assert_eq!(
            store
                .model_service_operation(
                    &original.operation_id,
                    &store.model_service_operation_store_id().unwrap()
                )
                .unwrap_err()
                .code,
            "operation_not_found"
        );
        assert_eq!(
            store
                .commit_model_service_operation(&original, |_| Ok(vec![]))
                .unwrap_err()
                .code,
            "conflict"
        );
        assert_eq!(
            store
                .model_service(original.action.identity().1)
                .unwrap()
                .revision,
            RECEIPT_LIMIT as u64 + 1
        );
    }
    #[test]
    fn v7_migration_backs_up_the_old_schema_and_keeps_services() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(&root.path().canonicalize().unwrap()).unwrap();
        let draft = super::super::tests::draft(&store);
        let service = store.save_model_service(draft).unwrap();
        store
            .connection
            .execute_batch("DROP TABLE local_token_cancellations;DROP TABLE local_token_owners;DROP TABLE local_token_values;DROP TABLE local_token_scope;DROP TABLE native_import_operations;DROP TABLE native_import_scope;DROP TABLE credential_legacy_operations;DROP TABLE credential_legacy_scope;DROP TABLE credential_recovery_values;DROP TABLE credential_recovery_scope;DROP TABLE model_service_operations; DROP TABLE model_service_operation_scope; PRAGMA user_version=7;")
            .unwrap();
        drop(store);
        let reopened = Store::open(&root.path().canonicalize().unwrap()).unwrap();
        assert_eq!(
            reopened.model_service(&service.id).unwrap().version_id,
            service.version_id
        );
        let backup = Connection::open(
            root.path()
                .join("local-agent.pre-model-service-operations-v7.sqlite3"),
        )
        .unwrap();
        assert_eq!(
            backup
                .query_row("PRAGMA user_version", [], |r| r.get::<_, u32>(0))
                .unwrap(),
            7
        );
        assert_eq!(
            backup
                .query_row(
                    "SELECT count(*) FROM sqlite_master WHERE name='model_service_operations'",
                    [],
                    |r| r.get::<_, u32>(0)
                )
                .unwrap(),
            0
        );
        assert_eq!(
            reopened
                .connection
                .query_row("PRAGMA user_version", [], |r| r.get::<_, u32>(0))
                .unwrap(),
            Store::SCHEMA_VERSION
        );
    }
    #[test]
    fn current_schema_never_replaces_a_missing_or_invalid_store_identity() {
        for damage in [
            "DELETE FROM model_service_operation_scope",
            "DROP TABLE model_service_operation_scope",
            "UPDATE model_service_operation_scope SET store_id='invalid-fixture'",
        ] {
            let root = tempfile::tempdir().unwrap();
            let directory = root.path().canonicalize().unwrap();
            let store = Store::open(&directory).unwrap();
            let request = request(&store);
            let receipt = store
                .commit_model_service_operation(&request, |_| Ok(vec![]))
                .unwrap();
            store.connection.execute_batch(damage).unwrap();
            drop(store);
            assert!(Store::open(&directory).is_err(), "{damage}");
            let db = Connection::open(directory.join("local-agent.sqlite3")).unwrap();
            assert_eq!(
                db.query_row("PRAGMA user_version", [], |r| r.get::<_, u32>(0))
                    .unwrap(),
                Store::SCHEMA_VERSION
            );
            assert_eq!(
                db.query_row("SELECT count(*) FROM model_service_operations", [], |r| r
                    .get::<_, u32>(
                    0
                ))
                .unwrap(),
                1
            );
            match damage {
                "DROP TABLE model_service_operation_scope" => assert_eq!(db.query_row("SELECT count(*) FROM sqlite_master WHERE name='model_service_operation_scope'", [], |r| r.get::<_, u32>(0)).unwrap(), 0),
                "DELETE FROM model_service_operation_scope" => assert_eq!(db.query_row("SELECT count(*) FROM model_service_operation_scope", [], |r| r.get::<_, u32>(0)).unwrap(), 0),
                _ => assert_eq!(db.query_row("SELECT store_id FROM model_service_operation_scope", [], |r| r.get::<_, String>(0)).unwrap(), "invalid-fixture"),
            }
            db.execute_batch("DROP TABLE IF EXISTS model_service_operation_scope; CREATE TABLE model_service_operation_scope(id INTEGER PRIMARY KEY CHECK(id=1),store_id TEXT NOT NULL UNIQUE);").unwrap();
            db.execute(
                "INSERT INTO model_service_operation_scope VALUES(1,?1)",
                [&request.store_id],
            )
            .unwrap();
            drop(db);
            let restored = Store::open(&directory).unwrap();
            assert_eq!(
                serde_json::to_value(
                    restored
                        .model_service_operation(&request.operation_id, &request.store_id)
                        .unwrap()
                )
                .unwrap(),
                serde_json::to_value(receipt).unwrap()
            );
        }
    }
    #[test]
    fn a_different_store_rejects_reads_and_commits_before_validation() {
        let a = tempfile::tempdir().unwrap();
        let a = Store::open(a.path()).unwrap();
        let request = request(&a);
        let b = tempfile::tempdir().unwrap();
        let b = Store::open(b.path()).unwrap();
        assert_eq!(
            b.commit_model_service_operation(&request, |_| panic!(
                "wrong store must not validate or call a model"
            ))
            .unwrap_err()
            .code,
            "operation_store_changed"
        );
        assert_eq!(
            b.model_service_operation(&request.operation_id, &request.store_id)
                .unwrap_err()
                .code,
            "operation_store_changed"
        );
        assert_eq!(
            b.connection
                .query_row("SELECT count(*) FROM model_service_operations", [], |r| r
                    .get::<_, i64>(
                    0
                ))
                .unwrap(),
            0
        );
    }
    #[test]
    fn compact_receipts_protect_the_immutable_version_without_duplicating_provider_data() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(root.path()).unwrap();
        let first = request(&store);
        let receipt = store
            .commit_model_service_operation(&first, |_| Ok(vec![]))
            .unwrap();
        let version = receipt.service.unwrap().version_id;
        let mut later = first.clone();
        later.operation_id = uuid::Uuid::new_v4().to_string();
        if let OperationAction::Save { draft } = &mut later.action {
            draft.revision = 1;
        }
        store
            .commit_model_service_operation(&later, |_| Ok(vec![]))
            .unwrap();
        let raw: String = store
            .connection
            .query_row(
                "SELECT result FROM model_service_operations WHERE operation_id=?1",
                [&first.operation_id],
                |r| r.get(0),
            )
            .unwrap();
        assert!(!raw.contains("http://"));
        assert!(!raw.contains("models"));
        assert!(raw.len() < 1024);
        assert!(
            store
                .connection
                .execute("DELETE FROM model_service_versions WHERE id=?1", [&version])
                .is_err()
        );
        assert_eq!(
            store
                .model_service_operation(&first.operation_id, &first.store_id)
                .unwrap()
                .service
                .unwrap()
                .version_id,
            version
        );
    }
    #[test]
    fn oversized_parameters_never_enter_validation_or_the_receipt_journal() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(root.path()).unwrap();
        let mut request = request(&store);
        if let OperationAction::Save { draft } = &mut request.action {
            draft.name = " ".repeat(MAX_REQUEST_BYTES) + "name";
        }
        assert_eq!(
            store
                .commit_model_service_operation(&request, |_| panic!())
                .unwrap_err()
                .code,
            "invalid_input"
        );
        assert_eq!(
            store
                .connection
                .query_row("SELECT count(*) FROM model_service_operations", [], |r| r
                    .get::<_, i64>(
                    0
                ))
                .unwrap(),
            0
        );
    }
}
