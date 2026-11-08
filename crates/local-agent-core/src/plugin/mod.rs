//! In-process, trusted plugin registration. No external code loading or permissions.
pub mod advanced;
pub mod builtin;
mod builtin_store;
pub use builtin_store::SavedComposition;
pub(crate) use builtin_store::initialize as initialize_composition;
pub mod contributions;
pub mod external;
pub mod full;
mod lifecycle;
pub mod object_contributions;
pub mod plans;
pub mod policy;
mod registry;
pub use lifecycle::{PluginLease, PluginResources, ResourceKind};
pub use registry::{HostSnapshot, PluginHost};
#[cfg(test)]
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct CapabilityKey {
    pub name: &'static str,
    pub version: u16,
    pub instance: &'static str,
}

#[derive(Clone, Debug, Eq, PartialEq)]
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
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
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

pub trait BuiltinPlugin: Send + Sync {
    fn descriptor(&self) -> PluginDescriptor;
    fn opt_in(&self) -> bool {
        false
    }
    fn display_name(&self) -> String {
        self.descriptor().id.into()
    }
    /// Optional capabilities affect only the consumer's enhancements, never its lease.
    fn optional_requires(&self) -> Vec<CapabilityKey> {
        Vec::new()
    }
    fn prepare(&self) -> Result<PluginResources, PluginError>;
    /// Metadata describes this registered operation; it never grants resource access.
    fn capability_metadata(&self, _key: &CapabilityKey) -> builtin::CapabilityMetadata {
        builtin::CapabilityMetadata::default()
    }
    /// Register scoped work only. Business I/O must wait until the lease is active.
    fn activate(
        &self,
        _lease: &PluginLease,
        _resources: &mut PluginResources,
    ) -> Result<(), PluginError> {
        Ok(())
    }
    fn health_check(&self) -> Result<(), PluginError> {
        Ok(())
    }
}

#[cfg(test)]
mod registry_tests;
#[cfg(test)]
mod tests;
