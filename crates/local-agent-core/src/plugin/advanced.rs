//! Protocol, finite configuration and connector capabilities of the shared plugin API.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub use super::external::API_VERSION;
pub const MAX_WIRE_CHUNK: usize = 48 * 1024;
pub const MAX_DOCUMENT: u64 = 256 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Policy {
    pub schema_version: u32,
    pub protocol: Option<Protocol>,
    pub config: Option<Config>,
    pub connector: Option<Connector>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Protocol {
    pub base_protocol: String,
    pub path: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Config {
    pub fields: Vec<ConfigField>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfigField {
    pub pointer: String,
    pub value: BindingValue,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum BindingValue {
    Endpoint,
    Model,
    Credential,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Connector {
    McpStdio,
    McpHttp,
    DshSessions,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Query {
    pub plugin_id: String,
    pub digest: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Request {
    pub plugin_id: String,
    pub digest: String,
    pub action: Action,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Action {
    #[serde(rename_all = "camelCase")]
    BindProtocol {
        provider_id: String,
        allow_private_network: bool,
    },
    UnbindProtocol,
    #[serde(rename_all = "camelCase")]
    PrepareConfig {
        provider_id: String,
    },
    #[serde(rename_all = "camelCase")]
    ApplyConfig {
        plan_id: String,
        revision: String,
    },
    #[serde(rename_all = "camelCase")]
    RestoreConfig {
        operation_id: String,
    },
    #[serde(rename_all = "camelCase")]
    DiscardConfig {
        plan_id: String,
    },
    StartConnector,
    StopConnector,
    RefreshConnector,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProtocolOpen {
    pub stream: bool,
    pub request: Value,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WireChunk {
    pub bytes: Vec<u8>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WireOutput {
    pub bytes: Vec<u8>,
    pub complete: bool,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct JsonEdit {
    pub pointer: String,
    pub value: Value,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfigCandidate {
    pub edits: Vec<JsonEdit>,
}
/// Selected by the user through a native file picker. Never supplied by a package.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "transport", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Connection {
    Stdio {
        command: String,
        args: Vec<String>,
    },
    #[serde(rename_all = "camelCase")]
    Http {
        url: String,
        allow_private_network: bool,
        provider_id: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    Dsh {
        url: String,
        launch_token: String,
        version: String,
    },
}
pub fn schemas() -> Value {
    serde_json::json!({
        "policy":schemars::schema_for!(Policy),
        "query":schemars::schema_for!(Query),
        "request":schemars::schema_for!(Request),
        "protocol-open":schemars::schema_for!(ProtocolOpen),
        "wire-chunk":schemars::schema_for!(WireChunk),
        "wire-output":schemars::schema_for!(WireOutput),
        "config-candidate":schemars::schema_for!(ConfigCandidate),
        "connection":schemars::schema_for!(Connection)
    })
}
