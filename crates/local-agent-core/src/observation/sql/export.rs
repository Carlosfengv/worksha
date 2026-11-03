use super::{History, requests};
use chrono::{DateTime, Duration, Utc};
use requests::export::{
    EXPORT_RECORD_BYTES, EXPORT_ROWS, EXPORT_TARGET_BYTES, ExportChunk, ExportError, ExportInfo,
};
use rusqlite::params;
use serde_json::Value;

pub(super) struct ExportLease {
    pub positions: String,
    consent: Value,
    info: ExportInfo,
    deadline: DateTime<Utc>,
    expires: DateTime<Utc>,
}

impl History {
    pub fn begin_csv_export(
        &mut self,
        operation: &str,
        snapshot: &str,
        now: DateTime<Utc>,
    ) -> Result<ExportInfo, ExportError> {
        if uuid::Uuid::parse_str(operation).is_err() || snapshot.len() > 100 {
            return Err(ExportError::Invalid);
        }
        self.expire_csv_exports(now)?;
        if let Some(existing) = self.exports.get(operation) {
            return if existing.info.snapshot == snapshot {
                Ok(existing.info.clone())
            } else {
                Err(ExportError::Invalid)
            };
        }
        if self.unified_exports.contains_key(operation) {
            return Err(ExportError::Invalid);
        }
        if self.exports.len() + self.unified_exports.len() >= 2 {
            return Err(ExportError::Busy);
        }
        let frozen = self
            .snapshots
            .get(snapshot)
            .filter(|s| (now - s.created).num_minutes() < 15)
            .ok_or(ExportError::Stale)?;
        let info = ExportInfo {
            operation_id: operation.into(),
            snapshot: snapshot.into(),
            count: frozen.dashboard["total"]
                .as_u64()
                .filter(|n| *n <= i64::MAX as u64)
                .ok_or(ExportError::Unavailable)?,
            as_of: frozen.dashboard["asOf"]
                .as_str()
                .ok_or(ExportError::Unavailable)?
                .into(),
        };
        self.exports.insert(
            operation.into(),
            ExportLease {
                positions: frozen.positions.clone(),
                consent: serde_json::json!({"collectors": frozen.dashboard["sourceReport"]["collectors"]}),
                info: info.clone(),
                deadline: now + Duration::seconds(60),
                expires: now + Duration::minutes(15),
            },
        );
        Ok(info)
    }

    pub fn csv_export_report(
        &self,
        operation: &str,
        now: DateTime<Utc>,
    ) -> Result<Value, ExportError> {
        let job = self
            .exports
            .get(operation)
            .filter(|j| now < j.deadline && now < j.expires)
            .ok_or(ExportError::Stale)?;
        Ok(job.consent.clone())
    }

    pub fn csv_export_chunk(
        &mut self,
        operation: &str,
        after: i64,
        now: DateTime<Utc>,
    ) -> Result<ExportChunk, ExportError> {
        self.csv_export_report(operation, now)?;
        let job = self.exports.get_mut(operation).ok_or(ExportError::Stale)?;
        if after < -1
            || (job.info.count > 0 && after >= job.info.count as i64)
            || (job.info.count == 0 && after != -1)
        {
            return Err(ExportError::Invalid);
        }
        let mut csv = if after == -1 {
            requests::csv::header()
        } else {
            String::new()
        };
        let mut rows = 0u64;
        let mut position = after;
        let mut statement = self.db.prepare_cached(
            "SELECT p.pos,length(CAST(r.value AS BLOB)),r.value FROM positions p JOIN record_payloads r ON r.v=p.v WHERE p.snapshot=?1 AND p.pos>?2 ORDER BY p.pos LIMIT ?3"
        ).map_err(|_| ExportError::Unavailable)?;
        let mut records = statement
            .query(params![job.positions, after, EXPORT_ROWS as i64])
            .map_err(|_| ExportError::Unavailable)?;
        while let Some(record) = records.next().map_err(|_| ExportError::Unavailable)? {
            let bytes: i64 = record.get(1).map_err(|_| ExportError::Unavailable)?;
            if bytes > EXPORT_RECORD_BYTES as i64 {
                return Err(ExportError::RecordTooLarge);
            }
            let index: i64 = record.get(0).map_err(|_| ExportError::Unavailable)?;
            if index != position + 1 {
                return Err(ExportError::Unavailable);
            }
            let raw: String = record.get(2).map_err(|_| ExportError::Unavailable)?;
            let value = serde_json::from_str(&raw).map_err(|_| ExportError::Unavailable)?;
            requests::csv::write_bounded(&mut csv, &value, EXPORT_RECORD_BYTES * 2)
                .map_err(|_| ExportError::RecordTooLarge)?;
            position = index;
            rows += 1;
            if csv.len() >= EXPORT_TARGET_BYTES {
                break;
            }
        }
        if rows == 0 && position + 1 != job.info.count as i64 {
            return Err(ExportError::Unavailable);
        }
        job.deadline = now + Duration::seconds(60);
        Ok(ExportChunk {
            operation_id: operation.into(),
            after,
            rows,
            csv,
            next_after: (position + 1 < job.info.count as i64).then_some(position),
        })
    }

    pub fn finish_csv_export(
        &mut self,
        operation: &str,
        after: i64,
        rows: u64,
        now: DateTime<Utc>,
    ) -> Result<ExportInfo, ExportError> {
        self.csv_export_report(operation, now)?;
        let job = self.exports.get(operation).ok_or(ExportError::Stale)?;
        if rows != job.info.count || after != rows as i64 - 1 {
            return Err(ExportError::Invalid);
        }
        let info = job.info.clone();
        self.cancel_csv_export(operation)?;
        Ok(info)
    }

    pub fn cancel_csv_export(&mut self, operation: &str) -> Result<(), ExportError> {
        if let Some(job) = self.exports.remove(operation) {
            let cleanup = (|| {
                self.db
                    .execute_batch("SAVEPOINT csv_export_release;")
                    .map_err(|_| ExportError::Unavailable)?;
                self.release_request_positions([job.positions.clone()].into_iter().collect())
                    .map_err(|_| ExportError::Unavailable)?;
                self.prune().map_err(|_| ExportError::Unavailable)?;
                self.db
                    .execute_batch("RELEASE csv_export_release;")
                    .map_err(|_| ExportError::Unavailable)
            })();
            if let Err(error) = cleanup {
                let _ = self
                    .db
                    .execute_batch("ROLLBACK TO csv_export_release; RELEASE csv_export_release;");
                self.exports.insert(operation.into(), job);
                return Err(error);
            }
        }
        Ok(())
    }
    pub(super) fn expire_csv_exports(&mut self, now: DateTime<Utc>) -> Result<(), ExportError> {
        let expired: Vec<_> = self
            .exports
            .iter()
            .filter(|(_, j)| now >= j.deadline || now >= j.expires)
            .map(|(id, _)| id.clone())
            .collect();
        for id in expired {
            self.cancel_csv_export(&id)?;
        }
        self.expire_unified_csv_exports(now)
    }
}
