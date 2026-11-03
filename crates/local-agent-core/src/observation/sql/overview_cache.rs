//! Exact reuse between data changes and sliding-window bucket transitions.
use super::*;
use chrono::{DateTime, Utc};

pub(super) struct Cached {
    key: String,
    epoch: String,
    created: DateTime<Utc>,
    expires: i64,
    value: Value,
}

impl History {
    fn timing_epoch(&self) -> Result<String, String> {
        let seq: i64 = self
            .db
            .query_row("SELECT seq FROM query_clock", [], |r| r.get(0))
            .map_err(fail)?;
        encode(&seq)
    }

    pub(super) fn cached_timing(
        &self,
        q: &super::super::OverviewQuery,
        now: DateTime<Utc>,
    ) -> Result<Option<Value>, String> {
        let key = encode(&(q.range, &q.agent))?;
        let epoch = self.timing_epoch()?;
        Ok(self
            .timing_cache
            .borrow()
            .iter()
            .find(|entry| {
                entry.key == key
                    && entry.epoch == epoch
                    && now >= entry.created
                    && now.timestamp_millis() < entry.expires
            })
            .map(|entry| entry.value.clone()))
    }

    pub(super) fn remember_timing(
        &self,
        q: &super::super::OverviewQuery,
        now: DateTime<Utc>,
        result: &Value,
    ) -> Result<(), String> {
        // Timing keeps its existing gateway-only semantics for either Overview source.
        // Conservative indexed bounds include other Agents too; they may expire early
        // but cannot keep a filtered percentile alive past a membership change.
        let end = now.timestamp_millis();
        let span = i64::from(q.range) * 3_600_000;
        let first: Option<i64> = self
            .db
            .query_row(
                "SELECT min(time) FROM analyses WHERE kind='gateway' AND time>=?1 AND time<?2",
                params![end - span, end],
                |r| r.get(0),
            )
            .map_err(fail)?;
        let future: Option<i64> = self
            .db
            .query_row(
                "SELECT min(time) FROM analyses WHERE kind='gateway' AND time>=?1",
                [end],
                |r| r.get(0),
            )
            .map_err(fail)?;
        let key = encode(&(q.range, &q.agent))?;
        let entry = Cached {
            key: key.clone(),
            epoch: self.timing_epoch()?,
            created: now,
            expires: first
                .and_then(|t| t.checked_add(span)?.checked_add(1))
                .into_iter()
                .chain(future.and_then(|t| t.checked_add(1)))
                .min()
                .unwrap_or(i64::MAX),
            value: result.clone(),
        };
        let mut cache = self.timing_cache.borrow_mut();
        cache.retain(|entry| entry.key != key);
        if cache.len() == 8 {
            cache.remove(0);
        }
        cache.push(entry);
        Ok(())
    }

    fn analysis_epoch(&self, q: &super::super::OverviewQuery) -> Result<String, String> {
        if q.source == "native" {
            let seq: i64 = self
                .db
                .query_row("SELECT seq FROM native_analysis_clock", [], |r| r.get(0))
                .map_err(fail)?;
            return encode(&seq);
        }
        let seq: i64 = self
            .db
            .query_row("SELECT seq FROM query_clock", [], |r| r.get(0))
            .map_err(fail)?;
        encode(&seq)
    }

    pub(super) fn cached_analysis(
        &self,
        q: &super::super::OverviewQuery,
        now: DateTime<Utc>,
    ) -> Result<Option<Value>, String> {
        let key = encode(q)?;
        let epoch = self.analysis_epoch(q)?;
        let cache = self.analysis_cache.borrow();
        let Some(entry) = cache.iter().find(|c| {
            c.key == key
                && c.epoch == epoch
                && now >= c.created
                && now.timestamp_millis() < c.expires
        }) else {
            return Ok(None);
        };
        let mut result = entry.value.clone();
        let delta = now.timestamp_millis() - entry.created.timestamp_millis();
        result["asOf"] = json!(now.to_rfc3339());
        for bucket in result["buckets"].as_array_mut().ok_or("总览统计快照无效")? {
            for field in ["start", "end"] {
                bucket[field] = json!(shift(&bucket[field], delta)?);
            }
        }
        result["windowStart"] = json!(shift(&result["windowStart"], delta)?);
        Ok(Some(result))
    }

    pub(super) fn remember_analysis(
        &self,
        q: &super::super::OverviewQuery,
        now: DateTime<Utc>,
        from: &str,
        step: i64,
        result: &Value,
    ) -> Result<(), String> {
        if result.to_string().len() > 1024 * 1024 {
            return Ok(());
        }
        let end = now.timestamp_millis();
        let start = end - i64::from(q.range) * 3_600_000;
        let future: Option<i64> = self
            .db
            .query_row(
                &format!("SELECT min(time) FROM {from} AND time>=?1"),
                [end],
                |r| r.get(0),
            )
            .map_err(fail)?;
        // A record remains in its previous bucket at the exact boundary;
        // the following millisecond changes its bucket or leaves the window.
        let transition: Option<i64> = self
            .db
            .query_row(
                &format!(
                    "SELECT min(time+((?1-time+?2-1)/?2)*?2+1) FROM {from} AND time>=?3 AND time<?1"
                ),
                params![end, step, start],
                |r| r.get(0),
            )
            .map_err(fail)?;
        let entry = Cached {
            key: encode(q)?,
            epoch: self.analysis_epoch(q)?,
            created: now,
            expires: future
                .and_then(|t| t.checked_add(1))
                .into_iter()
                .chain(transition)
                .min()
                .unwrap_or(i64::MAX),
            value: result.clone(),
        };
        let mut cache = self.analysis_cache.borrow_mut();
        cache.retain(|c| c.key != entry.key);
        if cache.len() == 8 {
            cache.remove(0);
        }
        cache.push(entry);
        Ok(())
    }
}

fn shift(v: &Value, delta: i64) -> Result<String, String> {
    let time = DateTime::parse_from_rfc3339(v.as_str().ok_or("总览时间无效")?).map_err(fail)?;
    time.timestamp_millis()
        .checked_add(delta)
        .and_then(DateTime::<Utc>::from_timestamp_millis)
        .map(|t| t.to_rfc3339())
        .ok_or("总览时间越界".into())
}
