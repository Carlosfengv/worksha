use super::*;
use std::{
    collections::BTreeSet,
    sync::atomic::{AtomicBool, Ordering},
};

struct Fixture {
    id: &'static str,
    fail: Option<&'static str>,
    events: Arc<Mutex<Vec<String>>>,
}
fn key(id: &'static str) -> CapabilityKey {
    CapabilityKey {
        name: "fixture.operation",
        version: 1,
        instance: id,
    }
}
fn fixture(id: &'static str, events: &Arc<Mutex<Vec<String>>>) -> Fixture {
    Fixture {
        id,
        fail: None,
        events: events.clone(),
    }
}
fn selected(ids: &[&str]) -> BTreeSet<String> {
    ids.iter().map(|id| (*id).into()).collect()
}
impl BuiltinPlugin for Fixture {
    fn descriptor(&self) -> PluginDescriptor {
        PluginDescriptor {
            id: self.id,
            version: "1.2.3",
            provides: vec![key(self.id)],
            requires: vec![],
        }
    }
    fn prepare(&self) -> Result<PluginResources, PluginError> {
        self.events
            .lock()
            .unwrap()
            .push(format!("prepare:{}", self.id));
        if self.fail == Some("prepare") {
            return Err(PluginError::new("fixture_failed", "fixture prepare"));
        }
        let events = self.events.clone();
        let id = self.id;
        let mut resources = PluginResources::default();
        resources.own(ResourceKind::Cache, move || {
            events.lock().unwrap().push(format!("release:{id}"))
        });
        Ok(resources)
    }
    fn activate(
        &self,
        lease: &PluginLease,
        resources: &mut PluginResources,
    ) -> Result<(), PluginError> {
        assert!(lease.is_preparing());
        assert!(lease.ensure_active().is_err());
        let events = self.events.clone();
        let id = self.id;
        resources.own(ResourceKind::Worker, move || {
            events.lock().unwrap().push(format!("cancel:{id}"))
        });
        if self.fail == Some("activate") {
            return Err(PluginError::new("fixture_failed", "fixture activate"));
        }
        Ok(())
    }
    fn health_check(&self) -> Result<(), PluginError> {
        if self.fail == Some("health") {
            Err(PluginError::new("fixture_failed", "fixture health"))
        } else {
            Ok(())
        }
    }
}

#[test]
fn preparation_activation_and_health_failures_preserve_the_published_generation() {
    for stage in ["prepare", "activate", "health"] {
        let events = Arc::new(Mutex::new(vec![]));
        let original = fixture("original", &events);
        let host = PluginHost::mount(&[&original]).unwrap();
        let before = host.snapshot().unwrap();
        let lease = host.acquire(&key("original")).unwrap();
        let a = fixture("a", &events);
        let mut b = fixture("b", &events);
        b.fail = Some(stage);
        let committed = AtomicBool::new(false);
        assert!(
            host.reconfigure(
                &before.generation,
                &[&a, &b],
                &selected(&["a", "b"]),
                || {
                    committed.store(true, Ordering::Release);
                    Ok(())
                }
            )
            .is_err()
        );
        assert!(!committed.load(Ordering::Acquire));
        assert_eq!(host.snapshot().unwrap(), before);
        lease.ensure_active().unwrap();
        let events = events.lock().unwrap();
        assert!(events.contains(&"cancel:a".into()));
        assert!(events.contains(&"release:a".into()));
        if stage != "prepare" {
            let b_cancel = events.iter().position(|s| s == "cancel:b").unwrap();
            let b_release = events.iter().position(|s| s == "release:b").unwrap();
            let a_cancel = events.iter().position(|s| s == "cancel:a").unwrap();
            assert!(b_cancel < b_release && b_release < a_cancel);
        }
    }
}

