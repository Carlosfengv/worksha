//! Bounded, lossless draft packing. It neither seals facts nor deletes staging or
//! original details. A preview checks the current staging snapshot before reuse.
use super::*;
use crate::observation::retention::{AttributionCoverage, PartDimension};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
mod codec;
mod preview;
use codec::{Entry, Frame, MAX_COMPRESSED, MAX_RAW, bytes_hash, hex, unhex};
pub use preview::PackedPreview;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PackedKind {
    Gateway,
    ConnectorTool,
}
impl PackedKind {
    fn tag(self) -> i64 {
        match self {
            Self::Gateway => 1,
            Self::ConnectorTool => 2,
        }
    }
    fn table(self) -> &'static str {
        match self {
            Self::Gateway => "retention_fact_staging",
            Self::ConnectorTool => "retention_tool_staging",
        }
    }
    fn validate(self, entry: &Entry, namespace: &str) -> Result<(), LedgerError> {
        if entry.order <= 0 || !valid_digest(&entry.source) {
            return Err(LedgerError::InvalidRecord);
        }
        match self {
            Self::Gateway => {
                super::validate(&entry.key, namespace, &entry.projection, &entry.value).map(|_| ())
            }
            Self::ConnectorTool => {
                tools::validate_fact(&entry.key, namespace, &entry.projection, &entry.value)
            }
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PackedBatch {
    pub block: String,
    pub facts: usize,
    pub frame_bytes: usize,
    pub compressed_payload_bytes: usize,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PackedCoverage {
    pub kind: PackedKind,
    pub packed_facts: usize,
    pub staged_facts: usize,
    pub unpacked_facts: usize,
    pub coverage: &'static str,
}

const BLOCK_COLUMNS: [&str; 7] = [
    "slot",
    "id",
    "version",
    "raw_bytes",
    "raw_digest",
    "payload_digest",
    "payload",
];
const MEMBER_COLUMNS: [&str; 4] = ["kind", "key", "block_slot", "ordinal"];
const WATERMARK_COLUMNS: [&str; 3] = ["kind", "last_order", "identity_digest"];
fn check_columns(db: &Connection, table: &str, expected: &[&str]) -> Result<(), LedgerError> {
    let found = db
        .prepare(&format!("PRAGMA table_info({table})"))
        .map_err(invalid)?
        .query_map([], |r| r.get::<_, String>(1))
        .map_err(invalid)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(invalid)?;
    if found != expected {
        return Err(LedgerError::InvalidRecord);
    }
    Ok(())
}
fn schema(db: &Connection) -> Result<(), LedgerError> {
    check_columns(db, "retention_packed_blocks", &BLOCK_COLUMNS)?;
    check_columns(db, "retention_packed_members", &MEMBER_COLUMNS)?;
    check_columns(db, "retention_packed_watermark", &WATERMARK_COLUMNS)?;
    let future:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM retention_packed_blocks WHERE version<1) OR EXISTS(SELECT 1 FROM retention_packed_blocks WHERE version>1)",[],|r|r.get(0)).map_err(invalid)?;
    if future {
        return Err(LedgerError::UnsupportedVersion);
    }
    for tag in [1, 2] {
        let missing:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM retention_packed_members WHERE kind=?1) AND NOT EXISTS(SELECT 1 FROM retention_packed_watermark WHERE kind=?1)",[tag],|r|r.get(0)).map_err(invalid)?;
        if missing {
            return Err(LedgerError::InvalidRecord);
        }
    }
    Ok(())
}
fn prepare(db: &Connection) -> Result<(), LedgerError> {
    let mut count = 0;
    for name in [
        "retention_packed_blocks",
        "retention_packed_members",
        "retention_packed_watermark",
    ] {
        count += usize::from(exists(db, name)?);
    }
    if count != 0 && count != 3 {
        return Err(LedgerError::InvalidRecord);
    }
    db.execute_batch("CREATE TABLE IF NOT EXISTS retention_packed_blocks(slot INTEGER PRIMARY KEY,id BLOB NOT NULL UNIQUE CHECK(length(id)=32),version INTEGER NOT NULL,raw_bytes INTEGER NOT NULL,raw_digest BLOB NOT NULL CHECK(length(raw_digest)=32),payload_digest BLOB NOT NULL CHECK(length(payload_digest)=32),payload BLOB NOT NULL);
        CREATE TABLE IF NOT EXISTS retention_packed_members(kind INTEGER NOT NULL CHECK(kind IN (1,2)),key BLOB NOT NULL CHECK(length(key)=32),block_slot INTEGER NOT NULL,ordinal INTEGER NOT NULL CHECK(ordinal>=0 AND ordinal<64),PRIMARY KEY(kind,key)) WITHOUT ROWID;
        CREATE TABLE IF NOT EXISTS retention_packed_watermark(kind INTEGER PRIMARY KEY CHECK(kind IN (1,2)),last_order INTEGER NOT NULL CHECK(last_order>0),identity_digest TEXT NOT NULL);
        CREATE INDEX IF NOT EXISTS retention_packed_block_members ON retention_packed_members(block_slot);
        CREATE INDEX IF NOT EXISTS retention_packed_format ON retention_packed_blocks(version);").map_err(unavailable)?;
    schema(db)
}
fn mark_digest(db: &Connection, kind: PackedKind, order: i64) -> Result<String, LedgerError> {
    let (owner, namespace): (String, String) = db
        .query_row(
            "SELECT store_id,namespace FROM retention_fact_scope WHERE id=1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(invalid)?;
    Ok(digest(&format!(
        "local-agent-packed-watermark-v1\0{owner}\0{namespace}\0{}\0{order}",
        kind.tag()
    )))
}
fn watermark(db: &Connection, kind: PackedKind) -> Result<i64, LedgerError> {
    let row: Option<(i64, String)> = db
        .query_row(
            "SELECT last_order,identity_digest FROM retention_packed_watermark WHERE kind=?1",
            [kind.tag()],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(invalid)?;
    if let Some((order, expected)) = row {
        if order <= 0 || expected != mark_digest(db, kind, order)? {
            return Err(LedgerError::InvalidRecord);
        }
        Ok(order)
    } else {
        Ok(0)
    }
}
fn advance_watermark(db: &Connection, kind: PackedKind, order: i64) -> Result<(), LedgerError> {
    let old = watermark(db, kind)?;
    let order = old.max(order);
    if old == order {
        return Ok(());
    }
    let changed=db.execute("INSERT INTO retention_packed_watermark VALUES(?1,?2,?3) ON CONFLICT(kind) DO UPDATE SET last_order=excluded.last_order,identity_digest=excluded.identity_digest",params![kind.tag(),order,mark_digest(db,kind,order)?]).map_err(unavailable)?;
    if changed != 1 {
        return Err(LedgerError::Unavailable);
    }
    Ok(())
}
fn block_id(raw: &[u8]) -> Vec<u8> {
    let mut hash = Sha256::new();
    hash.update(b"local-agent-retention-packed-v1\0");
    hash.update(raw);
    hash.finalize().to_vec()
}
fn source_entry(
    db: &Connection,
    kind: PackedKind,
    id: &str,
    limit: usize,
) -> Result<Entry, LedgerError> {
    if !valid_digest(id) {
        return Err(LedgerError::InvalidQuery);
    }
    let locator = if kind == PackedKind::Gateway {
        "NULL"
    } else {
        "CASE WHEN length(source_locator)=64 THEN source_locator END"
    };
    let sql = format!(
        "SELECT rowid,state,CASE WHEN length(source_fingerprint)=64 THEN source_fingerprint END,CASE WHEN length(projection_digest)=64 THEN projection_digest END,{locator},CASE WHEN length(CAST(value AS BLOB))<=?2 THEN value END FROM {} WHERE key=?1",
        kind.table()
    );
    let row = db
        .query_row(&sql, params![id, limit as i64], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, Option<String>>(4)?,
                r.get::<_, Option<String>>(5)?,
            ))
        })
        .optional()
        .map_err(invalid)?
        .ok_or(LedgerError::SourceChanged)?;
    if row.1 != "staged" {
        return Err(LedgerError::ProtectedFact);
    }
    if kind == PackedKind::ConnectorTool && !row.4.as_deref().is_some_and(valid_digest) {
        return Err(LedgerError::InvalidRecord);
    }
    Ok(Entry {
        order: row.0,
        key: id.into(),
        source: row.2,
        projection: row.3,
        locator: row.4,
        value: row.5.ok_or(LedgerError::BatchLimit)?,
    })
}

/// Continue the private contribution sequence after staged rows are removed.
/// It is not an original request/call ID and grants no deletion permission.
pub(super) fn next_order(db: &Connection, kind: PackedKind) -> Result<i64, LedgerError> {
    let staged: i64 = db
        .query_row(
            &format!("SELECT coalesce(max(rowid),0) FROM {}", kind.table()),
            [],
            |r| r.get(0),
        )
        .map_err(invalid)?;
    let packed = if exists(db, "retention_packed_members")? {
        schema(db)?;
        watermark(db, kind)?
    } else {
        0
    };
    staged
        .max(packed)
        .checked_add(1)
        .ok_or(LedgerError::Unavailable)
}
fn validate_frame(
    frame: &Frame,
    store: &str,
    namespace: &str,
    kind: PackedKind,
) -> Result<(), LedgerError> {
    if frame.version != 1 {
        return Err(LedgerError::UnsupportedVersion);
    }
    if frame.store_id != store || frame.namespace != namespace {
        return Err(LedgerError::WrongStore);
    }
    if frame
        .entries
        .iter()
        .map(|entry| entry.order)
        .collect::<HashSet<_>>()
        .len()
        != frame.entries.len()
    {
        return Err(LedgerError::InvalidRecord);
    }
    if frame.kind != kind
        || frame.entries.is_empty()
        || frame.entries.len() > MAX_BATCH
        || frame.entries.windows(2).any(|p| p[0].key >= p[1].key)
    {
        return Err(LedgerError::InvalidRecord);
    }
    for entry in &frame.entries {
        kind.validate(entry, namespace)?;
    }
    Ok(())
}

struct StoredBlock {
    slot: i64,
    frame: Frame,
    raw_bytes: usize,
    payload_bytes: usize,
}
fn read_block(
    db: &Connection,
    id: &[u8],
    store: &str,
    namespace: &str,
    kind: PackedKind,
) -> Result<StoredBlock, LedgerError> {
    if id.len() != 32 {
        return Err(LedgerError::InvalidRecord);
    }
    let row=db.query_row("SELECT slot,version,raw_bytes,CASE WHEN length(raw_digest)=32 THEN raw_digest END,CASE WHEN length(payload_digest)=32 THEN payload_digest END,CASE WHEN length(payload)<=?2 THEN payload END FROM retention_packed_blocks WHERE id=?1",params![id,MAX_COMPRESSED as i64],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,i64>(1)?,r.get::<_,i64>(2)?,r.get::<_,Vec<u8>>(3)?,r.get::<_,Vec<u8>>(4)?,r.get::<_,Option<Vec<u8>>>(5)?))).optional().map_err(invalid)?.ok_or(LedgerError::InvalidRecord)?;
    if row.1 != 1 {
        return Err(LedgerError::UnsupportedVersion);
    }
    if row.0 <= 0 {
        return Err(LedgerError::InvalidRecord);
    }
    let size = usize::try_from(row.2).map_err(invalid)?;
    let payload = row.5.ok_or(LedgerError::InvalidRecord)?;
    if bytes_hash(&payload).as_slice() != row.4 {
        return Err(LedgerError::InvalidRecord);
    }
    let raw = codec::decode(&payload, size)?;
    if bytes_hash(&raw).as_slice() != row.3 || block_id(&raw) != id {
        return Err(LedgerError::InvalidRecord);
    }
    let mut frame: Frame = serde_json::from_slice(&raw).map_err(invalid)?;
    for entry in &mut frame.entries {
        entry.projection = digest(&entry.value);
    }
    validate_frame(&frame, store, namespace, kind)?;
    let max_order = watermark(db, kind)?;
    if frame.entries.iter().any(|entry| entry.order > max_order) {
        return Err(LedgerError::InvalidRecord);
    }
    Ok(StoredBlock {
        slot: row.0,
        frame,
        raw_bytes: raw.len(),
        payload_bytes: payload.len(),
    })
}
fn read_slot(
    db: &Connection,
    slot: i64,
    store: &str,
    namespace: &str,
    kind: PackedKind,
) -> Result<StoredBlock, LedgerError> {
    let id: Vec<u8> = db
        .query_row(
            "SELECT CASE WHEN length(id)=32 THEN id END FROM retention_packed_blocks WHERE slot=?1",
            [slot],
            |r| r.get(0),
        )
        .map_err(invalid)?;
    read_block(db, &id, store, namespace, kind)
}
struct Member {
    kind: i64,
    key: Vec<u8>,
    block: i64,
    ordinal: usize,
}
fn member(row: &rusqlite::Row<'_>) -> rusqlite::Result<Member> {
    let ordinal: i64 = row.get(3)?;
    let ordinal = usize::try_from(ordinal)
        .map_err(|_| rusqlite::Error::IntegralValueOutOfRange(3, ordinal))?;
    Ok(Member {
        kind: row.get(0)?,
        key: row.get(1)?,
        block: row.get(2)?,
        ordinal,
    })
}
const MEMBER_SELECT: &str = "kind,CASE WHEN length(key)=32 THEN key END,block_slot,ordinal";
fn matches_member(member: &Member, entry: &Entry, kind: PackedKind) -> Result<(), LedgerError> {
    if member.kind != kind.tag() || member.key != unhex(&entry.key)? {
        return Err(LedgerError::InvalidRecord);
    }
    Ok(())
}

