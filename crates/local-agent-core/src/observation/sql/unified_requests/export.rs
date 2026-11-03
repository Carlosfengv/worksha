//! Pins immutable SQL positions without materializing the selected range.
use super::*;
use crate::{
    domain::requests::export as x,
    requests::export::{EXPORT_RECORD_BYTES, EXPORT_ROWS, EXPORT_TARGET_BYTES, ExportError},
    sessions::NativeSessions,
};
use chrono::Duration;
#[cfg(test)]
mod tests;

pub(in crate::observation::sql) struct ExportLease {
    pub positions: String,
    frozen: Frozen,
    info: x::Info,
    deadline: DateTime<Utc>,
    expires: DateTime<Utc>,
}
impl ExportLease {
    fn check(
        &self,
        signature: &str,
        sessions: &NativeSessions,
        now: DateTime<Utc>,
    ) -> Result<(), ExportError> {
        if now < self.frozen.created
            || now >= self.deadline
            || now >= self.expires
            || signature != self.frozen.signature
        {
            return Err(ExportError::Stale);
        }
        for authorization in &self.frozen.authorizations {
            authorization
                .ensure_current(sessions)
                .map_err(|_| ExportError::Stale)?;
        }
        Ok(())
    }
}
impl History {
    pub fn unified_csv_export_query(
        &self,
        operation: &str,
        now: DateTime<Utc>,
    ) -> Result<q::UnifiedRequestQuery, ExportError> {
        let job = self
            .unified_exports
            .get(operation)
            .filter(|j| now >= j.frozen.created && now < j.deadline && now < j.expires)
            .ok_or(ExportError::Stale)?;
        Ok(job.frozen.query.clone())
    }
    pub fn check_unified_csv_export(
        &self,
        operation: &str,
        signature: &str,
        sessions: &NativeSessions,
        now: DateTime<Utc>,
    ) -> Result<(), ExportError> {
        self.unified_exports
            .get(operation)
            .ok_or(ExportError::Stale)?
            .check(signature, sessions, now)
    }
    pub fn begin_unified_csv_export(
        &mut self,
        input: &x::Begin,
        signature: &str,
        sessions: &NativeSessions,
        now: DateTime<Utc>,
    ) -> Result<x::Info, ExportError> {
        input.validate()?;
        self.expire_csv_exports(now)?;
        if let Some(existing) = self.unified_exports.get(&input.operation_id) {
            if existing.info.snapshot_token != input.snapshot_token {
                return Err(ExportError::Invalid);
            }
            existing.check(signature, sessions, now)?;
            return Ok(existing.info.clone());
        }
        if self.exports.contains_key(&input.operation_id) {
            return Err(ExportError::Invalid);
        }
        if self.exports.len() + self.unified_exports.len() >= 2 {
            return Err(ExportError::Busy);
        }
        self.check_unified_requests(&input.snapshot_token, signature, sessions)
            .map_err(|_| ExportError::Stale)?;
        let frozen = self
            .unified_requests
            .get(&input.snapshot_token)
            .filter(|f| now >= f.created && (now - f.created).num_seconds() < 300)
            .ok_or(ExportError::Stale)?
            .clone();
        let info = x::Info {
            api_version: x::API_VERSION.into(),
            operation_id: input.operation_id.clone(),
            snapshot_token: input.snapshot_token.clone(),
            count: (frozen.total as u64).into(),
            as_of: frozen.page.as_of.clone(),
            applied_query: frozen.query.clone(),
            source_states: frozen.page.source_states.clone(),
            source_summaries: frozen.page.source_summaries.clone(),
            overlap_unknown: frozen.page.overlap_unknown,
        };
        // Check metadata capacity before admitting any export resource.
        x::csv::start(&info)?;
        self.unified_exports.insert(
            input.operation_id.clone(),
            ExportLease {
                positions: frozen.positions.clone(),
                frozen,
                info: info.clone(),
                deadline: now + Duration::seconds(60),
                expires: now + Duration::minutes(15),
            },
        );
        Ok(info)
    }
    pub fn unified_csv_export_chunk(
        &mut self,
        operation: &str,
        offset: u64,
        signature: &str,
        sessions: &NativeSessions,
        now: DateTime<Utc>,
    ) -> Result<x::Chunk, ExportError> {
        self.check_unified_csv_export(operation, signature, sessions, now)?;
        let job = self
            .unified_exports
            .get_mut(operation)
            .ok_or(ExportError::Stale)?;
        let count = job.info.count.value();
        if (count > 0 && offset >= count) || (count == 0 && offset != 0) {
            return Err(ExportError::Invalid);
        }
        let mut csv = if offset == 0 {
            x::csv::start(&job.info)?
        } else {
            String::new()
        };
        let mut rows = 0u64;
        let mut statement = self.db.prepare_cached("SELECT p.pos,length(CAST(p.external_value AS BLOB)),length(CAST(r.value AS BLOB)),p.external_value,r.value FROM unified_request_positions p LEFT JOIN record_payloads r ON r.v=p.host_v WHERE p.snapshot=?1 AND p.pos>=?2 ORDER BY p.pos LIMIT ?3").map_err(|_| ExportError::Unavailable)?;
        let mut records = statement
            .query(params![job.positions, offset as i64, EXPORT_ROWS as i64])
            .map_err(|_| ExportError::Unavailable)?;
        while let Some(record) = records.next().map_err(|_| ExportError::Unavailable)? {
            let index: i64 = record.get(0).map_err(|_| ExportError::Unavailable)?;
            if index < 0 || index as u64 != offset + rows || index as u64 >= count {
                return Err(ExportError::Unavailable);
            }
            let external_bytes: Option<i64> =
                record.get(1).map_err(|_| ExportError::Unavailable)?;
            let host_bytes: Option<i64> = record.get(2).map_err(|_| ExportError::Unavailable)?;
            if !matches!(
                (external_bytes, host_bytes),
                (Some(_), None) | (None, Some(_))
            ) {
                return Err(ExportError::Unavailable);
            }
            if external_bytes
                .or(host_bytes)
                .is_some_and(|n| n < 0 || n > EXPORT_RECORD_BYTES as i64)
            {
                return Err(ExportError::RecordTooLarge);
            }
            let external: Option<String> = record.get(3).map_err(|_| ExportError::Unavailable)?;
            let host: Option<String> = record.get(4).map_err(|_| ExportError::Unavailable)?;
            let item = match (external, host) {
                (Some(raw), None) => {
                    serde_json::from_str(&raw).map_err(|_| ExportError::Unavailable)?
                }
                (None, Some(raw)) => host_item(
                    serde_json::from_str(&raw).map_err(|_| ExportError::Unavailable)?,
                    job.frozen.query.unit,
                    &job.frozen.hidden,
                    job.frozen.query.include_evidence == Some(true),
                )
                .map_err(|_| ExportError::Unavailable)?,
                _ => return Err(ExportError::Unavailable),
            };
            x::csv::append(&mut csv, &job.info, &item, EXPORT_RECORD_BYTES * 2)?;
            rows += 1;
            if csv.len() >= EXPORT_TARGET_BYTES {
                break;
            }
        }
        let end = offset + rows;
        if rows == 0 && end != count {
            return Err(ExportError::Unavailable);
        }
        job.deadline = now + Duration::seconds(60);
        Ok(x::Chunk {
            operation_id: operation.into(),
            offset: offset.into(),
            next_offset: (end < count).then(|| end.into()),
            rows: rows.into(),
            csv,
        })
    }
    pub fn finish_unified_csv_export(
        &mut self,
        operation: &str,
        input: &x::Finish,
        signature: &str,
        sessions: &NativeSessions,
        now: DateTime<Utc>,
    ) -> Result<x::Info, ExportError> {
        self.check_unified_csv_export(operation, signature, sessions, now)?;
        let job = self
            .unified_exports
            .get(operation)
            .ok_or(ExportError::Stale)?;
        if input.rows != job.info.count || input.next_offset != job.info.count {
            return Err(ExportError::Invalid);
        }
        let info = job.info.clone();
        self.cancel_unified_csv_export(operation)?;
        Ok(info)
    }
    pub fn cancel_unified_csv_export(&mut self, operation: &str) -> Result<(), ExportError> {
        if let Some(job) = self.unified_exports.remove(operation) {
            let cleanup = (|| {
                self.db
                    .execute_batch("SAVEPOINT unified_csv_release;")
                    .map_err(|_| ExportError::Unavailable)?;
                self.release_request_positions([job.positions.clone()].into_iter().collect())
                    .map_err(|_| ExportError::Unavailable)?;
                self.prune().map_err(|_| ExportError::Unavailable)?;
                self.db
                    .execute_batch("RELEASE unified_csv_release;")
                    .map_err(|_| ExportError::Unavailable)
            })();
            if let Err(error) = cleanup {
                let _ = self
                    .db
                    .execute_batch("ROLLBACK TO unified_csv_release; RELEASE unified_csv_release;");
                self.unified_exports.insert(operation.into(), job);
                return Err(error);
            }
        }
        Ok(())
    }
    pub(in crate::observation::sql) fn expire_unified_csv_exports(
        &mut self,
        now: DateTime<Utc>,
    ) -> Result<(), ExportError> {
        let expired: Vec<_> = self
            .unified_exports
            .iter()
            .filter(|(_, j)| now < j.frozen.created || now >= j.deadline || now >= j.expires)
            .map(|(id, _)| id.clone())
            .collect();
        for id in expired {
            self.cancel_unified_csv_export(&id)?;
        }
        Ok(())
    }
}
