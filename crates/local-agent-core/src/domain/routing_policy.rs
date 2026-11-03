//! Pure, bounded ordering advice. Credentials, routes, budgets and sends stay in the host.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

pub const MAX_CANDIDATES: usize = 32;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CandidateCapabilities {
    pub context_window: u32,
    pub max_output_tokens: u32,
    pub tools: bool,
    pub thinking: bool,
    pub input_modalities: Vec<String>,
}
impl From<&crate::model_services::Capabilities> for CandidateCapabilities {
    fn from(value: &crate::model_services::Capabilities) -> Self {
        Self {
            context_window: value.context_window,
            max_output_tokens: value.max_output_tokens,
            tools: value.tools,
            thinking: value.thinking,
            input_modalities: value.input_modalities.clone(),
        }
    }
}
impl CandidateCapabilities {
    fn validate(&self) -> Result<(), RankError> {
        crate::model_services::Capabilities {
            context_window: self.context_window,
            max_output_tokens: self.max_output_tokens,
            tools: self.tools,
            thinking: self.thinking,
            input_modalities: self.input_modalities.clone(),
        }
        .validate()
        .map_err(|_| RankError::InvalidInput)
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum CandidateHealth {
    Unknown,
    Available,
    Cooling,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Candidate {
    /// Published service model identity, never an endpoint, Provider or wire model name.
    pub model_id: String,
    pub capabilities: CandidateCapabilities,
    pub health: CandidateHealth,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RankRequest {
    pub service_id: String,
    pub service_version_id: String,
    /// Only eligible identities are included. Disabled or incompatible entries stay host-private.
    pub preferred_model_id: Option<String>,
    pub default_model_id: Option<String>,
    pub configured_order: Vec<String>,
    /// In the published service order. Capabilities are already allowed by the host.
    pub candidates: Vec<Candidate>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum RankReason {
    BuiltinPriority,
    ExtensionOrder,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RankResult {
    pub request_identity: String,
    pub order: Vec<String>,
    pub reason: RankReason,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RankError {
    InvalidInput,
    StaleSnapshot,
    PermissionDenied,
    ResourceLimit,
}
impl RankError {
    pub fn code(self) -> &'static str {
        match self {
            Self::InvalidInput => "invalid_input",
            Self::StaleSnapshot => "stale_snapshot",
            Self::PermissionDenied => "permission_denied",
            Self::ResourceLimit => "resource_limit",
        }
    }
}
impl RankRequest {
    pub fn validate(&self) -> Result<(), RankError> {
        let valid_id = |id: &str| uuid::Uuid::parse_str(id).is_ok();
        if !valid_id(&self.service_id) || !valid_id(&self.service_version_id) {
            return Err(RankError::InvalidInput);
        }
        if self.candidates.len() > MAX_CANDIDATES || self.configured_order.len() > MAX_CANDIDATES {
            return Err(RankError::ResourceLimit);
        }
        let mut ids = HashSet::new();
        for candidate in &self.candidates {
            if !valid_id(&candidate.model_id) || !ids.insert(candidate.model_id.as_str()) {
                return Err(RankError::InvalidInput);
            }
            candidate.capabilities.validate()?;
        }
        if self
            .preferred_model_id
            .iter()
            .chain(self.default_model_id.iter())
            .any(|id| !ids.contains(id.as_str()))
        {
            return Err(RankError::InvalidInput);
        }
        let order: HashSet<_> = self.configured_order.iter().map(String::as_str).collect();
        if !self.configured_order.is_empty()
            && (order != ids || order.len() != self.configured_order.len())
        {
            return Err(RankError::InvalidInput);
        }
        Ok(())
    }
    pub fn identity(&self) -> String {
        crate::providers::files::hash(
            &serde_json::to_vec(self).expect("serializable ranking metadata"),
        )
    }
}
pub trait RoutingPolicy: Send + Sync {
    fn rank(&self, request: &RankRequest) -> Result<RankResult, RankError>;
    fn inspect_result(
        &self,
        request: &RankRequest,
        result: RankResult,
    ) -> Result<RankResult, RankError> {
        inspect_result(request, result)
    }
}
pub fn inspect_result(request: &RankRequest, result: RankResult) -> Result<RankResult, RankError> {
    request.validate()?;
    if result.request_identity != request.identity() {
        return Err(RankError::StaleSnapshot);
    }
    if result.order.len() > MAX_CANDIDATES {
        return Err(RankError::ResourceLimit);
    }
    let allowed: HashSet<_> = request
        .candidates
        .iter()
        .map(|c| c.model_id.as_str())
        .collect();
    let mut seen = HashSet::new();
    if result
        .order
        .iter()
        .any(|id| !allowed.contains(id.as_str()) || !seen.insert(id))
    {
        return Err(RankError::PermissionDenied);
    }
    Ok(result)
}
/// The original preferred/default/configured priority, with no network or health reads.
pub struct BuiltinPriority;
impl RoutingPolicy for BuiltinPriority {
    fn rank(&self, request: &RankRequest) -> Result<RankResult, RankError> {
        request.validate()?;
        let mut order: Vec<_> = request.preferred_model_id.iter().cloned().collect();
        if request.configured_order.is_empty() {
            order.extend(request.default_model_id.iter().cloned());
            order.extend(request.candidates.iter().map(|c| c.model_id.clone()));
        } else {
            order.extend(request.configured_order.iter().cloned());
        }
        let mut seen = HashSet::new();
        order.retain(|id| seen.insert(id.clone()));
        Ok(RankResult {
            request_identity: request.identity(),
            order,
            reason: RankReason::BuiltinPriority,
        })
    }
}
pub fn schemas() -> serde_json::Value {
    serde_json::json!({
        "rank-request": schemars::schema_for!(RankRequest),
        "rank-result": schemars::schema_for!(RankResult),
        "candidate": schemars::schema_for!(Candidate),
        "candidate-capabilities": schemars::schema_for!(CandidateCapabilities),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request() -> RankRequest {
        let ids: Vec<_> = (1..=3)
            .map(|i| format!("00000000-0000-4000-8000-{i:012}"))
            .collect();
        RankRequest {
            service_id: "10000000-0000-4000-8000-000000000001".into(),
            service_version_id: "20000000-0000-4000-8000-000000000001".into(),
            preferred_model_id: Some(ids[2].clone()),
            default_model_id: Some(ids[1].clone()),
            configured_order: vec![],
            candidates: ids
                .into_iter()
                .map(|model_id| Candidate {
                    model_id,
                    capabilities: (&crate::model_services::Capabilities::default()).into(),
                    health: CandidateHealth::Unknown,
                })
                .collect(),
        }
    }
    #[test]
    fn native_priority_matches_preference_default_and_configured_orders_without_extra_candidates() {
        let mut r = request();
        let ids: Vec<_> = r.candidates.iter().map(|c| c.model_id.clone()).collect();
        assert_eq!(
            BuiltinPriority.rank(&r).unwrap().order,
            vec![ids[2].clone(), ids[1].clone(), ids[0].clone()]
        );
        for permutation in [
            [0, 1, 2],
            [0, 2, 1],
            [1, 0, 2],
            [1, 2, 0],
            [2, 0, 1],
            [2, 1, 0],
        ] {
            r.configured_order = permutation.map(|i| ids[i].clone()).to_vec();
            for preferred in [
                None,
                Some(ids[0].clone()),
                Some(ids[1].clone()),
                Some(ids[2].clone()),
            ] {
                r.preferred_model_id = preferred.clone();
                let mut expected: Vec<_> = preferred.into_iter().collect();
                for id in &r.configured_order {
                    if !expected.contains(id) {
                        expected.push(id.clone());
                    }
                }
                let result = BuiltinPriority.rank(&r).unwrap();
                assert_eq!(result.order, expected);
                assert!(BuiltinPriority.inspect_result(&r, result).is_ok());
            }
        }
    }
    #[test]
    fn replies_cannot_expand_authority_or_cross_versions_and_can_only_return_a_subset() {
        let r = request();
        let good = BuiltinPriority.rank(&r).unwrap();
        let mut changed = r.clone();
        changed.service_version_id = "20000000-0000-4000-8000-000000000002".into();
        assert_eq!(
            inspect_result(&changed, good.clone()),
            Err(RankError::StaleSnapshot)
        );
        for order in [
            vec![r.candidates[0].model_id.clone(); 2],
            vec!["00000000-0000-4000-8000-000000000009".into()],
        ] {
            assert_eq!(
                inspect_result(
                    &r,
                    RankResult {
                        order,
                        ..good.clone()
                    }
                ),
                Err(RankError::PermissionDenied)
            );
        }
        assert!(
            inspect_result(
                &r,
                RankResult {
                    order: vec![r.candidates[1].model_id.clone()],
                    ..good.clone()
                }
            )
            .is_ok()
        );
        let mut changed = r.clone();
        changed.candidates[0].health = CandidateHealth::Cooling;
        assert_eq!(
            inspect_result(&changed, good),
            Err(RankError::StaleSnapshot)
        );
    }
    #[test]
    fn unknown_authority_fields_invalid_metadata_and_excessive_requests_are_refused() {
        let r = request();
        let mut json = serde_json::to_value(&r).unwrap();
        json["credential"] = "not-a-real-key".into();
        assert!(serde_json::from_value::<RankRequest>(json).is_err());
        let mut json = serde_json::to_value(BuiltinPriority.rank(&r).unwrap()).unwrap();
        json["maxAttempts"] = 99.into();
        assert!(serde_json::from_value::<RankResult>(json).is_err());
        let mut oversized = r.clone();
        oversized.candidates = vec![r.candidates[0].clone(); 33];
        assert_eq!(oversized.validate(), Err(RankError::ResourceLimit));
        let mut invalid = r.clone();
        invalid.candidates[0].capabilities.context_window = 1;
        assert_eq!(invalid.validate(), Err(RankError::InvalidInput));
        let empty = RankRequest {
            candidates: vec![],
            configured_order: vec![],
            preferred_model_id: None,
            default_model_id: None,
            ..r
        };
        assert!(BuiltinPriority.rank(&empty).unwrap().order.is_empty());
    }
    #[test]
    fn checked_in_contracts_equal_current_rust_serialization() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../contracts/domain/routing-policy");
        for (name, schema) in schemas().as_object().unwrap() {
            let old: serde_json::Value = serde_json::from_slice(
                &std::fs::read(root.join(format!("{name}.schema.json"))).unwrap(),
            )
            .unwrap();
            assert_eq!(&old, schema);
        }
    }
}
