//! Provider advice contains no origin, credentials, client, store or execution authority.
use crate::providers::ConfigError;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const API_VERSION: &str = "1.0.0";

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CatalogContext {
    pub protocol: String,
    pub vendor: String,
    pub base_path: String,
    /// Computed by the host from the exact official origin, never accepted from a guest.
    pub official_catalog_root: bool,
}
impl CatalogContext {
    pub fn prefix(&self) -> &str {
        let base = self.base_path.trim_end_matches('/');
        ["/chat/completions", "/responses", "/messages"]
            .iter()
            .find_map(|suffix| base.strip_suffix(suffix))
            .unwrap_or(base)
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CatalogRequest {
    pub path: String,
}
impl CatalogRequest {
    /// A relative route cannot redirect authentication or escape a custom endpoint prefix.
    pub fn validate(&self, context: &CatalogContext) -> Result<(), ConfigError> {
        let path = self.path.as_str();
        let suffix = if context.official_catalog_root && path == "/models" {
            "models"
        } else {
            path.strip_prefix(&format!("{}/", context.prefix()))
                .ok_or_else(|| invalid("模型目录路径超出已绑定服务的范围"))?
        };
        if path.len() > context.base_path.len().saturating_add(256)
            || !path.starts_with('/')
            || suffix.is_empty()
            || suffix.len() > 256
            || suffix
                .bytes()
                .any(|b| !b.is_ascii_alphanumeric() && !b"/_-.".contains(&b))
            || suffix
                .split('/')
                .any(|part| part.is_empty() || part == "." || part == "..")
        {
            return Err(invalid("模型目录路径无效，未发送请求"));
        }
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelCapabilities {
    pub tools: Option<bool>,
    pub vision: Option<bool>,
    pub reasoning: Option<bool>,
    pub structured_output: Option<bool>,
    pub json_output: Option<bool>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelMetadata {
    pub context_window: Option<u64>,
    pub max_input_tokens: Option<u64>,
    pub max_output_tokens: Option<u64>,
    pub capabilities: ModelCapabilities,
}
impl ModelMetadata {
    pub fn validate(&self) -> Result<(), ConfigError> {
        if [
            self.context_window,
            self.max_input_tokens,
            self.max_output_tokens,
        ]
        .into_iter()
        .flatten()
        .any(|n| n == 0 || n > 1_000_000_000)
        {
            return Err(invalid("模型目录包含无效容量，未替换已有模型"));
        }
        Ok(())
    }
    pub fn from_standard_row(row: &Value) -> Self {
        let positive = |v: &Value| v.as_u64().filter(|n| *n > 0 && *n <= 1_000_000_000);
        let capability = |key: &str| row["capabilities"][key]["supported"].as_bool();
        let parameters = row["supported_parameters"]
            .as_array()
            .filter(|items| items.len() <= 200 && items.iter().all(Value::is_string));
        let supports_parameter = |keys: &[&str]| {
            parameters.map(|items| {
                items
                    .iter()
                    .any(|item| keys.contains(&item.as_str().unwrap()))
            })
        };
        let string_list = |v: &Value| {
            v.as_array()
                .filter(|items| {
                    items.len() <= 32
                        && items.iter().all(|item| {
                            item.as_str()
                                .is_some_and(|s| !s.is_empty() && s.len() <= 64)
                        })
                })
                .cloned()
        };
        let modalities = string_list(&row["input_modalities"])
            .or_else(|| string_list(&row["architecture"]["input_modalities"]));
        let effort = string_list(&row["effort"]["supported_levels"])
            .filter(|items| !items.is_empty())
            .map(|_| true);
        Self {
            context_window: positive(&row["context_window"])
                .or_else(|| positive(&row["context_length"])),
            max_input_tokens: positive(&row["max_input_tokens"]),
            max_output_tokens: positive(&row["max_output_tokens"])
                .or_else(|| positive(&row["max_tokens"]))
                .or_else(|| positive(&row["top_provider"]["max_completion_tokens"])),
            capabilities: ModelCapabilities {
                tools: capability("tool_use").or_else(|| supports_parameter(&["tools"])),
                vision: capability("image_input").or_else(|| {
                    modalities
                        .as_ref()
                        .map(|items| items.iter().any(|item| item == "image"))
                }),
                reasoning: capability("thinking")
                    .or(effort)
                    .or_else(|| supports_parameter(&["reasoning", "reasoning_effort"])),
                structured_output: capability("structured_outputs")
                    .or_else(|| supports_parameter(&["structured_outputs"])),
                json_output: capability("json_output"),
            },
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CatalogModel {
    pub id: String,
    pub name: String,
    #[serde(flatten)]
    pub metadata: ModelMetadata,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CatalogContinuation {
    pub has_more: bool,
    pub after_id: Option<String>,
}

/// Execution adapters share advice. Only the host performs HTTP, validation and publication.
pub trait ProviderAdapter: Send + Sync {
    fn catalog_request(&self, context: &CatalogContext) -> Result<CatalogRequest, ConfigError>;
    fn parse_catalog(
        &self,
        body: &Value,
        emit: &mut dyn FnMut(CatalogModel) -> Result<(), ConfigError>,
    ) -> Result<CatalogContinuation, ConfigError>;
    /// Inference verification remains a separate, host-controlled short request.
    fn verification_protocol(&self) -> &'static str;
}

pub fn parse_standard_catalog(
    body: &Value,
    emit: &mut dyn FnMut(CatalogModel) -> Result<(), ConfigError>,
) -> Result<CatalogContinuation, ConfigError> {
    let rows = body["data"]
        .as_array()
        .ok_or_else(|| invalid("响应不是可识别的模型目录；可继续手动填写模型 ID"))?;
    for row in rows {
        let id = row["id"]
            .as_str()
            .ok_or_else(|| invalid("模型目录包含无效条目，未替换已有模型"))?;
        emit(CatalogModel {
            id: id.into(),
            name: row["display_name"]
                .as_str()
                .or(row["name"].as_str())
                .unwrap_or(id)
                .into(),
            metadata: ModelMetadata::from_standard_row(row),
        })?;
    }
    Ok(CatalogContinuation {
        has_more: body["has_more"] == true,
        after_id: body["last_id"].as_str().map(String::from),
    })
}
pub fn invalid(message: &str) -> ConfigError {
    ConfigError::new("models_unavailable", message)
}
pub fn schemas() -> Value {
    serde_json::json!({
        "catalog-context": schemars::schema_for!(CatalogContext),
        "catalog-request": schemars::schema_for!(CatalogRequest),
        "catalog-model": schemars::schema_for!(CatalogModel),
        "catalog-continuation": schemars::schema_for!(CatalogContinuation),
        "model-metadata": schemars::schema_for!(ModelMetadata),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn provider_schema_is_separate_from_the_existing_read_contracts() {
        assert_eq!(API_VERSION, "1.0.0");
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../contracts/domain/provider");
        let schemas = schemas();
        assert_eq!(schemas.as_object().unwrap().len(), 5);
        for (name, schema) in schemas.as_object().unwrap() {
            let saved: Value = serde_json::from_slice(
                &std::fs::read(root.join(format!("{name}.schema.json"))).unwrap(),
            )
            .unwrap();
            assert_eq!(&saved, schema, "{name}");
        }
    }

    #[test]
    fn catalog_advice_cannot_supply_authority_or_escape_the_host_prefix() {
        let context = CatalogContext {
            protocol: "responses".into(),
            vendor: "custom".into(),
            base_path: "/proxy/v1/responses".into(),
            official_catalog_root: false,
        };
        for path in ["/proxy/v1/models", "/proxy/v1/catalog"] {
            CatalogRequest { path: path.into() }
                .validate(&context)
                .unwrap();
        }
        for path in [
            "https://foreign.invalid/models",
            "//foreign.invalid/models",
            "/models",
            "/proxy/v1/../models",
            "/proxy/v1/%2e%2e/models",
            "/proxy/v1/models?key=x",
            "/proxy/v1/models#x",
            "/proxy/v1/a\\b",
        ] {
            assert!(
                CatalogRequest { path: path.into() }
                    .validate(&context)
                    .is_err(),
                "{path}"
            );
        }
        assert!(
            serde_json::from_value::<CatalogRequest>(
                json!({"path":"/proxy/v1/models","headers":{"Authorization":"secret"}})
            )
            .is_err()
        );
        assert!(serde_json::from_value::<CatalogContext>(json!({"protocol":"responses","vendor":"custom","basePath":"/v1","officialCatalogRoot":false,"credentialRef":"forbidden"})).is_err());
        let mut double_slash = context.clone();
        double_slash.base_path = "//proxy/v1".into();
        CatalogRequest {
            path: "//proxy/v1/models".into(),
        }
        .validate(&double_slash)
        .unwrap();
        let mut encoded = context.clone();
        encoded.base_path = format!("/{}/v1", "%E4%B8%AD".repeat(600));
        CatalogRequest {
            path: format!("{}/models", encoded.base_path),
        }
        .validate(&encoded)
        .unwrap();
    }

    #[test]
    fn common_models_keep_unknown_false_and_limits_without_arbitrary_metadata() {
        let mut rows = vec![];
        let continuation = parse_standard_catalog(&json!({"data":[{"id":"a","context_window":128000,"capabilities":{"tool_use":{"supported":false}}},{"id":"b","context_window":-1,"max_tokens":"invalid"}],"has_more":true,"last_id":"b"}), &mut |r| { rows.push(r); Ok(()) }).unwrap();
        assert!(continuation.has_more);
        assert_eq!(continuation.after_id.as_deref(), Some("b"));
        assert_eq!(rows[0].metadata.capabilities.tools, Some(false));
        assert_eq!(rows[1].metadata.context_window, None);
        assert_eq!(rows[1].metadata.max_output_tokens, None);
        let encoded = serde_json::to_value(&rows[0]).unwrap();
        let restored: CatalogModel = serde_json::from_value(encoded.clone()).unwrap();
        assert_eq!(restored.metadata, rows[0].metadata);
        let mut invalid_model = encoded;
        invalid_model["secret"] = json!("forbidden");
        assert!(serde_json::from_value::<CatalogModel>(invalid_model).is_err());
        rows[0].metadata.context_window = Some(1_000_000_001);
        assert!(rows[0].metadata.validate().is_err());
    }
}
