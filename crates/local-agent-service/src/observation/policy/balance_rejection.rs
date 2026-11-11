use crate::billing::BalanceProvider;
use axum::http::StatusCode;
use local_agent_core::model_services::policy::BalanceRejection;
use serde::Deserialize;
use serde_json::Value;

pub(in crate::observation) fn matches_status(
    provider: BalanceProvider,
    status: StatusCode,
) -> bool {
    match provider {
        BalanceProvider::DeepSeek => status == StatusCode::PAYMENT_REQUIRED,
        BalanceProvider::Kimi => status == StatusCode::TOO_MANY_REQUESTS,
    }
}

pub(in crate::observation) fn parse(
    (provider, credential): (BalanceProvider, String),
    status: StatusCode,
    bytes: &[u8],
    observed_at: chrono::DateTime<chrono::Utc>,
) -> Option<BalanceRejection> {
    #[derive(Deserialize)]
    struct Envelope {
        error: Evidence,
    }
    #[derive(Deserialize)]
    struct Evidence {
        #[serde(rename = "type")]
        kind: Option<String>,
        code: Option<Value>,
        message: String,
    }

    if !matches_status(provider, status) {
        return None;
    }
    let Envelope { error } = serde_json::from_slice(bytes).ok()?;
    if error.message.trim().is_empty() {
        return None;
    }
    let kinds = |value: &str| match provider {
        BalanceProvider::DeepSeek => matches!(
            value,
            "unknown_error"
                | "invalid_request_error"
                | "insufficient_balance"
                | "billing_error"
                | "api_error"
        ),
        BalanceProvider::Kimi => value == "exceeded_current_quota_error",
    };
    if error.code.as_ref().is_some_and(|code| {
        code.as_u64() != Some(u64::from(status.as_u16())) && !code.as_str().is_some_and(kinds)
    }) || match provider {
        BalanceProvider::DeepSeek => error.kind.as_deref().is_some_and(|kind| !kinds(kind)),
        BalanceProvider::Kimi => !error.kind.as_deref().is_some_and(kinds),
    } {
        return None;
    }
    Some(BalanceRejection {
        credential,
        observed_at,
    })
}
