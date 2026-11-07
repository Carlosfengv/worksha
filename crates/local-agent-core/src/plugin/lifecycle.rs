use super::PluginError;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::sync::{
    Arc, Mutex, RwLock,
    atomic::{AtomicBool, AtomicU8, Ordering},
};

pub(super) const PREPARED: u8 = 0;
pub(super) const ACTIVE: u8 = 1;
pub(super) const STOPPED: u8 = 2;

pub(super) struct GenerationLife {
    pub phase: AtomicU8,
    pub publication: RwLock<()>,
}
impl Default for GenerationLife {
    fn default() -> Self {
        Self {
            phase: AtomicU8::new(PREPARED),
            publication: RwLock::new(()),
        }
    }
}

/// A lease belongs to one activation, never to a reusable plugin name.
#[derive(Clone)]
pub struct PluginLease {
    pub(super) active: Arc<AtomicBool>,
    pub(super) generation: Arc<GenerationLife>,
    pub epoch: u64,
}
impl PluginLease {
    pub fn ensure_active(&self) -> Result<(), PluginError> {
        if self.active.load(Ordering::Acquire)
            && self.generation.phase.load(Ordering::Acquire) == ACTIVE
        {
            Ok(())
        } else {
            Err(PluginError::new(
                "plugin_disabled",
                "插件能力已停止或尚未激活",
            ))
        }
    }
    pub fn is_preparing(&self) -> bool {
        self.generation.phase.load(Ordering::Acquire) == PREPARED
    }
    /// Hold the generation barrier only for the final bounded mutation. The
    /// callback must not do I/O, invoke plugins, or recursively acquire this barrier.
    pub fn publish<T>(&self, commit: impl FnOnce() -> T) -> Result<T, PluginError> {
        let _guard = self
            .generation
            .publication
            .read()
            .map_err(|_| PluginError::new("unavailable", "插件发布边界不可用"))?;
        self.ensure_active()?;
        Ok(commit())
    }
}

#[derive(
    Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
pub enum ResourceKind {
    Registration,
    Listener,
    Worker,
    Task,
    Connection,
    Cache,
}

struct Resource {
    kind: ResourceKind,
    release: Box<dyn FnOnce() + Send>,
}
/// A failed preparation and a stopped activation release every owned resource
/// in reverse order, including when a different cleanup callback panics.
#[derive(Default)]
pub struct PluginResources {
    resources: Vec<Resource>,
}
impl PluginResources {
    pub fn on_release(release: impl FnOnce() + Send + 'static) -> Self {
        let mut resources = Self::default();
        resources.own(ResourceKind::Registration, release);
        resources
    }
    pub fn own(&mut self, kind: ResourceKind, release: impl FnOnce() + Send + 'static) {
        self.resources.push(Resource {
            kind,
            release: Box::new(release),
        });
    }
    pub(super) fn append(&mut self, other: &mut Self) -> Result<(), PluginError> {
        if self.resources.len().saturating_add(other.resources.len()) > 256 {
            return Err(PluginError::new("resource_limit", "插件资源作用域已满"));
        }
        self.resources.append(&mut other.resources);
        Ok(())
    }
    pub(super) fn counts(&self) -> Vec<(ResourceKind, usize)> {
        let mut counts = std::collections::BTreeMap::new();
        for resource in &self.resources {
            *counts.entry(resource.kind).or_insert(0) += 1;
        }
        counts.into_iter().collect()
    }
    pub(super) fn release(&mut self) -> bool {
        let mut complete = true;
        while let Some(resource) = self.resources.pop() {
            complete &=
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(resource.release)).is_ok();
        }
        complete
    }
}
impl Drop for PluginResources {
    fn drop(&mut self) {
        self.release();
    }
}

pub(super) fn take_resources(resources: &Mutex<PluginResources>) -> PluginResources {
    std::mem::take(&mut *resources.lock().unwrap_or_else(|e| e.into_inner()))
}
