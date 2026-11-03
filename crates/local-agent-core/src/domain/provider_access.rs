//! Opt-in external catalog advice. Network and credential authority never enter a guest.
use super::provider::{CatalogContext, CatalogContinuation, CatalogModel, CatalogRequest};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const CAPABILITY: &str = "provider.models@1";
pub const BROKER: &str = "host.provider-catalog@1";
pub const MAX_PAGE_MODELS: usize = 256;
pub const MAX_BODY: usize = 256 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Policy {
    pub schema_version: u32,
    pub base_protocol: String,
    pub path: String,
}
impl Policy {
    pub fn valid(&self) -> bool {
        self.schema_version == 1
            && ["messages", "responses", "chat-completions"].contains(&self.base_protocol.as_str())
            && self.path.len() <= 128
            && self.path.starts_with('/')
            && self.path.split('/').skip(1).all(|part| {
                !part.is_empty()
                    && part
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
            })
    }
    pub fn accepts(&self, context: &CatalogContext, advice: &CatalogRequest) -> bool {
        self.valid()
            && !context.official_catalog_root
            && context.protocol == self.base_protocol
            && advice.path == format!("{}{}", context.prefix(), self.path)
            && advice.validate(context).is_ok()
    }
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ParseRequest {
    pub snapshot: String,
    pub body: Value,
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Page {
    pub snapshot: String,
    pub models: Vec<CatalogModel>,
    pub continuation: CatalogContinuation,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Query {
    pub plugin_id: String,
    pub digest: String,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Target {
    #[serde(rename_all = "camelCase")]
    Saved {
        provider_id: String,
        expected_revision: String,
    },
    #[serde(rename_all = "camelCase")]
    Draft {
        label: String,
        endpoint: String,
        protocol: String,
        vendor: String,
        credential_ref: Option<String>,
    },
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Action {
    #[serde(rename_all = "camelCase")]
    Bind {
        target: Target,
        allow_private_network: bool,
    },
    #[serde(rename_all = "camelCase")]
    Unbind { binding_id: String },
    #[serde(rename_all = "camelCase")]
    Read { binding_id: String, target: Target },
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Request {
    pub plugin_id: String,
    pub digest: String,
    pub action: Action,
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Binding {
    pub binding_id: String,
    pub digest: String,
    pub label: String,
    pub provider_id: Option<String>,
    pub expected_revision: Option<String>,
    pub endpoint: String,
    pub protocol: String,
    pub allow_private_network: bool,
    pub authorized: bool,
    pub error_code: Option<String>,
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct State {
    pub policy: Option<Policy>,
    pub can_bind: bool,
    pub bindings: Vec<Binding>,
    pub error_code: Option<String>,
}
pub fn schemas() -> Value {
    serde_json::json!({"policy":schemars::schema_for!(Policy),"parse-request":schemars::schema_for!(ParseRequest),"page":schemars::schema_for!(Page),"query":schemars::schema_for!(Query),"target":schemars::schema_for!(Target),"request":schemars::schema_for!(Request),"binding":schemars::schema_for!(Binding),"state":schemars::schema_for!(State)})
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn provider_access_schemas_are_separate_and_match_rust_serialization() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../contracts/domain/provider-access");
        let schemas = schemas();
        assert_eq!(schemas.as_object().unwrap().len(), 8);
        for (name, schema) in schemas.as_object().unwrap() {
            let stored: Value = serde_json::from_slice(
                &std::fs::read(root.join(format!("{name}.schema.json"))).unwrap(),
            )
            .unwrap();
            assert_eq!(&stored, schema, "{name}");
        }
        assert!(
            serde_json::from_value::<Target>(serde_json::json!({
                "kind":"saved", "providerId":"owned", "expectedRevision":1
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<Request>(serde_json::json!({
                "pluginId":"owned", "digest":"digest", "action":{
                    "kind":"read", "bindingId":"owned", "target":{
                        "kind":"saved", "providerId":"owned", "expectedRevision":"1"
                    }, "headers":{"Authorization":"untrusted"}
                }
            }))
            .is_err()
        );
    }
    #[test]
    fn external_catalog_path_is_exactly_the_authorized_suffix_without_authority() {
        let p = Policy {
            schema_version: 1,
            base_protocol: "responses".into(),
            path: "/catalog".into(),
        };
        let c = CatalogContext {
            protocol: "responses".into(),
            vendor: "custom".into(),
            base_path: "/prefix/v1/responses".into(),
            official_catalog_root: false,
        };
        assert!(p.accepts(
            &c,
            &CatalogRequest {
                path: "/prefix/v1/catalog".into()
            }
        ));
        for path in [
            "/catalog",
            "/prefix/v1/models",
            "//foreign/catalog",
            "/prefix/v1/../catalog",
            "/prefix/v1/catalog?key=x",
        ] {
            assert!(!p.accepts(&c, &CatalogRequest { path: path.into() }));
        }
        for path in [
            "/",
            "//catalog",
            "/../catalog",
            "/ca%74alog",
            "/catalog?after=x",
        ] {
            assert!(
                !Policy {
                    path: path.into(),
                    ..p.clone()
                }
                .valid()
            );
        }
        assert!(serde_json::from_value::<Page>(serde_json::json!({"snapshot":"x","models":[],"continuation":{"hasMore":false,"afterId":null},"headers":{}})).is_err());
    }
}
