//! Native import ownership before any external write. No credential bytes are stored here.
use super::{ImportRequest, Imported};
use crate::{
    Provider, Store,
    providers::{ConfigError, fail, files, operations::valid_id},
};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::{Deserialize, Serialize};

pub const LIMIT: usize = 4096;
pub const PENDING_LIMIT: usize = 4;
const VALUE_LIMIT: usize = 2 * 1024 * 1024;
const BYTE_LIMIT: usize = 16 * 1024 * 1024;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Identity {
    pub store_id: String,
    pub namespace_id: String,
    pub operation_id: String,
    pub expected_sequence: u64,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Pending,
    Committed,
    Cancelled,
    Expired,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Operation {
    pub version: u32,
    pub sequence: u64,
    pub status: Status,
    pub request: ImportRequest,
    pub provider: Option<Provider>,
    pub reused: bool,
    pub creates_credential: bool,
}
impl Operation {
    pub(super) fn result(&self) -> Result<Imported, ConfigError> {
        match self.status {
            Status::Committed => Ok(Imported {
                provider: self.provider.clone().ok_or_else(corrupt)?,
                reused: self.reused,
            }),
            Status::Pending => Err(unknown()),
            Status::Expired => fail(
                "native_import_expired",
                "原导入请求的序号已由另一项操作占用，这次请求未被接纳；请重新识别后开始新的导入",
            ),
            Status::Cancelled => fail(
                "native_import_cancelled",
                "原导入已取消，未重新保存连接；已写入的凭据保留在原恢复空间中供独立核查",
            ),
        }
    }
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Protocol {
    pub version: u32,
    pub store_id: String,
    pub namespace_id: String,
    pub namespace_state: crate::providers::credential_scope::NamespaceState,
    pub sequence: u64,
    pub operation_limit: usize,
    pub pending_limit: usize,
    pub pending_requests: Vec<ImportRequest>,
}
pub(super) fn unknown() -> ConfigError {
    ConfigError::new(
        "native_import_result_unknown",
        "原导入结果尚未确认，请保留原操作编号核查；不要重新创建导入",
    )
}
fn corrupt() -> ConfigError {
    ConfigError::new(
        "native_import_record_unavailable",
        "原生导入记录不可核查，原记录与凭据已保留",
    )
}
pub fn validate(request: &ImportRequest) -> Result<&Identity, ConfigError> {
    let identity = request
        .operation
        .as_ref()
        .ok_or_else(|| ConfigError::new("invalid_native_import", "请提供原生导入的原操作身份"))?;
    if !valid_id(&identity.store_id)
        || !valid_id(&identity.namespace_id)
        || !valid_id(&identity.operation_id)
        || identity.expected_sequence >= LIMIT as u64
        || request.candidate_id.is_empty()
        || request.candidate_id.len() > 256
        || request.expected_revision.len() != 64
        || !request
            .expected_revision
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        || request
            .model
            .as_ref()
            .is_some_and(|m| m.len() > 800 || m.chars().any(char::is_control))
    {
        return fail(
            "invalid_native_import",
            "导入来源、编号、版本或模型无效，未提交导入",
        );
    }
    Ok(identity)
}
fn encoded<T: Serialize>(value: &T) -> Result<String, ConfigError> {
    serde_json::to_string(value).map_err(|_| corrupt())
}
fn request_hash(request: &ImportRequest) -> Result<String, ConfigError> {
    Ok(files::hash(encoded(request)?.as_bytes()))
}
pub(crate) fn initialize(tx: &Transaction<'_>, store_id: &str) -> Result<(), ConfigError> {
    tx.execute_batch("CREATE TABLE native_import_scope(id INTEGER PRIMARY KEY CHECK(id=1),format INTEGER NOT NULL,store_id TEXT NOT NULL,sequence INTEGER NOT NULL,operation_count INTEGER NOT NULL,value_bytes INTEGER NOT NULL);
        CREATE TABLE native_import_operations(sequence INTEGER PRIMARY KEY,operation_id TEXT NOT NULL UNIQUE,state TEXT NOT NULL,request_hash TEXT NOT NULL,value_hash TEXT NOT NULL,value TEXT NOT NULL CHECK(json_valid(value)));").map_err(|_| corrupt())?;
    if tx
        .execute(
            "INSERT INTO native_import_scope VALUES(1,1,?1,0,0,0)",
            [store_id],
        )
        .map_err(|_| corrupt())?
        != 1
    {
        return Err(corrupt());
    }
    scope(tx, store_id)?;
    Ok(())
}
fn scope(db: &Connection, store_id: &str) -> Result<u64, ConfigError> {
    let (format, actual, sequence, count, bytes): (u32,String,i64,i64,i64) = db.query_row("SELECT format,store_id,sequence,operation_count,value_bytes FROM native_import_scope WHERE id=1", [], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).map_err(|_| corrupt())?;
    let scopes: i64 = db
        .query_row(
            "SELECT count(*) FROM (SELECT 1 FROM native_import_scope LIMIT 2)",
            [],
            |r| r.get(0),
        )
        .map_err(|_| corrupt())?;
    let (rows,size,pending,first,last):(i64,i64,i64,i64,i64) = db.query_row("SELECT count(*),COALESCE(sum(length(CAST(value AS BLOB))),0),COALESCE(sum(state='pending'),0),COALESCE(min(sequence),0),COALESCE(max(sequence),0) FROM (SELECT sequence,state,value FROM native_import_operations LIMIT 4097)", [], |r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).map_err(|_| corrupt())?;
    if scopes != 1
        || format != 1
        || !valid_id(store_id)
        || actual != store_id
        || !(0..=LIMIT as i64).contains(&sequence)
        || sequence != count
        || count != rows
        || bytes != size
        || !(0..=BYTE_LIMIT as i64).contains(&bytes)
        || !(0..=PENDING_LIMIT as i64).contains(&pending)
        || first != if rows == 0 { 0 } else { 1 }
        || last != rows
        // A pending draft is already normalized at revision zero. Its terminal
        // record adds two bytes (pending -> committed/cancelled, revision 0 -> 1).
        // Reserve those bytes before Vault so capacity cannot prevent completion.
        || bytes + 2 * pending > BYTE_LIMIT as i64
    {
        return Err(corrupt());
    }
    Ok(sequence as u64)
}
pub(crate) fn validate_existing(db: &Connection, store_id: &str) -> Result<(), ConfigError> {
    scope(db, store_id).map(|_| ())
}
fn load(db: &Connection, identity: &Identity) -> Result<Option<Operation>, ConfigError> {
    let row:Option<(i64,String,String,String,String)> = db.query_row("SELECT sequence,state,request_hash,value_hash,CAST(substr(CAST(value AS BLOB),1,?2) AS TEXT) FROM native_import_operations WHERE operation_id=?1", params![identity.operation_id,(VALUE_LIMIT+1) as i64], |r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).optional().map_err(|_| corrupt())?;
    row.map(|(sequence, state, hash, value_hash, value)| {
        let sequence = u64::try_from(sequence).map_err(|_| corrupt())?;
        if value.len() > VALUE_LIMIT || files::hash(value.as_bytes()) != value_hash {
            return Err(corrupt());
        }
        let operation: Operation = serde_json::from_str(&value).map_err(|_| corrupt())?;
        let stored = validate(&operation.request).map_err(|_| corrupt())?;
        if stored != identity
            || operation.version != 1
            || operation.sequence != sequence
            || sequence != identity.expected_sequence + 1
            || encoded(&operation.status)? != encoded(&state)?
            || request_hash(&operation.request)? != hash
            || matches!(operation.status, Status::Pending | Status::Committed)
                && operation.provider.is_none()
            || operation.reused && operation.status != Status::Committed
            || operation.provider.as_ref().is_some_and(|p| {
                !operation.reused && p.id != format!("custom-{}", identity.operation_id)
            })
            || operation.status == Status::Expired
        {
            return Err(corrupt());
        }
        Ok(operation)
    })
    .transpose()
}
fn save(
    db: &Connection,
    operation: &Operation,
    previous: Option<&Operation>,
) -> Result<(), ConfigError> {
    let identity = validate(&operation.request)?;
    let value = encoded(operation)?;
    if value.len() > VALUE_LIMIT {
        return fail("capacity", "导入记录超出大小限制，未写入凭据");
    }
    let state = serde_json::to_value(&operation.status).map_err(|_| corrupt())?;
    let state = state.as_str().ok_or_else(corrupt)?;
    let changed=if let Some(previous)=previous {
        db.execute("UPDATE native_import_operations SET state=?1,value_hash=?2,value=?3 WHERE operation_id=?4 AND state='pending' AND value_hash=?5", params![state,files::hash(value.as_bytes()),value,identity.operation_id,files::hash(encoded(previous)?.as_bytes())])
    } else {
        db.execute("INSERT INTO native_import_operations VALUES(?1,?2,?3,?4,?5,?6)",params![operation.sequence as i64,identity.operation_id,state,request_hash(&operation.request)?,files::hash(value.as_bytes()),value])
    }.map_err(|_|corrupt())?;
    if changed != 1 {
        return Err(corrupt());
    }
    let previous_bytes = previous
        .map(encoded)
        .transpose()?
        .map_or(0, |value| value.len());
    let delta = value.len() as i64 - previous_bytes as i64;
    let changed=if previous.is_some() {
        db.execute("UPDATE native_import_scope SET value_bytes=value_bytes+?1 WHERE id=1 AND format=1 AND store_id=?2",params![delta,identity.store_id])
    } else {
        db.execute("UPDATE native_import_scope SET sequence=sequence+1,operation_count=operation_count+1,value_bytes=value_bytes+?1 WHERE id=1 AND format=1 AND store_id=?2 AND sequence=?3",params![delta,identity.store_id,identity.expected_sequence as i64])
    }.map_err(|_|corrupt())?;
    if changed != 1 || encoded(&load(db, identity)?.ok_or_else(corrupt)?)? != value {
        return Err(corrupt());
    }
    scope(db, &identity.store_id)?;
    Ok(())
}
impl Store {
    pub(super) fn import_sequence(&self) -> Result<u64, ConfigError> {
        scope(&self.connection, &self.database_id()?)
    }
    pub(super) fn pending_import_requests(&self) -> Result<Vec<ImportRequest>, ConfigError> {
        self.import_sequence()?;
        let mut query=self.connection.prepare("SELECT CAST(substr(CAST(value AS BLOB),1,?1) AS TEXT) FROM native_import_operations WHERE state='pending' ORDER BY sequence LIMIT 5").map_err(|_|corrupt())?;
        let values = query
            .query_map([(VALUE_LIMIT + 1) as i64], |r| r.get::<_, String>(0))
            .map_err(|_| corrupt())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| corrupt())?;
        if values.len() > PENDING_LIMIT {
            return Err(corrupt());
        }
        values
            .into_iter()
            .map(|value| {
                if value.len() > VALUE_LIMIT {
                    return Err(corrupt());
                }
                let operation: Operation = serde_json::from_str(&value).map_err(|_| corrupt())?;
                let identity = validate(&operation.request)?;
                let loaded = load(&self.connection, identity)?.ok_or_else(corrupt)?;
                Ok(loaded.request)
            })
            .collect()
    }
    pub(super) fn read_import(
        &self,
        request: &ImportRequest,
    ) -> Result<Option<Operation>, ConfigError> {
        let identity = validate(request)?;
        if self.database_id()? != identity.store_id {
            return fail(
                "credential_store_changed",
                "导入属于另一个数据库，请连接原后台核查",
            );
        }
        self.import_sequence()?;
        let operation = load(&self.connection, identity)?;
        if let Some(operation) = &operation
            && request_hash(&operation.request)? != request_hash(request)?
        {
            return fail(
                "operation_conflict",
                "同一导入编号对应不同来源或模型，未重新执行",
            );
        }
        Ok(operation)
    }
    /// The complete non-evicting sequence ledger can prove a request lost its
    /// reservation. An equal/future sequence or an unreadable occupant proves nothing.
    pub(super) fn expired_import(
        &self,
        request: &ImportRequest,
    ) -> Result<Option<Operation>, ConfigError> {
        let identity = validate(request)?;
        if self.import_sequence()? <= identity.expected_sequence {
            return Ok(None);
        }
        let value:String=self.connection.query_row("SELECT CAST(substr(CAST(value AS BLOB),1,?2) AS TEXT) FROM native_import_operations WHERE sequence=?1",params![(identity.expected_sequence+1) as i64,(VALUE_LIMIT+1) as i64],|r|r.get(0)).map_err(|_|corrupt())?;
        if value.len() > VALUE_LIMIT {
            return Err(corrupt());
        }
        let occupant: Operation = serde_json::from_str(&value).map_err(|_| corrupt())?;
        let owner = validate(&occupant.request).map_err(|_| corrupt())?;
        if owner.store_id != identity.store_id || owner.namespace_id != identity.namespace_id {
            return Err(corrupt());
        }
        load(&self.connection, owner)?.ok_or_else(corrupt)?;
        if owner.operation_id == identity.operation_id {
            return Ok(None);
        }
        Ok(Some(Operation {
            version: 1,
            sequence: identity.expected_sequence + 1,
            status: Status::Expired,
            request: request.clone(),
            provider: None,
            reused: false,
            creates_credential: false,
        }))
    }
    pub(super) fn begin_import(
        &self,
        request: &ImportRequest,
        provider: Option<Provider>,
        reused: bool,
        creates_credential: bool,
        cancelled: bool,
    ) -> Result<Operation, ConfigError> {
        let identity = validate(request)?;
        let tx = Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)
            .map_err(|_| unknown())?;
        if let Some(existing) = self.read_import(request)? {
            return Ok(existing);
        }
        if self.import_sequence()? != identity.expected_sequence {
            return Err(unknown());
        }
        if !cancelled && !reused && self.pending_import_requests()?.len() >= PENDING_LIMIT
            || identity.expected_sequence >= LIMIT as u64
        {
            return fail("capacity", "待核查导入或记录数量已达上限，请先核查原操作");
        }
        let operation = Operation {
            version: 1,
            sequence: identity.expected_sequence + 1,
            status: if cancelled {
                Status::Cancelled
            } else if reused {
                Status::Committed
            } else {
                Status::Pending
            },
            request: request.clone(),
            provider,
            reused,
            creates_credential,
        };
        save(&tx, &operation, None)?;
        tx.commit().map_err(|_| unknown())?;
        // Read committed bytes before the caller is permitted to touch Vault.
        self.read_import(request)?.ok_or_else(unknown)
    }
    pub(super) fn finish_import(
        &self,
        request: &ImportRequest,
        cancel: bool,
    ) -> Result<Operation, ConfigError> {
        let tx = Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)
            .map_err(|_| unknown())?;
        let previous = self.read_import(request)?.ok_or_else(unknown)?;
        if previous.status != Status::Pending {
            return Ok(previous);
        }
        let mut operation = previous.clone();
        if cancel {
            operation.status = Status::Cancelled;
        } else {
            let provider = self.write_provider(operation.provider.clone().ok_or_else(corrupt)?)?;
            let actual = self
                .providers()
                .map_err(|_| corrupt())?
                .into_iter()
                .find(|p| p.id == provider.id)
                .ok_or_else(corrupt)?;
            if encoded(&actual)? != encoded(&provider)? {
                return Err(corrupt());
            }
            operation.provider = Some(provider);
            operation.status = Status::Committed;
        }
        save(&tx, &operation, Some(&previous))?;
        tx.commit().map_err(|_| unknown())?;
        self.read_import(request)?.ok_or_else(unknown)
    }
}
