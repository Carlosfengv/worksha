//! Persist the admission result before an execution can leave the workbench.
use super::{Error, Storage, Task, Workbench, conflict, identifier, store_error, workbench};
use crate::{AppState, api};
use axum::{
    Json,
    extract::{Query, State},
    http::StatusCode,
};
use local_agent_core::providers::files;
use rusqlite::{Connection, OpenFlags, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    fs,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::Path,
};

const APPLICATION_ID: u32 = 0x4c415742;
const SCHEMA_VERSION: u32 = 1;
const TASK_COLUMNS: &str = "id,updated,phase,value,dev,ino";
const SHARED_COLUMNS: &str = "id,scope,updated,value";

pub(super) fn unknown() -> Error {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({
            "code":"workbench_operation_pending",
            "message":"原操作的结果待核查。请查看原任务或会话，避免重复发送。",
            "outcomeUnknown":true,
        })),
    )
}

pub(super) fn open(root: &Path) -> Result<(Connection, String), String> {
    let path = root.join("tasks.sqlite3");
    files::checked_path(&path).map_err(|error| error.message)?;
    if let Ok(meta) = fs::symlink_metadata(&path)
        && (!meta.is_file() || meta.nlink() != 1)
    {
        return Err("工作台数据库必须是单链接普通文件".into());
    }
    let mut db = Connection::open(path).map_err(|error| error.to_string())?;
    let store_id = initialize(&mut db, root)?;
    db.pragma_update(None, "journal_mode", "WAL")
        .map_err(|error| error.to_string())?;
    db.pragma_update(None, "synchronous", "FULL")
        .map_err(|error| error.to_string())?;
    Ok((db, store_id))
}

fn initialize(db: &mut Connection, root: &Path) -> Result<String, String> {
    let stamp = |key| {
        db.pragma_query_value(None, key, |row| row.get::<_, u32>(0))
            .map_err(|error| error.to_string())
    };
    let application = stamp("application_id")?;
    let version = stamp("user_version")?;
    if version > SCHEMA_VERSION {
        return Err("工作台数据库来自更新版本，请升级应用后重试".into());
    }
    let tables = {
        let mut query = db
            .prepare("SELECT name FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%' ORDER BY name")
            .map_err(|error| error.to_string())?;
        query
            .query_map([], |row| row.get::<_, String>(0))
            .and_then(|rows| rows.collect::<Result<Vec<_>, _>>())
            .map_err(|error| error.to_string())?
    };
    let columns = |table: &str| {
        db.query_row("SELECT group_concat(name, ',') FROM (SELECT name FROM pragma_table_info(?1) ORDER BY cid)", [table], |row| row.get::<_, String>(0))
            .map_err(|error| error.to_string())
    };
    let legacy = ["workbench_shared_sessions", "workbench_tasks"];
    let current = [
        "workbench_meta",
        "workbench_operations",
        "workbench_shared_sessions",
        "workbench_tasks",
    ];
    if application == APPLICATION_ID
        && version == SCHEMA_VERSION
        && tables == current
        && columns("workbench_tasks")? == TASK_COLUMNS
        && columns("workbench_shared_sessions")? == SHARED_COLUMNS
        && columns("workbench_meta")? == "id,store_id"
        && columns("workbench_operations")? == "kind,id,fingerprint,result"
    {
        let id: String = db
            .query_row(
                "SELECT store_id FROM workbench_meta WHERE id=1",
                [],
                |row| row.get(0),
            )
            .map_err(|_| "工作台数据库缺少身份，原数据已保留".to_owned())?;
        identifier(&id).map_err(|_| "工作台数据库身份无效，原数据已保留".to_owned())?;
        return Ok(id);
    }
    if application != 0 || version != 0 || (!tables.is_empty() && tables != legacy) {
        return Err("工作台数据库格式不兼容或不完整，原数据已保留".into());
    }
    if tables == legacy {
        if columns("workbench_tasks")? != TASK_COLUMNS
            || columns("workbench_shared_sessions")? != SHARED_COLUMNS
        {
            return Err("旧工作台数据库结构不兼容，原数据已保留".into());
        }
        backup(db, root)?;
    }
    let tx = db
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| error.to_string())?;
    let id = uuid::Uuid::new_v4().to_string();
    tx.execute_batch(
        "CREATE TABLE IF NOT EXISTS workbench_tasks (
             id TEXT PRIMARY KEY,updated INTEGER NOT NULL,phase INTEGER NOT NULL,
             value TEXT NOT NULL,dev INTEGER NOT NULL,ino INTEGER NOT NULL
         );
         CREATE TABLE IF NOT EXISTS workbench_shared_sessions (
             id TEXT PRIMARY KEY,scope TEXT NOT NULL,updated INTEGER NOT NULL,value TEXT NOT NULL
         );
         CREATE TABLE workbench_meta (
             id INTEGER PRIMARY KEY CHECK(id=1),store_id TEXT NOT NULL
         );
         CREATE TABLE workbench_operations (
             kind TEXT NOT NULL,id TEXT NOT NULL,fingerprint TEXT NOT NULL,result TEXT NOT NULL,
             PRIMARY KEY(kind,id)
         );",
    )
    .map_err(|error| error.to_string())?;
    tx.execute("INSERT INTO workbench_meta VALUES(1,?1)", [&id])
        .map_err(|error| error.to_string())?;
    tx.pragma_update(None, "application_id", APPLICATION_ID)
        .map_err(|error| error.to_string())?;
    tx.pragma_update(None, "user_version", SCHEMA_VERSION)
        .map_err(|error| error.to_string())?;
    tx.commit().map_err(|error| error.to_string())?;
    Ok(id)
}

