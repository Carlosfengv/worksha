//! In-process, trusted plugin registration. No external code loading or permissions.
pub mod advanced;
pub mod contributions;
pub mod external;
pub mod full;
pub mod object_contributions;
pub mod plans;
pub mod policy;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct CapabilityKey {
    pub name: &'static str,
    pub version: u16,
    pub instance: &'static str,
}

#[derive(Clone, Debug)]
pub struct PluginDescriptor {
    pub id: &'static str,
    pub version: &'static str,
    pub provides: Vec<CapabilityKey>,
    pub requires: Vec<CapabilityKey>,
}

#[derive(Debug)]
pub struct PluginError {
    pub code: &'static str,
    pub message: String,
}
impl PluginError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}
impl std::fmt::Display for PluginError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}
impl std::error::Error for PluginError {}

/// Resources prepared without domain I/O. Dropping releases registrations/tasks.
#[derive(Default)]
pub struct PluginResources {
    release: Option<Box<dyn FnOnce() + Send>>,
}
impl PluginResources {
    pub fn on_release(release: impl FnOnce() + Send + 'static) -> Self {
        Self {
            release: Some(Box::new(release)),
        }
    }
}
impl Drop for PluginResources {
    fn drop(&mut self) {
        if let Some(release) = self.release.take() {
            release();
        }
    }
}

pub trait BuiltinPlugin: Send + Sync {
    fn descriptor(&self) -> PluginDescriptor;
    /// Optional capabilities affect only the consumer's enhancements, never its lease.
    fn optional_requires(&self) -> Vec<CapabilityKey> {
        Vec::new()
    }
    fn prepare(&self) -> Result<PluginResources, PluginError>;
}

/// A stopped lease never becomes valid again, including after a fresh mount.
#[derive(Clone)]
pub struct PluginLease {
    active: Arc<AtomicBool>,
    pub epoch: u64,
}
impl PluginLease {
    pub fn ensure_active(&self) -> Result<(), PluginError> {
        if self.active.load(Ordering::Acquire) {
            Ok(())
        } else {
            Err(PluginError::new("plugin_disabled", "只读来源插件已停止"))
        }
    }
}

struct Entry {
    descriptor: PluginDescriptor,
    optional: Vec<CapabilityKey>,
    lease: PluginLease,
    resources: Mutex<PluginResources>,
}
impl Entry {
    fn stop(&self) {
        self.lease.active.store(false, Ordering::Release);
        // Release callbacks never run while the resources mutex is held.
        let resources = {
            let mut saved = self.resources.lock().unwrap_or_else(|e| e.into_inner());
            std::mem::take(&mut *saved)
        };
        drop(resources);
    }
}

pub struct PluginHost {
    entries: BTreeMap<&'static str, Entry>,
    providers: BTreeMap<CapabilityKey, &'static str>,
    order: Vec<&'static str>,
}
impl PluginHost {
    /// Validate the complete graph before calling any plugin's prepare method.
    pub fn mount(plugins: &[&dyn BuiltinPlugin]) -> Result<Self, PluginError> {
        let mut descriptors = BTreeMap::new();
        let mut factories = BTreeMap::new();
        let mut providers = BTreeMap::new();
        let mut optional = BTreeMap::new();
        for plugin in plugins {
            let descriptor = plugin.descriptor();
            if descriptor.id.is_empty()
                || descriptor.version.is_empty()
                || descriptor
                    .provides
                    .iter()
                    .any(|c| c.name.is_empty() || c.instance.is_empty() || c.version == 0)
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
            factories.insert(descriptor.id, *plugin);
            let requested = plugin.optional_requires();
            let mut unique = BTreeSet::new();
            if requested.iter().any(|c| {
                c.name.is_empty()
                    || c.instance.is_empty()
                    || c.version == 0
                    || descriptor.requires.contains(c)
                    || !unique.insert(c)
            }) {
                return Err(PluginError::new("invalid_input", "可选依赖描述无效或重复"));
            }
            optional.insert(descriptor.id, requested);
            descriptors.insert(descriptor.id, descriptor);
        }
        let mut pending: BTreeSet<_> = descriptors.keys().copied().collect();
        let mut order = Vec::new();
        while !pending.is_empty() {
            let mut ready = Vec::new();
            for id in &pending {
                let descriptor = &descriptors[id];
                let mut satisfied = true;
                for requirement in &descriptor.requires {
                    let provider = providers.get(requirement).ok_or_else(|| {
                        PluginError::new("dependency_missing", "内置插件依赖缺失或版本不匹配")
                    })?;
                    satisfied &= !pending.contains(provider);
                }
                for requirement in &optional[id] {
                    if let Some(provider) = providers.get(requirement) {
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
        let mut host = Self {
            entries: BTreeMap::new(),
            providers,
            order: Vec::new(),
        };
        for id in order {
            // On failure host Drop unwinds already prepared resources in reverse order.
            let resources = factories[id].prepare()?;
            host.entries.insert(
                id,
                Entry {
                    descriptor: descriptors.remove(id).expect("validated descriptor"),
                    optional: optional
                        .remove(id)
                        .expect("validated optional dependencies"),
                    lease: PluginLease {
                        active: Arc::new(AtomicBool::new(true)),
                        epoch: NEXT_EPOCH.fetch_add(1, Ordering::Relaxed),
                    },
                    resources: Mutex::new(resources),
                },
            );
            host.order.push(id);
        }
        Ok(host)
    }

    pub fn acquire(&self, capability: &CapabilityKey) -> Result<PluginLease, PluginError> {
        let id = self
            .providers
            .get(capability)
            .ok_or_else(|| PluginError::new("capability_missing", "只读来源能力尚未注册"))?;
        let lease = &self.entries[id].lease;
        lease.ensure_active()?;
        Ok(lease.clone())
    }

    /// Re-evaluated after stop; an absent or stopped provider never disables the consumer.
    pub fn optional_available(
        &self,
        id: &str,
        capability: &CapabilityKey,
    ) -> Result<bool, PluginError> {
        let entry = self
            .entries
            .get(id)
            .ok_or_else(|| PluginError::new("plugin_missing", "内置插件不存在"))?;
        entry.lease.ensure_active()?;
        if !entry.optional.contains(capability) {
            return Err(PluginError::new("invalid_input", "未声明的可选依赖"));
        }
        Ok(self
            .providers
            .get(capability)
            .is_some_and(|provider| self.entries[provider].lease.ensure_active().is_ok()))
    }

    /// Read-only work is cancelled; dependants stop before their provider.
    pub fn stop(&self, id: &str) -> Result<(), PluginError> {
        let Some((&id, _)) = self.entries.get_key_value(id) else {
            return Err(PluginError::new("plugin_missing", "内置插件不存在"));
        };
        let mut stopped = BTreeSet::from([id]);
        for candidate in &self.order {
            let entry = &self.entries[candidate];
            if entry.descriptor.requires.iter().any(|requirement| {
                self.providers
                    .get(requirement)
                    .is_some_and(|provider| stopped.contains(provider))
            }) {
                stopped.insert(candidate);
            }
        }
        for candidate in self.order.iter().rev() {
            if stopped.contains(candidate) {
                self.entries[candidate].stop();
            }
        }
        Ok(())
    }

    pub fn stop_all(&self) {
        for id in self.order.iter().rev() {
            self.entries[id].stop();
        }
    }
}
impl Drop for PluginHost {
    fn drop(&mut self) {
        self.stop_all();
    }
}

#[cfg(test)]
mod tests;
