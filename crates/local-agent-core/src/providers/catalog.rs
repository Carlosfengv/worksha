//! Validated built-in preset contributions, shared with the UI. Stored connections take precedence.
use crate::plugin::{
    BuiltinPlugin, CapabilityKey, PluginDescriptor, PluginError, PluginHost, PluginResources,
};
use serde::Deserialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::LazyLock,
};
#[derive(Clone, Deserialize)]
pub struct ProviderPreset {
    pub id: String,
    pub name: String,
    pub protocol: String,
    #[serde(default)]
    pub model: String,
    pub endpoints: BTreeMap<String, String>,
    pub documents: Option<String>,
}
struct Presets;
impl BuiltinPlugin for Presets {
    fn descriptor(&self) -> PluginDescriptor {
        PluginDescriptor {
            id: "provider.presets",
            version: "1.0.0",
            provides: vec![CapabilityKey {
                name: "provider.catalog",
                version: 1,
                instance: "builtin",
            }],
            requires: vec![],
        }
    }
    fn prepare(&self) -> Result<PluginResources, PluginError> {
        Ok(PluginResources::default())
    }
}
pub struct ProviderCatalogue {
    host: PluginHost,
    presets: Vec<ProviderPreset>,
}
impl ProviderCatalogue {
    pub fn from_json(source: &str) -> Result<Self, String> {
        let presets: Vec<ProviderPreset> =
            serde_json::from_str(source).map_err(|_| "供应商目录格式无效")?;
        let mut ids = BTreeSet::new();
        for preset in &presets {
            if preset.id.is_empty()
                || preset.name.is_empty()
                || preset.model.chars().count() > 200
                || preset.model.chars().any(char::is_control)
                || !ids.insert(&preset.id)
                || !["messages", "responses", "chat-completions"]
                    .contains(&preset.protocol.as_str())
            {
                return Err("供应商目录贡献无效或重复".into());
            }
            for endpoint in preset.endpoints.values() {
                super::validate_endpoint(endpoint).map_err(|e| e.message)?;
            }
        }
        Ok(Self {
            host: PluginHost::mount(&[&Presets]).map_err(|e| e.message)?,
            presets,
        })
    }
    pub fn presets(&self) -> &[ProviderPreset] {
        self.host
            .acquire(&Presets.descriptor().provides[0])
            .expect("immutable preset registration");
        &self.presets
    }
    pub fn contains(&self, id: &str) -> bool {
        self.presets().iter().any(|p| p.id == id)
    }
}
pub fn builtin() -> &'static ProviderCatalogue {
    static CATALOGUE: LazyLock<ProviderCatalogue> = LazyLock::new(|| {
        ProviderCatalogue::from_json(include_str!("../../../../resources/provider-catalog.json"))
            .expect("builtin provider presets")
    });
    &CATALOGUE
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn contributions_validate_duplicates_and_preserve_legacy_order() {
        assert_eq!(
            builtin()
                .presets()
                .iter()
                .map(|p| p.id.as_str())
                .collect::<Vec<_>>(),
            [
                "openai",
                "anthropic",
                "openrouter",
                "deepseek",
                "kimi",
                "minimax-plan",
                "ollama",
                "vllm",
                "custom"
            ]
        );
        assert!(
            ProviderCatalogue::from_json(
                r#"[{"id":"x","name":"X","protocol":"future","endpoints":{}}]"#
            )
            .is_err()
        );
        assert!(ProviderCatalogue::from_json(r#"[{"id":"x","name":"X","protocol":"messages","endpoints":{}},{"id":"x","name":"X","protocol":"messages","endpoints":{}}]"#).is_err());
    }
}
