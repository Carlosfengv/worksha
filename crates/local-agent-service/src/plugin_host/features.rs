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
    id == "builtin.workbench"
}

fn key(id: &'static str) -> CapabilityKey {
    CapabilityKey {
        name: "business.feature",
        version: 1,
        instance: id,
    }
}
pub(crate) struct Feature {
    pub(crate) id: &'static str,
    pub(crate) title: &'static str,
    pub(crate) requires: &'static [&'static str],
    pub(crate) opt_in: bool,
}
impl BuiltinPlugin for Feature {
    fn opt_in(&self) -> bool {
        self.opt_in
    }
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
            id: local_agent_core::iam::PLUGIN_ID,
            title: "账号管理",
            requires: &[],
            opt_in: true,
        },
        Feature {
            id: "builtin.providers",
            title: "Providers",
            requires: &[],
            opt_in: false,
        },
        Feature {
            id: "builtin.agents",
            title: "Agents",
            requires: &["builtin.providers"],
            opt_in: false,
        },
        Feature {
            id: "builtin.model-services",
            title: "模型服务",
            requires: &["builtin.providers"],
            opt_in: false,
        },
        Feature {
            id: "builtin.sessions",
            title: "Sessions",
            requires: &[],
            opt_in: false,
        },
        Feature {
            id: "builtin.requests",
            title: "请求",
            requires: &[],
            opt_in: false,
        },
        Feature {
            id: "builtin.overview",
            title: "总览",
            requires: &["builtin.sessions", "builtin.requests"],
            opt_in: false,
        },
        Feature {
            id: "builtin.mcp",
            title: "MCP",
            requires: &["builtin.sessions"],
            opt_in: false,
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
    if let Some(owner) = descriptor
        .requires
        .iter()
        .find(|key| key.name == "business.feature")
    {
        return Some(owner.instance);
    }
    if descriptor.id == crate::projects::LISTENER_ID {
        return Some(crate::projects::PLUGIN_ID);
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
        .any(|key| key.name == "agent.client-profile")
    {
        return Some("builtin.agents");
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
pub(crate) fn owned(inner: Arc<dyn BuiltinPlugin>, owner: &'static str) -> Arc<dyn BuiltinPlugin> {
    Arc::new(Support { inner, owner })
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
    factories.extend(crate::projects::plugins());
    factories.extend(
        support
            .into_iter()
            .map(|inner| match owner(inner.as_ref()) {
                Some(owner) => owned(inner, owner),
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
pub(super) fn recovery_path(route: &str, method: &axum::http::Method) -> bool {
    use axum::http::Method;
    // Only registered recovery routes bypass business admission. A path suffix
    // or an endpoint that also accepts new work cannot grant an exemption.
    match *method {
        Method::GET => matches!(
            route,
            "/api/provider-operations/protocol"
                | "/api/provider-operations/{id}"
                | "/api/provider-imports/protocol"
                | "/api/provider-credentials/protocol"
                | "/api/provider-credentials/v3/protocol"
                | "/api/provider-credential-operations/{id}"
                | "/api/provider-credential-operations/scoped/{id}"
                | "/api/provider-credential-operations/v3/{id}"
                | "/api/local-tokens/protocol"
                | "/api/model-service-operations/{id}"
                | "/api/gateway-lan/operations/{id}"
                | "/api/settings/operations/{id}"
                | "/api/settings/scoped/operations/{id}"
        ),
        Method::POST => matches!(
            route,
            "/api/provider-operations/cancel"
                | "/api/provider-imports/operations/query"
                | "/api/provider-imports/operations/cancel"
                | "/api/provider-source-operations/query"
                | "/api/provider-credential-operations/{id}/cancel"
                | "/api/provider-credential-operations/scoped/cancel"
                | "/api/provider-credential-operations/v3/cancel"
                | "/api/catalog-provider-operations/query"
                | "/api/provider-draft-recovery"
                | "/api/provider-switches/{id}/cancel"
                | "/api/provider-switches/{id}/restore"
                | "/api/provider-switches/{id}/check"
                | "/api/provider-switches/{id}/defer"
                | "/api/provider-switches/{id}/launched"
                | "/api/local-tokens/query"
                | "/api/local-tokens/cancel"
                | "/api/gateway-lan/clients/{id}/revoke"
        ),
        Method::DELETE => matches!(
            route,
            "/api/request-exports/{id}" | "/api/domain/request-exports/{id}"
        ),
        _ => false,
    }
}
