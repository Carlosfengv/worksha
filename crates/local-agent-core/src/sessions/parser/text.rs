use chrono::{DateTime, Utc};
use regex::Regex;
use serde_json::Value;
use std::sync::OnceLock;

pub(in crate::sessions) fn str_at<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key)?.as_str().filter(|s| !s.is_empty())
}
pub(in crate::sessions) fn num(value: &Value, key: &str) -> Option<u64> {
    value.get(key)?.as_u64()
}
pub(in crate::sessions) fn normalize_time(text: &str) -> Option<String> {
    DateTime::parse_from_rfc3339(text).ok().map(|t| {
        t.with_timezone(&Utc)
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
    })
}
pub(in crate::sessions) fn duration(v: &Value) -> Option<u64> {
    v.as_u64().or_else(|| {
        num(v, "secs").map(|s| {
            s.saturating_mul(1000)
                .saturating_add(num(v, "nanos").unwrap_or(0) / 1_000_000)
        })
    })
}
pub(in crate::sessions) fn is_error(v: &Value) -> bool {
    v["is_error"].as_bool() == Some(true)
        || v["isError"].as_bool() == Some(true)
        || v.get("error")
            .is_some_and(|e| !e.is_null() && e != &Value::Bool(false))
        || v["exit_code"].as_i64().is_some_and(|n| n != 0)
        || str_at(v, "status").is_some_and(|s| ["failed", "error", "declined"].contains(&s))
}
pub(in crate::sessions) fn is_context(text: &str) -> bool {
    if text.contains("## My request:") {
        return false;
    }
    let text = text.trim_start();
    [
        "# AGENTS.md instructions",
        "<environment_context>",
        "<system-reminder>",
        "<external_codex_apps_open_page>",
        "<recommended_plugins>",
        "<skills_instructions>",
        "<in-app-browser-context",
        "<app-context>",
    ]
    .iter()
    .any(|p| text.starts_with(p))
}
pub(in crate::sessions) fn content_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Array(items) => items
            .iter()
            .filter_map(|v| {
                if let Some(text) = v.as_str() {
                    Some(text.to_string())
                } else if let Some(text) = str_at(v, "text") {
                    Some(text.into())
                } else if str_at(v, "type").is_some_and(|s| s.contains("image")) {
                    Some("[图片内容，当前只读视图不加载二进制附件]".into())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Value::Null => String::new(),
        _ => serde_json::to_string_pretty(value).unwrap_or_default(),
    }
}

/// Redaction happens before data crosses IPC. No raw auth/config records are exposed.
pub(in crate::sessions) fn safe_text(text: &str, limit: usize) -> (String, bool) {
    static PATTERNS: OnceLock<Vec<Regex>> = OnceLock::new();
    let patterns = PATTERNS.get_or_init(|| [
        r"(?i)(?:bearer\s+)[a-z0-9._~+/=-]{8,}",
        r"\b(?:sk|rk|pk)-(?:ant-)?[a-zA-Z0-9_-]{12,}",
        r#"(?i)(?:api[_-]?key|access[_-]?token|refresh[_-]?token|authorization|password|client[_-]?secret)[\s\"':=]+[^\s\"',;}{&<>]+"#,
        r"(?i)https?://[^\s/@]+:[^\s/@]+@",
    ].iter().map(|p| Regex::new(p).unwrap()).collect());
    fn redact_fields(value: &mut Value) {
        match value {
            Value::Object(fields) => {
                for (key, value) in fields {
                    let key = key.to_ascii_lowercase().replace('-', "_");
                    if [
                        "password",
                        "authorization",
                        "api_key",
                        "apikey",
                        "access_token",
                        "refresh_token",
                        "client_secret",
                        "credential",
                        "credentials",
                        "secret",
                        "token",
                    ]
                    .contains(&key.as_str())
                        && !value.is_number()
                        && !value.is_null()
                    {
                        *value = Value::String("[已隐藏凭据]".into());
                    } else {
                        redact_fields(value);
                    }
                }
            }
            Value::Array(values) => {
                for value in values {
                    redact_fields(value);
                }
            }
            _ => {}
        }
    }
    let mut text = if let Ok(mut value) = serde_json::from_str::<Value>(text) {
        redact_fields(&mut value);
        serde_json::to_string_pretty(&value).unwrap_or_default()
    } else {
        text.to_string()
    };
    for pattern in patterns {
        text = pattern.replace_all(&text, "[已隐藏凭据]").into_owned();
    }
    let truncated = text.chars().count() > limit;
    (text.chars().take(limit).collect(), truncated)
}
