//! The same bounded domain operations can be implemented by a native broker or a worker bridge.
use super::*;
use std::collections::HashSet;

pub const BATCH_ITEMS: usize = 256;

/// A batch is tentative. The host may publish only after the checkpoint and final consent check.
pub trait SessionSource {
    fn scan(
        &mut self,
        request: &ScanRequest,
        emit: &mut dyn FnMut(SourceBatch) -> Result<()>,
    ) -> Result<Checkpoint>;
    fn read_conversation(&mut self, request: &ConversationRequest) -> Result<ConversationPage>;
}

/// Host-side protocol validation, independent of execution and private native indexes.
pub struct ScanPublication {
    source_id: String,
    revision: Option<String>,
    batches: u64,
    records: u64,
    identities: HashSet<String>,
    failed: bool,
    limit: u64,
}
impl ScanPublication {
    pub fn new(source_id: &str) -> Self {
        Self::with_limit(source_id, u64::MAX)
    }
    pub fn with_limit(source_id: &str, limit: u64) -> Self {
        Self {
            source_id: source_id.into(),
            revision: None,
            batches: 0,
            records: 0,
            identities: HashSet::new(),
            failed: false,
            limit,
        }
    }
    pub fn accept(&mut self, batch: SourceBatch) -> Result<()> {
        if self.failed
            || batch.source_id != self.source_id
            || batch.source_revision.is_empty()
            || batch.batch_id.value() != self.batches
            || batch.records.is_empty()
            || batch.records.len() > BATCH_ITEMS
            || self
                .revision
                .as_ref()
                .is_some_and(|r| r != &batch.source_revision)
            || batch
                .records
                .iter()
                .any(|r| r.id.is_empty() || r.native_id.is_empty())
        {
            self.failed = true;
            return Err(Failure::new(ErrorCode::InvalidInput));
        }
        if self.records.saturating_add(batch.records.len() as u64) > self.limit {
            self.failed = true;
            return Err(Failure::new(ErrorCode::ResourceLimit));
        }
        for record in &batch.records {
            if !self.identities.insert(record.id.clone()) {
                self.failed = true;
                return Err(Failure::new(ErrorCode::InvalidInput));
            }
        }
        self.revision = Some(batch.source_revision);
        self.batches += 1;
        self.records += batch.records.len() as u64;
        Ok(())
    }
    pub fn finish(self, checkpoint: &Checkpoint) -> Result<()> {
        if checkpoint.coverage == Coverage::Unavailable {
            return Err(Failure::new(ErrorCode::Unavailable));
        }
        if self.failed
            || !checkpoint.complete
            || checkpoint.source_revision.is_empty()
            || checkpoint.batches.value() != self.batches
            || checkpoint.records.value() != self.records
            || self
                .revision
                .as_ref()
                .is_some_and(|r| r != &checkpoint.source_revision)
        {
            return Err(Failure::new(ErrorCode::StaleSnapshot));
        }
        Ok(())
    }
}
