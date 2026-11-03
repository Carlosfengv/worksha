//! Trusted execution adapters implement the shared, authority-free provider contract.
use local_agent_core::{
    domain::provider::{self as d, ProviderAdapter},
    plugin::{
        BuiltinPlugin, CapabilityKey, PluginDescriptor, PluginError, PluginHost, PluginLease,
        PluginResources,
    },
    providers::ConfigError,
};
use serde_json::Value;
use std::sync::{Arc, LazyLock};

pub(crate) trait Contribution: ProviderAdapter + BuiltinPlugin {
    fn protocol(&self) -> &'static str;
}
struct Standard(&'static str);
fn key(id: &'static str) -> CapabilityKey {
    CapabilityKey {
        name: "domain.provider-adapter",
        version: 1,
        instance: id,
    }
}
impl BuiltinPlugin for Standard {
    fn descriptor(&self) -> PluginDescriptor {
        PluginDescriptor {
            id: self.0,
            version: "1.0.0",
            provides: vec![key(self.0)],
            requires: vec![],
        }
    }
    fn prepare(&self) -> Result<PluginResources, PluginError> {
        Ok(PluginResources::default())
    }
}
impl Contribution for Standard {
    fn protocol(&self) -> &'static str {
        self.0
    }
}
impl ProviderAdapter for Standard {
    fn catalog_request(&self, c: &d::CatalogContext) -> Result<d::CatalogRequest, ConfigError> {
        let base = c.prefix();
        let complete_messages = c.base_path.trim_end_matches('/').ends_with("/messages");
        let path = if c.official_catalog_root
            && ["", "/v1", "/anthropic", "/anthropic/v1"].contains(&base)
        {
            "/models".into()
        } else if self.0 == "messages" && !base.ends_with("/v1") && !complete_messages {
            format!("{base}/v1/models")
        } else {
            format!("{base}/models")
        };
        Ok(d::CatalogRequest { path })
    }
    fn parse_catalog(
        &self,
        body: &Value,
        emit: &mut dyn FnMut(d::CatalogModel) -> Result<(), ConfigError>,
    ) -> Result<d::CatalogContinuation, ConfigError> {
        d::parse_standard_catalog(body, emit)
    }
    fn verification_protocol(&self) -> &'static str {
        self.0
    }
}
static STANDARDS: [Standard; 3] = [
    Standard("messages"),
    Standard("responses"),
    Standard("chat-completions"),
];
pub(crate) struct Adapters {
    host: PluginHost,
    entries: Vec<&'static dyn Contribution>,
}
pub(crate) struct Resolved {
    adapter: &'static dyn Contribution,
    lease: PluginLease,
}
impl Resolved {
    pub fn ensure_active(&self) -> Result<(), ConfigError> {
        self.lease
            .ensure_active()
            .map_err(|_| d::invalid("Provider 适配器已停止"))
    }
}
impl std::ops::Deref for Resolved {
    type Target = dyn Contribution;
    fn deref(&self) -> &Self::Target {
        self.adapter
    }
}
impl Adapters {
    pub fn builtin() -> Result<Self, PluginError> {
        Self::from_contributions(STANDARDS.iter().map(|s| s as &dyn Contribution).collect())
    }
    pub fn from_contributions(
        entries: Vec<&'static dyn Contribution>,
    ) -> Result<Self, PluginError> {
        for entry in &entries {
            if !["messages", "responses", "chat-completions"].contains(&entry.protocol())
                || entry.verification_protocol() != entry.protocol()
                || !entry.descriptor().provides.contains(&key(entry.protocol()))
            {
                return Err(PluginError {
                    code: "invalid_input",
                    message: "Provider 适配器与协议声明不匹配".into(),
                });
            }
        }
        let plugins: Vec<&dyn BuiltinPlugin> =
            entries.iter().map(|p| *p as &dyn BuiltinPlugin).collect();
        let host = PluginHost::mount(&plugins)?;
        Ok(Self { host, entries })
    }
    pub fn resolve(&self, protocol: &str) -> Result<Resolved, ConfigError> {
        let entry = self
            .entries
            .iter()
            .copied()
            .find(|p| p.protocol() == protocol)
            .ok_or_else(|| d::invalid("不支持此接口协议的模型目录"))?;
        let lease = self
            .host
            .acquire(&key(entry.protocol()))
            .map_err(|_| d::invalid("Provider 适配器已停止"))?;
        Ok(Resolved {
            adapter: entry,
            lease,
        })
    }
    #[cfg(test)]
    pub(super) fn stop(&self, id: &str) {
        self.host.stop(id).unwrap();
    }
}
pub(crate) fn builtin() -> Arc<Adapters> {
    static ADAPTERS: LazyLock<Arc<Adapters>> =
        LazyLock::new(|| Arc::new(Adapters::builtin().expect("builtin provider adapters")));
    ADAPTERS.clone()
}
