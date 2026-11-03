//! Dated, protocol-specific supplements. Catalog declarations always take precedence.
use super::{Model, ModelsRequest};
use chrono::{DateTime, Utc};
use serde::Serialize;
use std::collections::BTreeMap;

const CHECKED_AT: &str = "2026-10-26T00:00:00Z";
const EXPIRES_AT: &str = "2026-11-09T00:00:00Z";
const PRICING: &str = "https://api-docs.deepseek.com/quick_start/pricing/";
const CHAT: &str = "https://api-docs.deepseek.com/api/create-chat-completion/";
const RESPONSES: &str = "https://api-docs.deepseek.com/api/create-response/";
const MESSAGES: &str = "https://api-docs.deepseek.com/guides/anthropic_api/";
const INTEGRATION: &str =
    "https://api-docs.deepseek.com/quick_start/agent_integrations/claude_code/";
const CLAUDE: &str =
    "https://code.claude.com/docs/en/model-config#context-window-and-auto-compaction";

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Evidence {
    fields: Vec<&'static str>,
    label: &'static str,
    url: &'static str,
    checked_at: &'static str,
    expires_at: &'static str,
}
impl Evidence {
    fn new(fields: Vec<&'static str>, label: &'static str, url: &'static str) -> Self {
        Self {
            fields,
            label,
            url,
            checked_at: CHECKED_AT,
            expires_at: EXPIRES_AT,
        }
    }
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ContextAlias {
    context_window: u64,
    upstream_context_window: u64,
    documentation: Vec<Evidence>,
}

pub(super) fn supplement(
    input: &ModelsRequest,
    models: &mut BTreeMap<String, Model>,
    now: DateTime<Utc>,
) {
    let valid_time = [CHECKED_AT, EXPIRES_AT].map(|s| DateTime::parse_from_rfc3339(s).unwrap());
    if now < valid_time[0] || now >= valid_time[1] {
        return;
    }
    let Ok(url) = url::Url::parse(&input.endpoint) else {
        return;
    };
    if url.scheme() != "https"
        || url.host_str() != Some("api.deepseek.com")
        || url.port_or_known_default() != Some(443)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return;
    }
    let path = url.path().trim_end_matches('/');
    let source = match (input.protocol.as_str(), path) {
        ("chat-completions", "" | "/v1") => CHAT,
        ("responses", "" | "/v1") => RESPONSES,
        ("messages", "/anthropic" | "/anthropic/v1") => MESSAGES,
        _ => return,
    };
    for model in models
        .values_mut()
        .filter(|m| matches!(m.id.as_str(), "deepseek-flash" | "deepseek-v4-pro"))
    {
        let metadata = &mut model.metadata;
        if metadata.capabilities.tools.is_none() {
            metadata.capabilities.tools = Some(true);
            metadata
                .documentation
                .push(Evidence::new(vec!["tools"], "工具支持", PRICING));
        }
        let mut output_fields = vec![];
        if metadata.capabilities.structured_output.is_none() {
            // Chat's format enum excludes json_schema; Messages supports only output_config.effort.
            metadata.capabilities.structured_output = Some(input.protocol == "responses");
            output_fields.push("structuredOutput");
        }
        if metadata.capabilities.json_output.is_none() && input.protocol != "messages" {
            metadata.capabilities.json_output = Some(true);
            output_fields.push("jsonOutput");
        }
        if !output_fields.is_empty() {
            metadata
                .documentation
                .push(Evidence::new(output_fields, "输出格式", source));
        }
        // This is a Claude Code window marker, not a directory model or a larger upstream limit.
        if input.protocol == "messages"
            && let Some(upstream) = metadata.context_window.filter(|n| *n >= 1_000_000)
        {
            metadata.context_alias = Some(ContextAlias {
                context_window: 1_000_000,
                upstream_context_window: upstream,
                documentation: vec![
                    Evidence::new(vec!["contextWindow"], "DeepSeek 接入说明", INTEGRATION),
                    Evidence::new(vec!["contextWindow"], "Claude Code 上下文标记", CLAUDE),
                ],
            });
        }
    }
}

#[cfg(test)]
#[path = "official_tests.rs"]
mod tests;
