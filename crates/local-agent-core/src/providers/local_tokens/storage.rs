use super::*;
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};

fn db(_: impl std::fmt::Display) -> ConfigError {
    corrupt()
}

pub(crate) fn initialize(
    tx: &Transaction<'_>,
    store_id: &str,
    namespace_id: &str,
) -> Result<(), ConfigError> {
    tx.execute_batch("CREATE TABLE local_token_scope(id INTEGER PRIMARY KEY CHECK(id=1),format INTEGER NOT NULL,store_id TEXT NOT NULL,namespace_id TEXT NOT NULL,owner_count INTEGER NOT NULL,owner_bytes INTEGER NOT NULL,pending_count INTEGER NOT NULL,value_count INTEGER NOT NULL,revision INTEGER NOT NULL,cancellation_count INTEGER NOT NULL DEFAULT 0,cancellation_bytes INTEGER NOT NULL DEFAULT 0);
        CREATE TABLE local_token_values(domain INTEGER NOT NULL CHECK(domain=4),name TEXT NOT NULL,value BLOB NOT NULL CHECK(typeof(value)='blob' AND length(value)<=4096),value_hash TEXT NOT NULL,revision INTEGER NOT NULL,PRIMARY KEY(domain,name));
        CREATE TABLE local_token_owners(operation_id TEXT PRIMARY KEY,state TEXT NOT NULL,request_hash TEXT NOT NULL,value_hash TEXT NOT NULL,value_bytes INTEGER NOT NULL,value TEXT NOT NULL CHECK(json_valid(value)));").map_err(db)?;
    if tx
        .execute(
            "INSERT INTO local_token_scope(id,format,store_id,namespace_id,owner_count,owner_bytes,pending_count,value_count,revision) VALUES(1,2,?1,?2,0,0,0,0,0)",
            params![store_id, namespace_id],
        )
        .map_err(db)?
        != 1
    {
        return Err(corrupt());
    }
    cancellations::initialize(tx)?;
    scope(tx, store_id, namespace_id)?;
    Ok(())
}

pub(crate) fn upgrade_previous(
    tx: &Transaction<'_>,
    store_id: &str,
    namespace_id: &str,
) -> Result<(), ConfigError> {
    let old = ownership_scope(tx, store_id, namespace_id, 1)?;
    value_metadata(tx, &old)?;
    tx.execute_batch("ALTER TABLE local_token_scope ADD COLUMN cancellation_count INTEGER NOT NULL DEFAULT 0; ALTER TABLE local_token_scope ADD COLUMN cancellation_bytes INTEGER NOT NULL DEFAULT 0;").map_err(db)?;
    cancellations::initialize(tx)?;
    if tx
        .execute(
            "UPDATE local_token_scope SET format=2 WHERE id=1 AND format=1",
            [],
        )
        .map_err(db)?
        != 1
    {
        return Err(corrupt());
    }
    scope(tx, store_id, namespace_id)?;
    Ok(())
}

/// Return the separate cryptographic journal clock/count. API-key discovery
/// continues to read only credential_recovery_values and its original clock.
pub(crate) fn crypto_metadata(
    db: &Connection,
    store_id: &str,
    namespace_id: &str,
) -> Result<(i64, i64), ConfigError> {
    let counters = scope(db, store_id, namespace_id)?;
    value_metadata(db, &counters)
}
fn value_metadata(db: &Connection, counters: &Counters) -> Result<(i64, i64), ConfigError> {
    let (count, revision) = (counters.values, counters.revision);
    let actual: i64 = db
        .query_row(
            "SELECT count(*) FROM (SELECT 1 FROM local_token_values LIMIT 4098)",
            [],
            |r| r.get(0),
        )
        .map_err(self::db)?;
    let invalid:i64=db.query_row("SELECT count(*) FROM local_token_values WHERE domain!=4 OR typeof(value)!='blob' OR length(value)>4096 OR length(value_hash)!=64 OR revision<1 OR revision>?1",[revision],|r|r.get(0)).map_err(self::db)?;
    if count != actual
        || !(0..=4097).contains(&count)
        || !(0..=9_007_199_254_740_991).contains(&revision)
        || invalid != 0
    {
        return Err(corrupt());
    }
    Ok((count, revision))
}
struct Counters {
    owners: i64,
    bytes: i64,
    pending: i64,
    values: i64,
    revision: i64,
}
fn scope(db: &Connection, store_id: &str, namespace_id: &str) -> Result<Counters, ConfigError> {
    let counters = ownership_scope(db, store_id, namespace_id, 2)?;
    cancellations::metadata(db, store_id, namespace_id)?;
    Ok(counters)
}
pub(super) fn validate_scope(
    db: &Connection,
    store_id: &str,
    namespace_id: &str,
) -> Result<(), ConfigError> {
    scope(db, store_id, namespace_id).map(|_| ())
}
fn ownership_scope(
    db: &Connection,
    store_id: &str,
    namespace_id: &str,
    expected_format: u32,
) -> Result<Counters, ConfigError> {
    let (format,actual_store,actual_namespace,count,bytes,pending_count,values,revision):(u32,String,String,i64,i64,i64,i64,i64)=db.query_row("SELECT format,store_id,namespace_id,owner_count,owner_bytes,pending_count,value_count,revision FROM local_token_scope WHERE id=1",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?,r.get(7)?))).map_err(self::db)?;
    let scopes: i64 = db
        .query_row("SELECT count(*) FROM local_token_scope", [], |r| r.get(0))
        .map_err(self::db)?;
    let (source_store, source_namespace): (String, String) = db
        .query_row(
            "SELECT store_id,namespace_id FROM credential_recovery_scope WHERE id=1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(self::db)?;
    // Check bounded metadata without parsing every historical Provider catalog.
    // The requested row is separately byte-limited, hashed and fully decoded.
    let (actual,size,pending,invalid):(i64,i64,i64,i64)=db.query_row("SELECT count(*),COALESCE(sum(value_bytes),0),COALESCE(sum(state='pending'),0),COALESCE(sum(state NOT IN('pending','committed','cancelled') OR value_bytes<0 OR value_bytes>2097152 OR length(value_hash)!=64 OR length(request_hash)!=64 OR length(operation_id)!=36),0) FROM (SELECT operation_id,state,request_hash,value_hash,value_bytes FROM local_token_owners LIMIT 4097)",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).map_err(self::db)?;
    if scopes != 1
        || format != expected_format
        || !valid_id(store_id)
        || !valid_id(namespace_id)
        || actual_store != store_id
        || actual_namespace != namespace_id
        || source_store != store_id
        || source_namespace != namespace_id
        || !(0..=LIMIT as i64).contains(&count)
        || count != actual
        || bytes != size
        || !(0..=BYTE_LIMIT as i64).contains(&bytes)
        || !(0..=PENDING_LIMIT as i64).contains(&pending)
        || pending_count != pending
        || bytes
            .checked_add(pending * PENDING_RESERVE as i64)
            .is_none_or(|n| n > BYTE_LIMIT as i64)
        || invalid != 0
    {
        return Err(corrupt());
    }
    Ok(Counters {
        owners: count,
        bytes,
        pending,
        values,
        revision,
    })
}
fn validate_stored(value: &Stored, identity: &IdentityV3) -> Result<(), ConfigError> {
    let operation = &value.operation;
    validate(&operation.request)?;
    if encoded(&operation.request.identity)? != encoded(identity)?
        || operation.version != 1
        || operation.owner_id != identity.operation_id
        || !valid_id(&operation.credential_ref)
        || operation
            .preview_id
            .as_ref()
            .is_some_and(|id| id != &identity.operation_id)
        || operation.status == Status::Committed
            && (!operation.owner_saved || operation.preview_id.is_none())
        || operation.status == Status::Pending && operation.preview_id.is_some()
        || value
            .protected_token
            .as_ref()
            .is_some_and(|v| v.len() != 64)
        || operation.owner_saved && value.protected_token.is_none()
    {
        return Err(corrupt());
    }
    Ok(())
}
fn load(db: &Connection, identity: &IdentityV3) -> Result<Option<Stored>, ConfigError> {
    let row:Option<(String,String,String,i64,String)>=db.query_row("SELECT state,request_hash,value_hash,value_bytes,CAST(substr(CAST(value AS BLOB),1,2097153) AS TEXT) FROM local_token_owners WHERE operation_id=?1",[&identity.operation_id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).optional().map_err(self::db)?;
    row.map(|(state, request_hash, value_hash, value_bytes, raw)| {
        if raw.len() > RECORD_LIMIT
            || raw.len() as i64 != value_bytes
            || files::hash(raw.as_bytes()) != value_hash
        {
            return Err(corrupt());
        }
        let value: Stored = serde_json::from_str(&raw).map_err(self::db)?;
        validate_stored(&value, identity)?;
        if encoded(&value.operation.status)? != format!("\"{state}\"")
            || files::hash(encoded(&value.operation.request)?.as_bytes()) != request_hash
        {
            return Err(corrupt());
        }
        Ok(value)
    })
    .transpose()
}
fn write(db: &Connection, value: &Stored, added: bool) -> Result<(), ConfigError> {
    let operation = &value.operation;
    let identity = &operation.request.identity;
    validate_stored(value, identity)?;
    let counters = scope(db, &identity.store_id, &identity.namespace_id)?;
    let (count, bytes) = (counters.owners, counters.bytes);
    let (old_bytes, old_pending): (i64, i64) = if added {
        (0, 0)
    } else {
        db.query_row(
            "SELECT value_bytes,state='pending' FROM local_token_owners WHERE operation_id=?1",
            [&identity.operation_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(self::db)?
    };
    let raw = encoded(value)?;
    if raw.len() > RECORD_LIMIT || count + i64::from(added) > LIMIT as i64 {
        return fail(
            "capacity",
            "本机令牌恢复记录已达上限；原材料保留，未创建新令牌",
        );
    }
    let state = encoded(&operation.status)?;
    let state = state.trim_matches('"');
    let hash = files::hash(raw.as_bytes());
    let request_hash = files::hash(encoded(&operation.request)?.as_bytes());
    let changed=if added {db.execute("INSERT INTO local_token_owners VALUES(?1,?2,?3,?4,?5,?6)",params![identity.operation_id,state,request_hash,hash,raw.len() as i64,raw])} else {db.execute("UPDATE local_token_owners SET state=?1,value_hash=?2,value=?3,value_bytes=?6 WHERE operation_id=?4 AND request_hash=?5",params![state,hash,raw,identity.operation_id,request_hash,raw.len() as i64])}.map_err(self::db)?;
    let pending = counters.pending - old_pending + i64::from(operation.status == Status::Pending);
    if changed!=1 || db.execute("UPDATE local_token_scope SET owner_count=?1,owner_bytes=?2,pending_count=?3 WHERE id=1 AND owner_count=?4 AND owner_bytes=?5 AND pending_count=?6",params![count+i64::from(added),bytes-old_bytes+raw.len() as i64,pending,count,bytes,counters.pending]).map_err(self::db)?!=1 {return Err(corrupt());}
    scope(db, &identity.store_id, &identity.namespace_id)?;
    let saved: String = db
        .query_row(
            "SELECT value FROM local_token_owners WHERE operation_id=?1",
            [&identity.operation_id],
            |r| r.get(0),
        )
        .map_err(self::db)?;
    if saved != raw || encoded(&load(db, identity)?.ok_or_else(corrupt)?)? != raw {
        return Err(corrupt());
    }
    Ok(())
}
pub(crate) fn save_binding_owner(
    db: &Connection,
    operation: &Operation,
) -> Result<(), ConfigError> {
    let mut saved = load(db, &operation.request.identity)?.ok_or_else(corrupt)?;
    if encoded(&saved.operation)? != encoded(operation)?
        || saved.operation.status != Status::Pending
        || saved.protected_token.is_none()
    {
        return Err(corrupt());
    }
    let (bytes, hash): (Vec<u8>, String) = db
        .query_row(
            "SELECT value,value_hash FROM local_token_values WHERE domain=4 AND name=?1",
            [format!(
                "operations/{}.json",
                operation.request.identity.operation_id
            )],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(self::db)?;
    if files::hash(&bytes) != hash {
        return Err(corrupt());
    }
    let credential: serde_json::Value = serde_json::from_slice(&bytes).map_err(self::db)?;
    if credential["version"] != 3
        || credential["storeId"] != operation.request.identity.store_id
        || credential["namespaceId"] != operation.request.identity.namespace_id
        || credential["operation"]["state"] != "ready"
        || credential["operation"]["reference"] != operation.credential_ref
    {
        return Err(corrupt());
    }
    saved.operation.owner_saved = true;
    write(db, &saved, false)
}
impl Store {
    /// Read ownership metadata only; does not read Vault or resubmit creation.
    pub fn local_token_record(
        &self,
        identity: &IdentityV3,
    ) -> Result<Option<Operation>, ConfigError> {
        let source = self.credential_scope(&identity.store_id)?;
        if source.journal.namespace_id() != identity.namespace_id
            || !valid_id(&identity.operation_id)
        {
            return Err(corrupt());
        }
        source.journal.domain(DOMAIN).status()?;
        let stored = self.read_local_token(identity)?;
        if stored.as_ref().is_some_and(|s| {
            s.operation.credential_ref
                != source
                    .journal
                    .domain(DOMAIN)
                    .reference(&identity.operation_id)
        }) {
            return Err(corrupt());
        }
        Ok(stored.map(|s| s.operation))
    }
    pub(super) fn begin_local_token(
        &self,
        request: Request,
        reference: String,
    ) -> Result<Stored, ConfigError> {
        let tx = Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)
            .map_err(db)?;
        let identity = &request.identity;
        scope(&tx, &identity.store_id, &identity.namespace_id)?;
        if cancellations::read(&tx, identity)?.is_some() {
            return fail(
                "local_token_cancelled",
                "原编号已阻止后续创建，已有本机入口材料保留",
            );
        }
        if let Some(value) = load(&tx, identity)? {
            if encoded(&value.operation.request)? != encoded(&request)?
                || value.operation.credential_ref != reference
            {
                return fail(
                    "operation_conflict",
                    "原本机令牌编号已用于其他输入，未创建新令牌",
                );
            }
            return Ok(value);
        }
        let pending = self.pending_local_tokens(&identity.store_id, &identity.namespace_id)?;
        if pending.iter().any(|o| {
            o.request.agent_id == request.agent_id
                && o.request.configuration_path == request.configuration_path
                && o.request.purpose == request.purpose
        }) {
            return fail(
                "local_token_recovery_required",
                "此用途有待核查的原本机入口，请先核查原操作或明确停止；未重新生成令牌",
            );
        }
        if pending.len() >= PENDING_LIMIT {
            return fail("capacity", "同时有 4 项待核查的本机令牌，请先处理原操作");
        }
        let value = Stored {
            operation: Operation {
                version: 1,
                status: Status::Pending,
                owner_id: identity.operation_id.clone(),
                credential_ref: reference,
                owner_saved: false,
                preview_id: None,
                request,
            },
            protected_token: None,
        };
        write(&tx, &value, true)?;
        tx.commit().map_err(db)?;
        Ok(value)
    }
    pub(super) fn read_local_token(
        &self,
        identity: &IdentityV3,
    ) -> Result<Option<Stored>, ConfigError> {
        let tx = self.connection.unchecked_transaction().map_err(db)?;
        scope(&tx, &identity.store_id, &identity.namespace_id)?;
        let value = load(&tx, identity)?;
        tx.commit().map_err(db)?;
        Ok(value)
    }
    pub(super) fn replace_local_token(&self, value: &Stored) -> Result<(), ConfigError> {
        let tx = Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)
            .map_err(db)?;
        let old = load(&tx, &value.operation.request.identity)?.ok_or_else(corrupt)?;
        if encoded(&old.operation.request)? != encoded(&value.operation.request)?
            || old.operation.owner_id != value.operation.owner_id
            || old.operation.credential_ref != value.operation.credential_ref
            || old.operation.owner_saved && !value.operation.owner_saved
            || old.protected_token.is_some() && old.protected_token != value.protected_token
            || old.operation.status != Status::Pending
                && old.operation.status != value.operation.status
            || old.operation.preview_id.is_some()
                && old.operation.preview_id != value.operation.preview_id
        {
            return Err(corrupt());
        }
        write(&tx, value, false)?;
        tx.commit().map_err(db)
    }
    pub(super) fn pending_local_tokens(
        &self,
        store_id: &str,
        namespace_id: &str,
    ) -> Result<Vec<Operation>, ConfigError> {
        scope(&self.connection, store_id, namespace_id)?;
        let ids=self.connection.prepare("SELECT operation_id FROM local_token_owners WHERE state='pending' ORDER BY rowid LIMIT 5").map_err(db)?.query_map([],|r|r.get::<_,String>(0)).map_err(db)?.collect::<Result<Vec<_>,_>>().map_err(db)?;
        if ids.len() > PENDING_LIMIT {
            return Err(corrupt());
        }
        ids.into_iter()
            .map(|operation_id| {
                let identity = IdentityV3 {
                    store_id: store_id.into(),
                    namespace_id: namespace_id.into(),
                    operation_id,
                };
                Ok(load(&self.connection, &identity)?
                    .ok_or_else(corrupt)?
                    .operation)
            })
            .collect()
    }
}
