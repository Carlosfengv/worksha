use super::builtin::{CapabilityAddress, CapabilityInfo, PluginInfo, ResourceCount};
use super::lifecycle::{ACTIVE, GenerationLife, take_resources};
use super::{
    BuiltinPlugin, CapabilityKey, PluginDescriptor, PluginError, PluginLease, PluginResources,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        Arc, Mutex, RwLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HostSnapshot {
    pub generation: String,
    pub plugins: Vec<PluginInfo>,
    pub cleanup_failed: bool,
}

struct Entry {
    descriptor: PluginDescriptor,
    display_name: String,
    optional: Vec<CapabilityKey>,
    contracts: Vec<CapabilityInfo>,
    desired_enabled: bool,
    opt_in: bool,
    lease: PluginLease,
    resources: Mutex<PluginResources>,
}
struct Generation {
    id: String,
    life: Arc<GenerationLife>,
    entries: BTreeMap<&'static str, Arc<Entry>>,
    providers: BTreeMap<CapabilityKey, &'static str>,
    order: Vec<&'static str>,
    cleanup_failed: AtomicBool,
}
impl Generation {
    fn retire(&self) -> bool {
        self.retire_matching(|_| true)
    }
    fn retire_matching(&self, retired: impl Fn(&str) -> bool) -> bool {
        let entries = || {
            self.entries
                .iter()
                .filter(|(id, _)| retired(id))
                .map(|(_, entry)| entry)
        };
        {
            // Revoke all affected publishers before releasing any shared resource.
            let mut cohorts = BTreeSet::new();
            let mut publication = Vec::new();
            for entry in entries() {
                let life = &entry.lease.generation;
                if cohorts.insert(Arc::as_ptr(life)) {
                    publication.push(life.publication.write().unwrap_or_else(|e| e.into_inner()));
                }
            }
            for entry in entries() {
                entry.lease.active.store(false, Ordering::Release);
            }
        }
        let mut complete = true;
        for id in self.order.iter().rev().filter(|id| retired(id)) {
            complete &= take_resources(&self.entries[id].resources).release();
        }
        if !complete {
            self.cleanup_failed.store(true, Ordering::Release);
        }
        complete
    }
    fn snapshot(&self) -> Result<HostSnapshot, PluginError> {
        let _publication = self.life.publication.read().map_err(|_| unavailable())?;
        let failed = self.cleanup_failed.load(Ordering::Acquire);
        let plugins = self
            .entries
            .values()
            .map(|entry| {
                let active = entry.lease.ensure_active().is_ok();
                PluginInfo {
                    id: entry.descriptor.id.into(),
                    display_name: entry.display_name.clone(),
                    version: entry.descriptor.version.into(),
                    execution_kind: crate::domain::ExecutionKind::Builtin,
                    desired_enabled: entry.desired_enabled,
                    opt_in: entry.opt_in,
                    runtime_active: active,
                    activation_epoch: entry.lease.epoch.into(),
                    capabilities: entry.contracts.clone(),
                    requires: entry
                        .descriptor
                        .requires
                        .iter()
                        .map(CapabilityAddress::from)
                        .collect(),
                    optional_requires: entry.optional.iter().map(CapabilityAddress::from).collect(),
                    resources: entry
                        .resources
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .counts()
                        .into_iter()
                        .map(|(kind, count)| ResourceCount { kind, count })
                        .collect(),
                    problem: if failed {
                        Some("cleanup_failed".into())
                    } else if !active && entry.desired_enabled {
                        Some("plugin_stopped".into())
                    } else {
                        None
                    },
                }
            })
            .collect();
        Ok(HostSnapshot {
            generation: self.id.clone(),
            plugins,
            cleanup_failed: failed,
        })
    }
}
impl Drop for Generation {
    fn drop(&mut self) {
        // Rollback owns only the new preparations. A published generation is
        // explicitly retired by the host and may share live entries with its successor.
        if self.life.phase.load(Ordering::Acquire) != ACTIVE {
            self.retire_matching(|id| Arc::ptr_eq(&self.entries[id].lease.generation, &self.life));
        }
    }
}
fn unavailable() -> PluginError {
    PluginError::new("unavailable", "插件能力目录暂不可用")
}
fn valid_key(key: &CapabilityKey) -> bool {
    !key.name.is_empty()
        && key.name.len() <= 128
        && key.version > 0
        && !key.instance.is_empty()
        && key.instance.len() <= 128
        && key
            .name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b".-_".contains(&b))
        && key
            .instance
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b".-_".contains(&b))
}

