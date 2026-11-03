//! Host-only, bounded process diagnostics. Reads never start a guest or touch sources.
use super::{Cached, Manager, Result};
use local_agent_core::plugin::external::Status;
use serde::Serialize;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

const MAX_ENTRIES: usize = 192;

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum Operation {
    Enable,
    Disable,
    Remove,
    Rollback,
    RevokeGrants,
    RevokePublisher,
    Refresh,
    ReadSession,
    AuthorizeReport,
    BindConnection,
    SaveSettings,
    CreateProvider,
    BindProtocol,
    UnbindProtocol,
    PrepareConfig,
    ApplyConfig,
    RestoreConfig,
    DiscardConfig,
    StartConnector,
    StopConnector,
    RefreshConnector,
}

#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct Counts {
    attempts: u64,
    succeeded: u64,
    failed: u64,
    interrupted: u64,
    timeouts: u64,
    resource_limits: u64,
    worker_crashes: u64,
    in_flight: u64,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Activity {
    since: String,
    #[serde(flatten)]
    counts: Counts,
    last_operation: Option<Operation>,
    last_finished_at: Option<String>,
    last_success_at: Option<String>,
    last_error_code: Option<&'static str>,
    last_duration_ms: Option<String>,
}
impl Default for Activity {
    fn default() -> Self {
        Self {
            since: chrono::Utc::now().to_rfc3339(),
            counts: Counts::default(),
            last_operation: None,
            last_finished_at: None,
            last_success_at: None,
            last_error_code: None,
            last_duration_ms: None,
        }
    }
}
pub(super) struct Diagnostics {
    since: String,
    state: Arc<Mutex<State>>,
}
#[derive(Default)]
struct State {
    entries: BTreeMap<(String, String), Activity>,
    dropped: u64,
}
impl Default for Diagnostics {
    fn default() -> Self {
        Self {
            since: chrono::Utc::now().to_rfc3339(),
            state: Arc::default(),
        }
    }
}

pub(super) struct Observation {
    state: Arc<Mutex<State>>,
    key: Option<(String, String)>,
    operation: Operation,
    started: Instant,
    finished: bool,
}
impl Observation {
    pub(super) fn finish<T>(&mut self, result: &Result<T>) {
        self.complete(Some(result.as_ref().map(|_| ()).map_err(|e| e.code)));
    }
    fn complete(&mut self, result: Option<std::result::Result<(), &'static str>>) {
        if self.finished {
            return;
        }
        self.finished = true;
        let Some(key) = &self.key else { return };
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        let Some(row) = state.entries.get_mut(key) else {
            return;
        };
        row.counts.in_flight = row.counts.in_flight.saturating_sub(1);
        row.last_operation = Some(self.operation);
        row.last_finished_at = Some(chrono::Utc::now().to_rfc3339());
        row.last_duration_ms = Some(self.started.elapsed().as_millis().to_string());
        match result {
            Some(Ok(())) => {
                row.counts.succeeded = row.counts.succeeded.saturating_add(1);
                row.last_success_at = row.last_finished_at.clone();
                row.last_error_code = None;
            }
            Some(Err(code)) => {
                row.counts.failed = row.counts.failed.saturating_add(1);
                row.counts.timeouts = row
                    .counts
                    .timeouts
                    .saturating_add(u64::from(code == "deadline_exceeded"));
                row.counts.resource_limits = row
                    .counts
                    .resource_limits
                    .saturating_add(u64::from(code == "resource_limit"));
                row.counts.worker_crashes = row
                    .counts
                    .worker_crashes
                    .saturating_add(u64::from(code == "worker_crashed"));
                row.last_error_code = Some(code);
            }
            None => {
                row.counts.interrupted = row.counts.interrupted.saturating_add(1);
                row.last_error_code = Some("interrupted");
            }
        }
    }
}
impl Drop for Observation {
    fn drop(&mut self) {
        self.complete(None);
    }
}

impl Manager {
    pub(super) fn observe(&self, id: &str, digest: &str, operation: Operation) -> Observation {
        // Bad or arbitrary identities must not grow diagnostics. Diagnostics failure
        // never changes the result of the operation being observed.
        let key = if self.installed(id, digest).is_ok() {
            let key = (id.to_owned(), digest.to_owned());
            self.diagnostics.state.lock().ok().and_then(|mut state| {
                if state.entries.len() >= MAX_ENTRIES && !state.entries.contains_key(&key) {
                    state.dropped = state.dropped.saturating_add(1);
                    return None;
                }
                let row = state.entries.entry(key.clone()).or_default();
                row.counts.attempts = row.counts.attempts.saturating_add(1);
                row.counts.in_flight = row.counts.in_flight.saturating_add(1);
                Some(key)
            })
        } else {
            None
        };
        Observation {
            state: self.diagnostics.state.clone(),
            key,
            operation,
            started: Instant::now(),
            finished: false,
        }
    }

    pub(super) fn diagnostics(&self, items: &[Status]) -> serde_json::Value {
        let Ok(state) = self.diagnostics.state.lock() else {
            return serde_json::json!({"version":1,"available":false});
        };
        let Ok(cache) = self.cache.lock() else {
            return serde_json::json!({"version":1,"available":false});
        };
        let rows: Vec<_> = items.iter().map(|s| {
            let activity = state.entries.get(&(s.manifest.id.clone(),s.digest.clone())).map(|a| {
                // Counts cross the renderer boundary as exact decimal strings.
                let mut value = serde_json::to_value(a).expect("diagnostic DTO");
                for name in ["attempts","succeeded","failed","interrupted","timeouts","resourceLimits","workerCrashes","inFlight"] {
                    value[name] = value[name].as_u64().expect("counter").to_string().into();
                }
                value
            });
            let applicable = s.manifest.capabilities.iter().any(|c| matches!(c.as_str(), "session.source@1"|"account.quota-source@1"|"report.view@1"));
            let authorized = (!s.manifest.capabilities.iter().any(|c| c == "session.source@1") || s.source_authorized)
                && (!s.manifest.capabilities.iter().any(|c| c == "account.quota-source@1") || s.connection_authorized)
                && (!s.manifest.capabilities.iter().any(|c| c == "report.view@1") || s.report_authorized);
            let saved = cache.get(&s.manifest.id).filter(|c| s.enabled && authorized && c.digest == s.digest && c.epoch == s.activation_epoch && c.revision == s.grant_revision);
            let snapshot = if !applicable { "not-applicable" }
                else if !s.enabled { "disabled" }
                else if !authorized { "awaiting-authorization" }
                else if let Some(c) = saved {
                    if c.expired() || s.error_code.is_some() { "stale" }
                    else if c.sessions.is_empty() && c.quota.is_none() && c.report.is_none() { "confirmed-empty" }
                    else { "available" }
                } else { "not-read" };
            serde_json::json!({"pluginId":s.manifest.id,"digest":s.digest,"activity":activity,
                "snapshot":{"state":snapshot,"observedAt":saved.map(|c|&c.observed_at),
                    "records":saved.filter(|_|s.manifest.capabilities.iter().any(|c|c=="session.source@1")).map(|c|c.sessions.len().to_string())}})
        }).collect();
        serde_json::json!({"version":1,"available":true,"scope":"service-lifetime","since":self.diagnostics.since,"dropped":state.dropped.to_string(),"items":rows})
    }
}
impl Cached {
    pub(super) fn expired(&self) -> bool {
        self.collected_at.elapsed() > Duration::from_secs(300)
            || self
                .quota
                .as_ref()
                .and_then(|q| q.expires_at.as_ref())
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .is_some_and(|d| d < chrono::Utc::now())
    }
}
