use super::{ConfigError, Store, create_binding, db, db_error, decode, encode, fail};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::{Deserialize, Serialize};

pub const AGENT: &str = "local-chat-completions";
pub(super) const SCOPE: &str = "local-chat";

pub(crate) fn initialize(conn: &Connection) -> Result<(), ConfigError> {
    conn.execute_batch(
        "BEGIN IMMEDIATE;
         CREATE TABLE IF NOT EXISTS gateway_chat_keys(
             binding TEXT PRIMARY KEY REFERENCES gateway_bindings(id), name TEXT NOT NULL,
             version INTEGER NOT NULL CHECK(version>0), digest TEXT,
             CHECK(digest IS NULL OR length(digest)=64));
         CREATE TABLE IF NOT EXISTS gateway_chat_operations(
             id TEXT PRIMARY KEY, request TEXT NOT NULL CHECK(json_valid(request)),
             receipt TEXT NOT NULL CHECK(json_valid(receipt)));
         CREATE TABLE IF NOT EXISTS gateway_chat_sends(
             observation TEXT PRIMARY KEY, binding TEXT NOT NULL REFERENCES gateway_bindings(id), version INTEGER NOT NULL);
         COMMIT;",
    )
    .map_err(db_error)
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Operation {
    pub store_id: String,
    pub operation_id: String,
    pub action: Action,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum Action {
    Create {
        name: String,
        service_id: String,
        service_version: String,
        key_digest: String,
    },
    Rotate {
        binding_id: String,
        expected_key_version: u32,
        key_digest: String,
    },
    Revoke {
        binding_id: String,
        expected_key_version: u32,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum OperationState {
    Pending,
    Active,
    Cancelled,
    Revoked,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Receipt {
    pub operation_id: String,
    pub binding_id: String,
    pub key_version: u32,
    pub state: OperationState,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub binding_id: String,
    pub name: String,
    pub key_version: u32,
    pub revoked: bool,
    pub service_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OperationQuery {
    pub store_id: String,
    pub operation_id: String,
}

#[derive(Clone)]
pub struct SendFence {
    pub binding_id: String,
    pub key_version: u32,
}

fn uuid(s: &str) -> bool {
    uuid::Uuid::parse_str(s).is_ok_and(|id| id.get_version_num() == 4 && id.to_string() == s)
}

fn digest(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || matches!(b, b'a'..=b'f'))
}

fn lookup(conn: &Connection, id: &str) -> Result<Option<(Operation, Receipt)>, ConfigError> {
    let row: Option<(String, String)> = conn
        .query_row(
            "SELECT request,receipt FROM gateway_chat_operations WHERE id=?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(db_error)?;
    row.map(|(query, receipt)| Ok((decode(&query)?, decode(&receipt)?)))
        .transpose()
}

fn key(conn: &Connection, binding: &str) -> Result<Option<(u32, Option<String>)>, ConfigError> {
    conn.query_row(
        "SELECT version,digest FROM gateway_chat_keys WHERE binding=?1",
        [binding],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .optional()
    .map_err(db_error)
}

fn require_version(conn: &Connection, binding: &str, expected: u32) -> Result<(), ConfigError> {
    match key(conn, binding)? {
        Some((v, Some(_))) if v == expected => Ok(()),
        _ => fail("chat_key_changed", "本机绑定的当前 Key 已变化或已撤销"),
    }
}

impl Store {
    fn chat_store(&self, store: &str, op: &str) -> Result<(), ConfigError> {
        if !uuid(op) || self.database_id()? != store {
            return fail("conflict", "操作编号或本机数据目录已变化");
        }
        Ok(())
    }

    pub fn chat_bindings(&self) -> Result<Vec<Entry>, ConfigError> {
        let mut stmt = self
            .connection
            .prepare(
                "SELECT k.binding,k.name,k.version,k.digest IS NULL,b.service
             FROM gateway_chat_keys k JOIN model_service_bindings b ON b.binding=k.binding
             ORDER BY k.name,k.binding",
            )
            .map_err(db_error)?;
        let rows = stmt
            .query_map([], |r| {
                Ok(Entry {
                    binding_id: r.get(0)?,
                    name: r.get(1)?,
                    key_version: r.get(2)?,
                    revoked: r.get(3)?,
                    service_id: r.get(4)?,
                })
            })
            .map_err(db_error)?;
        rows.collect::<Result<_, _>>().map_err(db_error)
    }

    pub fn chat_pending_operations(&self) -> Result<Vec<Receipt>, ConfigError> {
        let mut stmt = self.connection.prepare(
            "SELECT receipt FROM gateway_chat_operations WHERE json_extract(receipt,'$.state')='pending' ORDER BY id"
        ).map_err(db_error)?;
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(db_error)?;
        rows.map(|row| decode(&row.map_err(db_error)?)).collect()
    }

    pub fn chat_key_version(
        &self,
        binding: &str,
        candidate: &str,
    ) -> Result<Option<u32>, ConfigError> {
        let Some((version, Some(expected))) = key(&self.connection, binding)? else {
            return Ok(None);
        };
        Ok((candidate.len() == expected.len()
            && candidate
                .bytes()
                .zip(expected.bytes())
                .fold(0, |difference, (a, b)| difference | (a ^ b))
                == 0)
            .then_some(version))
    }

    pub fn chat_key_current(&self, binding: &str, version: u32) -> Result<(), ConfigError> {
        require_version(&self.connection, binding, version)
    }

    pub(crate) fn chat_mark_sending(
        &self,
        fence: &SendFence,
        observation: &str,
    ) -> Result<(), ConfigError> {
        self.chat_key_current(&fence.binding_id, fence.key_version)?;
        self.connection
            .execute(
                "INSERT INTO gateway_chat_sends VALUES(?1,?2,?3)",
                params![observation, fence.binding_id, fence.key_version],
            )
            .map_err(db_error)?;
        self.connection
            .execute(
                "DELETE FROM gateway_chat_sends WHERE rowid <= (
          SELECT rowid FROM gateway_chat_sends ORDER BY rowid DESC LIMIT 1 OFFSET 8192)",
                [],
            )
            .map_err(db_error)?;
        Ok(())
    }

    pub fn chat_operation(&self, query: &OperationQuery) -> Result<Receipt, ConfigError> {
        self.chat_store(&query.store_id, &query.operation_id)?;
        lookup(&self.connection, &query.operation_id)?
            .map(|(_, receipt)| receipt)
            .ok_or_else(|| ConfigError::new("not_found", "未找到此本机连接操作"))
    }

    pub fn chat_prepare(&self, request: &Operation) -> Result<Receipt, ConfigError> {
        self.chat_store(&request.store_id, &request.operation_id)?;
        let tx = Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)
            .map_err(db_error)?;
        if let Some((saved, receipt)) = lookup(&tx, &request.operation_id)? {
            if &saved != request {
                return fail("conflict", "同一操作编号的参数已变化");
            }
            return Ok(receipt);
        }
        let count: i64 = tx
            .query_row("SELECT count(*) FROM gateway_chat_operations", [], |r| {
                r.get(0)
            })
            .map_err(db_error)?;
        if count >= 8192 {
            return fail("capacity", "本机连接操作记录已达上限");
        }
        let (binding, version, state) = match &request.action {
            Action::Create {
                name,
                service_id,
                service_version,
                key_digest,
            } => {
                if name.trim().is_empty()
                    || name.len() > 160
                    || name.chars().any(char::is_control)
                    || !digest(key_digest)
                    || !uuid(service_id)
                    || !uuid(service_version)
                {
                    return fail("invalid_input", "本机连接名称、模型服务或 Key 摘要无效");
                }
                current(&tx, service_id, service_version)?;
                (request.operation_id.clone(), 1, OperationState::Pending)
            }
            Action::Rotate {
                binding_id,
                expected_key_version,
                key_digest,
            } => {
                if !digest(key_digest) || !uuid(binding_id) {
                    return fail("invalid_input", "绑定 ID 或 Key 摘要无效");
                }
                require_version(&tx, binding_id, *expected_key_version)?;
                let next = expected_key_version
                    .checked_add(1)
                    .ok_or_else(|| ConfigError::new("capacity", "本机 Key 版本已达上限"))?;
                (binding_id.clone(), next, OperationState::Pending)
            }
            Action::Revoke {
                binding_id,
                expected_key_version,
            } => {
                require_version(&tx, binding_id, *expected_key_version)?;
                tx.execute(
                    "UPDATE gateway_chat_keys SET digest=NULL WHERE binding=?1",
                    [binding_id],
                )
                .map_err(db_error)?;
                (
                    binding_id.clone(),
                    *expected_key_version,
                    OperationState::Revoked,
                )
            }
        };
        let receipt = Receipt {
            operation_id: request.operation_id.clone(),
            binding_id: binding,
            key_version: version,
            state,
        };
        tx.execute(
            "INSERT INTO gateway_chat_operations VALUES (?1,?2,?3)",
            params![request.operation_id, encode(request)?, encode(&receipt)?],
        )
        .map_err(db_error)?;
        tx.commit().map_err(db_error)?;
        Ok(receipt)
    }

    pub fn chat_finish(
        &self,
        query: &OperationQuery,
        verified_digest: Option<&str>,
    ) -> Result<Receipt, ConfigError> {
        self.chat_store(&query.store_id, &query.operation_id)?;
        let tx = Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)
            .map_err(db_error)?;
        let Some((request, mut receipt)) = lookup(&tx, &query.operation_id)? else {
            return fail("not_found", "未找到此本机连接操作");
        };
        if receipt.state != OperationState::Pending {
            return Ok(receipt);
        }
        if let Some(proof) = verified_digest {
            match request.action {
                Action::Create {
                    name,
                    service_id,
                    service_version,
                    key_digest,
                } if proof == key_digest => {
                    let service = current(&tx, &service_id, &service_version)?;
                    let model = service
                        .models
                        .iter()
                        .find(|m| m.id == service.default_model_id)
                        .ok_or_else(|| db("missing service default model"))?;
                    let binding = create_binding(
                        &tx,
                        &receipt.binding_id,
                        AGENT,
                        SCOPE,
                        &receipt.binding_id,
                        &model.provider,
                    )?;
                    let id = &binding.id;
                    crate::model_services::attach_binding(&tx, id, &service)?;
                    tx.execute(
                        "INSERT INTO gateway_chat_keys VALUES (?1,?2,1,?3)",
                        params![id, name.trim(), key_digest],
                    )
                    .map_err(db_error)?;
                    super::verify_binding(&tx, &binding)?;
                }
                Action::Rotate {
                    binding_id,
                    expected_key_version,
                    key_digest,
                } if proof == key_digest => {
                    require_version(&tx, &binding_id, expected_key_version)?;
                    tx.execute(
                        "UPDATE gateway_chat_keys SET version=?2,digest=?3 WHERE binding=?1",
                        params![binding_id, receipt.key_version, key_digest],
                    )
                    .map_err(db_error)?;
                }
                _ => return fail("conflict", "连接文件摘要与待激活操作不一致"),
            }
            receipt.state = OperationState::Active;
        } else {
            receipt.state = OperationState::Cancelled;
        }
        tx.execute(
            "UPDATE gateway_chat_operations SET receipt=?2 WHERE id=?1",
            params![query.operation_id, encode(&receipt)?],
        )
        .map_err(db_error)?;
        tx.commit().map_err(db_error)?;
        Ok(receipt)
    }
}

fn current(
    conn: &Connection,
    id: &str,
    version: &str,
) -> Result<crate::model_services::ModelService, ConfigError> {
    let service = crate::model_services::current(conn, id)?;
    if service.version_id != version {
        return fail("conflict", "模型服务版本或启用状态已变化");
    }
    Ok(service)
}