fn backup(db: &Connection, root: &Path) -> Result<(), String> {
    let path = root.join(format!("tasks.pre-v1.{}.sqlite3", uuid::Uuid::new_v4()));
    let problem = "工作台升级备份无法创建或校验，原数据库未迁移";
    db.execute("VACUUM INTO ?1", [path.to_str().ok_or(problem)?])
        .map_err(|_| problem)?;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).map_err(|_| problem)?;
    fs::File::open(&path)
        .and_then(|file| file.sync_all())
        .map_err(|_| problem)?;
    fs::File::open(root)
        .and_then(|dir| dir.sync_all())
        .map_err(|_| problem)?;
    let snapshot = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|_| problem)?;
    if snapshot
        .query_row("PRAGMA quick_check", [], |row| row.get::<_, String>(0))
        .is_ok_and(|status| status == "ok")
    {
        Ok(())
    } else {
        Err(problem.into())
    }
}

#[derive(Clone, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Scope {
    pub(super) store_id: String,
    authorization_revision: String,
}

impl Scope {
    pub(super) fn current(bench: &Workbench, state: &AppState) -> Result<Self, Error> {
        Ok(Self {
            store_id: bench.store_id.clone(),
            authorization_revision: state
                .readonly
                .host
                .snapshot()
                .map_err(store_error)?
                .generation,
        })
    }

    pub(super) fn check(
        bench: &Workbench,
        state: &AppState,
        expected: Option<&Self>,
    ) -> Result<(), Error> {
        if let Some(expected) = expected
            && expected != &Self::current(bench, state)?
        {
            return Err(conflict("任务后台或插件授权已变化，请核查原操作后重新读取"));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) enum Kind {
    Create,
    Send,
    SharedCreate,
    LiveSend,
}
impl Kind {
    fn name(self) -> &'static str {
        match self {
            Self::Create => "create",
            Self::Send => "send",
            Self::SharedCreate => "sharedCreate",
            Self::LiveSend => "liveSend",
        }
    }
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Receipt {
    pub(super) target_id: String,
    pub(super) accepted: bool,
    pub(super) committed_revision: Option<u64>,
    pub(super) session_id: Option<String>,
}

pub(super) struct Operation<'a> {
    kind: Kind,
    id: &'a str,
    target_id: &'a str,
    fingerprint: String,
}
impl<'a> Operation<'a> {
    pub(super) fn new(kind: Kind, id: &'a str, target_id: &'a str, payload: Value) -> Self {
        Self {
            kind,
            id,
            target_id,
            fingerprint: files::hash(payload.to_string().as_bytes()),
        }
    }

