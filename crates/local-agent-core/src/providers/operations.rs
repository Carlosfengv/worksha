//! Durable metadata results and non-replayable connection attempts.
//! The clock survives bounded receipt reclamation; missing receipts never authorize a new probe.
use super::{ConfigError, Verification, fail};
use crate::{Provider, Store};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const MAX_REVISION: u64 = 9_007_199_254_740_991;
pub const RECEIPT_LIMIT: usize = 256;
pub const RECEIPT_BYTES: usize = 16 * 1024 * 1024;
pub const PENDING_LIMIT: usize = 4;
// Include both the complete 2000-model catalog and its disabled list, with JSON escaping.
pub const MAX_REQUEST_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_RESPONSE_BYTES: usize = 2 * MAX_REQUEST_BYTES + 4096;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Protocol {
    pub version: u32,
    pub store_id: String,
    pub sequence: u64,
    pub receipt_limit: usize,
    pub receipt_bytes: usize,
    pub pending_limit: usize,
    pub pending_operation_ids: Vec<String>,
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Save,
    Connect,
}
#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schemars(rename = "ProviderOperationRequest")]
pub struct Request {
    pub store_id: String,
    pub operation_id: String,
    pub expected_sequence: u64,
    pub kind: Kind,
    pub provider: Provider,
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Pending,
    Committed,
    Failed,
    Cancelled,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Failure {
    pub code: String,
    pub message: String,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Operation {
    pub api_version: u32,
    pub sequence: u64,
    pub status: Status,
    pub request: Request,
    pub provider: Option<Provider>,
    pub verification: Option<Verification>,
    pub error: Option<Failure>,
}
pub struct Start {
    /// Only the successful creator may run the external probes.
    pub started: bool,
    pub operation: Operation,
}
fn unavailable(_: impl std::fmt::Display) -> ConfigError {
    ConfigError::new(
        "service_unavailable",
        "服务商操作暂不可读取，请保留原编号核查",
    )
}
fn invalid_record(_: impl std::fmt::Display) -> ConfigError {
    ConfigError::new("invalid_record", "服务商操作记录不可读取，原记录已保留")
}
pub fn valid_id(value: &str) -> bool {
    uuid::Uuid::parse_str(value).is_ok_and(|id| {
        id.get_version_num() == 4
            && id.get_variant() == uuid::Variant::RFC4122
            && id.to_string() == value
    })
}
fn validate(request: &Request) -> Result<(), ConfigError> {
    if !valid_id(&request.store_id)
        || !valid_id(&request.operation_id)
        || request.expected_sequence >= MAX_REVISION
        || request.provider.revision >= MAX_REVISION
        || serde_json::to_vec(request).map_err(unavailable)?.len() > MAX_REQUEST_BYTES
        || !(crate::storage::default_providers()
            .iter()
            .any(|p| p.id == request.provider.id)
            || request
                .provider
                .id
                .strip_prefix("custom-")
                .is_some_and(|id| uuid::Uuid::parse_str(id).is_ok()))
    {
        return fail("invalid_input", "服务商操作编号、身份、版本或大小无效");
    }
    Ok(())
}
fn digest(request: &Request) -> Result<String, ConfigError> {
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(request).map_err(unavailable)?)
    ))
}
fn clock(db: &Connection) -> Result<u64, ConfigError> {
    let (version, sequence): (u32, i64) = db
        .query_row(
            "SELECT version,sequence FROM provider_operation_scope WHERE id=1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(invalid_record)?;
    let sequence = u64::try_from(sequence).map_err(invalid_record)?;
    if version != 1 || sequence > MAX_REVISION {
        return Err(invalid_record("clock"));
    }
    Ok(sequence)
}
pub(crate) fn initialize(db: &Connection) -> Result<(), ConfigError> {
    let tx = Transaction::new_unchecked(db, TransactionBehavior::Immediate).map_err(unavailable)?;
    let schema: u32 = tx
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .map_err(unavailable)?;
    tx.execute_batch("CREATE TABLE IF NOT EXISTS provider_operation_scope(id INTEGER PRIMARY KEY CHECK(id=1),version INTEGER NOT NULL,sequence INTEGER NOT NULL CHECK(sequence>=0));
        CREATE TABLE IF NOT EXISTS provider_operations(sequence INTEGER PRIMARY KEY,operation_id TEXT NOT NULL UNIQUE,state TEXT NOT NULL,request_hash TEXT NOT NULL,value TEXT NOT NULL CHECK(json_valid(value)));").map_err(unavailable)?;
    if schema < 9 {
        tx.execute(
            "INSERT OR IGNORE INTO provider_operation_scope VALUES(1,1,0)",
            [],
        )
        .map_err(unavailable)?;
    }
    clock(&tx)?;
    tx.commit().map_err(unavailable)
}
fn load(db: &Connection, id: &str, store_id: &str) -> Result<Option<Operation>, ConfigError> {
    let row: Option<(i64,String,String,String)> = db.query_row(
        "SELECT sequence,state,request_hash,CAST(substr(CAST(value AS BLOB),1,?2) AS TEXT) FROM provider_operations WHERE operation_id=?1", params![id,(MAX_RESPONSE_BYTES+1) as i64],
        |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)),
    ).optional().map_err(unavailable)?;
    row.map(|(sequence, state, hash, value)| {
        let sequence = u64::try_from(sequence).map_err(invalid_record)?;
        if value.len() > MAX_RESPONSE_BYTES {
            return Err(invalid_record("size"));
        }
        let operation: Operation = serde_json::from_str(&value).map_err(invalid_record)?;
        validate(&operation.request).map_err(|error| invalid_record(error.message))?;
        let expected_state = serde_json::to_value(operation.status).map_err(invalid_record)?;
        let coherent = operation.api_version == 1
            && operation.sequence == sequence
            && sequence == operation.request.expected_sequence + 1
            && sequence <= clock(db)?
            && expected_state.as_str() == Some(state.as_str())
            && operation.request.operation_id == id
            && operation.request.store_id == store_id
            && digest(&operation.request)? == hash;
        if !coherent {
            return Err(invalid_record("identity"));
        }
        match operation.status {
            Status::Committed => {
                let p = operation
                    .provider
                    .as_ref()
                    .ok_or_else(|| invalid_record("provider"))?;
                if p.id != operation.request.provider.id
                    || p.revision != operation.request.provider.revision + 1
                    || !p.configured
                    || p.models.len() > 2000
                    || operation.error.is_some()
                {
                    return Err(invalid_record("committed"));
                }
                match (operation.request.kind, &operation.verification) {
                    (Kind::Save, None) => (),
                    (Kind::Connect, Some(v))
                        if v.provider_id == p.id
                            && v.model.as_ref() == Some(&p.model)
                            && v.state == "verified"
                            && v.fingerprint == super::fingerprint(p)
                            && chrono::DateTime::parse_from_rfc3339(&v.checked_at).is_ok() => {}
                    _ => return Err(invalid_record("verification")),
                }
            }
            Status::Pending | Status::Cancelled | Status::Failed => {
                if (operation.status != Status::Cancelled
                    && operation.request.kind != Kind::Connect)
                    || operation.provider.is_some()
                    || operation.verification.is_some()
                    || (operation.status == Status::Failed) != operation.error.is_some()
                    || operation.error.as_ref().is_some_and(|e| {
                        e.code.len() > 80 || e.message.len() > 512 || e.code.is_empty()
                    })
                {
                    return Err(invalid_record("state"));
                }
            }
        }
        Ok(operation)
    })
    .transpose()
}
fn save(db: &Connection, operation: &Operation, insert: bool) -> Result<(), ConfigError> {
    let value = serde_json::to_string(operation).map_err(unavailable)?;
    if value.len() > MAX_RESPONSE_BYTES {
        return fail("capacity", "服务商操作结果超过保存上限，未提交");
    }
    let state = serde_json::to_value(operation.status).map_err(unavailable)?;
    let hash = digest(&operation.request)?;
    let changed = if insert {
        db.execute("INSERT INTO provider_operations VALUES(?1,?2,?3,?4,?5)",params![operation.sequence as i64, operation.request.operation_id, state.as_str(), hash, value])
    } else {
        db.execute("UPDATE provider_operations SET state=?1,value=?2 WHERE operation_id=?3 AND sequence=?4 AND request_hash=?5 AND state='pending'",params![state.as_str(),value,operation.request.operation_id,operation.sequence as i64,hash])
    }.map_err(unavailable)?;
    if changed != 1 {
        return Err(unavailable("write ignored"));
    }
    Ok(())
}
fn reclaim(db: &Connection, store_id: &str) -> Result<(), ConfigError> {
    loop {
        let (count,bytes): (i64,i64) = db.query_row("SELECT count(*),COALESCE(sum(length(CAST(value AS BLOB))),0) FROM provider_operations WHERE state!='pending'",[],|r|Ok((r.get(0)?,r.get(1)?))).map_err(unavailable)?;
        if count < 0 || bytes < 0 {
            return Err(invalid_record("capacity"));
        }
        if count <= RECEIPT_LIMIT as i64 && bytes <= RECEIPT_BYTES as i64 {
            return Ok(());
        }
        let id: String = db.query_row("SELECT operation_id FROM provider_operations WHERE state!='pending' ORDER BY sequence LIMIT 1",[],|r|r.get(0)).map_err(unavailable)?;
        let oldest = load(db, &id, store_id)?.ok_or_else(|| invalid_record("reclamation"))?;
        if oldest.status == Status::Pending {
            return Err(invalid_record("pending reclamation"));
        }
        let changed = db
            .execute(
                "DELETE FROM provider_operations WHERE operation_id=?1",
                [id],
            )
            .map_err(unavailable)?;
        if changed != 1 {
            return Err(unavailable("reclamation ignored"));
        }
    }
}
impl Store {
    pub fn provider_write_protocol(&self) -> Result<Protocol, ConfigError> {
        let schema: u32 = self
            .connection
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .map_err(unavailable)?;
        if !matches!(
            schema,
            Store::SCHEMA_VERSION | crate::iam::PROTECTED_SCHEMA_VERSION
        ) {
            return fail(
                "write_protocol_unavailable",
                "当前数据库不支持服务商结果核查",
            );
        }
        let mut statement=self.connection.prepare("SELECT operation_id FROM provider_operations WHERE state='pending' ORDER BY sequence LIMIT ?1").map_err(unavailable)?;
        let store_id = self.database_id()?;
        if !valid_id(&store_id) {
            return Err(invalid_record("store identity"));
        }
        let pending_operation_ids = statement
            .query_map([(PENDING_LIMIT + 1) as i64], |r| r.get::<_, String>(0))
            .map_err(unavailable)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(invalid_record)?;
        if pending_operation_ids.len() > PENDING_LIMIT
            || pending_operation_ids.iter().any(|id| !valid_id(id))
        {
            return Err(invalid_record("pending identities"));
        }
        Ok(Protocol {
            version: 1,
            store_id,
            sequence: clock(&self.connection)?,
            receipt_limit: RECEIPT_LIMIT,
            receipt_bytes: RECEIPT_BYTES,
            pending_limit: PENDING_LIMIT,
            pending_operation_ids,
        })
    }
    fn provider_operation_scope(&self, store_id: &str) -> Result<Protocol, ConfigError> {
        let protocol = self.provider_write_protocol()?;
        if !valid_id(store_id) || protocol.store_id != store_id {
            return fail(
                "operation_store_changed",
                "操作属于另一个本机数据库，请连接原后台核查",
            );
        }
        Ok(protocol)
    }
    pub fn provider_operation(
        &self,
        store_id: &str,
        operation_id: &str,
    ) -> Result<Operation, ConfigError> {
        self.provider_operation_scope(store_id)?;
        if !valid_id(operation_id) {
            return fail("invalid_input", "操作编号无效");
        }
        load(&self.connection, operation_id, store_id)?.ok_or_else(|| {
            ConfigError::new(
                "operation_not_found",
                "未查到原操作；不能据此判断从未执行，请保留原请求核查",
            )
        })
    }
    pub fn existing_provider_operation(
        &self,
        request: &Request,
    ) -> Result<Option<Operation>, ConfigError> {
        validate(request)?;
        self.provider_operation_scope(&request.store_id)?;
        let operation = load(&self.connection, &request.operation_id, &request.store_id)?;
        if let Some(operation) = &operation
            && digest(&operation.request)? != digest(request)?
        {
            return fail(
                "operation_conflict",
                "同一操作编号对应不同输入，未重新保存或探测",
            );
        }
        Ok(operation)
    }
    pub fn start_provider_operation(&self, request: &Request) -> Result<Start, ConfigError> {
        self.start_provider_operation_before_commit(request, |_| Ok(()))
    }
    /// Only host-owned metadata may be staged here. The existing SQL outcome,
    /// revision and receipt still commit together; no network or Vault work.
    pub(crate) fn start_provider_operation_before_commit(
        &self,
        request: &Request,
        before_commit: impl FnOnce(&Operation) -> Result<(), ConfigError>,
    ) -> Result<Start, ConfigError> {
        validate(request)?;
        let tx = Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)
            .map_err(unavailable)?;
        let protocol = self.provider_operation_scope(&request.store_id)?;
        if let Some(operation) = load(&tx, &request.operation_id, &request.store_id)? {
            if digest(&operation.request)? != digest(request)? {
                return fail(
                    "operation_conflict",
                    "同一操作编号对应不同输入，未重新保存或探测",
                );
            }
            return Ok(Start {
                started: false,
                operation,
            });
        }
        if request.expected_sequence != protocol.sequence {
            return fail(
                "operation_expired",
                "操作序号已过期，未重新保存或探测；请核查原结果",
            );
        }
        let pending: i64 = tx
            .query_row(
                "SELECT count(*) FROM provider_operations WHERE state='pending'",
                [],
                |r| r.get(0),
            )
            .map_err(unavailable)?;
        if pending >= PENDING_LIMIT as i64 {
            return fail("capacity", "待核查连接已达上限，请先查询或取消已有操作");
        }
        let occupied: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM provider_operations WHERE state='pending' AND json_extract(value,'$.request.provider.id')=?1)",[&request.provider.id],|r|r.get(0)).map_err(unavailable)?;
        if occupied {
            return fail(
                "operation_in_progress",
                "该服务商已有待核查连接，请先查询或取消原操作",
            );
        }
        let current = self
            .providers()
            .map_err(unavailable)?
            .into_iter()
            .find(|p| p.id == request.provider.id);
        if current.as_ref().map_or(0, |p| p.revision) != request.provider.revision {
            return fail("conflict", "连接版本已改变，请重新读取");
        }
        if tx.execute("UPDATE provider_operation_scope SET sequence=sequence+1 WHERE id=1 AND version=1 AND sequence=?1",[protocol.sequence as i64]).map_err(unavailable)? != 1 { return Err(unavailable("clock ignored")); }
        let mut operation = Operation {
            api_version: 1,
            sequence: protocol.sequence + 1,
            status: Status::Pending,
            request: request.clone(),
            provider: None,
            verification: None,
            error: None,
        };
        if request.kind == Kind::Save {
            operation.provider = Some(self.write_provider(request.provider.clone())?);
            operation.status = Status::Committed;
        }
        save(&tx, &operation, true)?;
        reclaim(&tx, &request.store_id)?;
        before_commit(&operation)?;
        tx.commit().map_err(unavailable)?;
        Ok(Start {
            started: request.kind == Kind::Connect,
            operation,
        })
    }
    pub fn finish_provider_connection(
        &self,
        request: &Request,
        detected: Provider,
        checked_at: String,
        record_verification: impl FnOnce(&Verification) -> Result<(), ConfigError>,
    ) -> Result<Operation, ConfigError> {
        validate(request)?;
        let tx = Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)
            .map_err(unavailable)?;
        self.provider_operation_scope(&request.store_id)?;
        let mut operation = load(&tx, &request.operation_id, &request.store_id)?
            .ok_or_else(|| invalid_record("missing pending"))?;
        if digest(&operation.request)? != digest(request)? {
            return fail("operation_conflict", "原操作输入不一致，未保存连接");
        }
        if operation.status != Status::Pending {
            return Ok(operation);
        }
        if request.kind != Kind::Connect
            || detected.id != request.provider.id
            || detected.revision != request.provider.revision
            || detected.credential_ref != request.provider.credential_ref
            || chrono::DateTime::parse_from_rfc3339(&checked_at).is_err()
        {
            return fail("invalid_input", "已验证连接与原操作不一致，未保存");
        }
        let provider = self.write_provider(detected)?;
        let verification = Verification {
            provider_id: provider.id.clone(),
            model: Some(provider.model.clone()),
            fingerprint: super::fingerprint(&provider),
            state: "verified".into(),
            checked_at,
            message: "已自动识别并完成短请求验证；工具、结构化输出及完整容量仍以各自验证为准"
                .into(),
        };
        record_verification(&verification)?;
        operation.status = Status::Committed;
        operation.provider = Some(provider);
        operation.verification = Some(verification);
        save(&tx, &operation, false)?;
        reclaim(&tx, &request.store_id)?;
        tx.commit().map_err(unavailable)?;
        Ok(operation)
    }
    pub fn fail_provider_connection(
        &self,
        request: &Request,
        error: &ConfigError,
    ) -> Result<Operation, ConfigError> {
        validate(request)?;
        self.end_provider_connection(request, Some(error))
    }
    pub fn cancel_provider_connection(&self, request: &Request) -> Result<Operation, ConfigError> {
        validate(request)?;
        self.end_provider_connection(request, None)
    }
    fn end_provider_connection(
        &self,
        request: &Request,
        failure: Option<&ConfigError>,
    ) -> Result<Operation, ConfigError> {
        let tx = Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)
            .map_err(unavailable)?;
        let protocol = self.provider_operation_scope(&request.store_id)?;
        let mut operation = match load(&tx, &request.operation_id, &request.store_id)? {
            Some(operation) => operation,
            None if failure.is_none() && protocol.sequence == request.expected_sequence => {
                // Cancellation can win before an enqueued POST arrives. Its tombstone
                // consumes the same sequence, so that POST cannot begin later.
                if tx.execute("UPDATE provider_operation_scope SET sequence=sequence+1 WHERE id=1 AND version=1 AND sequence=?1",[protocol.sequence as i64]).map_err(unavailable)? != 1 {return Err(unavailable("cancel clock"));}
                let operation = Operation {
                    api_version: 1,
                    sequence: protocol.sequence + 1,
                    status: Status::Cancelled,
                    request: request.clone(),
                    provider: None,
                    verification: None,
                    error: None,
                };
                save(&tx, &operation, true)?;
                reclaim(&tx, &request.store_id)?;
                tx.commit().map_err(unavailable)?;
                return Ok(operation);
            }
            None => {
                return fail(
                    "operation_not_found",
                    "未查到原操作；可能已回收，不能确认取消，请保留原请求核查",
                );
            }
        };
        if digest(&operation.request)? != digest(request)? {
            return fail("operation_conflict", "原操作输入不一致");
        }
        if operation.status != Status::Pending {
            return Ok(operation);
        }
        if let Some(error) = failure {
            if error.code.is_empty() || error.code.len() > 80 || error.message.len() > 512 {
                return Err(invalid_record("failure"));
            }
            operation.status = Status::Failed;
            operation.error = Some(Failure {
                code: error.code.clone(),
                message: error.message.clone(),
            });
        } else {
            operation.status = Status::Cancelled;
        }
        save(&tx, &operation, false)?;
        reclaim(&tx, &request.store_id)?;
        tx.commit().map_err(unavailable)?;
        Ok(operation)
    }
}

#[cfg(test)]
mod tests;