#[test]
fn failed_persistence_and_stale_generation_leave_live_resources_untouched() {
    let events = Arc::new(Mutex::new(vec![]));
    let a = fixture("a", &events);
    let host = PluginHost::mount(&[&a]).unwrap();
    let before = host.snapshot().unwrap();
    let lease = host.acquire(&key("a")).unwrap();
    assert!(
        host.reconfigure(&before.generation, &[&a], &selected(&["a"]), || Err(
            PluginError::new("save_failed", "fixture")
        ))
        .is_err()
    );
    lease.ensure_active().unwrap();
    assert_eq!(host.snapshot().unwrap(), before);
    let count = events.lock().unwrap().len();
    assert_eq!(
        host.reconfigure("foreign", &[&a], &selected(&["a"]), || panic!(
            "stale write"
        ))
        .unwrap_err()
        .code,
        "stale_snapshot"
    );
    assert_eq!(events.lock().unwrap().len(), count);
}

#[test]
fn desired_disabled_is_discoverable_without_preparing_and_reenable_never_revives_old_calls() {
    let events = Arc::new(Mutex::new(vec![]));
    let a = fixture("a", &events);
    let host = PluginHost::mount(&[&a]).unwrap();
    let old = host.acquire(&key("a")).unwrap();
    let before = host.snapshot().unwrap();
    events.lock().unwrap().clear();
    let disabled = host
        .reconfigure(&before.generation, &[&a], &selected(&[]), || Ok(()))
        .unwrap();
    assert_eq!(*events.lock().unwrap(), ["cancel:a", "release:a"]);
    assert!(!disabled.plugins[0].desired_enabled && !disabled.plugins[0].runtime_active);
    assert!(disabled.plugins[0].resources.is_empty());
    assert!(host.acquire(&key("a")).is_err());
    let next = host
        .reconfigure(&disabled.generation, &[&a], &selected(&["a"]), || Ok(()))
        .unwrap();
    let fresh = host.acquire(&key("a")).unwrap();
    assert_ne!(old.epoch, fresh.epoch);
    assert!(old.ensure_active().is_err());
    assert!(next.plugins[0].runtime_active);
    fresh.ensure_active().unwrap();
}

#[test]
fn an_old_publication_finishes_before_switch_and_cannot_write_into_the_new_activation() {
    let events = Arc::new(Mutex::new(vec![]));
    let a = Arc::new(fixture("a", &events));
    let host = Arc::new(PluginHost::mount(&[a.as_ref()]).unwrap());
    let before = host.snapshot().unwrap();
    let old = host.acquire(&key("a")).unwrap();
    let saved = Arc::new(Mutex::new(0));
    let (entered, ready) = std::sync::mpsc::channel();
    let (resume, gate) = std::sync::mpsc::channel();
    let writer = std::thread::spawn({
        let lease = old.clone();
        let saved = saved.clone();
        move || {
            lease
                .publish(|| {
                    entered.send(()).unwrap();
                    gate.recv_timeout(std::time::Duration::from_secs(5))
                        .unwrap();
                    *saved.lock().unwrap() = 1;
                })
                .unwrap()
        }
    });
    ready
        .recv_timeout(std::time::Duration::from_secs(5))
        .unwrap();
    let switch = std::thread::spawn({
        let host = host.clone();
        let a = a.clone();
        move || {
            host.reconfigure(&before.generation, &[a.as_ref()], &selected(&["a"]), || {
                Ok(())
            })
            .unwrap()
        }
    });
    resume.send(()).unwrap();
    writer.join().unwrap();
    switch.join().unwrap();
    host.acquire(&key("a"))
        .unwrap()
        .publish(|| *saved.lock().unwrap() = 2)
        .unwrap();
    assert!(old.publish(|| *saved.lock().unwrap() = 3).is_err());
    assert_eq!(*saved.lock().unwrap(), 2);
}

