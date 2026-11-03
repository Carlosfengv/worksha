use super::*;

struct TestPlugin {
    id: &'static str,
    provides: Vec<CapabilityKey>,
    requires: Vec<CapabilityKey>,
    optional: Vec<CapabilityKey>,
    fail: bool,
    events: Arc<Mutex<Vec<String>>>,
}
impl BuiltinPlugin for TestPlugin {
    fn optional_requires(&self) -> Vec<CapabilityKey> {
        self.optional.clone()
    }
    fn descriptor(&self) -> PluginDescriptor {
        PluginDescriptor {
            id: self.id,
            version: "1.0.0",
            provides: self.provides.clone(),
            requires: self.requires.clone(),
        }
    }
    fn prepare(&self) -> Result<PluginResources, PluginError> {
        self.events
            .lock()
            .unwrap()
            .push(format!("prepare:{}", self.id));
        if self.fail {
            return Err(PluginError::new("prepare_failed", "fixture failure"));
        }
        let events = self.events.clone();
        let id = self.id;
        Ok(PluginResources::on_release(move || {
            events.lock().unwrap().push(format!("release:{id}"));
        }))
    }
}
fn capability(instance: &'static str) -> CapabilityKey {
    CapabilityKey {
        name: "session.source",
        version: 1,
        instance,
    }
}
fn plugin(id: &'static str, events: &Arc<Mutex<Vec<String>>>) -> TestPlugin {
    TestPlugin {
        id,
        provides: vec![capability(id)],
        requires: vec![],
        optional: vec![],
        fail: false,
        events: events.clone(),
    }
}

#[test]
fn optional_missing_wrong_version_and_stopped_provider_degrade_without_stopping_consumer() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let a = plugin("a", &events);
    let mut b = plugin("b", &events);
    b.optional = vec![
        capability("a"),
        CapabilityKey {
            version: 2,
            ..capability("a")
        },
    ];
    let host = PluginHost::mount(&[&b]).unwrap();
    assert!(!host.optional_available("b", &capability("a")).unwrap());
    assert!(host.acquire(&capability("b")).is_ok());
    drop(host);
    let host = PluginHost::mount(&[&b, &a]).unwrap();
    assert!(host.optional_available("b", &capability("a")).unwrap());
    assert!(!host.optional_available("b", &b.optional[1]).unwrap());
    let consumer = host.acquire(&capability("b")).unwrap();
    let provider = host.acquire(&capability("a")).unwrap();
    host.stop("a").unwrap();
    assert!(!host.optional_available("b", &capability("a")).unwrap());
    consumer.ensure_active().unwrap();
    assert!(provider.ensure_active().is_err());
    assert!(
        host.optional_available("b", &capability("undeclared"))
            .is_err()
    );
}

#[test]
fn optional_cycles_and_ambiguous_requirements_fail_before_prepare() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let mut a = plugin("a", &events);
    let mut b = plugin("b", &events);
    a.optional = vec![capability("b")];
    b.optional = vec![capability("a")];
    assert_eq!(failure(&[&a, &b]), "dependency_cycle");
    a.optional = vec![capability("a")];
    assert_eq!(failure(&[&a]), "dependency_cycle");
    a.optional = vec![capability("b"), capability("b")];
    assert_eq!(failure(&[&a, &b]), "invalid_input");
    a.optional = vec![capability("b")];
    a.requires = a.optional.clone();
    assert_eq!(failure(&[&a, &b]), "invalid_input");
    assert!(events.lock().unwrap().is_empty());
}
fn failure(plugins: &[&dyn BuiltinPlugin]) -> &'static str {
    PluginHost::mount(plugins).err().unwrap().code
}

#[test]
fn graph_validation_has_no_prepare_side_effects() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let a = plugin("a", &events);
    let mut b = plugin("b", &events);
    assert_eq!(failure(&[&a, &a]), "duplicate_plugin");
    b.provides = a.provides.clone();
    assert_eq!(failure(&[&a, &b]), "capability_conflict");
    b.provides = vec![capability("b")];
    b.requires = vec![capability("missing")];
    assert_eq!(failure(&[&a, &b]), "dependency_missing");
    b.requires = vec![CapabilityKey {
        version: 2,
        ..capability("a")
    }];
    assert_eq!(failure(&[&a, &b]), "dependency_missing");
    assert!(events.lock().unwrap().is_empty());
}

#[test]
fn cycles_and_self_dependencies_are_rejected_before_prepare() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let mut a = plugin("a", &events);
    let mut b = plugin("b", &events);
    a.requires = vec![capability("b")];
    b.requires = vec![capability("a")];
    assert_eq!(failure(&[&a, &b]), "dependency_cycle");
    a.requires = vec![capability("a")];
    assert_eq!(failure(&[&a]), "dependency_cycle");
    assert!(events.lock().unwrap().is_empty());
}

#[test]
fn stopping_a_provider_cancels_transitive_calls_and_releases_in_reverse_order() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let a = plugin("z-provider", &events);
    let mut b = plugin("b-consumer", &events);
    let mut c = plugin("a-consumer", &events);
    let independent = plugin("independent", &events);
    b.requires = a.provides.clone();
    c.requires = b.provides.clone();
    let host = PluginHost::mount(&[&c, &independent, &b, &a]).unwrap();
    let call = host.acquire(&capability(c.id)).unwrap();
    let other = host.acquire(&capability(independent.id)).unwrap();
    host.stop(a.id).unwrap();
    assert_eq!(call.ensure_active().unwrap_err().code, "plugin_disabled");
    assert!(host.acquire(&capability(c.id)).is_err());
    assert!(other.ensure_active().is_ok());
    host.stop(a.id).unwrap();
    drop(host);
    assert!(other.ensure_active().is_err());
    let released: Vec<_> = events
        .lock()
        .unwrap()
        .iter()
        .filter(|event| event.starts_with("release:"))
        .cloned()
        .collect();
    assert_eq!(
        released,
        [
            "release:a-consumer",
            "release:b-consumer",
            "release:z-provider",
            "release:independent"
        ]
    );
}

#[test]
fn failed_prepare_unwinds_previous_resources_and_fresh_mount_does_not_revive_calls() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let a = plugin("a", &events);
    let mut b = plugin("b", &events);
    b.requires = a.provides.clone();
    b.fail = true;
    assert_eq!(failure(&[&b, &a]), "prepare_failed");
    assert_eq!(
        *events.lock().unwrap(),
        ["prepare:a", "prepare:b", "release:a"]
    );
    let host = PluginHost::mount(&[&a]).unwrap();
    let old = host.acquire(&capability("a")).unwrap();
    drop(host);
    let host = PluginHost::mount(&[&a]).unwrap();
    let new = host.acquire(&capability("a")).unwrap();
    assert_ne!(old.epoch, new.epoch);
    assert!(old.ensure_active().is_err());
    assert!(new.ensure_active().is_ok());
}
