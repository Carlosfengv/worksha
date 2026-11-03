use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Settings {
    pub capture_content: bool,
    pub retention_days: u16,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            capture_content: false,
            retention_days: 30,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelCatalogSource {
    pub fetched_at: String,
    pub source_url: String,
    pub endpoint: String,
    pub protocol: String,
    pub credential_ref: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Provider {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub vendor: String,
    pub endpoint: String,
    pub protocol: String,
    pub model: String,
    pub configured: bool,
    #[serde(default)]
    pub credential_ref: Option<String>,
    #[serde(default)]
    pub credential_label: Option<String>,
    #[serde(default)]
    pub models: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub disabled_models: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_catalog: Option<ModelCatalogSource>,
    #[serde(default)]
    pub revision: u64,
    #[serde(default = "provider_category")]
    pub category: String,
}

impl Provider {
    pub fn enabled_model_ids(&self) -> Vec<String> {
        let disabled: std::collections::HashSet<_> =
            self.disabled_models.iter().map(String::as_str).collect();
        std::iter::once(&self.model)
            .chain(&self.models)
            .filter(|model| !model.is_empty() && !disabled.contains(model.as_str()))
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .cloned()
            .collect()
    }

    pub fn model_enabled(&self, model: &str) -> bool {
        !model.is_empty()
            && (model == self.model || self.models.iter().any(|id| id == model))
            && !self.disabled_models.iter().any(|id| id == model)
    }
}

fn provider_category() -> String {
    "cloud".into()
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentDiscovery {
    pub id: String,
    pub family: String,
    pub config_id: String,
    pub name: String,
    pub found: bool,
    pub location: Option<String>,
    pub connected: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub hidden_projects: Vec<crate::projects::visibility::HiddenProject>,
    pub version: &'static str,
    pub database_path: String,
    pub gateway_status: &'static str,
    pub agents: Vec<AgentDiscovery>,
    pub providers: Vec<Provider>,
    pub provider_write_protocol: crate::providers::operations::Protocol,
    pub settings: Settings,
    pub settings_revision: u64,
    pub settings_write_protocol: crate::settings::SettingsWriteProtocol,
}
