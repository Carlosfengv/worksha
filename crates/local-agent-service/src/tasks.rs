//! Own the three existing polling loops. No collector is automatically added by registration.
use crate::AppState;
use local_agent_core::plugin::{
    BuiltinPlugin, CapabilityKey, PluginDescriptor, PluginError, PluginHost, PluginLease,
    PluginResources,
};
use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
type Work = Arc<dyn Fn(PluginLease) + Send + Sync>;
type Cleanup = Arc<dyn Fn() + Send + Sync>;

struct Collector {
    id: &'static str,
    interval: Duration,
    cancelled: tokio::sync::watch::Sender<bool>,
    cleanup: Arc<Mutex<Option<Cleanup>>>,
}
impl BuiltinPlugin for Collector {
    fn descriptor(&self) -> PluginDescriptor {
        PluginDescriptor {
            id: self.id,
            version: "1.0.0",
            requires: vec![],
            provides: vec![CapabilityKey {
                name: "collector",
                version: 1,
                instance: self.id,
            }],
        }
    }
    fn prepare(&self) -> Result<PluginResources, PluginError> {
        let cancelled = self.cancelled.clone();
        let cleanup = self.cleanup.clone();
        Ok(PluginResources::on_release(move || {
            cancelled.send_replace(true);
            let callback = cleanup.lock().unwrap_or_else(|e| e.into_inner()).take();
            if let Some(callback) = callback {
                callback();
            }
        }))
    }
}
pub(crate) struct CollectorScheduler {
    host: PluginHost,
    collectors: Vec<Collector>,
    started: AtomicBool,
    handles: Mutex<Vec<tokio::task::JoinHandle<()>>>,
}
impl CollectorScheduler {
    pub(crate) fn builtin() -> Result<Self, PluginError> {
        Self::new(vec![
            ("listeners", Duration::from_secs(10)),
            ("activity", Duration::from_secs(1)),
            ("sessions", Duration::from_secs(30)),
        ])
    }
    fn new(specs: Vec<(&'static str, Duration)>) -> Result<Self, PluginError> {
        let collectors: Vec<_> = specs
            .into_iter()
            .map(|(id, interval)| Collector {
                id,
                interval,
                cancelled: tokio::sync::watch::channel(false).0,
                cleanup: Arc::new(Mutex::new(None)),
            })
            .collect();
        let descriptors: Vec<&dyn BuiltinPlugin> =
            collectors.iter().map(|c| c as &dyn BuiltinPlugin).collect();
        let host = PluginHost::mount(&descriptors)?;
        Ok(Self {
            host,
            collectors,
            started: AtomicBool::new(false),
            handles: Mutex::new(vec![]),
        })
    }
    pub(crate) fn start(&self, mut jobs: BTreeMap<&'static str, (Work, Cleanup)>) -> bool {
        if self.started.swap(true, Ordering::AcqRel) {
            return false;
        }
        let mut handles = self.handles.lock().unwrap_or_else(|e| e.into_inner());
        for collector in &self.collectors {
            let Some((work, cleanup)) = jobs.remove(collector.id) else {
                continue;
            };
            let capability = CapabilityKey {
                name: "collector",
                version: 1,
                instance: collector.id,
            };
            let Ok(lease) = self.host.acquire(&capability) else {
                cleanup();
                continue;
            };
            *collector.cleanup.lock().unwrap_or_else(|e| e.into_inner()) = Some(cleanup);
            if lease.ensure_active().is_err() {
                let callback = collector
                    .cleanup
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .take();
                if let Some(callback) = callback {
                    callback();
                }
                continue;
            }
            let mut cancelled = collector.cancelled.subscribe();
            let interval = collector.interval;
            handles.push(tokio::spawn(async move {
                let mut timer = tokio::time::interval(interval);
                loop {
                    tokio::select! { biased;
                        _ = cancelled.changed() => break,
                        _ = timer.tick() => {}
                    }
                    if lease.ensure_active().is_err() {
                        break;
                    }
                    let work = work.clone();
                    let active = lease.clone();
                    // Await the blocking worker; never overlap executions of this collector.
                    let _ = tokio::task::spawn_blocking(move || work(active)).await;
                }
            }));
        }
        true
    }
    pub(crate) fn stop_all(&self) {
        self.host.stop_all();
    }
    #[cfg(test)]
    fn stop(&self, id: &str) {
        self.host.stop(id).unwrap();
    }
    pub(crate) async fn drain(&self, budget: Duration) -> bool {
        self.stop_all();
        let mut handles =
            std::mem::take(&mut *self.handles.lock().unwrap_or_else(|e| e.into_inner()));
        let complete = tokio::time::timeout(budget, async {
            for handle in &mut handles {
                let _ = handle.await;
            }
        })
        .await
        .is_ok();
        if !complete {
            for handle in handles {
                handle.abort();
            }
        }
        complete
    }
}

pub(crate) fn start(state: &AppState) {
    let mut jobs: BTreeMap<&'static str, (Work, Cleanup)> = BTreeMap::new();
    let listeners = state.listeners.clone();
    let readers = state.readonly.projects.clone();
    let cleanup_listeners = listeners.clone();
    jobs.insert(
        "listeners",
        (
            Arc::new(move |lease| {
                let mut latest = readers.collect_listeners();
                if let Ok(mut saved) = listeners.lock() {
                    if lease.ensure_active().is_err() {
                        *saved = local_agent_core::projects::listeners::ListenerSnapshot {
                            state: "unavailable".into(),
                            error: Some("监听采集器已停止".into()),
                            ..Default::default()
                        };
                        return;
                    }
                    if latest.state == "unavailable"
                        && saved.as_of.is_some()
                        && readers.listeners_active()
                    {
                        latest.items = saved.items.clone();
                        latest.as_of = saved.as_of.clone();
                        latest.state = "stale".into();
                    }
                    *saved = latest;
                }
            }),
            Arc::new(move || {
                // Collection happens outside this short publication lock. Wait for
                // an in-flight publication so it cannot overwrite the paused state.
                if let Ok(mut saved) = cleanup_listeners.lock() {
                    saved.state = if saved.as_of.is_some() {
                        "stale"
                    } else {
                        "unavailable"
                    }
                    .into();
                    saved.error = Some("监听采集器已停止".into());
                }
            }),
        ),
    );
    let observation = state.observation.clone();
    let sessions = state.sessions.clone();
    let cleanup_observation = observation.clone();
    jobs.insert(
        "activity",
        (
            Arc::new(move |lease| {
                observation.poll_native_guarded(&sessions, &|| lease.ensure_active().is_ok())
            }),
            Arc::new(move || cleanup_observation.pause_native()),
        ),
    );
    let sessions = state.sessions.clone();
    let cleanup_sessions = sessions.clone();
    jobs.insert(
        "sessions",
        (
            Arc::new(move |lease| {
                let _ = sessions.refresh_guarded(&|| lease.ensure_active().is_ok());
            }),
            Arc::new(move || {
                let _ = cleanup_sessions.pause_background_scan();
            }),
        ),
    );
    state.readonly.collectors.start(jobs);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    #[test]
    fn registration_keeps_the_original_intervals_without_starting_tasks() {
        let scheduler = CollectorScheduler::builtin().unwrap();
        assert_eq!(
            scheduler
                .collectors
                .iter()
                .map(|c| (c.id, c.interval.as_secs()))
                .collect::<Vec<_>>(),
            [("listeners", 10), ("activity", 1), ("sessions", 30)]
        );
        assert!(scheduler.handles.lock().unwrap().is_empty());
        assert!(!scheduler.started.load(Ordering::Acquire));
    }
    #[tokio::test]
    async fn one_total_drain_budget_cancels_a_pending_worker() {
        let scheduler =
            CollectorScheduler::new(vec![("pending", Duration::from_secs(30))]).unwrap();
        let (release, gate) = std::sync::mpsc::channel();
        let gate = Arc::new(Mutex::new(gate));
        let entered = Arc::new(tokio::sync::Notify::new());
        let ready = entered.clone();
        let published = Arc::new(AtomicBool::new(false));
        let late = published.clone();
        let mut jobs: BTreeMap<&'static str, (Work, Cleanup)> = BTreeMap::new();
        jobs.insert(
            "pending",
            (
                Arc::new(move |lease| {
                    ready.notify_one();
                    gate.lock()
                        .unwrap()
                        .recv_timeout(Duration::from_secs(5))
                        .unwrap();
                    if lease.ensure_active().is_ok() {
                        late.store(true, Ordering::Release);
                    }
                }),
                Arc::new(|| {}),
            ),
        );
        scheduler.start(jobs);
        entered.notified().await;
        assert!(!scheduler.drain(Duration::from_millis(1)).await);
        release.send(()).unwrap();
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert!(!published.load(Ordering::Acquire));
    }
    #[tokio::test]
    async fn one_worker_per_collector_and_independent_stop_and_shared_drain() {
        let scheduler = CollectorScheduler::new(vec![
            ("slow", Duration::from_millis(5)),
            ("other", Duration::from_millis(5)),
        ])
        .unwrap();
        let active = Arc::new(AtomicUsize::new(0));
        let peak = active.clone();
        let entered = Arc::new(tokio::sync::Notify::new());
        let (release, gate) = std::sync::mpsc::channel();
        let gate = Arc::new(Mutex::new(gate));
        let late_published = Arc::new(AtomicBool::new(false));
        let published = late_published.clone();
        let work_entered = entered.clone();
        let active_work = active.clone();
        let other = Arc::new(AtomicUsize::new(0));
        let other_work = other.clone();
        let cleanup = Arc::new(AtomicBool::new(false));
        let cleaned = cleanup.clone();
        let mut jobs: BTreeMap<&'static str, (Work, Cleanup)> = BTreeMap::new();
        jobs.insert(
            "slow",
            (
                Arc::new(move |lease| {
                    assert_eq!(active_work.fetch_add(1, Ordering::SeqCst), 0);
                    work_entered.notify_one();
                    gate.lock()
                        .unwrap()
                        .recv_timeout(Duration::from_secs(5))
                        .unwrap();
                    if lease.ensure_active().is_ok() {
                        published.store(true, Ordering::SeqCst);
                    }
                    active_work.fetch_sub(1, Ordering::SeqCst);
                }),
                Arc::new(move || {
                    cleaned.store(true, Ordering::SeqCst);
                }),
            ),
        );
        jobs.insert(
            "other",
            (
                Arc::new(move |_| {
                    other_work.fetch_add(1, Ordering::SeqCst);
                }),
                Arc::new(|| {}),
            ),
        );
        assert!(scheduler.start(jobs));
        assert!(!scheduler.start(BTreeMap::new()));
        entered.notified().await;
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(peak.load(Ordering::SeqCst), 1);
        scheduler.stop("slow");
        assert!(cleanup.load(Ordering::SeqCst));
        release.send(()).unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(!late_published.load(Ordering::SeqCst));
        assert!(other.load(Ordering::SeqCst) > 1);
        assert!(scheduler.drain(Duration::from_secs(1)).await);
        assert_eq!(active.load(Ordering::SeqCst), 0);
    }
}
