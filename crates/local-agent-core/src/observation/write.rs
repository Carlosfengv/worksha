use super::{ModelObservation, ObservationStore, content_epoch};
use rusqlite::{Connection, OpenFlags, Transaction, TransactionBehavior, params};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ObservationWriteError {
    ScopeChanged,
    Unavailable,
    InvalidRecord,
}

impl std::fmt::Display for ObservationWriteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::ScopeChanged => "观测内容版本已变化或不可确认，旧更新未重放，请核查后台状态",
            Self::Unavailable => "观测记录未能保存，请检查存储状态",
            Self::InvalidRecord => "观测记录无效，未提交本批更新",
        })
    }
}
impl std::error::Error for ObservationWriteError {}

fn require_epoch(
    transaction: &Transaction<'_>,
    expected: &content_epoch::ContentEpoch,
) -> Result<(), ObservationWriteError> {
    use ObservationWriteError::{ScopeChanged, Unavailable};
    let current = content_epoch::read(transaction).map_err(|error| match error {
        content_epoch::EpochError::Unavailable => Unavailable,
        _ => ScopeChanged,
    })?;
    if current != *expected {
        return Err(ScopeChanged);
    }
    Ok(())
}

impl ObservationStore {
    /// Verify an existing writer source without creating/migrating a database
    /// or saving a row. The worker may still be waiting for SQLite's writer slot
    /// after its caller has stopped waiting for this verification.
    pub fn check_existing_epoch(
        directory: &std::path::Path,
        expected: &content_epoch::ContentEpoch,
    ) -> Result<(), ObservationWriteError> {
        let db = Connection::open_with_flags(
            directory.join("observations.sqlite3"),
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )
        .map_err(|_| ObservationWriteError::Unavailable)?;
        db.busy_timeout(std::time::Duration::from_secs(3))
            .map_err(|_| ObservationWriteError::Unavailable)?;
        let tx = Transaction::new_unchecked(&db, TransactionBehavior::Immediate)
            .map_err(|_| ObservationWriteError::Unavailable)?;
        require_epoch(&tx, expected)?;
        tx.commit().map_err(|_| ObservationWriteError::Unavailable)
    }

    /// The epoch pinned when this writer connection was opened, not a fresh
    /// permission to adopt a different content lifetime after a retry.
    pub fn write_epoch(&self) -> &content_epoch::ContentEpoch {
        &self.1
    }

    pub fn save(&self, row: &ModelObservation) -> Result<(), String> {
        self.save_batch(std::slice::from_ref(row))
    }

    pub fn save_batch(&self, rows: &[ModelObservation]) -> Result<(), String> {
        self.save_batch_scoped(rows, &self.1)
            .map_err(|error| error.to_string())
    }

    /// The expected epoch must survive connection reopen/retry. Acquire the
    /// SQLite writer slot before checking it, then commit the batch atomically.
    /// This does not coordinate external JSON caches or authorize cleanup.
    pub fn save_batch_scoped(
        &self,
        rows: &[ModelObservation],
        expected: &content_epoch::ContentEpoch,
    ) -> Result<(), ObservationWriteError> {
        use ObservationWriteError::{InvalidRecord, ScopeChanged, Unavailable};
        if self.1 != *expected {
            return Err(ScopeChanged);
        }
        let check_file = || {
            self.2.check().map_err(|error| match error {
                super::source_file::Error::Changed => ScopeChanged,
                super::source_file::Error::Unavailable => Unavailable,
            })
        };
        check_file()?;
        let transaction = Transaction::new_unchecked(&self.0, TransactionBehavior::Immediate)
            .map_err(|_| Unavailable)?;
        require_epoch(&transaction, expected)?;
        {
            let mut statement = transaction.prepare_cached("INSERT INTO model_observations(id,started_at,revision,value) VALUES(?1,?2,?3,?4) ON CONFLICT(id) DO UPDATE SET started_at=excluded.started_at,revision=excluded.revision,value=excluded.value WHERE excluded.revision>=model_observations.revision").map_err(|_| Unavailable)?;
            for row in rows {
                let value = serde_json::to_string(row).map_err(|_| InvalidRecord)?;
                statement
                    .execute(params![row.id, row.started_at, row.revision as i64, value])
                    .map_err(|_| Unavailable)?;
            }
        }
        require_epoch(&transaction, expected)?;
        check_file()?;
        transaction.commit().map_err(|_| Unavailable)
    }
}

#[cfg(test)]
mod tests;
