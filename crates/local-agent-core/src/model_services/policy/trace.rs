use super::RoutingPolicy;
use crate::model_services::ServiceSelection;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RouteDecisionReason {
    DisabledModel,
    RoutingDisabled,
    CapabilityMismatch,
    CrossModelFallbackDisabled,
    StrictAffinity,
    ConfirmedTurnRequired,
    PolicyExcluded,
    PolicyUnavailable,
    ChildUnavailable,
    AccountUnavailable,
    AccountQuotaExhausted,
    AccessCredentialRejected,
    AccessModelDenied,
    DuplicateTarget,
    RateLimitCooldown,
    FailureCooldown,
    RecoveryInProgress,
    ConfiguredRateLimit,
    AttemptLimit,
    Deadline,
    FallbackStopped,
    EarlierCandidateAccepted,
    AdmissionFailed,
    OutcomeUnknown,
    UpstreamOutcomeUnconfirmed,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RouteStep {
    pub service: String,
    pub version_id: String,
    pub model: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub model_id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RouteDecision {
    pub path: Vec<RouteStep>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_position: Option<u8>,
    pub reason: RouteDecisionReason,
}

impl RouteDecision {
    pub fn candidate(
        selection: &ServiceSelection,
        policy: &RoutingPolicy,
        reason: RouteDecisionReason,
    ) -> Self {
        let model = selection
            .nested_route
            .last()
            .map_or(selection.model_id.as_str(), |hop| hop.model_id.as_str());
        let position = selection
            .upstream_account
            .as_ref()
            .and_then(|account| {
                policy
                    .accounts(model)
                    .iter()
                    .position(|candidate| candidate == account)
            })
            .map(|index| index as u8 + 1);
        Self::new(selection, position, reason)
    }

    pub fn new(
        selection: &ServiceSelection,
        account_position: Option<u8>,
        reason: RouteDecisionReason,
    ) -> Self {
        let display = |name: &str| name.chars().filter(|c| !c.is_control()).take(100).collect();
        let path = std::iter::once(RouteStep {
            service: display(&selection.service_name),
            version_id: selection.service_version_id.clone(),
            model: display(&selection.model_name),
            model_id: selection.model_id.clone(),
        })
        .chain(selection.nested_route.iter().map(|hop| RouteStep {
            service: display(&hop.service_name),
            version_id: hop.version_id.clone(),
            model: display(&hop.model_name),
            model_id: hop.model_id.clone(),
        }))
        .collect();
        Self {
            path,
            account_position,
            reason,
        }
    }

    pub(super) fn prepend(&mut self, group: &ServiceSelection) {
        let mut path = Self::new(group, None, self.reason).path;
        path.append(&mut self.path);
        self.path = path;
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RouteTrace {
    pub decisions: Vec<RouteDecision>,
    pub truncated: bool,
    pub sent_upstream: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_status: Option<u16>,
}

impl RouteTrace {
    pub fn extend(&mut self, decisions: impl IntoIterator<Item = RouteDecision>) {
        // A three-level graph can have more entries than the leaf bound.
        const MAX_DECISIONS: usize = 512;
        let mut entries = decisions.into_iter();
        self.decisions.extend(
            entries
                .by_ref()
                .take(MAX_DECISIONS.saturating_sub(self.decisions.len())),
        );
        self.truncated |= entries.next().is_some();
    }
}
