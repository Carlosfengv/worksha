use axum::http::{Extensions, HeaderValue, StatusCode};
use chrono::{DateTime, Utc};
use local_agent_core::model_services::policy::{
    OpenRouterBudgetRejection, OpenRouterBudgetScope as Scope,
};
use serde::Deserialize;
use serde_json::Value;

#[derive(Clone)]
pub(in crate::observation) struct InFlightBudget;

pub(in crate::observation) enum BudgetRejection {
    Exhausted(OpenRouterBudgetRejection),
    InFlight,
}

impl BudgetRejection {
    pub(in crate::observation) fn message(&self) -> &'static str {
        match self {
            Self::Exhausted(_) => "OpenRouter 确认此凭据的上游预算已用尽",
            Self::InFlight => "OpenRouter 的在途请求预算暂满，请等待重试时间",
        }
    }

    pub(in crate::observation) fn mark(self, extensions: &mut Extensions) {
        match self {
            Self::Exhausted(rejection) => {
                extensions.insert(super::ConfirmedQuota::OpenRouter(rejection));
            }
            Self::InFlight => {
                extensions.insert(InFlightBudget);
            }
        }
    }
}

#[derive(Deserialize)]
struct Envelope {
    error: WireError,
}

#[derive(Deserialize)]
struct WireError {
    code: Option<Value>,
    #[serde(rename = "type")]
    kind: Option<String>,
    error_type: Option<String>,
    metadata: Metadata,
}

#[derive(Deserialize)]
struct Metadata {
    limit_source: String,
    scope: Option<String>,
    reason: Option<String>,
    limit_usd: Option<f64>,
    spend_usd: Option<f64>,
    resets_at: Option<String>,
    error_type: Option<String>,
    provider_code: Option<Value>,
    provider_name: Option<Value>,
    patterns: Option<Value>,
    reasons: Option<Value>,
}

pub(in crate::observation) fn parse(
    status: StatusCode,
    bytes: &[u8],
    retry_after: Option<&HeaderValue>,
    credential: String,
    observed_at: DateTime<Utc>,
) -> Option<BudgetRejection> {
    let (native, canonical) = match status {
        StatusCode::FORBIDDEN => ("permission_error", "permission_denied"),
        StatusCode::PAYMENT_REQUIRED => ("billing_error", "payment_required"),
        _ => return None,
    };
    let Envelope { error } = serde_json::from_slice(bytes).ok()?;
    let metadata = error.metadata;
    // Match the gateway's status and the documented envelope. Provider-side
    // errors and conflicting content-filter evidence cannot authorize failover.
    if error
        .code
        .as_ref()
        .is_some_and(|code| code.as_u64() != Some(u64::from(status.as_u16())))
        || error.kind.as_deref().is_some_and(|kind| kind != native)
        || (error.code.is_none() && error.kind.is_none())
        || [error.error_type.as_deref(), metadata.error_type.as_deref()]
            .into_iter()
            .flatten()
            .any(|kind| kind != canonical)
        || [
            metadata.provider_code,
            metadata.provider_name,
            metadata.patterns,
            metadata.reasons,
        ]
        .into_iter()
        .any(|field| field.is_some())
    {
        return None;
    }
    if status == StatusCode::PAYMENT_REQUIRED
        && metadata.limit_source == "openrouter_in_flight_budget"
        && metadata.reason.as_deref() == Some("in_flight_budget_exhausted")
        && retry_after
            .and_then(|header| super::retry_after_seconds(header, observed_at))
            .is_some_and(|seconds| seconds > 0)
    {
        return Some(BudgetRejection::InFlight);
    }
    if matches!(
        metadata.reason.as_deref(),
        Some("in_flight_budget_exhausted" | "weight_exceeds_budget")
    ) {
        return None;
    }
    let scope = match (
        status,
        metadata.limit_source.as_str(),
        metadata.scope.as_deref(),
    ) {
        (StatusCode::PAYMENT_REQUIRED, "openrouter_key_limit", None | Some("key_limit"))
        | (StatusCode::FORBIDDEN, "openrouter_key_limit", Some("key_limit")) => Scope::KeyLimit,
        (StatusCode::FORBIDDEN, "openrouter_guardrail_budget", Some("api_key")) => Scope::ApiKey,
        (StatusCode::FORBIDDEN, "openrouter_guardrail_budget", Some("workspace_default_key")) => {
            Scope::DefaultKey
        }
        (StatusCode::FORBIDDEN, "openrouter_guardrail_budget", Some("member")) => Scope::Member,
        (
            StatusCode::FORBIDDEN,
            "openrouter_guardrail_budget",
            Some("workspace_default_member"),
        ) => Scope::DefaultMember,
        (StatusCode::FORBIDDEN, "openrouter_workspace_budget", Some("workspace")) => {
            Scope::Workspace
        }
        _ => return None,
    };
    match (metadata.limit_usd, metadata.spend_usd) {
        (Some(limit), Some(spend)) if limit >= 0.0 && spend >= limit => (),
        (None, None) if status == StatusCode::PAYMENT_REQUIRED => (),
        _ => return None,
    }
    let reset_at = if let Some(reset) = metadata.resets_at {
        let reset = DateTime::parse_from_rfc3339(&reset).ok()?.timestamp();
        // An already expired window is not evidence for the current budget.
        if reset <= observed_at.timestamp() {
            return None;
        }
        Some(reset)
    } else {
        None
    };
    Some(BudgetRejection::Exhausted(OpenRouterBudgetRejection {
        credential,
        scope,
        observed_at,
        reset_at,
    }))
}
