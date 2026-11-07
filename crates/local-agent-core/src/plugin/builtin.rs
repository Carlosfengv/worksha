//! Trusted, compiled composition metadata. This is not the external Wasm manifest.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum CapabilityKind {
    #[default]
    Query,
    Command,
    Stream,
    UiContribution,
}
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum ResourceScope {
    #[default]
    PluginInstance,
    NativeSource,
    Project,
    Provider,
    Application,
}
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CapabilityMetadata {
    pub kind: CapabilityKind,
    pub resource_scope: ResourceScope,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CapabilityAddress {
    pub name: String,
    pub version: u16,
    pub instance: String,
}
impl From<&super::CapabilityKey> for CapabilityAddress {
    fn from(key: &super::CapabilityKey) -> Self {
        Self {
            name: key.name.into(),
            version: key.version,
            instance: key.instance.into(),
        }
    }
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CapabilityInfo {
    pub address: CapabilityAddress,
    pub metadata: CapabilityMetadata,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResourceCount {
    pub kind: super::ResourceKind,
    pub count: usize,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginInfo {
    pub id: String,
    pub display_name: String,
    pub version: String,
    pub execution_kind: crate::domain::ExecutionKind,
    pub desired_enabled: bool,
    pub runtime_active: bool,
    pub activation_epoch: crate::domain::Decimal,
    pub capabilities: Vec<CapabilityInfo>,
    pub requires: Vec<CapabilityAddress>,
    pub optional_requires: Vec<CapabilityAddress>,
    pub resources: Vec<ResourceCount>,
    pub problem: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Profile {
    #[default]
    Desktop,
    Background,
    Recovery,
}
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Composition {
    pub profile: Profile,
    pub disabled_plugin_ids: Vec<String>,
    /// Removing a bundled plugin unregisters its functionality; the signed app
    /// remains the trusted reinstall source. Historical domain data is retained.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub removed_plugin_ids: Vec<String>,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CompositionChange {
    pub operation_id: String,
    pub store_id: String,
    pub expected_revision: crate::domain::Decimal,
    pub expected_generation: String,
    pub composition: Composition,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_impact: Option<ImpactContext>,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CompositionReceipt {
    pub operation_id: String,
    pub store_id: String,
    pub revision: crate::domain::Decimal,
    pub composition: Composition,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CompositionCatalog {
    pub schema_version: u16,
    pub store_id: String,
    pub revision: crate::domain::Decimal,
    pub composition: Composition,
    pub effective_profile: Profile,
    pub host: super::HostSnapshot,
    pub problem: Option<String>,
    #[serde(default)]
    pub impact: ImpactContext,
}
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ImpactContext {
    pub active_tasks: bool,
    pub model_services: Vec<String>,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CompositionReply {
    pub receipt: Option<CompositionReceipt>,
    pub catalog: CompositionCatalog,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CompositionOperation {
    pub store_id: String,
    pub operation_id: String,
}

pub fn schemas() -> serde_json::Value {
    serde_json::json!({
        "composition": schemars::schema_for!(Composition),
        "change": schemars::schema_for!(CompositionChange),
        "receipt": schemars::schema_for!(CompositionReceipt),
        "catalog": schemars::schema_for!(CompositionCatalog),
        "reply": schemars::schema_for!(CompositionReply),
        "operation": schemars::schema_for!(CompositionOperation)
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn builtin_host_schemas_remain_separate_from_external_plugin_permissions() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../contracts/plugins/builtin");
        let schemas = super::schemas();
        assert_eq!(schemas.as_object().unwrap().len(), 6);
        for (name, expected) in schemas.as_object().unwrap() {
            let actual: serde_json::Value = serde_json::from_slice(
                &std::fs::read(root.join(format!("{name}.schema.json"))).unwrap(),
            )
            .unwrap();
            assert_eq!(&actual, expected, "{name}");
        }
    }
}
