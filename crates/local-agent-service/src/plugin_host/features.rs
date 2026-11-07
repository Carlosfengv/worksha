//! Bundled business plugins and ownership of their existing readonly support.
//! HTTP admissions use the same leases as background workers, never UI state.
use local_agent_core::plugin::{
    BuiltinPlugin, CapabilityKey, PluginDescriptor, PluginError, PluginHost, PluginLease,
    PluginResources,
    builtin::{CapabilityKind, CapabilityMetadata, ResourceScope},
};
use std::sync::Arc;

// Legacy composition records and receipts can still contain these IDs.
pub(super) fn deferred(id: &str) -> bool {
    matches!(
        id,
        "builtin.workbench"
            | "builtin.projects"
            | "listeners"
            | "project.source"
            | "project.assets"
            | "project.git"
            | "project.listeners"
    )
}

fn key(id: &'static str) -> CapabilityKey {
    CapabilityKey {
        name: "business.feature",
        version: 1,
        instance: id,
    }
}
struct Feature {
    id: &'static str,
    title: &'static str,
    requires: &'static [&'static str],
}
impl BuiltinPlugin for Feature {
    fn descriptor(&self) -> PluginDescriptor {
        PluginDescriptor {
            id: self.id,
            version: env!("CARGO_PKG_VERSION"),
            provides: vec![key(self.id)],
            requires: self.requires.iter().map(|id| key(id)).collect(),
        }
    }
    fn display_name(&self) -> String {
        self.title.into()
    }
    fn capability_metadata(&self, _: &CapabilityKey) -> CapabilityMetadata {
        CapabilityMetadata {
            kind: CapabilityKind::Command,
            resource_scope: ResourceScope::Application,
        }
    }
    fn prepare(&self) -> Result<PluginResources, PluginError> {
        Ok(PluginResources::default())
    }
}
pub(crate) fn definitions() -> Vec<Arc<dyn BuiltinPlugin>> {
    [
        Feature {
            id: "builtin.providers",
            title: "Providers",
            requires: &[],
        },
        Feature {
            id: "builtin.agents",
            title: "Agents",
            requires: &["builtin.providers"],
        },
        Feature {
            id: "builtin.model-services",
            title: "模型服务",
            requires: &["builtin.providers"],
        },
        Feature {
            id: "builtin.sessions",
            title: "Sessions",
            requires: &[],
        },
        Feature {
            id: "builtin.requests",
            title: "请求",
            requires: &[],
        },
        Feature {
            id: "builtin.overview",
            title: "总览",
            requires: &["builtin.sessions", "builtin.requests"],
        },
        Feature {
            id: "builtin.mcp",
            title: "MCP",
            requires: &["builtin.sessions"],
        },
    ]
    .into_iter()
    .map(|feature| Arc::new(feature) as Arc<dyn BuiltinPlugin>)
    .collect()
}
pub(crate) fn owner(plugin: &dyn BuiltinPlugin) -> Option<&'static str> {
    let descriptor = plugin.descriptor();
    if descriptor.id.starts_with("builtin.") {
        return None;
    }
    if descriptor.id == "activity" {
        return Some("builtin.overview");
    }
    if descriptor.id == "sessions" {
        return Some("builtin.sessions");
    }
    if descriptor
        .provides
        .iter()
        .any(|key| key.name == "account.source")
    {
        return Some("builtin.providers");
    }
    if descriptor
        .provides
        .iter()
        .any(|key| key.name == "session.source")
    {
        return Some("builtin.sessions");
    }
    None
}
struct Support {
    inner: Arc<dyn BuiltinPlugin>,
    owner: &'static str,
}
impl BuiltinPlugin for Support {
    fn descriptor(&self) -> PluginDescriptor {
        let mut descriptor = self.inner.descriptor();
        descriptor.requires.push(key(self.owner));
        descriptor
    }
    fn display_name(&self) -> String {
        self.inner.display_name()
    }
    fn optional_requires(&self) -> Vec<CapabilityKey> {
        self.inner.optional_requires()
    }
    fn capability_metadata(&self, key: &CapabilityKey) -> CapabilityMetadata {
        self.inner.capability_metadata(key)
    }
    fn prepare(&self) -> Result<PluginResources, PluginError> {
        self.inner.prepare()
    }
    fn activate(
        &self,
        lease: &PluginLease,
        resources: &mut PluginResources,
    ) -> Result<(), PluginError> {
        self.inner.activate(lease, resources)
    }
    fn health_check(&self) -> Result<(), PluginError> {
        self.inner.health_check()
    }
}
pub(crate) fn register(support: Vec<Arc<dyn BuiltinPlugin>>) -> Vec<Arc<dyn BuiltinPlugin>> {
    let mut factories = definitions();
    factories.extend(
        support
            .into_iter()
            .map(|inner| match owner(inner.as_ref()) {
                Some(owner) => Arc::new(Support { inner, owner }) as Arc<dyn BuiltinPlugin>,
                None => inner,
            }),
    );
    factories
}
pub(crate) fn active(state: &crate::AppState, id: &'static str) -> bool {
    mounted(&state.readonly.host, id)
}
pub(crate) fn mounted(host: &PluginHost, id: &'static str) -> bool {
    host.acquire(&key(id)).is_ok()
}
/// These cover legacy and domain consumers, including the network gateway.
/// Revocation/recovery endpoints remain host-owned and are never removed.
pub(crate) fn owner_of_path(path: &str) -> Option<&'static str> {
    if path.starts_with("/api/workbench") {
        Some("builtin.workbench")
    } else if path.starts_with("/api/model-service") {
        Some("builtin.model-services")
    } else if path.starts_with("/api/projects") || path == "/api/project-visibility" {
        Some("builtin.projects")
    } else if path.starts_with("/gateway/") || path.starts_with("/capture/") {
        Some("builtin.agents")
    } else if path.starts_with("/api/provider")
        || path.starts_with("/api/catalog-provider")
        || path.starts_with("/api/accounts")
        || path.starts_with("/api/account-collectors")
        || path.starts_with("/api/dynamic-")
        || path.starts_with("/v1/")
    {
        Some("builtin.providers")
    } else if path.starts_with("/api/agents/") {
        Some("builtin.agents")
    } else if path.starts_with("/api/sessions")
        || path == "/api/calls"
        || [
            "/api/domain/sessions",
            "/api/domain/conversation",
            "/api/domain/facts",
            "/api/domain/fact-conversation",
            "/api/domain/project-visibility",
            "/api/domain/source-refresh",
        ]
        .contains(&path)
    {
        Some("builtin.sessions")
    } else if path == "/api/mcp" {
        Some("builtin.mcp")
    } else if path == "/api/overview" || path == "/api/domain/activity" {
        Some("builtin.overview")
    } else if path.starts_with("/api/request")
        || path.starts_with("/api/domain/request")
        || path == "/api/agent-usage"
    {
        Some("builtin.requests")
    } else {
        None
    }
}
pub(crate) fn recovery_path(path: &str, method: &axum::http::Method) -> bool {
    // Receipts, cancellation and explicit file restoration remain host-owned.
    (method == axum::http::Method::GET
        && (path.starts_with("/api/model-service-operations/")
            || path.contains("-operations/")
            || path.ends_with("/protocol")))
        || (method == axum::http::Method::POST
            && (path.ends_with("/cancel")
                || path == "/api/catalog-provider-operations/query"
                || path == "/api/provider-imports/operations/query"
                || (path.starts_with("/api/provider-switches/")
                    && ["/restore", "/check", "/defer", "/launched"]
                        .iter()
                        .any(|suffix| path.ends_with(suffix)))))
        || path == "/api/provider-draft-recovery"
}