impl FactLedger<'_> {
    /// Draft packing only. Validate the current staging rows, preserve every byte,
    /// and replace draft membership atomically. No source or staging row is deleted.
    pub fn pack_staging_batch(
        &self,
        kind: PackedKind,
        keys: &[&str],
    ) -> Result<PackedBatch, LedgerError> {
        if keys.is_empty() {
            return Err(LedgerError::InvalidQuery);
        }
        if keys.len() > MAX_BATCH {
            return Err(LedgerError::BatchLimit);
        }
        if keys.iter().copied().collect::<HashSet<_>>().len() != keys.len() {
            return Err(LedgerError::InvalidQuery);
        }
        let tx = Transaction::new_unchecked(self.db, TransactionBehavior::Immediate)
            .map_err(unavailable)?;
        self.check_identity()?;
        let mut entries = Vec::with_capacity(keys.len());
        let mut remaining = MAX_RAW / 2;
        for id in keys {
            let entry = source_entry(&tx, kind, id, remaining)?;
            kind.validate(&entry, &self.namespace)?;
            remaining -= entry.value.len();
            entries.push(entry);
        }
        entries.sort_by(|a, b| a.key.cmp(&b.key));
        let frame = Frame {
            version: 1,
            store_id: self.store_id.clone(),
            namespace: self.namespace.clone(),
            kind,
            entries,
        };
        let (raw, payload) = codec::encode(&frame)?;
        let id = block_id(&raw);
        prepare(&tx)?;
        advance_watermark(
            &tx,
            kind,
            frame
                .entries
                .iter()
                .map(|entry| entry.order)
                .max()
                .ok_or(LedgerError::InvalidRecord)?,
        )?;
        let mut touched = HashSet::new();
        // Inspect old envelopes before replacing references, including future or
        // damaged drafts. A superseded source digest may legitimately differ.
        let mut previous: Option<(i64, StoredBlock)> = None;
        for entry in &frame.entries {
            let old=tx.query_row(&format!("SELECT {MEMBER_SELECT} FROM retention_packed_members WHERE kind=?1 AND key=?2"),params![kind.tag(),unhex(&entry.key)?],member).optional().map_err(invalid)?;
            if let Some(old) = old {
                if previous.as_ref().is_none_or(|(id, _)| id != &old.block) {
                    previous = Some((
                        old.block,
                        read_slot(&tx, old.block, &self.store_id, &self.namespace, kind)?,
                    ));
                }
                let block = &previous.as_ref().ok_or(LedgerError::InvalidRecord)?.1;
                let old_entry = block
                    .frame
                    .entries
                    .get(old.ordinal)
                    .ok_or(LedgerError::InvalidRecord)?;
                matches_member(&old, old_entry, kind)?;
                touched.insert(old.block);
            }
        }
        let existing: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM retention_packed_blocks WHERE id=?1)",
                [&id],
                |r| r.get(0),
            )
            .map_err(unavailable)?;
        let (slot, raw_bytes, payload_bytes) = if existing {
            let stored = read_block(&tx, &id, &self.store_id, &self.namespace, kind)?;
            (stored.slot, stored.raw_bytes, stored.payload_bytes)
        } else {
            let changed = tx
                .execute(
                    "INSERT INTO retention_packed_blocks(id,version,raw_bytes,raw_digest,payload_digest,payload) VALUES(?1,1,?2,?3,?4,?5)",
                    params![
                        id,
                        raw.len() as i64,
                        bytes_hash(&raw).as_slice(),
                        bytes_hash(&payload).as_slice(),
                        payload
                    ],
                )
                .map_err(unavailable)?;
            if changed != 1 {
                return Err(LedgerError::Unavailable);
            }
            (tx.last_insert_rowid(), raw.len(), payload.len())
        };
        for (ordinal, entry) in frame.entries.iter().enumerate() {
            let changed=tx.execute("INSERT INTO retention_packed_members VALUES(?1,?2,?3,?4) ON CONFLICT(kind,key) DO UPDATE SET block_slot=excluded.block_slot,ordinal=excluded.ordinal",params![kind.tag(),unhex(&entry.key)?,slot,ordinal as i64]).map_err(unavailable)?;
            if changed != 1 {
                return Err(LedgerError::Unavailable);
            }
        }
        for old in touched {
            if old != slot {
                let referenced: bool = tx
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM retention_packed_members WHERE block_slot=?1)",
                        [old],
                        |r| r.get(0),
                    )
                    .map_err(unavailable)?;
                if !referenced {
                    let changed = tx
                        .execute("DELETE FROM retention_packed_blocks WHERE slot=?1", [old])
                        .map_err(unavailable)?;
                    if changed != 1 {
                        return Err(LedgerError::Unavailable);
                    }
                }
            }
        }
        tx.commit().map_err(unavailable)?;
        Ok(PackedBatch {
            block: hex(&id),
            facts: frame.entries.len(),
            frame_bytes: raw_bytes,
            compressed_payload_bytes: payload_bytes,
        })
    }
}

#[cfg(test)]
mod tests;
