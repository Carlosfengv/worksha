//! Durable first-token and body sampling totals. Live token windows are never replayed.
use super::metrics::TextTotals;
use local_agent_core::sessions::activity_performance::{MessageUsage, RateSample};
use local_agent_core::{providers::files, sessions::live::NativeRealtime};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashMap},
    path::PathBuf,
};

const CAPACITY: usize = 1024;
const LIMIT_MS: f64 = 86_400_000.0;

#[derive(Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Timing {
    session: String,
    turn: String,
    started: i64,
    ttft_ms: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    body: Option<TextTotals>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    recent: Option<RateSample>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reported: Option<MessageUsage>,
}
impl Timing {
    fn valid(&self) -> bool {
        uuid::Uuid::parse_str(&self.session).is_ok()
            && uuid::Uuid::parse_str(&self.turn).is_ok()
            && self.started > 0
            && (self.ttft_ms.is_some()
                || self.body.is_some()
                || self.recent.is_some()
                || self.reported.is_some())
            && self
                .ttft_ms
                .is_none_or(|ms| ms.is_finite() && (0.0..=LIMIT_MS).contains(&ms))
            && self.body.is_none_or(|total| total.average().is_some())
            && self.recent.as_ref().is_none_or(|sample| {
                sample.valid()
                    && sample.at >= self.started
                    && (sample.at - self.started) as f64 <= LIMIT_MS
            })
            && self.reported.as_ref().is_none_or(MessageUsage::valid)
    }
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Saved {
    version: u8,
    rows: Vec<Timing>,
}
#[derive(Default)]
pub(super) struct Timings {
    path: Option<PathBuf>,
    rows: BTreeMap<(String, String), Timing>,
    dirty: bool,
    pub error: Option<String>,
}
impl Timings {
    pub fn open(path: PathBuf) -> Self {
        let mut result = Self {
            path: Some(path.clone()),
            ..Default::default()
        };
        let read = || -> Result<Option<Saved>, ()> {
            let file = files::read(&path).map_err(|_| ())?;
            let Some(bytes) = file.bytes else {
                return Ok(None);
            };
            if file.stamp.mode & 0o077 != 0 {
                return Err(());
            }
            let saved: Saved = serde_json::from_slice(&bytes).map_err(|_| ())?;
            if ![1, 2, 3].contains(&saved.version)
                || saved.rows.len() > CAPACITY
                || saved.rows.iter().any(|row| !row.valid())
            {
                return Err(());
            }
            Ok(Some(saved))
        };
        match read() {
            Ok(Some(saved)) => {
                for row in saved.rows {
                    result
                        .rows
                        .entry((row.session.clone(), row.turn.clone()))
                        .or_insert(row);
                }
            }
            Ok(None) => {}
            Err(()) => result.error = Some("原生指标记录暂不可恢复，已测数值将重新保存".into()),
        }
        result
    }
    pub fn text_totals(&self) -> HashMap<(String, String), TextTotals> {
        self.rows
            .iter()
            .filter_map(|(key, row)| row.body.map(|total| (key.clone(), total)))
            .collect()
    }
    /// Only called for source-authorized rows. UUIDs are the native server IDs,
    /// never UI entity IDs, titles, or the time of the latest tool step.
    pub fn apply(
        &mut self,
        session: &str,
        turn: &str,
        started: Option<i64>,
        measurement: &mut NativeRealtime,
        body: Option<TextTotals>,
    ) {
        let key = (session.to_owned(), turn.to_owned());
        let saved = self.rows.get(&key);
        let started = saved.map(|r| r.started).or(started).unwrap_or(0);
        let sampling = measurement.sampling.as_ref();
        let recent = sampling
            .and_then(|s| s.recent.clone())
            .into_iter()
            .chain(saved.and_then(|r| r.recent.clone()))
            .filter(|s| s.valid() && s.at >= started && (s.at - started) as f64 <= LIMIT_MS)
            .max_by_key(|s| s.at);
        let mut reported = saved.and_then(|r| r.reported.clone());
        MessageUsage::merge(
            &mut reported,
            sampling.and_then(|s| s.reported.clone()).filter(|u| {
                chrono::DateTime::parse_from_rfc3339(&u.at)
                    .is_ok_and(|at| at.timestamp_millis() >= started)
            }),
        );
        let row = Timing {
            session: session.into(),
            turn: turn.into(),
            started,
            ttft_ms: saved.and_then(|r| r.ttft_ms).or(measurement.first_token_ms),
            body: body.or_else(|| saved.and_then(|r| r.body)),
            recent,
            reported,
        };
        if row.valid() && saved != Some(&row) {
            self.rows.insert(key.clone(), row);
            self.dirty = true;
        }
        if let Some(saved) = self.rows.get(&key) {
            if let Some(sampling) = &mut measurement.sampling {
                sampling.recent = saved.recent.clone();
                sampling.reported = saved.reported.clone();
                // Persisted samples are historical, never restored into the live window.
            }
            // Keep the measured duration, even if a recovered native start
            // clock has different precision. Server turn IDs are immutable.
            if let Some(ttft) = saved.ttft_ms {
                measurement.first_token_ms = Some(ttft);
                measurement.first_token_missed = false;
            }
            if let Some(average) = saved.body.and_then(TextTotals::average) {
                measurement.average_text_tokens_per_second = Some(average);
            }
        }
        while self.rows.len() > CAPACITY {
            let oldest = self
                .rows
                .iter()
                .min_by_key(|(key, row)| (row.started, *key))
                .map(|(key, _)| key.clone())
                .expect("nonempty timings");
            self.rows.remove(&oldest);
        }
    }
    pub fn forget(&mut self) {
        self.dirty |= !self.rows.is_empty() || self.error.is_some();
        self.rows.clear();
    }
    pub fn commit(&mut self) -> Result<(), String> {
        if !self.dirty {
            return Ok(());
        }
        let Some(path) = &self.path else {
            return Ok(());
        };
        let write = || -> Result<(), String> {
            // Recheck the target as well as its parents; reject symlinks,
            // hardlinks, other owners, nonfiles and oversized replacements.
            let file = files::read(path).map_err(|_| "原生指标存储不可写".to_owned())?;
            if file.bytes.is_some() && file.stamp.mode & 0o077 != 0 {
                return Err("原生指标存储权限不安全".into());
            }
            let bytes = serde_json::to_vec(&Saved {
                version: 3,
                rows: self.rows.values().cloned().collect(),
            })
            .map_err(|_| "原生指标记录无法保存".to_owned())?;
            files::atomic_write(path, &bytes, 0o600)
                .map_err(|_| "原生指标尚未保存，请检查本地存储".to_owned())
        };
        match write() {
            Ok(()) => {
                self.dirty = false;
                self.error = None;
                Ok(())
            }
            Err(error) => {
                self.error = Some(error.clone());
                Err(error)
            }
        }
    }
}
