//! Coalesce request-triggered scans and committed revocation cleanup before spawn.
use crate::AppState;
use axum::{Json, http::StatusCode};
use std::{
    collections::BTreeSet,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::Mutex,
    time::Duration,
};
use tokio::sync::Notify;

#[cfg(test)]
mod tests;

#[derive(Default)]
struct Batch {
    refresh: bool,
    // Only IDs accepted by NativeSessions::set_enabled enter this set. Its
    // cardinality is bounded by the registered native source directory.
    revoked: BTreeSet<String>,
}
#[derive(Default)]
struct Queue {
    running: bool,
    stopped: bool,
    pending: Batch,
}
#[derive(Default)]
pub(crate) struct SessionWork {
    queue: Mutex<Queue>,
    changed: Notify,
    #[cfg(test)]
    completed: std::sync::atomic::AtomicUsize,
    #[cfg(test)]
    pub(super) source_started: Notify,
}

fn unavailable(committed: bool) -> (StatusCode, Json<serde_json::Value>) {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(
            serde_json::json!({"code":"unavailable","message":"会话后台工作暂不可用","outcomeUnknown":committed}),
        ),
    )
}

impl SessionWork {
    pub(crate) fn ensure_open(&self) -> Result<(), (StatusCode, Json<serde_json::Value>)> {
        if self.queue.lock().unwrap_or_else(|e| e.into_inner()).stopped {
            Err(unavailable(false))
        } else {
            Ok(())
        }
    }

    fn enqueue(
        &self,
        revoked: Option<String>,
        committed: bool,
    ) -> Result<bool, (StatusCode, Json<serde_json::Value>)> {
        let mut queue = self.queue.lock().unwrap_or_else(|e| e.into_inner());
        if queue.stopped && !committed {
            return Err(unavailable(false));
        }
        if let Some(agent) = revoked {
            queue.pending.revoked.insert(agent);
        }
        queue.pending.refresh |= !queue.stopped;
        if queue.running || (!queue.pending.refresh && queue.pending.revoked.is_empty()) {
            return Ok(false);
        }
        queue.running = true;
        Ok(true)
    }

    fn run(&self, mut process: impl FnMut(&Batch)) {
        loop {
            let batch = {
                let mut queue = self.queue.lock().unwrap_or_else(|e| e.into_inner());
                if !queue.pending.refresh && queue.pending.revoked.is_empty() {
                    queue.running = false;
                    self.changed.notify_waiters();
                    return;
                }
                std::mem::take(&mut queue.pending)
            };
            if catch_unwind(AssertUnwindSafe(|| process(&batch))).is_err() {
                // Keep unfinished idempotent cleanup for an explicit later trigger;
                // do not spin or silently discard it after a worker panic.
                let mut queue = self.queue.lock().unwrap_or_else(|e| e.into_inner());
                queue.pending.revoked.extend(batch.revoked);
                queue.pending.refresh |= batch.refresh && !queue.stopped;
                queue.running = false;
                self.changed.notify_waiters();
                return;
            }
            #[cfg(test)]
            self.completed
                .fetch_add(1, std::sync::atomic::Ordering::Release);
        }
    }

    pub(crate) fn stop(&self) {
        let mut queue = self.queue.lock().unwrap_or_else(|e| e.into_inner());
        queue.stopped = true;
        queue.pending.refresh = false;
        self.changed.notify_waiters();
    }

    pub(crate) async fn drain(&self, budget: Duration) -> bool {
        tokio::time::timeout(budget, async {
            loop {
                let changed = self.changed.notified();
                tokio::pin!(changed);
                changed.as_mut().enable();
                {
                    let queue = self.queue.lock().unwrap_or_else(|e| e.into_inner());
                    if !queue.running {
                        return !queue.pending.refresh && queue.pending.revoked.is_empty();
                    }
                }
                changed.await;
            }
        })
        .await
        .unwrap_or(false)
    }
}

fn schedule(
    state: &AppState,
    revoked: Option<String>,
    committed: bool,
) -> Result<(), (StatusCode, Json<serde_json::Value>)> {
    let runtime = tokio::runtime::Handle::try_current().map_err(|_| unavailable(committed))?;
    if state.session_work.enqueue(revoked, committed)? {
        let state = state.clone();
        runtime.spawn_blocking(move || {
            state.session_work.run(|batch| {
                for agent in &batch.revoked {
                    state.observation.forget_native_inputs(agent);
                }
                if !batch.revoked.is_empty() {
                    state.observation.release_native_snapshots();
                }
                if batch.refresh {
                    let _ = state
                        .sessions
                        .refresh_guarded(&|| state.session_work.ensure_open().is_ok());
                }
            });
        });
    }
    Ok(())
}

pub(super) fn refresh(state: &AppState) -> Result<(), (StatusCode, Json<serde_json::Value>)> {
    schedule(state, None, false)
}

pub(super) fn after_source_change(
    state: &AppState,
    revoked: Option<String>,
) -> Result<(), (StatusCode, Json<serde_json::Value>)> {
    schedule(state, revoked, true)
}
