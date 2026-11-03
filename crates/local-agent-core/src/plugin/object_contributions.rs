//! Opt-in extension object declarations. The original contributions/common contract is unchanged.
use super::contributions::{DependencyStatus, DetailPlacement};
use crate::domain::{
    Decimal,
    object_summary::{self as summary, ObjectSelector, ObjectSummary, ObjectTarget, SummaryField},
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const FEATURE: &str = "host.object-detail-slots@1";
pub const QUERY_PERMISSION: &str = "object.summary@1";
pub const DECLARATION_FILE: &str = "object-contributions.json";
pub const MAX_SLOTS: usize = 16;
pub const OBJECT_REF_TTL_MS: u64 = 60_000;
pub const MAX_OBJECT_REFS: usize = 128;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum ObjectQueryKind {
    Summary,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ObjectSlot {
    pub id: String,
    pub target: ObjectTarget,
    pub placement: DetailPlacement,
    pub view_id: String,
    pub title: String,
    pub query_kind: ObjectQueryKind,
    pub fields: Vec<SummaryField>,
    /// Human-readable, separately reviewed purpose. No executable expression.
    pub purpose: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ObjectContributions {
    pub schema_version: u32,
    pub detail_slots: Vec<ObjectSlot>,
}
impl ObjectContributions {
    pub fn validate(&self) -> crate::domain::Result<()> {
        let invalid = || crate::domain::Failure::new(crate::domain::ErrorCode::InvalidInput);
        let mut ids = BTreeSet::new();
        if self.schema_version != 1
            || self.detail_slots.is_empty()
            || self.detail_slots.len() > MAX_SLOTS
        {
            return Err(invalid());
        }
        for slot in &self.detail_slots {
            if !local_id(&slot.id)
                || !local_id(&slot.view_id)
                || !ids.insert(&slot.id)
                || !summary::label(&slot.title, 100)
                || !summary::label(&slot.purpose, 600)
            {
                return Err(invalid());
            }
            summary::validate_fields(slot.target, &slot.fields)?;
        }
        Ok(())
    }
}
fn local_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"_-".contains(&b))
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ObjectSlotStatus {
    pub declaration: ObjectSlot,
    /// Hash of the entire fixed declaration, including purpose and fields.
    pub descriptor_identity: String,
    /// Host instance, installation scope and current summary-grant revision.
    pub context_identity: String,
    pub authorized: bool,
    pub error_code: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ObjectContributionStatus {
    pub plugin_id: String,
    pub digest: String,
    pub enabled: bool,
    pub active: bool,
    pub activation_epoch: Decimal,
    pub grant_revision: Decimal,
    pub dependencies: Vec<DependencyStatus>,
    pub slots: Vec<ObjectSlotStatus>,
    pub error_code: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ObjectContributionCatalog {
    pub schema_version: u32,
    pub items: Vec<ObjectContributionStatus>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ObjectSlotQuery {
    pub plugin_id: String,
    pub digest: String,
    pub slot_id: String,
}
impl ObjectSlotQuery {
    pub fn validate(&self) -> crate::domain::Result<()> {
        if self.plugin_id.len() < 5
            || self.plugin_id.len() > 100
            || !self.plugin_id.contains('.')
            || self.plugin_id.starts_with("builtin.")
            || self.plugin_id.starts_with("local-agent.")
            || !self.plugin_id.split('.').all(|part| {
                !part.is_empty()
                    && part
                        .bytes()
                        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            })
            || !hash(&self.digest)
            || !local_id(&self.slot_id)
        {
            return Err(crate::domain::Failure::new(
                crate::domain::ErrorCode::InvalidInput,
            ));
        }
        Ok(())
    }
}
fn hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ObjectGrantRequest {
    pub slot: ObjectSlotQuery,
    pub expected_descriptor_identity: String,
    pub expected_context_identity: String,
    pub expected_activation_epoch: Decimal,
    pub expected_grant_revision: Decimal,
}
impl ObjectGrantRequest {
    pub fn validate(&self) -> crate::domain::Result<()> {
        self.slot.validate()?;
        if !hash(&self.expected_descriptor_identity) || !hash(&self.expected_context_identity) {
            return Err(crate::domain::Failure::new(
                crate::domain::ErrorCode::InvalidInput,
            ));
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ObjectRefRequest {
    pub slot: ObjectSlotQuery,
    pub expected_context_identity: String,
    /// Host input only. Never forwarded to a guest or returned as summary data.
    pub object: ObjectSelector,
}
impl ObjectRefRequest {
    pub fn validate(&self) -> crate::domain::Result<()> {
        self.slot.validate()?;
        if !hash(&self.expected_context_identity) {
            return Err(crate::domain::Failure::new(
                crate::domain::ErrorCode::InvalidInput,
            ));
        }
        self.object.validate()
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ObjectRefQuery {
    pub slot: ObjectSlotQuery,
    pub object_ref: String,
}
impl ObjectRefQuery {
    pub fn validate(&self) -> crate::domain::Result<()> {
        self.slot.validate()?;
        if uuid::Uuid::parse_str(&self.object_ref).is_err() {
            return Err(crate::domain::Failure::new(
                crate::domain::ErrorCode::InvalidInput,
            ));
        }
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ObjectReference {
    pub slot: ObjectSlotQuery,
    pub context_identity: String,
    pub object_ref: String,
    pub target: ObjectTarget,
    pub expires_after_ms: Decimal,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum ObjectContributionAction {
    Catalog {},
    Grant { request: ObjectGrantRequest },
    Revoke { request: ObjectGrantRequest },
    Issue { request: ObjectRefRequest },
    Read { query: ObjectRefQuery },
    Release { query: ObjectRefQuery },
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum ObjectContributionReply {
    Catalog {
        catalog: ObjectContributionCatalog,
    },
    Status {
        status: ObjectContributionStatus,
    },
    Reference {
        reference: ObjectReference,
    },
    #[serde(rename_all = "camelCase")]
    Summary {
        object_ref: String,
        summary: ObjectSummary,
    },
    #[serde(rename_all = "camelCase")]
    Released {
        object_ref: String,
    },
}
pub fn schemas() -> serde_json::Value {
    serde_json::json!({
        "object-contributions":schemars::schema_for!(ObjectContributions),
        "object-contribution-catalog":schemars::schema_for!(ObjectContributionCatalog),
        "object-contribution-action":schemars::schema_for!(ObjectContributionAction),
        "object-contribution-reply":schemars::schema_for!(ObjectContributionReply),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn declaration() -> ObjectContributions {
        serde_json::from_value(serde_json::json!({"schemaVersion":1,"detailSlots":[{"id":"usage","target":"session","placement":"card","viewId":"summary","title":"已记录用量","queryKind":"summary","fields":["input-tokens","total-tokens"],"purpose":"在当前会话中显示已有用量摘要"}]})).unwrap()
    }
    #[test]
    fn new_declaration_is_explicit_bounded_and_cannot_expand_into_content_queries() {
        let mut c = declaration();
        c.validate().unwrap();
        c.detail_slots[0].fields.push(SummaryField::ModelCount);
        assert!(c.validate().is_err());
        let mut c = declaration();
        c.detail_slots[0].purpose = " \n".into();
        assert!(c.validate().is_err());
        let mut c = declaration();
        c.detail_slots.push(c.detail_slots[0].clone());
        assert!(c.validate().is_err());
        let mut raw = serde_json::to_value(declaration()).unwrap();
        raw["detailSlots"][0]["queryKind"] = "conversation".into();
        assert!(serde_json::from_value::<ObjectContributions>(raw).is_err());
        let mut raw = serde_json::to_value(declaration()).unwrap();
        raw["detailSlots"][0]["path"] = "/private/data".into();
        assert!(serde_json::from_value::<ObjectContributions>(raw).is_err());
    }
    #[test]
    fn references_do_not_accept_replacement_objects_fields_or_queries() {
        assert!(serde_json::from_value::<ObjectContributionAction>(serde_json::json!({"kind":"issue","request":{"slot":{"pluginId":"org.example.details","digest":"a".repeat(64),"slotId":"usage"},"object":{"target":"provider","id":"p"}}})).is_err());
        let raw = serde_json::json!({"kind":"read","query":{"slot":{"pluginId":"org.example.details","digest":"a".repeat(64),"slotId":"usage"},"objectRef":"opaque","object":{"target":"session","id":"other"}}});
        assert!(serde_json::from_value::<ObjectContributionAction>(raw).is_err());
        assert!(
            serde_json::from_value::<ObjectContributionAction>(
                serde_json::json!({"kind":"catalog","refresh":true})
            )
            .is_err()
        );
        assert!(serde_json::from_value::<ObjectContributionAction>(serde_json::json!({"kind":"read","query":{"slot":{"pluginId":"org.example.details","digest":"a".repeat(64),"slotId":"usage"},"objectRef":"opaque","fields":["title"]}})).is_err());
        let mut slot = ObjectSlotQuery {
            plugin_id: "org.example.details".into(),
            digest: "a".repeat(64),
            slot_id: "usage".into(),
        };
        slot.validate().unwrap();
        slot.digest = "../archive".into();
        assert!(slot.validate().is_err());
    }
    #[test]
    fn prior_basic_targets_remain_closed_while_extension_accepts_new_targets() {
        let old = serde_json::json!({"schemaVersion":1,"detailSlots":[{"id":"summary","target":"project","objectIds":["*"],"placement":"card","viewId":"summary","title":"项目"}]});
        assert!(
            serde_json::from_value::<crate::plugin::contributions::Contributions>(old).is_err()
        );
        let mut c = declaration();
        c.detail_slots[0].target = ObjectTarget::Project;
        c.detail_slots[0].fields = vec![SummaryField::SessionCount];
        c.validate().unwrap();
    }
}
