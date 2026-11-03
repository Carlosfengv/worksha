//! Bounded, disposable read results. Public snapshot tokens retain their own
//! lifetime while unchanged queries share immutable SQLite position rows.
use super::*;
use chrono::{DateTime, Duration, Utc};

pub(super) struct Report {
    pub key: String,
    pub epoch: String,
    pub created: DateTime<Utc>,
    pub expires: DateTime<Utc>,
    pub values: Value,
}

const FIELDS: &[&str] = &[
    "gateway",
    "native",
    "linked",
    "matchedNative",
    "ambiguousNative",
    "usageConflicts",
    "unlinkedNative",
    "nativeMissingTime",
];

pub(super) fn query_key(query: &requests::Query) -> Result<String, String> {
    let mut query = query.clone();
    query.cursor = None;
    query.snapshot = None;
    encode(&query)
}

impl History {
    pub(super) fn request_epoch(&self) -> Result<String, String> {
        let clock: i64 = self
            .db
            .query_row("SELECT seq FROM request_clock", [], |r| r.get(0))
            .map_err(fail)?;
        let version: i64 = self
            .db
            .query_row("SELECT coalesce(max(v),0) FROM records", [], |r| r.get(0))
            .map_err(fail)?;
        // Fee projections and collector progress can change independently of
        // the indexed requests. Only actual request mutations invalidate the
        // immutable positions and numeric aggregates shared by these pages.
        encode(&(clock, version))
    }

    pub(super) fn next_request_time(&self, now: DateTime<Utc>) -> Result<DateTime<Utc>, String> {
        let raw: Option<String> = self.db.query_row(
            "SELECT r.started FROM records r JOIN current c ON c.v=r.v WHERE r.time>=?1 AND instant_cmp(r.started,?2)>=0 ORDER BY r.time,r.instant,r.id LIMIT 1",
            params![now.timestamp_millis(), now.to_rfc3339()], |r| r.get(0),
        ).optional().map_err(fail)?;
        Ok(raw
            .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
            .map(|t| t.with_timezone(&Utc))
            .unwrap_or(DateTime::<Utc>::MAX_UTC))
    }

    pub(super) fn cached_request_report(
        &self,
        query: &requests::Query,
        now: DateTime<Utc>,
        report: &mut Value,
    ) -> Result<bool, String> {
        let key = query_key(query)?;
        let epoch = self.request_epoch()?;
        if let Some(cache) = self
            .request_reports
            .borrow()
            .iter()
            .find(|c| c.key == key && c.epoch == epoch && now >= c.created && now < c.expires)
        {
            for field in FIELDS {
                report[*field] = cache.values[*field].clone();
            }
            return Ok(true);
        }
        Ok(false)
    }

    pub(super) fn remember_request_report(
        &self,
        query: &requests::Query,
        now: DateTime<Utc>,
        report: &Value,
    ) -> Result<(), String> {
        let key = query_key(query)?;
        let mut values = json!({});
        for field in FIELDS {
            values[*field] = report[*field].clone();
        }
        let entry = Report {
            key: key.clone(),
            epoch: self.request_epoch()?,
            created: now,
            expires: self.next_request_time(now)?,
            values,
        };
        let mut cache = self.request_reports.borrow_mut();
        cache.retain(|c| c.key != key);
        if cache.len() == 8 {
            cache.remove(0);
        }
        cache.push(entry);
        Ok(())
    }

