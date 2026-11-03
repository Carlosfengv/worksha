//! Independent cancellation evidence; never synthesizes an unreceived request.
use super::*;
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};

const RECORD_LIMIT: usize = 1024;
fn unavailable(_: impl std::fmt::Display) -> ConfigError {
    corrupt()
}
pub(super) fn initialize(db: &Connection) -> Result<(), ConfigError> {
    db.execute_batch("CREATE TABLE local_token_cancellations(operation_id TEXT PRIMARY KEY NOT NULL,store_id TEXT NOT NULL,namespace_id TEXT NOT NULL,value BLOB NOT NULL CHECK(typeof(value)='blob' AND length(value)<=1024),value_hash TEXT NOT NULL);").map_err(unavailable)
}
pub(super) fn metadata(
    db: &Connection,
    store_id: &str,
    namespace_id: &str,
) -> Result<(i64, i64), ConfigError> {
    let (count, bytes): (i64, i64) = db
        .query_row(
            "SELECT cancellation_count,cancellation_bytes FROM local_token_scope WHERE id=1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(unavailable)?;
    let (actual,size,invalid):(i64,i64,i64)=db.query_row("SELECT count(*),COALESCE(sum(length(value)),0),COALESCE(sum(typeof(value)!='blob' OR length(value)>1024 OR length(value_hash)!=64 OR typeof(operation_id)!='text' OR length(operation_id)!=36 OR store_id!=?1 OR namespace_id!=?2),0) FROM (SELECT * FROM local_token_cancellations LIMIT 4097)",params![store_id,namespace_id],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?))).map_err(unavailable)?;
    let collisions:i64=db.query_row("SELECT count(*) FROM local_token_cancellations c JOIN local_token_owners o ON c.operation_id=o.operation_id",[],|row|row.get(0)).map_err(unavailable)?;
    if !(0..=CANCELLATION_LIMIT as i64).contains(&count)
        || count != actual
        || bytes != size
        || !(0..=(CANCELLATION_LIMIT * RECORD_LIMIT) as i64).contains(&bytes)
        || invalid != 0
        || collisions != 0
    {
        return Err(corrupt());
    }
    // Absence is an admission decision. A moved index must not hide a proof
    // under its original ID. Unlike large owner catalogs, this entire ledger
    // is bounded to 4096 x 1024 bytes; validate every mapping and checksum.
    let mut query=db.prepare("SELECT operation_id,store_id,namespace_id,substr(value,1,1025),value_hash FROM local_token_cancellations LIMIT 4097").map_err(unavailable)?;
    let mut rows = query.query([]).map_err(unavailable)?;
    while let Some(row) = rows.next().map_err(unavailable)? {
        let identity = IdentityV3 {
            operation_id: row.get(0).map_err(unavailable)?,
            store_id: row.get(1).map_err(unavailable)?,
            namespace_id: row.get(2).map_err(unavailable)?,
        };
        let raw: Vec<u8> = row.get(3).map_err(unavailable)?;
        let hash: String = row.get(4).map_err(unavailable)?;
        decode(&identity, &raw, &hash)?;
    }
    Ok((count, bytes))
}
fn decode(identity: &IdentityV3, bytes: &[u8], hash: &str) -> Result<Cancellation, ConfigError> {
    if bytes.len() > RECORD_LIMIT
        || files::hash(bytes) != hash
        || !valid_id(&identity.store_id)
        || !valid_id(&identity.namespace_id)
        || !valid_id(&identity.operation_id)
    {
        return Err(corrupt());
    }
    let value: Cancellation = serde_json::from_slice(bytes).map_err(unavailable)?;
    if value.version != 1 || value.identity != *identity {
        return Err(corrupt());
    }
    Ok(value)
}
pub(super) fn read(
    db: &Connection,
    identity: &IdentityV3,
) -> Result<Option<Cancellation>, ConfigError> {
    let row:Option<(String,String,Vec<u8>,String)>=db.query_row("SELECT store_id,namespace_id,substr(value,1,1025),value_hash FROM local_token_cancellations WHERE operation_id=?1",[&identity.operation_id],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?))).optional().map_err(unavailable)?;
    row.map(|(store_id, namespace_id, bytes, hash)| {
        if store_id != identity.store_id || namespace_id != identity.namespace_id {
            return Err(corrupt());
        }
        decode(identity, &bytes, &hash)
    })
    .transpose()
}
impl Store {
    pub fn local_token_cancellation(
        &self,
        identity: &IdentityV3,
    ) -> Result<Option<Cancellation>, ConfigError> {
        let source = self.credential_scope(&identity.store_id)?;
        if source.journal.namespace_id() != identity.namespace_id
            || !valid_id(&identity.operation_id)
        {
            return Err(corrupt());
        }
        source.journal.domain(DOMAIN).status()?;
        let tx = self
            .connection
            .unchecked_transaction()
            .map_err(unavailable)?;
        storage::validate_scope(&tx, &identity.store_id, &identity.namespace_id)?;
        read(&tx, identity)
    }
    pub(super) fn cancel_unowned_local_token(
        &self,
        identity: &IdentityV3,
    ) -> Result<Cancellation, ConfigError> {
        let tx = Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)
            .map_err(unavailable)?;
        storage::validate_scope(&tx, &identity.store_id, &identity.namespace_id)?;
        if let Some(value) = read(&tx, identity)? {
            return Ok(value);
        }
        let owned: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM local_token_owners WHERE operation_id=?1)",
                [&identity.operation_id],
                |row| row.get(0),
            )
            .map_err(unavailable)?;
        if owned {
            return Err(unknown());
        }
        let (count, bytes) = metadata(&tx, &identity.store_id, &identity.namespace_id)?;
        if count >= CANCELLATION_LIMIT as i64 {
            return fail(
                "capacity",
                "取消记录已达上限；原编号暂不能确认停止，请保留原提示核查",
            );
        }
        let value = Cancellation {
            version: 1,
            identity: identity.clone(),
        };
        let raw = serde_json::to_vec(&value).map_err(unavailable)?;
        if raw.len() > RECORD_LIMIT {
            return Err(corrupt());
        }
        if tx
            .execute(
                "INSERT INTO local_token_cancellations VALUES(?1,?2,?3,?4,?5)",
                params![
                    identity.operation_id,
                    identity.store_id,
                    identity.namespace_id,
                    raw,
                    files::hash(&raw)
                ],
            )
            .map_err(unavailable)?
            != 1
        {
            return Err(corrupt());
        }
        if tx.execute("UPDATE local_token_scope SET cancellation_count=?1,cancellation_bytes=?2 WHERE id=1 AND cancellation_count=?3 AND cancellation_bytes=?4",params![count+1,bytes+raw.len() as i64,count,bytes]).map_err(unavailable)?!=1 {return Err(corrupt());}
        storage::validate_scope(&tx, &identity.store_id, &identity.namespace_id)?;
        let saved = read(&tx, identity)?.ok_or_else(corrupt)?;
        if saved != value
            || metadata(&tx, &identity.store_id, &identity.namespace_id)?
                != (count + 1, bytes + raw.len() as i64)
        {
            return Err(corrupt());
        }
        tx.commit().map_err(unavailable)?;
        Ok(saved)
    }
}
