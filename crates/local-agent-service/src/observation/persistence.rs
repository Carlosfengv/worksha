//! One bounded, supervised metadata writer. Retrying storage never resends a model request.
use local_agent_core::observation::{
    ModelObservation, ObservationStore, ObservationWriteError, content_epoch::ContentEpoch,
};
use local_agent_core::sessions::activity_input_cache::{CachedInput, InputCache};
use serde::Serialize;
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::{Arc, Condvar, Mutex},
    time::{Duration, Instant},
};

const CAPACITY: usize = 4096;
const BATCH: usize = 100;
mod checkpoint;
pub use checkpoint::Checkpoint;

#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Health {
    pub pending: usize,
    pub last_success_at: Option<String>,
    pub error: Option<String>,
    pub dropped_updates: u64,
}
#[derive(Default)]
struct State {
    rows: HashMap<String, ModelObservation>,
    inputs: HashMap<String, CachedInput>,
    writing: usize,
    in_flight: HashSet<String>,
    stopped: bool,
    scope_changed: bool,
    epoch: Option<ContentEpoch>,
    checkpoint: Option<CheckpointState>,
    next_checkpoint: u64,
    checking_checkpoint: Option<u64>,
    health: Health,
}
struct CheckpointState {
    id: u64,
    ready: bool,
    rows: HashMap<String, ModelObservation>,
    inputs: HashMap<String, CachedInput>,
}
impl State {
    fn pending(&self) -> usize {
        self.rows.len()
            + self.inputs.len()
            + self.writing
            + self
                .checkpoint
                .as_ref()
                .map_or(0, |pause| pause.rows.len() + pause.inputs.len())
    }
}
pub struct Writer {
    shared: Arc<(Mutex<State>, Condvar)>,
    thread: Option<std::thread::JoinHandle<()>>,
}
enum Failure {
    Storage(String),
    ScopeChanged,
}
impl From<ObservationWriteError> for Failure {
    fn from(error: ObservationWriteError) -> Self {
        match error {
            ObservationWriteError::ScopeChanged => Self::ScopeChanged,
            _ => Self::Storage(error.to_string()),
        }
    }
}
impl Writer {
    pub fn enqueue_input(&self, input: CachedInput) {
        let (lock, changed) = &*self.shared;
        if let Ok(mut state) = lock.lock() {
            let deferred = state.checkpoint.is_some() && !state.inputs.contains_key(&input.key);
            let existing = if deferred {
                state
                    .checkpoint
                    .as_ref()
                    .is_some_and(|pause| pause.inputs.contains_key(&input.key))
            } else {
                state.inputs.contains_key(&input.key)
            };
            if state.stopped || (!existing && state.pending() >= CAPACITY) {
                state.health.dropped_updates += 1;
                return;
            }
            if deferred {
                if let Some(pause) = state.checkpoint.as_mut() {
                    pause.inputs.insert(input.key.clone(), input);
                }
            } else {
                state.inputs.insert(input.key.clone(), input);
            }
            changed.notify_all();
        }
    }
    pub fn pending_for_epoch(&self, epoch: &ContentEpoch) -> Option<HashSet<String>> {
        self.shared
            .0
            .lock()
            .ok()
            .filter(|s| !s.scope_changed && s.epoch.as_ref() == Some(epoch))
            .map(|s| {
                s.rows
                    .keys()
                    .cloned()
                    .chain(s.in_flight.iter().cloned())
                    .chain(
                        s.checkpoint
                            .iter()
                            .flat_map(|pause| pause.rows.keys().cloned()),
                    )
                    .collect()
            })
    }
    pub fn new(directory: PathBuf, epoch: Option<ContentEpoch>) -> Result<Self, String> {
        let shared = Arc::new((
            Mutex::new(State {
                epoch: epoch.clone(),
                ..Default::default()
            }),
            Condvar::new(),
        ));
        let worker = shared.clone();
        let thread = std::thread::Builder::new()
            .name("observation-writer".into())
            .spawn(move || run(worker, directory, epoch))
            .map_err(|_| "无法启动观测写入器")?;
        Ok(Self {
            shared,
            thread: Some(thread),
        })
    }
    pub fn enqueue(&self, row: ModelObservation) {
        let (lock, changed) = &*self.shared;
        if let Ok(mut state) = lock.lock() {
            let deferred = state.checkpoint.is_some() && !state.rows.contains_key(&row.id);
            let existing = if deferred {
                state
                    .checkpoint
                    .as_ref()
                    .is_some_and(|pause| pause.rows.contains_key(&row.id))
            } else {
                state.rows.contains_key(&row.id)
            };
            if state.stopped || (!existing && state.pending() >= CAPACITY) {
                state.health.dropped_updates += 1;
                return;
            }
            let rows = if deferred {
                let Some(pause) = state.checkpoint.as_mut() else {
                    return;
                };
                &mut pause.rows
            } else {
                &mut state.rows
            };
            if rows
                .get(&row.id)
                .is_none_or(|old| old.revision <= row.revision)
            {
                rows.insert(row.id.clone(), row);
            }
            changed.notify_all();
        }
    }
    pub fn health(&self) -> Health {
        self.shared
            .0
            .lock()
            .map(|state| {
                let mut health = state.health.clone();
                health.pending = state.pending();
                if health.dropped_updates > 0 {
                    health.error =
                        Some("观测写入积压超过容量，部分更新未入队；历史完整性未确认".into());
                }
                health
            })
            .unwrap_or_else(|_| Health {
                error: Some("观测写入器状态不可用".into()),
                ..Default::default()
            })
    }
    pub fn flush(&self, timeout: Duration) -> Result<(), String> {
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err("观测写入尚未全部提交，请检查存储状态".into());
            }
            let checkpoint = self.checkpoint(remaining)?;
            if checkpoint.is_empty()? {
                return Ok(());
            }
            drop(checkpoint);
        }
    }
}
impl Drop for Writer {
    fn drop(&mut self) {
        {
            let mut state = self
                .shared
                .0
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            checkpoint::release(&mut state, None);
            state.stopped = true;
            self.shared.1.notify_all();
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
fn run(shared: Arc<(Mutex<State>, Condvar)>, directory: PathBuf, mut epoch: Option<ContentEpoch>) {
    let (lock, changed) = &*shared;
    let mut store = None;
    let mut inputs = InputCache::open(directory.join("gateway-activity-inputs.json"));
    loop {
        let (batch, input_batch, checkpoint_id) = {
            let Ok(mut state) = lock.lock() else { return };
            while ((state.rows.is_empty()
                && state.inputs.is_empty()
                && state.checkpoint.as_ref().is_none_or(|pause| pause.ready))
                || state.scope_changed)
                && !state.stopped
            {
                let Ok(next) = changed.wait(state) else {
                    return;
                };
                state = next;
            }
            if state.scope_changed
                || (state.stopped && state.rows.is_empty() && state.inputs.is_empty())
            {
                return;
            }
            if state.rows.is_empty() && state.inputs.is_empty() {
                let Some(id) = state
                    .checkpoint
                    .as_ref()
                    .filter(|pause| !pause.ready)
                    .map(|pause| pause.id)
                else {
                    continue;
                };
                state.checking_checkpoint = Some(id);
                (Vec::new(), Vec::new(), Some(id))
            } else {
                let mut keys: Vec<_> = state.rows.values().collect();
                keys.sort_by_key(|r| (r.status == "in_progress", &r.started_at, &r.id));
                let ids: Vec<_> = keys.into_iter().take(BATCH).map(|r| r.id.clone()).collect();
                let batch: Vec<_> = ids.iter().filter_map(|id| state.rows.remove(id)).collect();
                let input_ids = state.inputs.keys().take(BATCH).cloned().collect::<Vec<_>>();
                let input_batch = input_ids
                    .iter()
                    .filter_map(|id| state.inputs.remove(id))
                    .collect::<Vec<_>>();
                state.writing = batch.len() + input_batch.len();
                state.in_flight = batch.iter().map(|r| r.id.clone()).collect();
                (batch, input_batch, None)
            }
        };
        let result = (|| {
            if checkpoint_id.is_some() {
                let expected = epoch
                    .as_ref()
                    .ok_or_else(|| Failure::Storage("观测内容版本不可确认".into()))?;
                ObservationStore::check_existing_epoch(&directory, expected)
                    .map_err(Failure::from)?;
                return Ok(());
            }
            if store.is_none() {
                store = Some(ObservationStore::open(&directory).map_err(Failure::Storage)?);
            }
            let store = store
                .as_ref()
                .ok_or_else(|| Failure::Storage("观测存储不可用".into()))?;
            // A known epoch never changes on retries. Initially unavailable
            // storage binds once on first successful open, as before; cleanup
            // must not proceed while that writer's scope/work is unverified.
            if epoch.is_none() {
                let initial = store.write_epoch().clone();
                lock.lock()
                    .map_err(|_| Failure::Storage("观测写入器不可用".into()))?
                    .epoch = Some(initial.clone());
                epoch = Some(initial);
            }
            let expected = epoch
                .as_ref()
                .ok_or_else(|| Failure::Storage("观测内容版本不可确认".into()))?;
            store
                .save_batch_scoped(&batch, expected)
                .map_err(Failure::from)?;
            for input in &input_batch {
                inputs.insert(input.clone());
            }
            inputs.commit().map_err(Failure::Storage)?;
            Ok(())
        })();
        let Ok(mut state) = lock.lock() else { return };
        state.writing = 0;
        state.in_flight.clear();
        state.checking_checkpoint = None;
        match result {
            Ok(()) => {
                state.health.error = None;
                if let Some(id) = checkpoint_id {
                    if let Some(pause) = state.checkpoint.as_mut().filter(|pause| pause.id == id) {
                        pause.ready = true;
                    }
                } else {
                    state.health.last_success_at = Some(chrono::Utc::now().to_rfc3339());
                }
            }
            Err(error) => {
                let message = match error {
                    Failure::ScopeChanged => {
                        state.scope_changed = true;
                        ObservationWriteError::ScopeChanged.to_string()
                    }
                    Failure::Storage(message) => message,
                };
                store = None;
                state.health.error = Some(message);
                for row in batch {
                    if state
                        .rows
                        .get(&row.id)
                        .is_none_or(|old| old.revision < row.revision)
                    {
                        state.rows.insert(row.id.clone(), row);
                    }
                }
                for input in input_batch {
                    if state
                        .inputs
                        .get(&input.key)
                        .is_none_or(|old| old.input.at <= input.input.at)
                    {
                        state.inputs.insert(input.key.clone(), input);
                    }
                }
                changed.notify_all();
                if state.stopped {
                    return;
                }
                if state.scope_changed {
                    // Keep pending rows/inputs visible; neither reopen/adopt a
                    // new epoch nor busy-loop on a permanent lifetime mismatch.
                    continue;
                }
                // Bounded retries leave model traffic independent of disk failure.
                if changed
                    .wait_timeout(state, Duration::from_millis(250))
                    .is_err()
                {
                    return;
                }
                continue;
            }
        }
        changed.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    mod checkpoint_tests;
    mod epoch_tests;
    fn row(revision: u64) -> ModelObservation {
        serde_json::from_value(serde_json::json!({
            "id":"r","agentId":"claude","provider":"fixture","endpoint":"http://localhost",
            "bindingRevision":1,"startedAt":"2026-10-21T00:00:00Z","status":if revision > 1 {"success"}else{"in_progress"},
            "protocol":"messages","revision":revision,"usage":{"inputTokens":10,"outputTokens":2,"totalTokens":12}
        })).unwrap()
    }
    #[test]
    fn storage_failure_is_visible_and_recovers_without_losing_the_latest_revision() {
        let temp = tempfile::tempdir().unwrap();
        let database = temp.path().join("observations.sqlite3");
        std::fs::create_dir(&database).unwrap();
        let writer = Writer::new(temp.path().canonicalize().unwrap(), None).unwrap();
        writer.enqueue(row(1));
        let deadline = Instant::now() + Duration::from_secs(2);
        while writer.health().error.is_none() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(writer.health().error.is_some());
        writer.enqueue(row(3));
        writer.enqueue(row(2));
        assert!(writer.flush(Duration::from_millis(20)).is_err());
        std::fs::remove_dir(database).unwrap();
        writer.flush(Duration::from_secs(3)).unwrap();
        assert!(writer.health().error.is_none());
        assert!(writer.health().last_success_at.is_some());
        let saved = ObservationStore::open(temp.path())
            .unwrap()
            .load_all()
            .unwrap();
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].revision, 3);
        assert_eq!(saved[0].usage.total_tokens, Some(12));
    }
    #[test]
    fn shutdown_joins_the_writer_and_keeps_terminal_metadata() {
        let temp = tempfile::tempdir().unwrap();
        {
            let writer = Writer::new(temp.path().canonicalize().unwrap(), None).unwrap();
            writer.enqueue(row(1));
            writer.enqueue(row(2));
        }
        let saved = ObservationStore::open(temp.path())
            .unwrap()
            .load_all()
            .unwrap();
        assert_eq!(saved[0].status, "success");
    }
}
