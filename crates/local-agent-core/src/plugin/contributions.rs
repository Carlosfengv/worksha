//! Additive, signed contributions.json. Existing manifest, IPC and worker DTOs stay unchanged.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const OPTIONAL: &str = "host.optional-dependencies@1";
pub const DETAILS: &str = "host.detail-slots@1";

#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Contributions {
    pub schema_version: u32,
    #[serde(default)]
    pub optional_dependencies: Vec<OptionalDependency>,
    #[serde(default)]
    pub detail_slots: Vec<DetailSlot>,
}
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OptionalDependency {
    pub plugin_id: String,
    pub version: String,
    /// Only these existing views degrade. No cross-plugin data or permission is granted.
    pub views: Vec<String>,
}
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DetailSlot {
    pub id: String,
    pub target: DetailTarget,
    /// Exact public object IDs or the single wildcard "*". Never sent to the guest.
    pub object_ids: Vec<String>,
    pub placement: DetailPlacement,
    pub view_id: String,
    pub title: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum DetailTarget {
    Agent,
    Provider,
    Account,
}
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum DetailPlacement {
    Tab,
    Card,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DependencyStatus {
    pub plugin_id: String,
    pub version: String,
    pub views: Vec<String>,
    pub available: bool,
    pub selected_version: Option<String>,
    pub reason: Option<String>,
}
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ContributionStatus {
    pub plugin_id: String,
    pub digest: String,
    pub dependencies: Vec<DependencyStatus>,
    pub slots: Vec<DetailSlot>,
    pub error_code: Option<String>,
}
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ContributionCatalog {
    pub schema_version: u32,
    pub items: Vec<ContributionStatus>,
}
pub fn schemas() -> serde_json::Value {
    serde_json::json!({
        "contributions": schemars::schema_for!(Contributions),
        "contribution-catalog": schemars::schema_for!(ContributionCatalog),
    })
}
