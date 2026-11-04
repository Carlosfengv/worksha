//! Trusted native format contributions. The host owns all mutation and recovery.
mod formats;
mod planner;
use super::{ConfigError, fail};
use crate::{
    Provider,
    plugin::{
        BuiltinPlugin, CapabilityKey, PluginDescriptor, PluginError, PluginHost, PluginLease,
        PluginResources,
    },
};
pub use formats::{NativeConfig, endpoint};
pub(crate) use formats::{parse_json, parse_toml, routing_endpoint};
use std::{
    path::{Path, PathBuf},
    sync::LazyLock,
};

pub trait NativeConfigAdapter: BuiltinPlugin {
    fn id(&self) -> &'static str;
    fn protocol(&self) -> Option<&'static str>;
    fn version_supported(&self, version: &str) -> bool;
    fn model_id(&self, provider_id: &str, model: &str) -> String;
    fn supports_model_catalog(&self) -> bool;
    fn requires_inline_credential(&self) -> bool;
    fn path(&self, home: &Path) -> Result<PathBuf, ConfigError>;
    fn inspect(&self, home: &Path) -> NativeConfig;
    /// Returns bytes only. No vault, journal, process control or write access is passed.
    fn render(
        &self,
        before: Option<&[u8]>,
        provider: Option<&Provider>,
        helper: &Path,
    ) -> Result<Vec<u8>, ConfigError>;
    fn models(
        &self,
        bytes: &mut Vec<u8>,
        provider: &Provider,
        models: &[String],
        credential: Option<&str>,
    ) -> Result<(), ConfigError>;
    fn service_models(
        &self,
        _bytes: &mut Vec<u8>,
        _provider: &Provider,
        _catalog: &crate::model_services::ServiceAttachment,
    ) -> Result<(), ConfigError> {
        fail("unsupported", "此配置适配器尚不支持模型服务目录")
    }
}
struct Format {
    id: &'static str,
    protocol: Option<&'static str>,
}
fn key(id: &'static str) -> CapabilityKey {
    CapabilityKey {
        name: "agent.config-adapter",
        version: 1,
        instance: id,
    }
}
impl BuiltinPlugin for Format {
    fn descriptor(&self) -> PluginDescriptor {
        PluginDescriptor {
            id: self.id,
            version: "1.0.0",
            provides: vec![
                key(self.id),
                CapabilityKey {
                    name: "gateway.client-profile",
                    version: 1,
                    instance: self.id,
                },
                CapabilityKey {
                    name: "gateway.model-alias",
                    version: 1,
                    instance: self.id,
                },
            ],
            requires: vec![],
        }
    }
    fn prepare(&self) -> Result<PluginResources, PluginError> {
        Ok(PluginResources::default())
    }
}
impl NativeConfigAdapter for Format {
    fn service_models(
        &self,
        bytes: &mut Vec<u8>,
        provider: &Provider,
        catalog: &crate::model_services::ServiceAttachment,
    ) -> Result<(), ConfigError> {
        formats::service_models(self.id, bytes, provider, catalog)
    }
    fn id(&self) -> &'static str {
        self.id
    }
    fn supports_model_catalog(&self) -> bool {
        ["codex", "opencode", "claude-desktop"].contains(&self.id)
    }
    fn requires_inline_credential(&self) -> bool {
        self.id == "opencode"
    }
    fn protocol(&self) -> Option<&'static str> {
        self.protocol
    }
    fn version_supported(&self, version: &str) -> bool {
        let Ok(version) = semver::Version::parse(version) else {
            return false;
        };
        let stable = version.pre.is_empty();
        match (self.id, version.major, version.minor) {
            ("claude", 2, 1) => stable && (286..=318).contains(&version.patch),
            ("codex", 0, 163) => true,
            ("codex", 0, 164..=166) => {
                version.patch == 0
                    && (stable || (version.minor == 166 && version.pre.as_str() == "alpha.5.1"))
            }
            ("opencode", 1, 18) => true,
            ("opencode", 1, 19) => stable && version.patch <= 5,
            ("claude-desktop", 2, 2553 | 42304) => true,
            ("claude-desktop", 3, 6556) => stable && version.patch == 2,
            _ => false,
        }
    }
    fn model_id(&self, provider_id: &str, model: &str) -> String {
        if self.id != "claude-desktop" {
            return model.into();
        }
        let hash = super::files::hash(&serde_json::to_vec(&(provider_id, model)).unwrap());
        let suffix = u128::from_str_radix(&hash[..32], 16).unwrap();
        format!("claude-sonnet-5-local-agent-{suffix}")
    }
    fn path(&self, home: &Path) -> Result<PathBuf, ConfigError> {
        formats::path(home, self.id)
    }
    fn inspect(&self, home: &Path) -> NativeConfig {
        formats::inspect(home, self.id)
    }
    fn render(
        &self,
        before: Option<&[u8]>,
        provider: Option<&Provider>,
        helper: &Path,
    ) -> Result<Vec<u8>, ConfigError> {
        formats::render(self.id, before, provider, helper)
    }
    fn models(
        &self,
        bytes: &mut Vec<u8>,
        provider: &Provider,
        models: &[String],
        credential: Option<&str>,
    ) -> Result<(), ConfigError> {
        match self.id {
            "codex" if !models.is_empty() => formats::codex_models(bytes, provider),
            "claude-desktop" if !models.is_empty() => {
                super::desktop_clients::claude_models(bytes, provider, models)
            }
            "opencode" => super::desktop_clients::opencode_credential(
                bytes,
                provider,
                credential
                    .ok_or_else(|| ConfigError::new("invalid_credential", "本机路由凭据缺失"))?,
                models,
            ),
            _ => Ok(()),
        }
    }
}
static FORMATS: [Format; 5] = [
    Format {
        id: "claude",
        protocol: Some("messages"),
    },
    Format {
        id: "codex",
        protocol: Some("responses"),
    },
    Format {
        id: "opencode",
        protocol: Some("messages"),
    },
    Format {
        id: "claude-desktop",
        protocol: Some("messages"),
    },
    Format {
        id: "antigravity",
        protocol: None,
    },
];
pub struct ConfigAdapters {
    host: PluginHost,
    adapters: Vec<&'static dyn NativeConfigAdapter>,
}
impl ConfigAdapters {
    pub fn builtin() -> Result<Self, PluginError> {
        Self::from_contributions(
            FORMATS
                .iter()
                .map(|format| format as &dyn NativeConfigAdapter)
                .collect(),
        )
    }
    pub fn from_contributions(
        adapters: Vec<&'static dyn NativeConfigAdapter>,
    ) -> Result<Self, PluginError> {
        for adapter in &adapters {
            if !adapter.descriptor().provides.contains(&key(adapter.id())) {
                return Err(PluginError {
                    code: "invalid_input",
                    message: "配置适配器与能力描述不匹配".into(),
                });
            }
        }
        let host = PluginHost::mount(
            &adapters
                .iter()
                .map(|adapter| *adapter as &dyn BuiltinPlugin)
                .collect::<Vec<_>>(),
        )?;
        for adapter in &adapters {
            host.acquire(&key(adapter.id()))?;
        }
        Ok(Self { host, adapters })
    }
    pub fn resolve(&self, agent: &str) -> Result<&'static dyn NativeConfigAdapter, ConfigError> {
        let Some(format) = self.adapters.iter().copied().find(|f| f.id() == agent) else {
            return fail("unsupported", "当前仅支持 Claude Code 和 Codex");
        };
        self.host.acquire(&key(format.id())).map_err(|_| {
            ConfigError::new("plugin_disabled", "配置适配器已停止；待应用事务仍保留")
        })?;
        Ok(format)
    }
    pub fn ids(&self) -> impl Iterator<Item = &'static str> {
        self.adapters.iter().map(|f| f.id())
    }
    pub(crate) fn plan_native(
        &self,
        agent: &str,
        before: Option<&[u8]>,
        provider: Option<&Provider>,
        helper: &Path,
    ) -> Result<(Vec<u8>, PluginLease), ConfigError> {
        let adapter = self.resolve(agent)?;
        let lease = self
            .host
            .acquire(&key(adapter.id()))
            .map_err(|_| stopped())?;
        let bytes = planner::render(adapter, before, provider, helper)?;
        lease.ensure_active().map_err(|_| stopped())?;
        Ok((bytes, lease))
    }
    pub fn stop(&self, agent: &str) -> Result<(), PluginError> {
        let id = self
            .adapters
            .iter()
            .find(|adapter| adapter.id() == agent)
            .map(|adapter| adapter.descriptor().id)
            .unwrap_or(agent);
        self.host.stop(id)
    }
}
pub fn builtin() -> &'static ConfigAdapters {
    static REGISTRY: LazyLock<ConfigAdapters> =
        LazyLock::new(|| ConfigAdapters::builtin().expect("builtin config adapters"));
    &REGISTRY
}
pub fn path(home: &Path, agent: &str) -> Result<PathBuf, ConfigError> {
    builtin().resolve(agent)?.path(home)
}
pub fn codex_models(bytes: &mut Vec<u8>, provider: &Provider) -> Result<(), ConfigError> {
    builtin()
        .resolve("codex")?
        .models(bytes, provider, std::slice::from_ref(&provider.model), None)
}
pub fn inspect(home: &Path, agent: &str) -> NativeConfig {
    inspect_with(builtin(), home, agent)
}
pub fn inspect_with(registry: &ConfigAdapters, home: &Path, agent: &str) -> NativeConfig {
    match registry.resolve(agent) {
        Ok(adapter) => adapter.inspect(home),
        Err(error) => NativeConfig {
            catalog_endpoints: vec![],
            agent_id: agent.into(),
            path: String::new(),
            state: "unavailable".into(),
            revision: None,
            mode: "unknown".into(),
            endpoint: None,
            model: None,
            provider_name: String::new(),
            credential: "未读取凭据".into(),
            error: Some(error.message),
            warnings: vec![],
        },
    }
}
pub fn render(
    agent: &str,
    before: Option<&[u8]>,
    provider: Option<&Provider>,
    helper: &Path,
) -> Result<Vec<u8>, ConfigError> {
    builtin()
        .plan_native(agent, before, provider, helper)
        .map(|p| p.0)
}
pub(crate) fn stopped() -> ConfigError {
    ConfigError::new("plugin_disabled", "配置适配器已停止；待应用事务仍保留")
}
pub(super) fn preserve_service_selection(
    agent: &str,
    before: Option<&[u8]>,
    after: &mut Vec<u8>,
    catalog: &crate::model_services::ServiceAttachment,
) -> Result<(), ConfigError> {
    formats::preserve_service_selection(agent, before, after, catalog)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn composition_accepts_a_subset_and_rejects_duplicate_contributions() {
        let registry = ConfigAdapters::from_contributions(vec![&FORMATS[0]]).unwrap();
        assert_eq!(registry.ids().collect::<Vec<_>>(), ["claude"]);
        assert!(registry.resolve("codex").is_err());
        assert!(ConfigAdapters::from_contributions(vec![&FORMATS[0], &FORMATS[0]]).is_err());
    }
    #[test]
    fn stop_blocks_new_plans_without_replacing_other_formats() {
        let registry = ConfigAdapters::builtin().unwrap();
        registry.stop("codex").unwrap();
        assert_eq!(
            registry.resolve("codex").err().unwrap().code,
            "plugin_disabled"
        );
        assert_eq!(
            registry.resolve("claude").unwrap().protocol(),
            Some("messages")
        );
        assert!(registry.resolve("future").is_err());
    }
}