#[test]
fn stale_resource_ownership_is_rejected_and_all_resource_kinds_release_once_in_reverse_order() {
    let events = Arc::new(Mutex::new(vec![]));
    let a = fixture("a", &events);
    let host = PluginHost::mount(&[&a]).unwrap();
    let lease = host.acquire(&key("a")).unwrap();
    let mut resources = PluginResources::default();
    let rejected = Arc::new(AtomicBool::new(false));
    let copy = rejected.clone();
    resources.own(ResourceKind::Connection, move || {
        copy.store(true, Ordering::Release);
    });
    assert!(
        host.own_resources(&key("a"), lease.epoch + 1, resources)
            .is_err()
    );
    assert!(rejected.load(Ordering::Acquire));
    events.lock().unwrap().clear();
    let mut resources = PluginResources::default();
    for kind in [
        ResourceKind::Registration,
        ResourceKind::Listener,
        ResourceKind::Worker,
        ResourceKind::Task,
        ResourceKind::Connection,
        ResourceKind::Cache,
    ] {
        let events = events.clone();
        resources.own(kind, move || {
            events.lock().unwrap().push(format!("{kind:?}"))
        });
    }
    host.own_resources(&key("a"), lease.epoch, resources)
        .unwrap();
    assert_eq!(host.snapshot().unwrap().plugins[0].resources.len(), 6);
    host.stop("a").unwrap();
    host.stop("a").unwrap();
    assert_eq!(
        *events.lock().unwrap(),
        [
            "Cache",
            "Connection",
            "Task",
            "Worker",
            "Listener",
            "Registration",
            "cancel:a",
            "release:a"
        ]
    );
}

#[test]
fn cleanup_panic_does_not_skip_other_resources_or_publish_a_healthy_replacement() {
    let events = Arc::new(Mutex::new(vec![]));
    let a = fixture("a", &events);
    let host = PluginHost::mount(&[&a]).unwrap();
    let lease = host.acquire(&key("a")).unwrap();
    let mut resources = PluginResources::default();
    resources.own(ResourceKind::Connection, || panic!("fixture cleanup"));
    host.own_resources(&key("a"), lease.epoch, resources)
        .unwrap();
    let before = host.snapshot().unwrap();
    let next = host
        .reconfigure(&before.generation, &[&a], &selected(&["a"]), || Ok(()))
        .unwrap();
    assert!(next.cleanup_failed);
    assert!(!next.plugins[0].runtime_active);
    assert!(next.plugins[0].desired_enabled);
    assert!(host.acquire(&key("a")).is_err());
    assert!(events.lock().unwrap().contains(&"release:a".into()));
}

#[test]
fn metadata_supports_all_operation_kinds_without_changing_operation_addresses() {
    use builtin::{CapabilityKind, CapabilityMetadata, ResourceScope};
    struct Contract(CapabilityKind, ResourceScope);
    impl BuiltinPlugin for Contract {
        fn descriptor(&self) -> PluginDescriptor {
            PluginDescriptor {
                id: "contract",
                version: "2.1.0",
                provides: vec![key("contract")],
                requires: vec![],
            }
        }
        fn prepare(&self) -> Result<PluginResources, PluginError> {
            Ok(PluginResources::default())
        }
        fn capability_metadata(&self, _: &CapabilityKey) -> CapabilityMetadata {
            CapabilityMetadata {
                kind: self.0,
                resource_scope: self.1,
            }
        }
    }
    for kind in [
        CapabilityKind::Query,
        CapabilityKind::Command,
        CapabilityKind::Stream,
        CapabilityKind::UiContribution,
    ] {
        let plugin = Contract(kind, ResourceScope::NativeSource);
        let host = PluginHost::mount(&[&plugin]).unwrap();
        let snapshot = host.snapshot().unwrap();
        assert_eq!(snapshot.plugins[0].capabilities[0].metadata.kind, kind);
        assert_eq!(
            snapshot.plugins[0].capabilities[0].metadata.resource_scope,
            ResourceScope::NativeSource
        );
        host.acquire(&key("contract")).unwrap();
    }
}
