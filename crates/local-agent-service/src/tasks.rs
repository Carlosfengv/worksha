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

pub(crate) struct Collector {
    id: &'static str,
    interval: Duration,
    jobs: Mutex<Option<(Work, Cleanup)>>,
    handles: Arc<Mutex<Vec<tokio::task::JoinHandle<()>>>>,
}
impl BuiltinPlugin for Collector {
    fn display_name(&self) -> String {
        match self.id {
            "listeners" => "监听采集",
            "activity" => "实时活动采集",
            "sessions" => "会话扫描",
            _ => self.id,
        }
        .into()
    }
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
    fn optional_requires(&self) -> Vec<CapabilityKey> {
        match self.id {
            "listeners" => vec![CapabilityKey {
                name: "project.listeners",
                version: 1,
                instance: "native",
            }],
            "activity" | "sessions" => ["claude", "codex", "opencode"]
                .into_iter()
                .map(|instance| CapabilityKey {
                    name: "session.source",
                    version: 1,
                    instance,
                })
                .collect(),
            _ => vec![],
        }
    }
    fn capability_metadata(
        &self,
        _: &CapabilityKey,
    ) -> local_agent_core::plugin::builtin::CapabilityMetadata {
        use local_agent_core::plugin::builtin::{
            CapabilityKind, CapabilityMetadata, ResourceScope,
        };
        CapabilityMetadata {
            kind: CapabilityKind::Stream,
            resource_scope: ResourceScope::Application,
        }
    }
    fn prepare(&self) -> Result<PluginResources, PluginError> {
        Ok(PluginResources::default())
    }
    fn activate(
        &self,
        lease: &PluginLease,
        resources: &mut PluginResources,
    ) -> Result<(), PluginError> {
        let Some((work, cleanup)) = self.jobs.lock().unwrap_or_else(|e| e.into_inner()).clone()
        else {
            return Ok(());
        };
        let runtime = tokio::runtime::Handle::try_current()
            .map_err(|_| PluginError::new("unavailable", "后台采集运行时不可用"))?;
        let (cancelled, mut cancellation) = tokio::sync::watch::channel(false);
        let performed = Arc::new(AtomicBool::new(false));
        let cleanup_performed = performed.clone();
        resources.own(local_agent_core::plugin::ResourceKind::Cache, move || {
            // Rolling back an unpublished preparation must not clear the old
            // generation's cache, pause its observer, or otherwise do domain I/O.
            if cleanup_performed.load(Ordering::Acquire) {
                cleanup();
            }
        });
        resources.own(local_agent_core::plugin::ResourceKind::Task, move || {
            cancelled.send_replace(true);
        });
        let lease = lease.clone();
        let interval = self.interval;
        let handle = runtime.spawn(async move {
            while lease.is_preparing() && !*cancellation.borrow() {
                tokio::select! { biased; _ = cancellation.changed() => return,
                    _ = tokio::time::sleep(Duration::from_millis(1)) => {} }
            }
            if lease.ensure_active().is_err() || *cancellation.borrow() { return; }
            let mut timer = tokio::time::interval(interval);
            timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! { biased; _ = cancellation.changed() => break, _ = timer.tick() => {} }
                if *cancellation.borrow() || lease.publish(||performed.store(true, Ordering::Release)).is_err() { break; }
                let work = work.clone(); let active = lease.clone();
                // A blocking read is awaited rather than detached. Cancellation
                // revokes its lease; shared drain owns the bounded final wait.
                let _ = tokio::task::spawn_blocking(move || work(active)).await;
            }
        });
        let mut handles = self.handles.lock().unwrap_or_else(|e| e.into_inner());
        handles.retain(|handle| !handle.is_finished());
        handles.push(handle);
        Ok(())
    }
}
pub(crate) struct CollectorScheduler {
    host: Arc<PluginHost>,
    collectors: Vec<Arc<Collector>>,
    started: AtomicBool,
    handles: Arc<Mutex<Vec<tokio::task::JoinHandle<()>>>>,
}
impl CollectorScheduler {
    pub(crate) fn activity_active(&self) -> bool {
        self.started.load(Ordering::Acquire)
            && self
                .host
                .acquire(&CapabilityKey {
                    name: "collector",
                    version: 1,
                    instance: "activity",
                })
                .is_ok()
    }
    pub(crate) fn definitions() -> Vec<Arc<Collector>> {
        Self::definitions_for(vec![
            ("listeners", Duration::from_secs(10)),
            ("activity", Duration::from_secs(1)),
            ("sessions", Duration::from_secs(30)),
        ])
    }
    fn definitions_for(specs: Vec<(&'static str, Duration)>) -> Vec<Arc<Collector>> {
        let handles = Arc::new(Mutex::new(vec![]));
        specs
            .into_iter()
            .map(|(id, interval)| {
                Arc::new(Collector {
                    id,
                    interval,
                    jobs: Mutex::new(None),
                    handles: handles.clone(),
                })
            })
            .collect()
    }
    pub(crate) fn with_host(host: Arc<PluginHost>, collectors: Vec<Arc<Collector>>) -> Self {
        let handles = collectors
            .first()
            .map(|collector| collector.handles.clone())
            .unwrap_or_default();
        Self {
            host,
            collectors,
            started: AtomicBool::new(false),
            handles,
        }
    }
    #[cfg(test)]
    pub(crate) fn builtin() -> Result<Self, PluginError> {
        Self::from_definitions(Self::definitions())
    }
    #[cfg(test)]
    fn new(specs: Vec<(&'static str, Duration)>) -> Result<Self, PluginError> {
        Self::from_definitions(Self::definitions_for(specs))
    }
    #[cfg(test)]
    fn from_definitions(collectors: Vec<Arc<Collector>>) -> Result<Self, PluginError> {
        let descriptors: Vec<&dyn BuiltinPlugin> = collectors
            .iter()
            .map(|collector| collector.as_ref() as &dyn BuiltinPlugin)
            .collect();
        let host = Arc::new(PluginHost::mount(&descriptors)?);
        Ok(Self::with_host(host, collectors))
    }
    pub(crate) fn start(&self, mut jobs: BTreeMap<&'static str, (Work, Cleanup)>) -> bool {
        if self.started.swap(true, Ordering::AcqRel) {
            return false;
        }
        for collector in &self.collectors {
            let Some(job) = jobs.remove(collector.id) else {
                continue;
            };
            *collector.jobs.lock().unwrap_or_else(|e| e.into_inner()) = Some(job);
            let key = CapabilityKey {
                name: "collector",
                version: 1,
                instance: collector.id,
            };
            if let Ok(lease) = self.host.acquire(&key) {
                let mut resources = PluginResources::default();
                if collector.activate(&lease, &mut resources).is_ok() {
                    let _ = self.host.own_resources(&key, lease.epoch, resources);
                }
            }
        }
        true
    }
    pub(crate) fn stop_all(&self) {
        for collector in &self.collectors {
            let _ = self.host.stop(collector.id);
        }
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
    let readers = Arc::downgrade(&state.readonly.projects);
    let cleanup_listeners = listeners.clone();
    jobs.insert(
        "listeners",
        (
            Arc::new(move |lease| {
                let Some(readers) = readers.upgrade() else {
                    return;
                };
                let mut latest = readers.collect_listeners();
                if let Ok(mut saved) = listeners.lock() {
                    if lease.ensure_active().is_err() {
                        // Its scope already published the paused state. An old
                        // worker must never replace a newer generation's result.
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
    let observation = Arc::downgrade(&state.observation);
    let sessions = Arc::downgrade(&state.sessions);
    let cleanup_observation = observation.clone();
    jobs.insert(
        "activity",
        (
            Arc::new(move |lease| {
                let (Some(observation), Some(sessions)) =
                    (observation.upgrade(), sessions.upgrade())
                else {
                    return;
                };
                if lease.publish(|| observation.resume_native()).is_err() {
                    return;
                }
                observation.poll_native_guarded(&sessions, &|| lease.ensure_active().is_ok())
            }),
            Arc::new(move || {
                if let Some(observation) = cleanup_observation.upgrade() {
                    observation.pause_native();
                }
            }),
        ),
    );
    let sessions = Arc::downgrade(&state.sessions);
    let cleanup_sessions = sessions.clone();
    jobs.insert(
        "sessions",
        (
            Arc::new(move |lease| {
                if let Some(sessions) = sessions.upgrade() {
                    let _ = sessions.refresh_guarded(&|| lease.ensure_active().is_ok());
                }
            }),
            Arc::new(move || {
                if let Some(sessions) = cleanup_sessions.upgrade() {
                    let _ = sessions.pause_background_scan();
                }
            }),
        ),
    );
    state.readonly.collectors.start(jobs);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    #[tokio::test]
    async fn reactivation_runs_a_new_worker_and_discards_the_old_workers_final_publication() {
        let scheduler =
            CollectorScheduler::new(vec![("activity", Duration::from_secs(60))]).unwrap();
        let first = Arc::new(tokio::sync::Notify::new());
        let second = Arc::new(tokio::sync::Notify::new());
        let (release, gate) = std::sync::mpsc::channel();
        let gate = Arc::new(Mutex::new(gate));
        let calls = Arc::new(AtomicUsize::new(0));
        let value = Arc::new(AtomicUsize::new(0));
        let cleanup = Arc::new(AtomicUsize::new(0));
        let mut jobs: BTreeMap<&'static str, (Work, Cleanup)> = BTreeMap::new();
        jobs.insert(
            "activity",
            (
                {
                    let first = first.clone();
                    let second = second.clone();
                    let calls = calls.clone();
                    let value = value.clone();
                    Arc::new(move |lease| {
                        let call = calls.fetch_add(1, Ordering::SeqCst) + 1;
                        if call == 1 {
                            first.notify_one();
                            gate.lock()
                                .unwrap()
                                .recv_timeout(Duration::from_secs(5))
                                .unwrap();
                        }
                        let _ = lease.publish(|| value.store(call, Ordering::SeqCst));
                        if call == 2 {
                            second.notify_one();
                        }
                    })
                },
                {
                    let cleanup = cleanup.clone();
                    Arc::new(move || {
                        cleanup.fetch_add(1, Ordering::SeqCst);
                    })
                },
            ),
        );
        scheduler.start(jobs);
        tokio::time::timeout(Duration::from_secs(2), first.notified())
            .await
            .unwrap();
        let factories: Vec<&dyn BuiltinPlugin> = scheduler
            .collectors
            .iter()
            .map(|c| c.as_ref() as &dyn BuiltinPlugin)
            .collect();
        let enabled = std::collections::BTreeSet::from(["activity".to_owned()]);
        let generation = scheduler.host.snapshot().unwrap().generation;
        assert!(
            scheduler
                .host
                .reconfigure(&generation, &factories, &enabled, || Err(PluginError::new(
                    "fixture_failed",
                    "save failed"
                )))
                .is_err()
        );
        assert_eq!(cleanup.load(Ordering::SeqCst), 0);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        scheduler
            .host
            .reconfigure(&generation, &factories, &enabled, || Ok(()))
            .unwrap();
        tokio::time::timeout(Duration::from_secs(2), second.notified())
            .await
            .unwrap();
        assert_eq!(value.load(Ordering::SeqCst), 2);
        release.send(()).unwrap();
        assert!(scheduler.drain(Duration::from_secs(2)).await);
        assert_eq!(value.load(Ordering::SeqCst), 2);
        assert_eq!(cleanup.load(Ordering::SeqCst), 2);
    }
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
