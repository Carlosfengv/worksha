//! Non-content metadata shared by built-in readers and object contributions.
//! Selectors stay in the host; summaries never carry paths, titles or credentials.
use super::{Coverage, Decimal, ErrorCode, Failure, Freshness, QueryScope, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const API_VERSION: &str = "1.0.0";
pub const MAX_FIELDS: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum ObjectTarget {
    Agent,
    Provider,
    Account,
    Project,
    Session,
    Request,
}

/// A unified object must come from an existing authorized, frozen query.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum ObjectOrigin {
    Builtin {},
    #[serde(rename_all = "camelCase")]
    Unified {
        scope: QueryScope,
        source_id: String,
        snapshot_token: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "target", rename_all = "kebab-case", deny_unknown_fields)]
pub enum ObjectSelector {
    Agent {
        id: String,
    },
    Provider {
        id: String,
    },
    Account {
        id: String,
    },
    Project {
        id: String,
        origin: ObjectOrigin,
    },
    Session {
        id: String,
        origin: ObjectOrigin,
    },
    Request {
        id: String,
        origin: ObjectOrigin,
        unit: super::requests::RequestUnit,
    },
}
impl ObjectSelector {
    pub fn target(&self) -> ObjectTarget {
        match self {
            Self::Agent { .. } => ObjectTarget::Agent,
            Self::Provider { .. } => ObjectTarget::Provider,
            Self::Account { .. } => ObjectTarget::Account,
            Self::Project { .. } => ObjectTarget::Project,
            Self::Session { .. } => ObjectTarget::Session,
            Self::Request { .. } => ObjectTarget::Request,
        }
    }
    pub fn id(&self) -> &str {
        match self {
            Self::Agent { id }
            | Self::Provider { id }
            | Self::Account { id }
            | Self::Project { id, .. }
            | Self::Session { id, .. }
            | Self::Request { id, .. } => id,
        }
    }
    pub fn validate(&self) -> Result<()> {
        if !label(self.id(), 512) {
            return Err(invalid());
        }
        let origin = match self {
            Self::Project { origin, .. }
            | Self::Session { origin, .. }
            | Self::Request { origin, .. } => Some(origin),
            _ => None,
        };
        if let Some(ObjectOrigin::Unified {
            scope,
            source_id,
            snapshot_token,
        }) = origin
        {
            scope.validate()?;
            if !label(source_id, 240)
                || !scope.source_ids.contains(source_id)
                || !label(snapshot_token, 512)
            {
                return Err(invalid());
            }
        }
        Ok(())
    }
}

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
pub enum SummaryField {
    SessionCount,
    ModelCount,
    Configuration,
    QuotaWindowCount,
    InputTokens,
    OutputTokens,
    TotalTokens,
    AttemptCount,
    DurationMs,
    RequestOutcome,
}
impl SummaryField {
    pub fn allowed(self, target: ObjectTarget) -> bool {
        use ObjectTarget as T;
        match self {
            Self::SessionCount => matches!(target, T::Agent | T::Project),
            Self::ModelCount | Self::Configuration => target == T::Provider,
            Self::QuotaWindowCount => target == T::Account,
            Self::InputTokens | Self::OutputTokens | Self::TotalTokens => {
                matches!(target, T::Session | T::Request)
            }
            Self::AttemptCount | Self::DurationMs | Self::RequestOutcome => target == T::Request,
        }
    }
}
pub fn validate_fields(target: ObjectTarget, fields: &[SummaryField]) -> Result<()> {
    let unique: BTreeSet<_> = fields.iter().collect();
    if fields.is_empty()
        || fields.len() > MAX_FIELDS
        || unique.len() != fields.len()
        || fields.iter().any(|f| !f.allowed(target))
    {
        return Err(invalid());
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum SummaryOutcome {
    Succeeded,
    Failed,
    Cancelled,
    InProgress,
    Unknown,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum SummaryValue {
    Count {
        value: Decimal,
    },
    Tokens {
        value: Decimal,
    },
    Configuration {
        configured: bool,
    },
    /// Preserve recorded fractional milliseconds; counts and tokens use Decimal.
    Milliseconds {
        value: f64,
    },
    Outcome {
        value: SummaryOutcome,
    },
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum MissingReason {
    Unread,
    NotRecorded,
    Unavailable,
    Partial,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SummaryMetric {
    pub field: SummaryField,
    pub value: Option<SummaryValue>,
    pub missing_reason: Option<MissingReason>,
}
impl SummaryMetric {
    fn validate(&self) -> Result<()> {
        if self.value.is_some() == self.missing_reason.is_some() {
            return Err(invalid());
        }
        let valid = match (&self.field, &self.value) {
            (_, None) => true,
            (
                SummaryField::SessionCount
                | SummaryField::ModelCount
                | SummaryField::QuotaWindowCount
                | SummaryField::AttemptCount,
                Some(SummaryValue::Count { .. }),
            )
            | (
                SummaryField::InputTokens | SummaryField::OutputTokens | SummaryField::TotalTokens,
                Some(SummaryValue::Tokens { .. }),
            )
            | (SummaryField::Configuration, Some(SummaryValue::Configuration { .. }))
            | (SummaryField::RequestOutcome, Some(SummaryValue::Outcome { .. })) => true,
            (SummaryField::DurationMs, Some(SummaryValue::Milliseconds { value })) => {
                value.is_finite() && *value >= 0.0
            }
            _ => false,
        };
        if !valid {
            return Err(invalid());
        }
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ObjectSummary {
    pub api_version: String,
    pub target: ObjectTarget,
    pub coverage: Coverage,
    pub freshness: Freshness,
    pub captured_at: String,
    pub metrics: Vec<SummaryMetric>,
}
impl ObjectSummary {
    pub fn validate(&self) -> Result<()> {
        if self.api_version != API_VERSION || super::timestamp(&self.captured_at).is_none() {
            return Err(invalid());
        }
        validate_fields(
            self.target,
            &self.metrics.iter().map(|m| m.field).collect::<Vec<_>>(),
        )?;
        for metric in &self.metrics {
            metric.validate()?;
        }
        if (self.coverage == Coverage::Unavailable || self.freshness == Freshness::Unread)
            && self.metrics.iter().any(|m| m.value.is_some())
        {
            return Err(invalid());
        }
        Ok(())
    }
    /// Never supply unrequested fields or silently manufacture an absent metric.
    pub fn project(&self, fields: &[SummaryField]) -> Result<Self> {
        self.validate()?;
        validate_fields(self.target, fields)?;
        let metrics = fields
            .iter()
            .map(|field| {
                self.metrics
                    .iter()
                    .find(|metric| metric.field == *field)
                    .cloned()
                    .ok_or_else(|| Failure::new(ErrorCode::Unsupported))
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Self {
            metrics,
            ..self.clone()
        })
    }
}

/// Host-only read identity. This type is deliberately absent from wire schemas.
#[derive(Clone, Debug, PartialEq)]
pub struct ObjectSummaryRead {
    pub selector: ObjectSelector,
    pub read_identity: String,
    pub summary: ObjectSummary,
}
impl ObjectSummaryRead {
    pub fn validate(&self, selector: &ObjectSelector, fields: &[SummaryField]) -> Result<()> {
        selector.validate()?;
        validate_fields(selector.target(), fields)?;
        self.summary.validate()?;
        if self.selector != *selector
            || self.summary.target != selector.target()
            || !label(&self.read_identity, 512)
        {
            return Err(Failure::new(ErrorCode::StaleSnapshot));
        }
        if self
            .summary
            .metrics
            .iter()
            .map(|m| m.field)
            .collect::<Vec<_>>()
            != fields
        {
            return Err(invalid());
        }
        Ok(())
    }
}
/// The implementation resolves existing authorized metadata. It never scans or writes.
pub trait ObjectSummarySource {
    fn read_object_summary(
        &self,
        selector: &ObjectSelector,
        fields: &[SummaryField],
    ) -> Result<ObjectSummaryRead>;
    /// Recheck the source revision, visibility and authorization before delivery.
    fn ensure_object_summary_current(&self, read: &ObjectSummaryRead) -> Result<()>;
}
pub(crate) fn label(value: &str, max_bytes: usize) -> bool {
    !value.trim().is_empty() && value.len() <= max_bytes && !value.chars().any(char::is_control)
}
fn invalid() -> Failure {
    Failure::new(ErrorCode::InvalidInput)
}
pub fn schemas() -> serde_json::Value {
    serde_json::json!({
        "object-selector": schemars::schema_for!(ObjectSelector),
        "object-summary": schemars::schema_for!(ObjectSummary),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn summary() -> ObjectSummary {
        ObjectSummary {
            api_version: API_VERSION.into(),
            target: ObjectTarget::Session,
            coverage: Coverage::Partial,
            freshness: Freshness::Stale,
            captured_at: "2026-11-01T00:00:00Z".into(),
            metrics: vec![
                SummaryMetric {
                    field: SummaryField::InputTokens,
                    value: Some(SummaryValue::Tokens {
                        value: 0_u64.into(),
                    }),
                    missing_reason: None,
                },
                SummaryMetric {
                    field: SummaryField::TotalTokens,
                    value: None,
                    missing_reason: Some(MissingReason::NotRecorded),
                },
            ],
        }
    }
    #[test]
    fn fields_are_target_specific_and_projection_keeps_real_zero_and_unknown_distinct() {
        let s = summary();
        s.validate().unwrap();
        let p = s.project(&[SummaryField::TotalTokens]).unwrap();
        assert_eq!(p.metrics.len(), 1);
        assert_eq!(p.metrics[0].value, None);
        assert_eq!(
            p.metrics[0].missing_reason,
            Some(MissingReason::NotRecorded)
        );
        assert_eq!(p.coverage, Coverage::Partial);
        assert!(s.project(&[SummaryField::ModelCount]).is_err());
        assert!(s.project(&[SummaryField::OutputTokens]).is_err());
        assert!(
            s.project(&[SummaryField::InputTokens, SummaryField::InputTokens])
                .is_err()
        );
        let raw = serde_json::to_value(s).unwrap();
        assert_eq!(raw["metrics"][0]["value"]["value"], "0");
    }
    #[test]
    fn wrong_units_missing_explanations_and_invalid_durations_are_rejected() {
        let mut s = summary();
        s.metrics[0].value = Some(SummaryValue::Count {
            value: 1_u64.into(),
        });
        assert!(s.validate().is_err());
        s.metrics[0].value = None;
        s.metrics[0].missing_reason = None;
        assert!(s.validate().is_err());
        for value in [-1.0, f64::INFINITY, f64::NAN] {
            let m = SummaryMetric {
                field: SummaryField::DurationMs,
                value: Some(SummaryValue::Milliseconds { value }),
                missing_reason: None,
            };
            assert!(m.validate().is_err());
        }
        SummaryMetric {
            field: SummaryField::DurationMs,
            value: Some(SummaryValue::Milliseconds { value: 1.25 }),
            missing_reason: None,
        }
        .validate()
        .unwrap();
    }
    #[test]
    fn selectors_are_strict_and_unified_identity_requires_a_frozen_matching_scope() {
        assert!(serde_json::from_value::<ObjectSelector>(serde_json::json!({"target":"session","id":"s","origin":{"kind":"builtin","refresh":true}})).is_err());
        let raw = serde_json::json!({"target":"request","id":"r1","origin":{"kind":"unified","scope":{"kind":"external","sourceIds":["org.example.history:one"]},"sourceId":"org.example.history:other","snapshotToken":"opaque"},"unit":"model-response"});
        let s: ObjectSelector = serde_json::from_value(raw).unwrap();
        assert!(s.validate().is_err());
        assert!(
            serde_json::from_value::<ObjectSelector>(
                serde_json::json!({"target":"provider","id":"p","endpoint":"https://example.com"})
            )
            .is_err()
        );
        assert!(
            serde_json::from_value::<SummaryValue>(
                serde_json::json!({"kind":"tokens","value":"18446744073709551616"})
            )
            .is_err()
        );
        assert!(
            serde_json::from_value::<SummaryValue>(
                serde_json::json!({"kind":"tokens","value":9007199254740993_u64})
            )
            .is_err()
        );
    }
    #[test]
    fn read_identity_cannot_be_reused_for_another_object_or_expose_unrequested_fields() {
        let selector = ObjectSelector::Session {
            id: "session-a".into(),
            origin: ObjectOrigin::Builtin {},
        };
        let read = ObjectSummaryRead {
            selector: selector.clone(),
            read_identity: "host-revision".into(),
            summary: summary(),
        };
        read.validate(
            &selector,
            &[SummaryField::InputTokens, SummaryField::TotalTokens],
        )
        .unwrap();
        assert!(
            read.validate(&selector, &[SummaryField::InputTokens])
                .is_err()
        );
        let other = ObjectSelector::Session {
            id: "session-b".into(),
            origin: ObjectOrigin::Builtin {},
        };
        assert_eq!(
            read.validate(
                &other,
                &[SummaryField::InputTokens, SummaryField::TotalTokens]
            )
            .unwrap_err()
            .code,
            ErrorCode::StaleSnapshot
        );
        let mut unavailable = summary();
        unavailable.coverage = Coverage::Unavailable;
        assert!(unavailable.validate().is_err());
    }
}
