//! Only explicit native failures are classified. No prompt text is interpreted as an error.
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelFailure {
    pub category: String,
    pub code: Option<String>,
    pub http_status: Option<u16>,
    pub retryable: Option<bool>,
    pub message: Option<String>,
    pub source: String,
    pub occurrences: usize,
    pub last_observed_at: Option<String>,
}

pub(super) fn native(value: &Value, source: &str) -> ModelFailure {
    let data = value.get("data").unwrap_or(value);
    let info = &value["codex_error_info"];
    let code = value["name"]
        .as_str()
        .or_else(|| value["code"].as_str())
        .or_else(|| info.as_str())
        .or_else(|| {
            info.as_object()
                .and_then(|v| v.keys().next())
                .map(String::as_str)
        })
        .or_else(|| value.as_str())
        .map(|s| super::parser::safe_text(s, 160).0);
    let message = data["message"].as_str().or_else(|| value.as_str());
    let status = data["statusCode"]
        .as_u64()
        .or_else(|| data["status"].as_u64())
        .or_else(|| {
            info.as_object()
                .and_then(|v| v.values().next())
                .and_then(|v| v["http_status_code"].as_u64())
        })
        .and_then(|n| u16::try_from(n).ok())
        .filter(|n| (100..=599).contains(n));
    from_parts(
        code.as_deref(),
        message,
        status,
        data["isRetryable"].as_bool(),
        source,
    )
}

pub(super) fn from_parts(
    code: Option<&str>,
    message: Option<&str>,
    http_status: Option<u16>,
    retryable: Option<bool>,
    source: &str,
) -> ModelFailure {
    let evidence = format!("{} {}", code.unwrap_or(""), message.unwrap_or("")).to_lowercase();
    let category = match http_status {
        Some(401 | 403) => "authentication",
        Some(429) => "rate_limit",
        Some(500..=599) => "upstream",
        _ if ["messageaborted", "cancelled", "canceled", "aborted"]
            .iter()
            .any(|s| evidence.contains(s)) =>
        {
            "cancelled"
        }
        _ if ["timeout", "timed out", "etimedout"]
            .iter()
            .any(|s| evidence.contains(s)) =>
        {
            "timeout"
        }
        _ if [
            "cannot connect",
            "connection",
            "econn",
            "enotfound",
            "fetch failed",
            "network",
        ]
        .iter()
        .any(|s| evidence.contains(s)) =>
        {
            "connection"
        }
        _ if ["unauthorized", "authentication", "invalid_api_key"]
            .iter()
            .any(|s| evidence.contains(s)) =>
        {
            "authentication"
        }
        _ if ["rate_limit", "ratelimit", "usage_limit"]
            .iter()
            .any(|s| evidence.contains(s)) =>
        {
            "rate_limit"
        }
        _ if evidence.contains("context") && evidence.contains("exceed") => "context_limit",
        _ => "unknown",
    };
    ModelFailure {
        category: category.into(),
        code: code.map(|s| super::parser::safe_text(s, 160).0),
        http_status,
        retryable,
        message: message.map(|s| super::parser::safe_text(s, 4096).0),
        source: source.into(),
        occurrences: 1,
        last_observed_at: None,
    }
}