/// One authority for registration, activation and operation leases. Domain
/// catalogs derive status from this host; configuration never grants permission.
pub struct PluginHost {
    current: RwLock<Arc<Generation>>,
    transition: Mutex<()>,
}
impl PluginHost {
    pub fn mount(plugins: &[&dyn BuiltinPlugin]) -> Result<Self, PluginError> {
        let enabled = plugins
            .iter()
            .map(|p| p.descriptor().id.to_owned())
            .collect();
        Self::mount_enabled(plugins, &enabled)
    }
    pub fn mount_enabled(
        plugins: &[&dyn BuiltinPlugin],
        enabled: &BTreeSet<String>,
    ) -> Result<Self, PluginError> {
        let generation = Arc::new(Self::prepare(plugins, enabled, None)?);
        generation.life.phase.store(ACTIVE, Ordering::Release);
        Ok(Self {
            current: RwLock::new(generation),
            transition: Mutex::new(()),
        })
    }
    fn generation(&self) -> Result<Arc<Generation>, PluginError> {
        self.current
            .read()
            .map(|g| g.clone())
            .map_err(|_| unavailable())
    }
    pub fn snapshot(&self) -> Result<HostSnapshot, PluginError> {
        self.generation()?.snapshot()
    }

    /// No factory preparation, task registration or business I/O occurs before
    /// the entire selected dependency graph passes validation.
    fn prepare(
        plugins: &[&dyn BuiltinPlugin],
        enabled: &BTreeSet<String>,
        retain: Option<(&Generation, &BTreeSet<String>)>,
    ) -> Result<Generation, PluginError> {
        let mut descriptors = BTreeMap::new();
        let mut factories = BTreeMap::new();
        let mut providers = BTreeMap::new();
        let mut optional = BTreeMap::new();
        let mut contracts = BTreeMap::new();
        let mut names = BTreeMap::new();
        if plugins.len() > 256 {
            return Err(PluginError::new("resource_limit", "内置插件数量超过限制"));
        }
        for plugin in plugins {
            let descriptor = plugin.descriptor();
            let display_name = plugin.display_name();
            if display_name.is_empty()
                || display_name.len() > 200
                || display_name.chars().any(char::is_control)
            {
                return Err(PluginError::new("invalid_input", "内置插件名称无效"));
            }
            if descriptor.id.is_empty()
                || descriptor.id.len() > 100
                || descriptor.version.is_empty()
                || descriptor.version.len() > 100
                || descriptor.provides.is_empty()
                || descriptor.provides.len() > 64
                || descriptor.requires.len() > 64
                || descriptor.provides.iter().any(|c| !valid_key(c))
                || descriptor.requires.iter().any(|c| !valid_key(c))
                || descriptor.requires.iter().collect::<BTreeSet<_>>().len()
                    != descriptor.requires.len()
            {
                return Err(PluginError::new("invalid_input", "内置插件描述无效"));
            }
            if descriptors.contains_key(descriptor.id) {
                return Err(PluginError::new("duplicate_plugin", "内置插件 ID 重复"));
            }
            for capability in &descriptor.provides {
                if providers
                    .insert(capability.clone(), descriptor.id)
                    .is_some()
                {
                    return Err(PluginError::new(
                        "capability_conflict",
                        "内置插件能力注册冲突",
                    ));
                }
            }
            let requested = plugin.optional_requires();
            if requested.len() > 64
                || requested
                    .iter()
                    .any(|c| !valid_key(c) || descriptor.requires.contains(c))
                || requested.iter().collect::<BTreeSet<_>>().len() != requested.len()
            {
                return Err(PluginError::new("invalid_input", "可选依赖描述无效或重复"));
            }
            contracts.insert(
                descriptor.id,
                descriptor
                    .provides
                    .iter()
                    .map(|key| CapabilityInfo {
                        address: key.into(),
                        metadata: plugin.capability_metadata(key),
                    })
                    .collect::<Vec<_>>(),
            );
            optional.insert(descriptor.id, requested);
            factories.insert(descriptor.id, *plugin);
            names.insert(descriptor.id, display_name);
            descriptors.insert(descriptor.id, descriptor);
        }
        if enabled
            .iter()
            .any(|id| !descriptors.contains_key(id.as_str()))
        {
            return Err(PluginError::new(
                "plugin_missing",
                "组合包含未注册的内置插件",
            ));
        }
        let mut pending: BTreeSet<&'static str> = descriptors
            .keys()
            .copied()
            .filter(|id| enabled.contains(*id))
            .collect();
        let mut order = Vec::new();
        while !pending.is_empty() {
            let mut ready = Vec::new();
            for id in &pending {
                let descriptor = &descriptors[id];
                let mut satisfied = true;
                for requirement in &descriptor.requires {
                    let provider = providers
                        .get(requirement)
                        .filter(|id| enabled.contains(**id))
                        .ok_or_else(|| {
                            PluginError::new(
                                "dependency_missing",
                                "内置插件必需依赖缺失、停用或版本不匹配",
                            )
                        })?;
                    satisfied &= !pending.contains(provider);
                }
                for requirement in &optional[id] {
                    if let Some(provider) = providers
                        .get(requirement)
                        .filter(|id| enabled.contains(**id))
                    {
                        satisfied &= !pending.contains(provider);
                    }
                }
                if satisfied {
                    ready.push(*id);
                }
            }
            if ready.is_empty() {
                return Err(PluginError::new("dependency_cycle", "内置插件依赖存在循环"));
            }
            for id in ready {
                pending.remove(id);
                order.push(id);
            }
        }
        static NEXT_EPOCH: AtomicU64 = AtomicU64::new(1);
        let life = Arc::new(GenerationLife::default());
        let mut generation = Generation {
            id: uuid::Uuid::new_v4().to_string(),
            life: life.clone(),
            entries: BTreeMap::new(),
            providers,
            order: Vec::new(),
            cleanup_failed: AtomicBool::new(false),
        };
        // Even disabled declarations remain discoverable. They have no resources
        // and their operations cannot be acquired.
        for (id, descriptor) in descriptors {
            let opt_in = factories[id].opt_in();
            if let Some((previous, retained)) = retain
                && retained.contains(id)
            {
                let entry = previous
                    .entries
                    .get(id)
                    .filter(|entry| {
                        entry.descriptor == descriptor
                            && entry.desired_enabled == enabled.contains(id)
                            && entry.lease.ensure_active().is_ok() == enabled.contains(id)
                            && entry.opt_in == opt_in
                            && entry.optional == optional[id]
                            && entry.contracts == contracts[id]
                            && entry.display_name == names[id]
                    })
                    .ok_or_else(|| {
                        PluginError::new("stale_snapshot", "保留插件的能力或状态已变化")
                    })?;
                generation.entries.insert(id, entry.clone());
                continue;
            }
            let epoch = NEXT_EPOCH
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                    value.checked_add(1)
                })
                .map_err(|_| PluginError::new("resource_limit", "插件激活编号已耗尽"))?;
            generation.entries.insert(
                id,
                Arc::new(Entry {
                    descriptor,
                    display_name: names.remove(id).expect("validated display name"),
                    optional: optional
                        .remove(id)
                        .expect("validated optional dependencies"),
                    contracts: contracts
                        .remove(id)
                        .expect("validated capability contracts"),
                    desired_enabled: enabled.contains(id),
                    opt_in,
                    lease: PluginLease {
                        active: Arc::new(AtomicBool::new(enabled.contains(id))),
                        generation: life.clone(),
                        epoch,
                    },
                    resources: Mutex::new(PluginResources::default()),
                }),
            );
        }
        for id in order {
            let entry = &generation.entries[id];
            if Arc::ptr_eq(&entry.lease.generation, &life) {
                let mut resources = factories[id].prepare()?;
                factories[id].activate(&entry.lease, &mut resources)?;
                factories[id].health_check()?;
                *entry.resources.lock().unwrap_or_else(|e| e.into_inner()) = resources;
            }
            generation.order.push(id);
        }
        Ok(generation)
    }

    /// Prepare and check the next generation first. A rejected graph, factory,
    /// health check or persistence callback leaves the previous generation live.
    pub fn reconfigure(
        &self,
        expected_generation: &str,
        plugins: &[&dyn BuiltinPlugin],
        enabled: &BTreeSet<String>,
        persist: impl FnOnce() -> Result<(), PluginError>,
    ) -> Result<HostSnapshot, PluginError> {
        self.reconfigure_retaining(
            expected_generation,
            plugins,
            enabled,
            &BTreeSet::new(),
            persist,
        )
    }

    pub fn reconfigure_retaining(
        &self,
        expected_generation: &str,
        plugins: &[&dyn BuiltinPlugin],
        enabled: &BTreeSet<String>,
        retained: &BTreeSet<String>,
        persist: impl FnOnce() -> Result<(), PluginError>,
    ) -> Result<HostSnapshot, PluginError> {
        let _transition = self.transition.lock().map_err(|_| unavailable())?;
        let previous = self.generation()?;
        if previous.id != expected_generation {
            return Err(PluginError::new(
                "stale_snapshot",
                "插件组合已变化，请重新读取",
            ));
        }
        let next = Arc::new(Self::prepare(
            plugins,
            enabled,
            Some((&previous, retained)),
        )?);
        if retained.iter().any(|id| {
            next.entries.get(id.as_str()).is_none_or(|entry| {
                entry.desired_enabled
                    && entry.descriptor.requires.iter().any(|required| {
                        next.providers
                            .get(required)
                            .is_none_or(|owner| !retained.contains(*owner))
                    })
            })
        }) {
            return Err(PluginError::new(
                "stale_snapshot",
                "保留插件的必需依赖已变化",
            ));
        }
        persist()?;
        let released = previous.retire_matching(|id| !retained.contains(id));
        if !released {
            next.cleanup_failed.store(true, Ordering::Release);
            next.retire();
        }
        {
            let mut current = self.current.write().map_err(|_| unavailable())?;
            let _publication = next.life.publication.write().map_err(|_| unavailable())?;
            *current = next.clone();
            if released {
                next.life.phase.store(ACTIVE, Ordering::Release);
            }
        }
        next.snapshot()
    }
    pub fn acquire(&self, capability: &CapabilityKey) -> Result<PluginLease, PluginError> {
        let generation = self.generation()?;
        let id = generation
            .providers
            .get(capability)
            .ok_or_else(|| PluginError::new("capability_missing", "插件能力尚未注册"))?;
        let lease = &generation.entries[id].lease;
        lease.ensure_active()?;
        Ok(lease.clone())
    }
    pub fn own_resources(
        &self,
        capability: &CapabilityKey,
        epoch: u64,
        mut resources: PluginResources,
    ) -> Result<(), PluginError> {
        let generation = self.generation()?;
        let id = generation
            .providers
            .get(capability)
            .ok_or_else(|| PluginError::new("capability_missing", "插件能力尚未注册"))?;
        let entry = &generation.entries[id];
        if entry.lease.epoch != epoch {
            return Err(PluginError::new("stale_snapshot", "插件资源所属激活已变化"));
        }
        entry.lease.publish(|| {
            entry
                .resources
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .append(&mut resources)
        })?
    }
    pub fn optional_available(
        &self,
        id: &str,
        capability: &CapabilityKey,
    ) -> Result<bool, PluginError> {
        let generation = self.generation()?;
        let entry = generation
            .entries
            .get(id)
            .ok_or_else(|| PluginError::new("plugin_missing", "内置插件不存在"))?;
        entry.lease.ensure_active()?;
        if !entry.optional.contains(capability) {
            return Err(PluginError::new("invalid_input", "未声明的可选依赖"));
        }
        Ok(generation
            .providers
            .get(capability)
            .is_some_and(|provider| generation.entries[provider].lease.ensure_active().is_ok()))
    }
    pub fn stop(&self, id: &str) -> Result<(), PluginError> {
        let _transition = self.transition.lock().map_err(|_| unavailable())?;
        let generation = self.generation()?;
        let (&id, _) = generation
            .entries
            .get_key_value(id)
            .ok_or_else(|| PluginError::new("plugin_missing", "内置插件不存在"))?;
        let mut stopped = BTreeSet::from([id]);
        for candidate in &generation.order {
            if generation.entries[candidate]
                .descriptor
                .requires
                .iter()
                .any(|requirement| {
                    generation
                        .providers
                        .get(requirement)
                        .is_some_and(|provider| stopped.contains(provider))
                })
            {
                stopped.insert(candidate);
            }
        }
        generation.retire_matching(|id| stopped.contains(id));
        Ok(())
    }
    pub fn stop_all(&self) {
        let _transition = self.transition.lock().unwrap_or_else(|e| e.into_inner());
        if let Ok(generation) = self.generation() {
            generation.retire();
        }
    }
}
impl Drop for PluginHost {
    fn drop(&mut self) {
        self.stop_all();
    }
}
