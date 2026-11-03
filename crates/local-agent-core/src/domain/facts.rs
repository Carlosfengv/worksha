//! Complete source facts, separate from the accepted session-only v1 contract.
//! Candidates cannot supply host authorization, observation time or trust claims.
use super::{Checkpoint, Coverage, Decimal, ErrorCode, Failure, Result, Usage};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub mod publication;
pub mod query;
mod summary;
pub use summary::{FactSummary, FactTally, UnitUsage, UsageUnit};

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum FactKind {
    Project,
    Session,
    Response,
    Activity,
    Write,
    McpCatalog,
    McpExecution,
}
impl FactKind {
    pub const ALL: [Self; 7] = [
        Self::Project,
        Self::Session,
        Self::Response,
        Self::Activity,
        Self::Write,
        Self::McpCatalog,
        Self::McpExecution,
    ];
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProjectCandidate {
    pub native_id: String,
    pub name: String,
    /// Display evidence only. Never passed to native identity/filesystem readers.
    pub path_hint: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionCandidate {
    pub native_id: String,
    pub product_ids: Vec<String>,
    pub unknown_client: bool,
    pub title: String,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    pub project_ref: Option<String>,
    pub usage: Usage,
    pub partial: bool,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Outcome {
    Success,
    Failure,
    Unknown,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResponseCandidate {
    pub native_id: String,
    pub session_ref: String,
    pub product_id: Option<String>,
    pub occurred_at: Option<String>,
    pub model: Option<String>,
    pub outcome: Outcome,
    pub usage: Usage,
    /// A source claim, never sufficient for cross-source gateway correlation.
    pub reported_response_id: Option<String>,
    pub duration_ms: Option<Decimal>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum ActivityPhase {
    Thinking,
    Generating,
    Tool,
    Idle,
    Unknown,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ActivityCandidate {
    pub native_id: String,
    pub session_ref: String,
    pub response_ref: Option<String>,
    pub occurred_at: Option<String>,
    pub phase: ActivityPhase,
    /// Historical source duration; does not establish a host monotonic sample.
    pub duration_ms: Option<Decimal>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WriteCandidate {
    pub native_id: String,
    pub session_ref: String,
    pub project_ref: String,
    pub response_ref: Option<String>,
    pub relative_path: String,
    pub occurred_at: Option<String>,
    pub outcome: Outcome,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpCatalogCandidate {
    pub native_id: String,
    pub server_name: String,
    /// None means not observed; an observed empty catalog is Some([]).
    pub tool_names: Option<Vec<String>>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpExecutionCandidate {
    pub native_id: String,
    pub server_ref: Option<String>,
    pub session_ref: Option<String>,
    pub response_ref: Option<String>,
    pub tool_name: String,
    pub occurred_at: Option<String>,
    pub duration_ms: Option<Decimal>,
    pub outcome: Outcome,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(
    tag = "kind",
    content = "value",
    rename_all = "kebab-case",
    deny_unknown_fields
)]
pub enum FactCandidate {
    Project(ProjectCandidate),
    Session(SessionCandidate),
    Response(ResponseCandidate),
    Activity(ActivityCandidate),
    Write(WriteCandidate),
    McpCatalog(McpCatalogCandidate),
    McpExecution(McpExecutionCandidate),
}
impl FactCandidate {
    pub fn fact_kind(&self) -> FactKind {
        match self {
            Self::Project(_) => FactKind::Project,
            Self::Session(_) => FactKind::Session,
            Self::Response(_) => FactKind::Response,
            Self::Activity(_) => FactKind::Activity,
            Self::Write(_) => FactKind::Write,
            Self::McpCatalog(_) => FactKind::McpCatalog,
            Self::McpExecution(_) => FactKind::McpExecution,
        }
    }
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Project(_) => "project",
            Self::Session(_) => "session",
            Self::Response(_) => "response",
            Self::Activity(_) => "activity",
            Self::Write(_) => "write",
            Self::McpCatalog(_) => "mcp-catalog",
            Self::McpExecution(_) => "mcp-execution",
        }
    }
    pub fn native_id(&self) -> &str {
        match self {
            Self::Project(v) => &v.native_id,
            Self::Session(v) => &v.native_id,
            Self::Response(v) => &v.native_id,
            Self::Activity(v) => &v.native_id,
            Self::Write(v) => &v.native_id,
            Self::McpCatalog(v) => &v.native_id,
            Self::McpExecution(v) => &v.native_id,
        }
    }
    pub fn occurred_at(&self) -> Option<&str> {
        match self {
            Self::Project(_) | Self::McpCatalog(_) => None,
            Self::Session(v) => v.updated_at.as_deref().or(v.created_at.as_deref()),
            Self::Response(v) => v.occurred_at.as_deref(),
            Self::Activity(v) => v.occurred_at.as_deref(),
            Self::Write(v) => v.occurred_at.as_deref(),
            Self::McpExecution(v) => v.occurred_at.as_deref(),
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FactBatch {
    pub source_revision: String,
    pub batch_id: Decimal,
    pub records: Vec<FactCandidate>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum EvidenceLevel {
    BuiltinNative,
    SourceReported,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FactIdentity {
    pub provider_key: String,
    pub source_id: String,
    pub implementation_identity: String,
    pub epoch: Decimal,
    pub grant_revision: Decimal,
    pub observed_at: String,
    pub evidence: EvidenceLevel,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FactRecord {
    pub id: String,
    pub identity: FactIdentity,
    pub source_revision: String,
    pub value: FactCandidate,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FactSnapshot {
    pub identity: FactIdentity,
    pub available_kinds: Vec<FactKind>,
    pub source_revision: String,
    pub coverage: Coverage,
    pub records: Vec<FactRecord>,
}

pub fn schemas() -> serde_json::Value {
    serde_json::json!({
        "fact-batch": schemars::schema_for!(FactBatch),
        "fact-snapshot": schemars::schema_for!(FactSnapshot),
        "fact-checkpoint": schemars::schema_for!(Checkpoint),
        "fact-summary": schemars::schema_for!(FactSummary),
        "fact-query": schemars::schema_for!(query::FactQuery),
        "fact-page": schemars::schema_for!(query::FactPage),
        "project-visibility": schemars::schema_for!(query::ProjectVisibility),
        "capability-catalog": schemars::schema_for!(super::CapabilityCatalog),
        "quota-request": schemars::schema_for!(super::accounts::QuotaRequest),
        "account-quota": schemars::schema_for!(super::accounts::AccountQuota),
        "mcp-request": schemars::schema_for!(super::mcp::McpRequest),
        "mcp-facts": schemars::schema_for!(super::mcp::McpFacts),
        "mcp-catalog-request": schemars::schema_for!(super::mcp::McpCatalogRequest),
        "mcp-catalog-snapshot": schemars::schema_for!(super::mcp::McpCatalogSnapshot),
        "memory-request": schemars::schema_for!(super::memory::MemoryRequest),
        "memory-snapshot": schemars::schema_for!(super::memory::MemorySnapshot),
        "activity-request": schemars::schema_for!(super::activity::ActivityRequest),
        "activity-snapshot": schemars::schema_for!(super::activity::ActivitySnapshot),
        "unified-activity-query": schemars::schema_for!(super::activity::query::UnifiedActivityQuery),
        "unified-activity-page": schemars::schema_for!(super::activity::query::UnifiedActivityPage),
        "request-read": schemars::schema_for!(super::requests::RequestRead),
        "request-snapshot": schemars::schema_for!(super::requests::RequestSnapshot),
        "unified-request-query": schemars::schema_for!(super::requests::query::UnifiedRequestQuery),
        "unified-request-page": schemars::schema_for!(super::requests::query::UnifiedRequestPage)
    })
}
