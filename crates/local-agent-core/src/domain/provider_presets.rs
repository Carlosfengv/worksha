//! Static initial values, never connection, catalog, credential or write authority.
use crate::providers::ConfigError;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const MAX_PRESETS: usize = 9 + 64 * 16;
pub const MAX_RESPONSE_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum PresetOrigin {
    #[serde(rename_all = "camelCase")]
    Builtin { preset_id: String },
    #[serde(rename_all = "camelCase")]
    External {
        plugin_id: String,
        digest: String,
        preset_id: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PresetAdvice {
    pub origin: PresetOrigin,
    pub source_name: String,
    pub name: String,
    pub protocol: String,
    pub endpoints: BTreeMap<String, String>,
    pub model: String,
    pub documents: Option<String>,
}

impl PresetAdvice {
    pub fn validate(&self) -> Result<(), ConfigError> {
        let protocols = ["responses", "messages", "chat-completions"];
        let label = |s: &str| {
            !s.trim().is_empty() && s.chars().count() <= 100 && !s.chars().any(char::is_control)
        };
        if !label(&self.name)
            || !label(&self.source_name)
            || !protocols.contains(&self.protocol.as_str())
            || self.endpoints.len() > 3
            || self.model.chars().count() > 200
            || self.model.chars().any(char::is_control)
        {
            return Err(invalid());
        }
        for (protocol, endpoint) in &self.endpoints {
            if !protocols.contains(&protocol.as_str()) || endpoint.len() > 2048 {
                return Err(invalid());
            }
            crate::providers::validate_endpoint(endpoint)?;
        }
        if let Some(documents) = &self.documents {
            if documents.len() > 2048 {
                return Err(invalid());
            }
            crate::providers::validate_endpoint(documents)?;
        }
        match &self.origin {
            PresetOrigin::Builtin { preset_id } => {
                if !crate::providers::catalog::builtin().contains(preset_id) {
                    return Err(invalid());
                }
            }
            PresetOrigin::External {
                plugin_id,
                digest,
                preset_id,
            } => {
                if !local_id(plugin_id, 128)
                    || !local_id(preset_id, 64)
                    || digest.len() != 64
                    || !digest
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                    || self.endpoints.len() != 1
                    || !self.endpoints.contains_key(&self.protocol)
                    || self.documents.is_some()
                {
                    return Err(invalid());
                }
            }
        }
        Ok(())
    }
}

fn local_id(value: &str, max: usize) -> bool {
    !value.is_empty()
        && value.len() <= max
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
}
fn invalid() -> ConfigError {
    ConfigError::new("invalid_input", "供应商预设内容无效，草稿未被替换")
}

/// Both trusted built-in metadata and verified legacy packages produce this
/// advice. The host supplies origin identities; a guest cannot claim one.
pub trait PresetAdapter: Send + Sync {
    fn presets(&self) -> Result<Vec<PresetAdvice>, ConfigError>;
}
pub fn checked_presets(adapter: &dyn PresetAdapter) -> Result<Vec<PresetAdvice>, ConfigError> {
    let presets = adapter.presets()?;
    if presets.len() > MAX_PRESETS {
        return Err(ConfigError::new("capacity", "供应商预设超过本次读取容量"));
    }
    let mut identities = BTreeSet::new();
    for preset in &presets {
        preset.validate()?;
        if !identities.insert(preset.origin.clone()) {
            return Err(invalid());
        }
    }
    Ok(presets)
}

pub struct BuiltinPresets;
impl PresetAdapter for BuiltinPresets {
    fn presets(&self) -> Result<Vec<PresetAdvice>, ConfigError> {
        Ok(crate::providers::catalog::builtin()
            .presets()
            .iter()
            .map(|p| PresetAdvice {
                origin: PresetOrigin::Builtin {
                    preset_id: p.id.clone(),
                },
                source_name: "内置".into(),
                name: p.name.clone(),
                protocol: p.protocol.clone(),
                endpoints: p.endpoints.clone(),
                model: p.model.clone(),
                documents: p.documents.clone(),
            })
            .collect())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PresetIssue {
    pub plugin_id: Option<String>,
    pub digest: Option<String>,
    pub error_code: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum PresetAction {
    #[serde(rename_all = "camelCase")]
    List { store_id: String },
    #[serde(rename_all = "camelCase")]
    Resolve {
        store_id: String,
        origin: PresetOrigin,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum PresetReply {
    #[serde(rename_all = "camelCase")]
    Listed {
        store_id: String,
        items: Vec<PresetAdvice>,
        issues: Vec<PresetIssue>,
    },
    #[serde(rename_all = "camelCase")]
    Resolved {
        store_id: String,
        advice: Box<PresetAdvice>,
    },
}
pub fn schemas() -> serde_json::Value {
    serde_json::json!({
        "origin": schemars::schema_for!(PresetOrigin),
        "advice": schemars::schema_for!(PresetAdvice),
        "action": schemars::schema_for!(PresetAction),
        "reply": schemars::schema_for!(PresetReply),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    struct Fixture(Vec<PresetAdvice>);
    impl PresetAdapter for Fixture {
        fn presets(&self) -> Result<Vec<PresetAdvice>, ConfigError> {
            Ok(self.0.clone())
        }
    }
    fn external() -> PresetAdvice {
        PresetAdvice {
            origin: PresetOrigin::External {
                plugin_id: "org.example.presets".into(),
                digest: "a".repeat(64),
                preset_id: "openai".into(),
            },
            source_name: "External".into(),
            name: "OpenAI".into(),
            protocol: "responses".into(),
            endpoints: BTreeMap::from([("responses".into(), "https://example.invalid/v1".into())]),
            model: "initial-model".into(),
            documents: None,
        }
    }
    #[test]
    fn builtin_advice_retains_every_original_preset_and_external_identity_cannot_collide() {
        let builtin = checked_presets(&BuiltinPresets).unwrap();
        let original = crate::providers::catalog::builtin().presets();
        assert_eq!(builtin.len(), 9);
        for (advice, old) in builtin.iter().zip(original) {
            assert_eq!(
                advice.origin,
                PresetOrigin::Builtin {
                    preset_id: old.id.clone()
                }
            );
            assert_eq!(
                (
                    &advice.name,
                    &advice.protocol,
                    &advice.endpoints,
                    &advice.documents
                ),
                (&old.name, &old.protocol, &old.endpoints, &old.documents)
            );
            assert_eq!(advice.model, old.model);
        }
        let mut combined = builtin;
        combined.push(external());
        assert_eq!(checked_presets(&Fixture(combined)).unwrap().len(), 10);
    }
    #[test]
    fn invalid_and_duplicate_advice_is_refused_without_normalizing_or_running_an_adapter() {
        let good = external();
        assert!(checked_presets(&Fixture(vec![good.clone(), good.clone()])).is_err());
        for endpoint in [
            "https://user:secret@example.invalid",
            "https://example.invalid?key=secret",
            "file:///tmp/private",
            "https://example.invalid/#secret",
        ] {
            let mut bad = good.clone();
            bad.endpoints.insert("responses".into(), endpoint.into());
            assert!(checked_presets(&Fixture(vec![bad])).is_err());
        }
        let mut bad = good.clone();
        bad.endpoints
            .insert("messages".into(), "https://example.invalid".into());
        assert!(checked_presets(&Fixture(vec![bad])).is_err());
        let mut bad = good;
        bad.model = "line\nbreak".into();
        assert!(checked_presets(&Fixture(vec![bad])).is_err());
    }
    #[test]
    fn preset_actions_cannot_create_authority_supply_values_or_submit() {
        let origin = serde_json::to_value(external().origin).unwrap();
        let action = json!({"kind":"resolve", "storeId":"original", "origin":origin});
        assert!(serde_json::from_value::<PresetAction>(action.clone()).is_ok());
        for field in [
            "endpoint",
            "apiKey",
            "providerId",
            "force",
            "allowPrivateNetwork",
            "request",
        ] {
            let mut bad = action.clone();
            bad[field] = json!("forbidden");
            assert!(
                serde_json::from_value::<PresetAction>(bad).is_err(),
                "{field}"
            );
        }
        for kind in ["create", "save", "grant", "retry", "connect"] {
            assert!(
                serde_json::from_value::<PresetAction>(
                    json!({"kind":kind,"storeId":"original","origin":origin})
                )
                .is_err()
            );
        }
    }
    #[test]
    fn preset_schemas_are_separate_and_exact() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../contracts/domain/provider-presets");
        let value = schemas();
        assert_eq!(value.as_object().unwrap().len(), 4);
        for (name, schema) in value.as_object().unwrap() {
            let saved: serde_json::Value = serde_json::from_slice(
                &std::fs::read(root.join(format!("{name}.schema.json"))).unwrap(),
            )
            .unwrap();
            assert_eq!(schema, &saved, "{name}");
        }
    }
}