    pub(super) fn prior(&self, db: &Connection) -> Result<Option<Receipt>, Error> {
        db.query_row(
            "SELECT fingerprint,result FROM workbench_operations WHERE kind=?1 AND id=?2",
            params![self.kind.name(), self.id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()
        .map_err(store_error)?
        .map(|(hash, value)| {
            if hash != self.fingerprint {
                return Err(conflict("该操作编号已用于不同的任务或参数"));
            }
            serde_json::from_str(&value).map_err(store_error)
        })
        .transpose()
    }

    pub(super) fn save(
        &self,
        db: &Connection,
        accepted: bool,
        revision: Option<u64>,
        session_id: Option<&str>,
    ) -> Result<(), Error> {
        let receipt = Receipt {
            target_id: self.target_id.into(),
            accepted,
            committed_revision: revision,
            session_id: session_id.map(str::to_owned),
        };
        let updated = db
            .execute(
                "INSERT INTO workbench_operations VALUES (?1,?2,?3,?4)
             ON CONFLICT(kind,id) DO UPDATE SET result=excluded.result
             WHERE fingerprint=excluded.fingerprint",
                params![
                    self.kind.name(),
                    self.id,
                    self.fingerprint,
                    serde_json::to_string(&receipt).map_err(store_error)?
                ],
            )
            .map_err(store_error)?;
        if updated != 1 {
            return Err(conflict("该操作编号已用于不同的任务或参数"));
        }
        Ok(())
    }
}

impl Storage {
    pub(super) fn accept(
        &mut self,
        operation: &Operation<'_>,
        task: &mut Task,
    ) -> Result<(), Error> {
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(store_error)?;
        Self::save_task(&tx, task)?;
        operation.save(&tx, true, Some(task.revision), task.session_id.as_deref())?;
        tx.commit().map_err(store_error)
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct OperationQuery {
    kind: Kind,
    operation_id: String,
    store_id: String,
}

pub(super) async fn read(
    State(state): State<AppState>,
    Query(query): Query<OperationQuery>,
) -> api::ApiResult {
    identifier(&query.operation_id)?;
    let bench = workbench(&state)?;
    if query.store_id != bench.store_id {
        return Err(conflict("当前后台与原操作的数据仓库不同"));
    }
    let receipt = bench
        .lock()?
        .db
        .query_row(
            "SELECT result FROM workbench_operations WHERE kind=?1 AND id=?2",
            params![query.kind.name(), query.operation_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(store_error)?
        .map(|value| serde_json::from_str::<Receipt>(&value))
        .transpose()
        .map_err(store_error)?;
    api::json_value(json!({
        "storeId":bench.store_id,"operationId":query.operation_id,"kind":query.kind,"receipt":receipt
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrates_both_legacy_tables_and_backs_up_committed_wal_rows() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let legacy = Connection::open(root.join("tasks.sqlite3")).unwrap();
        legacy.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0;
             CREATE TABLE workbench_tasks (id TEXT PRIMARY KEY,updated INTEGER NOT NULL,phase INTEGER NOT NULL,value TEXT NOT NULL,dev INTEGER NOT NULL,ino INTEGER NOT NULL);
             CREATE TABLE workbench_shared_sessions (id TEXT PRIMARY KEY,scope TEXT NOT NULL,updated INTEGER NOT NULL,value TEXT NOT NULL);
             INSERT INTO workbench_tasks VALUES('local-task',1,0,'original-task',2,3);
             INSERT INTO workbench_shared_sessions VALUES('shared-task','original-scope',4,'native-session');",
        ).unwrap();
        assert!(root.join("tasks.sqlite3-wal").metadata().unwrap().len() > 0);
        let (current, id) = open(&root).unwrap();
        assert_ne!(id, "");
        let backup = fs::read_dir(&root)
            .unwrap()
            .filter_map(Result::ok)
            .find(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("tasks.pre-v1.")
            })
            .unwrap()
            .path();
        assert_eq!(backup.metadata().unwrap().mode() & 0o777, 0o600);
        let snapshot =
            Connection::open_with_flags(backup, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        for db in [&current, &snapshot] {
            assert_eq!(
                db.query_row("SELECT value,dev,ino FROM workbench_tasks", [], |row| Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?
                )),)
                    .unwrap(),
                ("original-task".into(), 2, 3)
            );
            assert_eq!(
                db.query_row(
                    "SELECT scope,value FROM workbench_shared_sessions",
                    [],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                )
                .unwrap(),
                ("original-scope".into(), "native-session".into())
            );
        }
        assert_eq!(
            snapshot
                .pragma_query_value(None, "user_version", |row| row.get::<_, u32>(0))
                .unwrap(),
            0
        );
        assert_eq!(open(&root).unwrap().1, id);
    }

    #[test]
    fn refuses_future_and_incomplete_databases_without_recreating_schema() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let path = root.join("tasks.sqlite3");
        let (db, _) = open(&root).unwrap();
        db.pragma_update(None, "user_version", SCHEMA_VERSION + 1)
            .unwrap();
        drop(db);
        let before = fs::read(&path).unwrap();
        assert!(matches!(open(&root), Err(error) if error.contains("更新版本")));
        assert_eq!(fs::read(&path).unwrap(), before);
        let db = Connection::open(&path).unwrap();
        db.execute_batch("PRAGMA user_version=1; DROP TABLE workbench_operations;")
            .unwrap();
        drop(db);
        let before = fs::read(&path).unwrap();
        assert!(open(&root).is_err());
        assert_eq!(fs::read(&path).unwrap(), before);
    }

    #[test]
    fn changed_payload_cannot_replace_pending_or_committed_receipt() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let (db, _) = open(&root).unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let original = Operation::new(Kind::LiveSend, &id, "session", json!(["first"]));
        let changed = Operation::new(Kind::LiveSend, &id, "session", json!(["changed"]));
        original.save(&db, false, None, Some("session")).unwrap();
        assert!(changed.prior(&db).is_err());
        assert!(changed.save(&db, true, None, Some("other")).is_err());
        assert!(!original.prior(&db).unwrap().unwrap().accepted);
        original.save(&db, true, None, Some("session")).unwrap();
        assert!(original.prior(&db).unwrap().unwrap().accepted);
    }
}
