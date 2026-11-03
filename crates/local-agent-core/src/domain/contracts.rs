use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;

/// JSON integers never pass through a JavaScript number. Construction is canonical.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct Decimal(String);
impl From<u64> for Decimal {
    fn from(value: u64) -> Self {
        Self(value.to_string())
    }
}
impl Decimal {
    pub fn value(&self) -> u64 {
        self.0
            .parse()
            .expect("Decimal is validated on construction")
    }
}
impl<'de> Deserialize<'de> for Decimal {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let text = String::deserialize(d)?;
        let value = text.parse::<u64>().map_err(serde::de::Error::custom)?;
        if value.to_string() != text {
            return Err(serde::de::Error::custom("non-canonical unsigned decimal"));
        }
        Ok(Self(text))
    }
}
impl JsonSchema for Decimal {
    fn schema_name() -> Cow<'static, str> {
        "Decimal".into()
    }
    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({"type":"string","pattern":"^(0|[1-9][0-9]{0,19})$","maxLength":20})
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum ExecutionKind {
    Builtin,
    WasmComponent,
    Declarative,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Coverage {
    Complete,
    Partial,
    Unavailable,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Authorization {
    Required,
    Authorized,
    Revoked,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Freshness {
    Unread,
    Current,
    Stale,
    Failed,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderStatus {
    pub enabled: bool,
    pub authorization: Authorization,
    pub grant_revision: Decimal,
    pub runtime_active: bool,
    pub freshness: Freshness,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Capability {
    pub name: String,
    pub major: u16,
    pub instance_id: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Invocation {
    pub invocation_id: String,
    pub provider_key: String,
    pub implementation_identity: String,
    pub epoch: Decimal,
    pub grant_revision: Decimal,
    pub catalog_revision: Decimal,
    pub resource_ref: String,
    /// None preserves the existing native scan policy; external adapters must set a deadline.
    pub deadline_ms: Option<Decimal>,
    pub budget: Budget,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Budget {
    pub batch_items: u16,
    pub max_records: Decimal,
    pub max_document_bytes: Decimal,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScanRequest {
    pub invocation: Invocation,
    pub source_id: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Usage {
    pub input_tokens: Option<Decimal>,
    pub output_tokens: Option<Decimal>,
    pub cached_input_tokens: Option<Decimal>,
    pub cache_write_input_tokens: Option<Decimal>,
    pub reasoning_output_tokens: Option<Decimal>,
    pub total_tokens: Option<Decimal>,
}
impl From<&crate::usage::Usage> for Usage {
    fn from(u: &crate::usage::Usage) -> Self {
        Self {
            input_tokens: u.input_tokens.map(Into::into),
            output_tokens: u.output_tokens.map(Into::into),
            cached_input_tokens: u.cached_input_tokens.map(Into::into),
            cache_write_input_tokens: u.cache_write_input_tokens.map(Into::into),
            reasoning_output_tokens: u.reasoning_output_tokens.map(Into::into),
            total_tokens: u.total_tokens.map(Into::into),
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionRecord {
    pub id: String,
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
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceBatch {
    pub source_id: String,
    pub source_revision: String,
    pub batch_id: Decimal,
    pub records: Vec<SessionRecord>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Checkpoint {
    pub source_revision: String,
    pub batches: Decimal,
    pub records: Decimal,
    /// Transport finished; coverage separately describes incomplete source evidence.
    pub complete: bool,
    pub coverage: Coverage,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationRequest {
    pub invocation: Invocation,
    pub source_id: String,
    pub session_id: String,
    pub cursor: Option<String>,
    pub anchor: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationEvent {
    pub id: String,
    pub native_id: String,
    pub turn_id: String,
    pub kind: String,
    pub role: Option<String>,
    pub status: String,
    pub text: Option<String>,
    pub input: Option<String>,
    pub output: Option<String>,
    pub model: Option<String>,
    pub response_id: Option<String>,
    pub usage: Option<Usage>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationPage {
    pub session_id: String,
    pub events: Vec<ConversationEvent>,
    pub next_cursor: Option<String>,
    pub complete: bool,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    InvalidInput,
    Unsupported,
    NotFound,
    PluginDisabled,
    IncompatibleApi,
    CapabilityConflict,
    StaleSnapshot,
    GrantRevoked,
    Cancelled,
    ResourceLimit,
    DeadlineExceeded,
    Unavailable,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Failure {
    pub code: ErrorCode,
    pub retryable: bool,
}
pub type Result<T> = std::result::Result<T, Failure>;
impl Failure {
    pub fn new(code: ErrorCode) -> Self {
        Self {
            retryable: matches!(code, ErrorCode::Unavailable),
            code,
        }
    }
}
