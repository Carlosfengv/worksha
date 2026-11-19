//! Signed, host-rendered platform declarations and request contracts.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

use super::{contributions::NavigationGroup, external::Manifest};
mod authoring;
pub use authoring::compile_rules;

pub const CAPABILITY: &str = "ui.platform@1";
pub const STORAGE: &str = "storage.plugin-data@1";
pub const DIRECTORY_SOURCE: &str = "source.directory.read";
pub const PROVIDER_ADAPTER: &str = "provider.adapter@1";
pub const MAX_ROWS: u64 = 10_000;
pub const MAX_BYTES: u64 = 8 * 1_048_576;
pub const MAX_DECLARATION_BYTES: usize = 128 * 1024;

pub type Fields = BTreeMap<String, Value>;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RulePackage {
    pub manifest: Manifest,
    pub platform: PlatformDefinition,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlatformDefinition {
    pub schema_version: u32,
    #[serde(default)]
    pub collections: BTreeMap<String, CollectionDef>,
    #[serde(default)]
    pub views: BTreeMap<String, PlatformViewDef>,
    #[serde(default)]
    pub slots: Vec<PlatformSlotDef>,
    pub log_sink: Option<LogSinkDef>,
    pub adapter: Option<AdapterDef>,
    pub wasm: Option<PlatformWasmDef>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlatformWasmDef {
    #[serde(default)]
    pub render: bool,
    #[serde(default)]
    pub action: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CollectionDef {
    pub fields: BTreeMap<String, FieldDef>,
    pub group_by: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FieldDef {
    pub label: String,
    pub kind: FieldKind,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub options: Vec<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum FieldKind {
    Text,
    Textarea,
    Number,
    Boolean,
    Enum,
    #[serde(rename = "ref:session")]
    Session,
    #[serde(rename = "ref:project")]
    Project,
    #[serde(rename = "ref:agent")]
    Agent,
    #[serde(rename = "ref:provider")]
    Provider,
}

impl FieldKind {
    pub fn reference(self) -> Option<EntityKind> {
        match self {
            Self::Session => Some(EntityKind::Session),
            Self::Project => Some(EntityKind::Project),
            Self::Agent => Some(EntityKind::Agent),
            Self::Provider => Some(EntityKind::Provider),
            _ => None,
        }
    }
}

#[derive(
    Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, Eq, Ord, PartialEq, PartialOrd,
)]
#[serde(rename_all = "kebab-case")]
pub enum EntityKind {
    Session,
    Project,
    Agent,
    Provider,
    Request,
    Account,
}

impl EntityKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Session => "session",
            Self::Project => "project",
            Self::Agent => "agent",
            Self::Provider => "provider",
            Self::Request => "request",
            Self::Account => "account",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlatformViewDef {
    pub title: String,
    pub kind: ViewKind,
    pub collection: Option<String>,
    pub primary_field: Option<String>,
    #[serde(default)]
    pub actions: Vec<PlatformAction>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum ViewKind {
    Kanban,
    DataTable,
    Form,
    StatRow,
    DetailList,
    ActionBar,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlatformSlotDef {
    pub id: String,
    pub point: SlotPoint,
    pub view_id: String,
    pub filter_by_context: Option<String>,
    pub group: Option<NavigationGroup>,
    pub icon: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, Eq, PartialEq)]
pub enum SlotPoint {
    #[serde(rename = "nav.home-sidebar")]
    NavHomeSidebar,
    #[serde(rename = "overview.section")]
    OverviewSection,
    #[serde(rename = "sessions.toolbar.actions")]
    SessionsToolbarActions,
    #[serde(rename = "requests.toolbar.actions")]
    RequestsToolbarActions,
    #[serde(rename = "session.inspector.card")]
    SessionInspectorCard,
    #[serde(rename = "session.inspector.tab")]
    SessionInspectorTab,
    #[serde(rename = "session.detail.actions")]
    SessionDetailActions,
    #[serde(rename = "project.detail.tab")]
    ProjectDetailTab,
    #[serde(rename = "project.detail.card")]
    ProjectDetailCard,
    #[serde(rename = "project.detail.actions")]
    ProjectDetailActions,
    #[serde(rename = "request.detail.card")]
    RequestDetailCard,
    #[serde(rename = "request.detail.actions")]
    RequestDetailActions,
    #[serde(rename = "agent.detail.tab")]
    AgentDetailTab,
    #[serde(rename = "agent.detail.card")]
    AgentDetailCard,
    #[serde(rename = "agent.detail.actions")]
    AgentDetailActions,
    #[serde(rename = "provider.detail.tab")]
    ProviderDetailTab,
    #[serde(rename = "provider.detail.card")]
    ProviderDetailCard,
    #[serde(rename = "provider.detail.actions")]
    ProviderDetailActions,
    #[serde(rename = "account.detail.tab")]
    AccountDetailTab,
    #[serde(rename = "account.detail.card")]
    AccountDetailCard,
    #[serde(rename = "account.detail.actions")]
    AccountDetailActions,
}

impl SlotPoint {
    pub const fn target(self) -> Option<EntityKind> {
        match self {
            Self::SessionInspectorCard | Self::SessionInspectorTab | Self::SessionDetailActions => {
                Some(EntityKind::Session)
            }
            Self::ProjectDetailTab | Self::ProjectDetailCard | Self::ProjectDetailActions => {
                Some(EntityKind::Project)
            }
            Self::RequestDetailCard | Self::RequestDetailActions => Some(EntityKind::Request),
            Self::AgentDetailTab | Self::AgentDetailCard | Self::AgentDetailActions => {
                Some(EntityKind::Agent)
            }
            Self::ProviderDetailTab | Self::ProviderDetailCard | Self::ProviderDetailActions => {
                Some(EntityKind::Provider)
            }
            Self::AccountDetailTab | Self::AccountDetailCard | Self::AccountDetailActions => {
                Some(EntityKind::Account)
            }
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, Eq, PartialEq)]
pub enum PlatformAction {
    #[serde(rename = "records.create")]
    Create,
    #[serde(rename = "records.update")]
    Update,
    #[serde(rename = "records.move")]
    Move,
    #[serde(rename = "records.delete")]
    Delete,
    #[serde(rename = "kv.set")]
    SetValue,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LogSinkDef {
    pub format: LogFormat,
    pub rotate_policy: RotatePolicy,
    #[serde(default)]
    pub filter: LogFilter,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum LogFormat {
    Jsonl,
    Csv,
    Wasm,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum RotatePolicy {
    Daily,
    Hourly,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LogFilter {
    pub client_scope: Option<String>,
    pub outcome: Option<String>,
    pub model: Option<String>,
    pub agent: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(
    tag = "kind",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum AdapterDef {
    AgentAdapter { source_mapper: Box<SourceMapper> },
    ProviderAdapter,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceMapper {
    pub format: SourceFormat,
    pub glob: String,
    pub session_id: String,
    pub title: String,
    pub project: Option<String>,
    pub model: Option<String>,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    pub usage: Option<UsageMapper>,
    pub activity: Option<SourceActivityMapper>,
    pub conversation: Option<SourceConversationMapper>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceActivityMapper {
    pub phase: String,
    pub occurred_at: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceConversationMapper {
    pub messages: String,
    pub role: String,
    pub text: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UsageMapper {
    pub input_tokens: String,
    pub output_tokens: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum SourceFormat {
    Json,
    Jsonl,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlatformContext {
    pub target: EntityKind,
    pub object_id: String,
    pub scope: Option<String>,
    pub unit: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlatformSlotQuery {
    pub plugin_id: String,
    pub digest: String,
    pub slot_or_view_id: String,
    pub context: Option<PlatformContext>,
    pub cursor: Option<String>,
    #[serde(default)]
    pub filter: Fields,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlatformActionRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receipt_only: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub store_id: Option<String>,
    pub plugin_id: String,
    pub digest: String,
    pub expected_epoch: String,
    pub expected_grant_revision: String,
    pub action_id: PlatformAction,
    pub collection: Option<String>,
    pub record_id: Option<String>,
    #[serde(default)]
    pub values: Fields,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_values: Option<Fields>,
    pub context: Option<PlatformContext>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum DirectoryPurpose {
    SourceRead,
    LogSink,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DirectoryGrantRequest {
    pub plugin_id: String,
    pub digest: String,
    pub purpose: DirectoryPurpose,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlatformManageRequest {
    pub plugin_id: String,
    pub digest: String,
    pub expected_grant_revision: String,
    pub operation: PlatformManagement,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(
    tag = "kind",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum PlatformManagement {
    GrantEntity {
        target: EntityKind,
    },
    RevokeEntity {
        target: EntityKind,
    },
    RevokeDirectory {
        purpose: DirectoryPurpose,
    },
    ConfigureSink {
        filter: LogFilter,
        rotate_policy: RotatePolicy,
    },
    FlushSink,
    ClearData,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlatformContribution {
    pub plugin_id: String,
    pub digest: String,
    pub epoch: String,
    pub grant_revision: String,
    pub enabled: bool,
    pub definition: Option<PlatformDefinition>,
    pub entity_grants: Vec<EntityKind>,
    pub directory_grants: Vec<DirectoryPurpose>,
    pub used_rows: u64,
    pub used_bytes: u64,
    pub sink: Option<SinkStatus>,
    pub diagnostic: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SinkStatus {
    #[serde(default)]
    pub legacy_coverage_unknown: bool,
    pub exported_rows: u64,
    pub written_bytes: u64,
    pub quota_bytes: u64,
    pub last_flushed_at: Option<String>,
    pub last_error: Option<String>,
    pub filter: LogFilter,
    pub rotate_policy: RotatePolicy,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlatformContributionList {
    pub items: Vec<PlatformContribution>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlatformSlotView {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action_store_id: Option<String>,
    pub blocks: Vec<PlatformViewDef>,
    pub records: Vec<PlatformRecord>,
    pub next_cursor: Option<String>,
    pub coverage: Coverage,
    pub freshness: Freshness,
    pub authorization: Authorization,
    pub epoch: String,
    pub grant_revision: String,
    pub sink: Option<SinkStatus>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Coverage {
    Complete,
    Partial,
    Unknown,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Freshness {
    Current,
    Unknown,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Authorization {
    Authorized,
    Unauthorized,
    Unknown,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlatformRecord {
    pub record_id: String,
    pub values: Fields,
    #[serde(rename = "_hydrated")]
    pub hydrated: BTreeMap<String, HydratedEntity>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HydratedEntity {
    pub authorization: Authorization,
    pub freshness: Freshness,
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_id: Option<String>,
    pub activity_state: Option<String>,
    pub model: Option<String>,
    pub total_tokens: Option<u64>,
    pub route_hash: Option<String>,
}

pub fn schemas() -> Value {
    serde_json::json!({
        "definition": schemars::schema_for!(PlatformDefinition),
        "contributions": schemars::schema_for!(PlatformContributionList),
        "slot-query": schemars::schema_for!(PlatformSlotQuery),
        "slot-view": schemars::schema_for!(PlatformSlotView),
        "action": schemars::schema_for!(PlatformActionRequest),
        "directory-grant": schemars::schema_for!(DirectoryGrantRequest),
        "manage": schemars::schema_for!(PlatformManageRequest),
    })
}
