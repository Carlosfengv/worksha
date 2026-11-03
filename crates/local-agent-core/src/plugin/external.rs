//! Shared external plugin manifest and host DTOs. Capabilities select execution;
//! package format, API negotiation and signatures are shared by every plugin.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const API_VERSION: &str = "1.0.0";
pub const MANIFEST_VERSION: u32 = 1;
pub const SIGNING_DOMAIN: &[u8] = b"local-agent-plugin\n";
pub const MAX_FRAME: usize = 1_048_576;
pub const MAX_CHUNK: usize = 262_144;
pub const MAX_ITEMS: usize = 256;
pub const MAX_SOURCE: u64 = 8 * 1_048_576;

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Manifest {
    pub manifest_version: u32,
    pub id: String,
    pub version: String,
    pub display_name: String,
    pub publisher: String,
    pub host_api: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<super::policy::PackageKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<super::full::Source>,
    pub runtime: Runtime,
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub requires: Vec<String>,
    #[serde(default)]
    pub permissions: Vec<Permission>,
    pub agent: Option<Agent>,
    #[serde(default)]
    pub providers: Vec<ProviderPreset>,
    #[serde(default)]
    pub settings: Vec<Setting>,
    #[serde(default)]
    pub views: Vec<View>,
}
impl Manifest {
    pub fn is_extension(&self) -> bool {
        self.capabilities.iter().any(|capability| {
            matches!(
                capability.as_str(),
                "gateway.protocol@1" | "config.plan-provider@1" | "integration.connector@1"
            )
        })
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Runtime {
    Declarative,
    WasmComponent { entry: String, abi: String },
}
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Permission {
    #[serde(rename = "source.content.read")]
    SourceContentRead { resource: String },
    #[serde(rename = "connection.request")]
    ConnectionRequest { path: String },
    #[serde(rename = "ui.contribute")]
    UiContribute,
    #[serde(rename = "domain.query")]
    DomainQuery { query: String },
}
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Agent {
    pub product_id: String,
    pub display_name: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderPreset {
    pub id: String,
    pub name: String,
    pub protocol: String,
    pub endpoint: String,
    pub model: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Setting {
    pub key: String,
    pub label: String,
    pub options: Vec<String>,
    pub default: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct View {
    pub id: String,
    pub title: String,
    pub description: String,
    pub kind: ViewKind,
}
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ViewKind {
    Report,
    Sessions,
    Quota,
    Providers,
    Settings,
}
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Signature {
    pub algorithm: String,
    pub public_key: String,
    pub signature: String,
}
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PackageReview {
    pub package_ref: String,
    pub digest: String,
    pub manifest: Manifest,
    pub signing_key: Option<String>,
    pub trusted_publisher: bool,
    pub development: bool,
    pub compressed_bytes: u64,
    pub expanded_bytes: u64,
}
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InstallRequest {
    pub package_ref: String,
    pub digest: String,
    pub approve_publisher: bool,
}
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StateRequest {
    pub plugin_id: String,
    pub digest: String,
    pub action: StateAction,
}
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum StateAction {
    Enable,
    Disable,
    Remove,
    Rollback,
    RevokeGrants,
    RevokePublisher,
}
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ViewQuery {
    pub plugin_id: String,
    pub view_id: String,
    pub cursor: Option<String>,
}
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ActionRequest {
    pub plugin_id: String,
    pub digest: String,
    pub action: PublicAction,
}
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum PublicAction {
    AuthorizeReport,
    Refresh,
    #[serde(rename_all = "camelCase")]
    ReadSession {
        session_id: String,
        snapshot: String,
    },
    #[serde(rename_all = "camelCase")]
    BindConnection {
        provider_id: String,
        allow_private_network: bool,
    },
    SaveSettings {
        values: BTreeMap<String, String>,
    },
    CreateProvider {
        preset: String,
    },
}
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Status {
    pub report_authorized: bool,
    pub manifest: Manifest,
    pub digest: String,
    pub signing_key: Option<String>,
    pub enabled: bool,
    pub runtime_status: String,
    pub error_code: Option<String>,
    pub activation_epoch: String,
    pub grant_revision: String,
    pub source_authorized: bool,
    pub connection_authorized: bool,
    pub source_label: Option<String>,
    pub connection_label: Option<String>,
    pub previous_digest: Option<String>,
    pub settings: BTreeMap<String, String>,
}
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Initialize {
    pub settings: BTreeMap<String, String>,
    pub api_version: String,
    pub capabilities: Vec<String>,
    pub package_digest: String,
    pub activation_epoch: String,
    pub grant_revision: String,
    pub context_id: String,
    pub fuel: String,
    pub max_memory_bytes: String,
}
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceRequest {
    pub context_id: String,
    pub source_ref: String,
    pub activation_epoch: String,
    pub grant_revision: String,
    pub cursor: Option<String>,
}
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReadChunk {
    pub context_id: String,
    pub source_ref: String,
    pub offset: String,
    pub max_bytes: u32,
}
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceChunk {
    pub text: String,
    pub next_offset: String,
    pub complete: bool,
    pub source_revision: String,
}
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Usage {
    pub input_tokens: String,
    pub output_tokens: String,
    pub cache_read_tokens: Option<String>,
    pub reasoning_tokens: Option<String>,
}
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionCandidate {
    pub native_id: String,
    pub title: String,
    pub created_at: Option<String>,
    pub model: Option<String>,
    pub usage: Option<Usage>,
}
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceBatch {
    pub schema_version: u32,
    pub source_ref: String,
    pub source_revision: String,
    pub batch_id: String,
    pub upserts: Vec<SessionCandidate>,
    pub next_cursor: Option<String>,
    pub complete: bool,
}
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationRequest {
    pub source: SourceRequest,
    pub native_id: String,
}
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Message {
    pub role: String,
    pub text: String,
}
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Conversation {
    pub native_id: String,
    pub messages: Vec<Message>,
}
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Quota {
    pub remaining: String,
    pub limit: Option<String>,
    pub unit: String,
    pub observed_at: String,
    pub expires_at: Option<String>,
}
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct QuotaRequest {
    pub context_id: String,
    pub connection_ref: String,
    pub observed_at: String,
}
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConnectionRequest {
    pub context_id: String,
    pub connection_ref: String,
}
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GatewayReport {
    pub requests: String,
    pub attempts: String,
    pub input_tokens: Option<String>,
    pub output_tokens: Option<String>,
    pub total_tokens: Option<String>,
    pub range_start: String,
    pub range_end: String,
}

/// Generation and runtime deserialization share one Rust authority. The SDK checker
/// invokes the shipped host's validator, including semantic limits absent from JSON schema.
pub fn schemas() -> serde_json::Value {
    serde_json::json!({
        "manifest": schemars::schema_for!(Manifest),
        "package-review": schemars::schema_for!(PackageReview),
        "install": schemars::schema_for!(InstallRequest),
        "state": schemars::schema_for!(StateRequest),
        "action": schemars::schema_for!(ActionRequest),
        "view-query": schemars::schema_for!(ViewQuery),
        "status": schemars::schema_for!(Status),
        "initialize": schemars::schema_for!(Initialize),
        "source-request": schemars::schema_for!(SourceRequest),
        "read-chunk": schemars::schema_for!(ReadChunk),
        "source-chunk": schemars::schema_for!(SourceChunk),
        "source-batch": schemars::schema_for!(SourceBatch),
        "conversation-request": schemars::schema_for!(ConversationRequest),
        "conversation": schemars::schema_for!(Conversation),
        "quota-request": schemars::schema_for!(QuotaRequest),
        "connection-request": schemars::schema_for!(ConnectionRequest),
        "quota": schemars::schema_for!(Quota),
        "session-candidate": schemars::schema_for!(SessionCandidate),
        "view": schemars::schema_for!(View),
        "gateway-report": schemars::schema_for!(GatewayReport),
    })
}