    pub(super) fn dashboard_expiry(
        &self,
        q: &requests::Query,
        page: &Value,
        now: DateTime<Utc>,
    ) -> Result<DateTime<Utc>, String> {
        let parse = |v: &Value| {
            v.as_str()
                .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                .map(|t| t.with_timezone(&Utc))
        };
        let mut expires = self.next_request_time(now)?;
        if let Some(start) = q
            .start
            .as_deref()
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|t| t.with_timezone(&Utc))
            .filter(|t| *t >= now)
        {
            expires = expires.min(start);
        }
        // Heatmap calendar boundaries use the product's existing UTC+8 day.
        let day = (now + Duration::hours(8))
            .date_naive()
            .succ_opt()
            .ok_or("日期越界")?;
        expires =
            expires.min(day.and_hms_opt(0, 0, 0).ok_or("日期越界")?.and_utc() - Duration::hours(8));
        let start = page["buckets"]
            .as_array()
            .and_then(|b| b.first())
            .and_then(|b| parse(&b["start"]));
        let end = q
            .end
            .as_deref()
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|t| t.with_timezone(&Utc));
        if end.is_none_or(|t| t > now) {
            if let Some(start) = start {
                let span = (now - start).num_seconds().max(1);
                let step = if span <= 86400 {
                    3600
                } else if span <= 86400 * 7 {
                    21600
                } else if span <= 86400 * 60 {
                    86400
                } else {
                    ((span / 86400 / 52) + 1) * 86400
                };
                if let Some(last) = page["buckets"]
                    .as_array()
                    .and_then(|b| b.last())
                    .and_then(|b| parse(&b["start"]))
                {
                    expires = expires.min(last + Duration::seconds(step));
                }
                for days in [1, 7, 60] {
                    let boundary = start + Duration::days(days) + Duration::seconds(1);
                    if boundary > now {
                        expires = expires.min(boundary);
                    }
                }
            }
            if let Some(end) = end {
                expires = expires.min(end);
            }
        }
        Ok(expires)
    }

    pub(super) fn cached_dashboard(
        &mut self,
        q: &requests::Query,
        now: DateTime<Utc>,
        report: &Value,
        error: &Option<String>,
    ) -> Result<Option<dashboard::Frozen>, String> {
        let key = query_key(q)?;
        let epoch = self.request_epoch()?;
        let Some(mut frozen) = self
            .request_templates
            .iter()
            .find(|t| t.key == key && t.epoch == epoch && now >= t.created && now < t.expires)
            .cloned()
        else {
            return Ok(None);
        };
        let id = uuid::Uuid::new_v4().to_string();
        frozen.query = q.clone();
        frozen.created = now;
        frozen.dashboard["snapshot"] = json!(id);
        frozen.dashboard["asOf"] = json!(now.to_rfc3339());
        frozen.dashboard["sourceReport"] = report.clone();
        frozen.dashboard["sourceError"] = json!(error);
        if let Some(last) = frozen.dashboard["buckets"]
            .as_array_mut()
            .and_then(|b| b.last_mut())
        {
            let end = q
                .end
                .as_deref()
                .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                .map(|t| t.with_timezone(&Utc))
                .unwrap_or(now)
                .min(now);
            last["end"] = json!(end.to_rfc3339());
        }
        self.snapshots.insert(id, frozen.clone());
        Ok(Some(frozen))
    }

    pub(super) fn remember_dashboard(&mut self, frozen: &dashboard::Frozen) -> Result<(), String> {
        // Empty histories without an explicit start use a moving 24-hour range.
        if frozen.query.start.is_none() && frozen.dashboard["recordedStart"].is_null() {
            return Ok(());
        }
        if frozen.dashboard.to_string().len() > 1024 * 1024 {
            return Ok(());
        }
        let mut removed = HashSet::new();
        self.request_templates.retain(|t| {
            if t.key == frozen.key {
                removed.insert(t.positions.clone());
                false
            } else {
                true
            }
        });
        if self.request_templates.len() == 8 {
            removed.insert(self.request_templates.remove(0).positions);
        }
        self.request_templates.push(frozen.clone());
        self.release_request_positions(removed)
    }

    pub(super) fn release_request_positions(
        &self,
        candidates: HashSet<String>,
    ) -> Result<(), String> {
        let retained = self
            .snapshots
            .values()
            .chain(self.request_templates.iter())
            .map(|s| s.positions.as_str())
            .chain(self.exports.values().map(|job| job.positions.as_str()))
            .chain(
                self.unified_exports
                    .values()
                    .map(|job| job.positions.as_str()),
            )
            .chain(self.unified_requests.values().map(|s| s.positions.as_str()))
            .collect::<HashSet<_>>();
        for id in candidates {
            if !retained.contains(id.as_str()) {
                execute(&self.db, "DELETE FROM positions WHERE snapshot=?1", [&id])
                    .map_err(fail)?;
                execute(
                    &self.db,
                    "DELETE FROM unified_request_positions WHERE snapshot=?1",
                    [&id],
                )
                .map_err(fail)?;
            }
        }
        Ok(())
    }
}
