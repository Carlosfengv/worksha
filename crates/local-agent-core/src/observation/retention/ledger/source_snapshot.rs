//! One connection-owned, disk-backed membership snapshot. It is an optimization
//! for explicit staging, never consent, a deletion permit, or a cross-DB commit.
use super::*;
use crate::observation::{
    content_epoch::ContentEpoch,
    source_file::{Error as FileError, SourceFile},
};
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    time::{Duration, Instant},
};

const MAX_WORK_PAGES: usize = 65_536;
const MAX_AGE: Duration = Duration::from_secs(300);

pub(super) fn file_error(error: FileError) -> LedgerError {
    match error {
        FileError::Changed => LedgerError::SourceChanged,
        FileError::Unavailable => LedgerError::Unavailable,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Header {
    marker: source_clock::Marker,
    data_version: i64,
    schema_version: i64,
    changes: u64,
}

pub(super) struct Source<'db> {
    db: &'db Connection,
    file: &'db SourceFile,
    epoch: &'db ContentEpoch,
    lineage: String,
    cache: RefCell<Option<Snapshot>>,
}

impl<'db> Source<'db> {
    pub fn new(
        db: &'db Connection,
        file: &'db SourceFile,
        epoch: &'db ContentEpoch,
        lineage: String,
    ) -> Self {
        Self {
            db,
            file,
            epoch,
            lineage,
            cache: RefCell::new(None),
        }
    }
    pub fn check_file(&self) -> Result<(), LedgerError> {
        self.file.check().map_err(file_error)
    }
    pub fn clear(&self) {
        if let Ok(mut cache) = self.cache.try_borrow_mut() {
            *cache = None;
        }
    }
    pub fn detach(&self) -> Result<Option<Snapshot>, LedgerError> {
        Ok(self
            .cache
            .try_borrow_mut()
            .map_err(|_| LedgerError::Unavailable)?
            .take())
    }
    pub fn header(
        &self,
        tx: &Transaction<'_>,
        store_id: &str,
        namespace: &str,
    ) -> Result<Header, LedgerError> {
        self.check_file()?;
        if !std::ptr::eq::<Connection>(&**tx, self.db) || content_epoch_read(tx)? != *self.epoch {
            return Err(LedgerError::SourceChanged);
        }
        let marker = source_clock::read(tx, store_id, namespace)?;
        if marker.lineage != self.lineage {
            return Err(LedgerError::SourceChanged);
        }
        Ok(Header {
            marker,
            data_version: tx
                .query_row("PRAGMA main.data_version", [], |r| r.get(0))
                .map_err(unavailable)?,
            schema_version: tx
                .query_row("PRAGMA main.schema_version", [], |r| r.get(0))
                .map_err(unavailable)?,
            changes: tx.total_changes(),
        })
    }
    pub fn take(
        &self,
        tx: &Transaction<'_>,
        store_id: &str,
        namespace: &str,
        cached: Option<Snapshot>,
        started: Instant,
    ) -> Result<Snapshot, LedgerError> {
        let header = self.header(tx, store_id, namespace)?;
        if let Some(cached) = cached {
            if cached.header == header && cached.created.elapsed() < MAX_AGE {
                cached.check_work()?;
                return Ok(cached);
            }
            drop(cached);
        }
        let snapshot = Snapshot::capture(
            tx,
            store_id,
            namespace,
            header.clone(),
            started,
            committed::Limits::default(),
            MAX_WORK_PAGES,
        )?;
        if self.header(tx, store_id, namespace)? != header {
            return Err(LedgerError::SourceChanged);
        }
        Ok(snapshot)
    }
    pub fn finish(
        &self,
        tx: &Transaction<'_>,
        store_id: &str,
        namespace: &str,
        snapshot: &Snapshot,
        result: StageResult,
        started: Instant,
    ) -> Result<Header, LedgerError> {
        if started.elapsed() >= committed::Limits::default().budget {
            return Err(LedgerError::BatchLimit);
        }
        let mut expected = snapshot.header.clone();
        let writes =
            u64::try_from(result.inserted + result.updated).map_err(|_| LedgerError::BatchLimit)?;
        expected.changes = expected
            .changes
            .checked_add(writes)
            .ok_or(LedgerError::BatchLimit)?;
        let after = self.header(tx, store_id, namespace)?;
        // Includes trigger DML. Restoring a nonce cannot hide extra mutations.
        if after != expected {
            return Err(LedgerError::SourceChanged);
        }
        snapshot.check_work()?;
        Ok(after)
    }
    pub fn keep(&self, mut snapshot: Snapshot, header: Header) {
        snapshot.header = header;
        if let Ok(mut cache) = self.cache.try_borrow_mut() {
            *cache = Some(snapshot);
        }
    }
}

pub(super) struct Snapshot {
    work: Connection,
    header: Header,
    salt: [u8; 32],
    work_changes: u64,
    work_schema: i64,
    created: Instant,
}

fn work_error(error: rusqlite::Error) -> LedgerError {
    match error.sqlite_error_code() {
        Some(rusqlite::ErrorCode::DiskFull) => LedgerError::BatchLimit,
        Some(rusqlite::ErrorCode::ConstraintViolation) => LedgerError::InvalidRecord,
        _ => LedgerError::Unavailable,
    }
}
fn hash(salt: &[u8; 32], kind: u8, bytes: &[u8]) -> [u8; 32] {
    let mut hash = Sha256::new();
    hash.update(b"retention-source-member-v1\0");
    hash.update(salt);
    hash.update([kind]);
    hash.update((bytes.len() as u64).to_be_bytes());
    hash.update(bytes);
    hash.finalize().into()
}

impl Snapshot {
    fn capture(
        tx: &Transaction<'_>,
        store_id: &str,
        namespace: &str,
        header: Header,
        started: Instant,
        limits: committed::Limits,
        max_pages: usize,
    ) -> Result<Self, LedgerError> {
        let salt: [u8; 32] = Sha256::digest(format!(
            "retention-source-snapshot-v1\0{store_id}\0{namespace}\0{}",
            header.marker.lineage
        ))
        .into();
        let work = Connection::open("").map_err(work_error)?;
        work.execute_batch(&format!("PRAGMA page_size=4096;PRAGMA max_page_count={max_pages};PRAGMA cache_size=-2048;PRAGMA temp_store=FILE;PRAGMA journal_mode=OFF;PRAGMA synchronous=OFF;
            CREATE TABLE members(group_key BLOB NOT NULL,member_key BLOB PRIMARY KEY,raw_digest BLOB NOT NULL) WITHOUT ROWID;
            CREATE INDEX members_group ON members(group_key,member_key);BEGIN;")).map_err(work_error)?;
        let pages: i64 = work
            .query_row("PRAGMA max_page_count", [], |r| r.get(0))
            .map_err(work_error)?;
        if usize::try_from(pages).ok() != Some(max_pages) || max_pages > MAX_WORK_PAGES {
            return Err(LedgerError::BatchLimit);
        }
        {
            let mut insert = work
                .prepare("INSERT INTO members VALUES(?1,?2,?3)")
                .map_err(work_error)?;
            committed::scan(tx, limits, started, |row, raw| {
                let group = hash(
                    &salt,
                    0,
                    crate::requests::logical_request_key(row).as_bytes(),
                );
                let member = hash(&salt, 1, row.id.as_bytes());
                let raw = hash(&salt, 2, raw.as_bytes());
                insert
                    .execute(params![group.as_slice(), member.as_slice(), raw.as_slice()])
                    .map_err(work_error)?;
                Ok(())
            })?;
        }
        work.execute_batch("COMMIT;PRAGMA query_only=ON;")
            .map_err(work_error)?;
        let work_changes = work.total_changes();
        let work_schema = work
            .query_row("PRAGMA main.schema_version", [], |r| r.get(0))
            .map_err(work_error)?;
        Ok(Self {
            work,
            header,
            salt,
            work_changes,
            work_schema,
            created: Instant::now(),
        })
    }
    fn check_work(&self) -> Result<(), LedgerError> {
        let query_only: i64 = self
            .work
            .query_row("PRAGMA query_only", [], |r| r.get(0))
            .map_err(work_error)?;
        let schema: i64 = self
            .work
            .query_row("PRAGMA main.schema_version", [], |r| r.get(0))
            .map_err(work_error)?;
        let temp_schema: i64 = self
            .work
            .query_row("PRAGMA temp.schema_version", [], |r| r.get(0))
            .map_err(work_error)?;
        if query_only != 1
            || schema != self.work_schema
            || temp_schema != 0
            || self.work.total_changes() != self.work_changes
        {
            return Err(LedgerError::InvalidRecord);
        }
        Ok(())
    }
    pub fn members(
        &self,
        requests: &[&Request],
        started: Instant,
    ) -> Result<HashMap<[u8; 32], [u8; 32]>, LedgerError> {
        self.check_work()?;
        let expected = committed::expected(requests)?;
        let mut found = HashMap::new();
        let mut query=self.work.prepare("SELECT member_key,raw_digest FROM main.members WHERE group_key=?1 ORDER BY member_key LIMIT ?2").map_err(work_error)?;
        for (group, ids) in expected {
            if started.elapsed() >= committed::Limits::default().budget {
                return Err(LedgerError::BatchLimit);
            }
            let key = hash(&self.salt, 0, group.as_bytes());
            let mut missing = ids
                .iter()
                .map(|id| hash(&self.salt, 1, id.as_bytes()))
                .collect::<HashSet<_>>();
            let mut rows = query
                .query(params![key.as_slice(), ids.len() as i64 + 1])
                .map_err(work_error)?;
            while let Some(row) = rows.next().map_err(work_error)? {
                let member: [u8; 32] = row
                    .get::<_, Vec<u8>>(0)
                    .map_err(invalid)?
                    .try_into()
                    .map_err(|_| LedgerError::InvalidRecord)?;
                let raw: [u8; 32] = row
                    .get::<_, Vec<u8>>(1)
                    .map_err(invalid)?
                    .try_into()
                    .map_err(|_| LedgerError::InvalidRecord)?;
                if !missing.remove(&member) {
                    return Err(LedgerError::SourceChanged);
                }
                found.insert(member, raw);
            }
            if !missing.is_empty() {
                return Err(LedgerError::SourceChanged);
            }
        }
        Ok(found)
    }
    pub fn verify_raw(
        &self,
        members: &HashMap<[u8; 32], [u8; 32]>,
        id: &str,
        raw: &str,
    ) -> Result<(), LedgerError> {
        let key = hash(&self.salt, 1, id.as_bytes());
        if members.get(&key) != Some(&hash(&self.salt, 2, raw.as_bytes())) {
            return Err(LedgerError::SourceChanged);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
