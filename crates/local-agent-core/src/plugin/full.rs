//! Complete read-only source capabilities of the shared external plugin API.
use crate::domain::{
    self as d,
    facts::{FactBatch, FactKind},
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub use super::external::{API_VERSION, Manifest, Permission, Runtime, SIGNING_DOMAIN};
pub const SOURCE_BROKER: &str = "host.source-broker@1";
pub const FACT_FEATURE: &str = "host.domain-facts@1";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum Capability {
    #[serde(rename = "domain.agent-directory@1")]
    AgentDirectory,
    #[serde(rename = "domain.session-source@1")]
    SessionSource,
    #[serde(rename = "domain.call-source@1")]
    CallSource,
    #[serde(rename = "domain.project-source@1")]
    ProjectSource,
    #[serde(rename = "domain.activity-source@1")]
    ActivitySource,
    #[serde(rename = "domain.mcp-source@1")]
    McpSource,
}
impl Capability {
    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "domain.agent-directory@1" => Self::AgentDirectory,
            "domain.session-source@1" => Self::SessionSource,
            "domain.call-source@1" => Self::CallSource,
            "domain.project-source@1" => Self::ProjectSource,
            "domain.activity-source@1" => Self::ActivitySource,
            "domain.mcp-source@1" => Self::McpSource,
            _ => return None,
        })
    }
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::AgentDirectory => "domain.agent-directory@1",
            Self::SessionSource => "domain.session-source@1",
            Self::CallSource => "domain.call-source@1",
            Self::ProjectSource => "domain.project-source@1",
            Self::ActivitySource => "domain.activity-source@1",
            Self::McpSource => "domain.mcp-source@1",
        }
    }
    pub fn name(&self) -> &'static str {
        match self {
            Self::AgentDirectory => "domain.agent-directory",
            Self::SessionSource => "domain.session-source",
            Self::CallSource => "domain.call-source",
            Self::ProjectSource => "domain.project-source",
            Self::ActivitySource => "domain.activity-source",
            Self::McpSource => "domain.mcp-source",
        }
    }
    pub fn fact_kinds(&self) -> &'static [FactKind] {
        match self {
            Self::AgentDirectory => &[],
            Self::SessionSource => &[FactKind::Session],
            Self::CallSource => &[FactKind::Response],
            Self::ProjectSource => &[FactKind::Project, FactKind::Write],
            Self::ActivitySource => &[FactKind::Activity],
            Self::McpSource => &[FactKind::McpCatalog, FactKind::McpExecution],
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Product {
    pub id: String,
    pub display_name: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Source {
    /// Package-local stable name; the host prefixes the plugin namespace.
    pub id: String,
    pub products: Vec<Product>,
    /// Explicit package opt-in; selecting this resource authorizes periodic reads.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_seconds: Option<u32>,
    /// For live reporters, a heartbeat older than this is stale.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_age_seconds: Option<u32>,
}
impl Manifest {
    pub fn source_id(&self) -> Option<String> {
        self.source
            .as_ref()
            .map(|source| format!("{}:{}", self.id, source.id))
    }
    pub fn product_ids(&self) -> Vec<String> {
        self.source
            .iter()
            .flat_map(|source| &source.products)
            .map(|p| format!("{}:{}", self.id, p.id))
            .collect()
    }
    pub fn fact_kinds(&self) -> Vec<FactKind> {
        self.capabilities
            .iter()
            .filter_map(|c| Capability::parse(c))
            .flat_map(|c| c.fact_kinds().iter().copied())
            .collect()
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Review {
    pub package_ref: String,
    pub digest: String,
    pub manifest: Manifest,
    pub signing_key: String,
    pub trusted_publisher: bool,
    pub compressed_bytes: d::Decimal,
    pub expanded_bytes: d::Decimal,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PackageQuery {
    pub plugin_id: String,
    pub digest: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Install {
    pub package_ref: String,
    pub digest: String,
    pub approve_publisher: bool,
    /// The key visible in the review is bound again at submission.
    pub expected_signing_key: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum StateAction {
    Enable,
    Disable,
    Remove,
    Rollback,
    RevokeGrants,
    RevokePublisher,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StateChange {
    pub package: PackageQuery,
    pub action: StateAction,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Status {
    pub manifest: Manifest,
    pub digest: String,
    pub signing_key: String,
    pub enabled: bool,
    pub runtime_active: bool,
    pub publisher_trusted: bool,
    pub activation_epoch: d::Decimal,
    pub grant_revision: d::Decimal,
    pub source_authorized: bool,
    pub source_label: Option<String>,
    pub unified_authorized: bool,
    pub persist_metadata: bool,
    pub freshness: d::Freshness,
    pub fact_summary: Option<d::facts::FactSummary>,
    pub problem: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Catalog {
    pub api_version: String,
    pub items: Vec<Status>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScanRequest {
    pub invocation: d::Invocation,
    pub source_id: String,
    pub source_revision: String,
    pub cursor: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScanPage {
    pub batch: Option<FactBatch>,
    pub checkpoint: Option<d::Checkpoint>,
    pub next_cursor: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationRequest {
    pub invocation: d::Invocation,
    pub source_id: String,
    pub source_revision: String,
    pub native_id: String,
    pub cursor: Option<String>,
    pub anchor: Option<String>,
}
pub fn schemas() -> serde_json::Value {
    serde_json::json!({
        "manifest": schemars::schema_for!(Manifest),
        "review": schemars::schema_for!(Review),
        "install": schemars::schema_for!(Install),
        "package-query": schemars::schema_for!(PackageQuery),
        "state-change": schemars::schema_for!(StateChange),
        "status": schemars::schema_for!(Status),
        "catalog": schemars::schema_for!(Catalog),
        "scan-request": schemars::schema_for!(ScanRequest),
        "scan-page": schemars::schema_for!(ScanPage)
        ,"conversation-request": schemars::schema_for!(ConversationRequest)
        ,"conversation-page": schemars::schema_for!(d::ConversationPage)
    })
}
