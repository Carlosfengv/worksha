//! Original Agent matching evidence, without persisting arbitrary input names.
use super::key;
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct AgentIdentity {
    pub family: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub product: Option<String>,
}

impl AgentIdentity {
    pub fn new(namespace: &str, family: &str, product: Option<&str>) -> Self {
        Self {
            family: key(namespace, "agent-family", family),
            product: product.map(|id| key(namespace, "agent-product", id)),
        }
    }

    pub fn valid(&self) -> bool {
        let valid = |s: &str| {
            s.len() == 64
                && s.bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        };
        valid(&self.family) && self.product.as_deref().is_none_or(valid)
    }
}
