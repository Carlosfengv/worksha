//! Logical name rollups retain source configurations rather than identifying
//! unrelated Endpoints as one connection. Usage belongs to model responses.
use super::mcp::{McpCall, McpService};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

pub(super) fn group_id(service: &McpService) -> String {
    crate::projects::hash(
        &serde_json::json!([
            "mcp_name",
            service.name,
            service.name == "来源未知" && service.configuration_status == "unknown"
        ])
        .to_string(),
    )
}
pub(super) fn facet_id(kind: &str, value: Option<&str>) -> String {
    value.map_or_else(
        || "unknown".into(),
        |v| crate::projects::hash(&serde_json::json!([kind, v]).to_string()),
    )
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpFacet {
    pub id: String,
    pub label: String,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpFacets {
    pub models: Vec<McpFacet>,
    pub projects: Vec<McpFacet>,
}
pub(super) fn facets(calls: &[McpCall], group: Option<&str>) -> McpFacets {
    let mut models = BTreeMap::new();
    let mut projects = BTreeMap::new();
    for row in calls
        .iter()
        .filter(|row| group.is_none_or(|g| row.group_id == g))
    {
        models.insert(
            facet_id("model", row.call.model.as_deref()),
            row.call.model.clone().unwrap_or_else(|| "模型未知".into()),
        );
        projects.insert(
            facet_id("project", row.project_path.as_deref()),
            row.project_path
                .clone()
                .unwrap_or_else(|| "项目未知".into()),
        );
    }
    fn sorted(values: BTreeMap<String, String>) -> Vec<McpFacet> {
        let mut result: Vec<_> = values
            .into_iter()
            .map(|(id, label)| McpFacet { id, label })
            .collect();
        result.sort_by(|a, b| a.label.cmp(&b.label));
        result
    }
    McpFacets {
        models: sorted(models),
        projects: sorted(projects),
    }
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpGroup {
    pub id: String,
    pub name: String,
    pub identity: &'static str,
    pub service_ids: Vec<String>,
    pub agent_ids: Vec<String>,
    pub configured_agent_ids: Vec<String>,
    pub project_paths: Vec<String>,
    pub configuration_count: usize,
    pub call_count: Option<usize>,
    pub last_call_at: Option<String>,
    pub returned_count: usize,
    pub error_count: usize,
    pub unknown_count: usize,
    pub error_rate: Option<f64>,
    pub duration_samples: usize,
    pub p95_ms: Option<u64>,
    pub session_count: usize,
    pub related_tokens: Option<u64>,
    pub related_responses: usize,
    pub token_covered_calls: usize,
    pub token_missing_calls: usize,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpUsage {
    pub related_tokens: Option<u64>,
    pub related_responses: usize,
    pub token_covered_calls: usize,
    pub token_missing_calls: usize,
}
pub(super) fn usage<'a>(rows: impl Iterator<Item = &'a McpCall>, known_empty: bool) -> McpUsage {
    let mut responses = BTreeSet::new();
    let mut tokens = 0u64;
    let mut covered = 0;
    let mut total = 0;
    for row in rows {
        total += 1;
        if let Some((response, usage)) = row
            .call
            .model_response_id
            .as_ref()
            .zip(row.related_usage.as_ref().and_then(|u| u.total_tokens))
        {
            covered += 1;
            if responses.insert(response) {
                tokens = tokens.saturating_add(usage);
            }
        }
    }
    McpUsage {
        related_tokens: (covered > 0 || known_empty).then_some(tokens),
        related_responses: responses.len(),
        token_covered_calls: covered,
        token_missing_calls: total - covered,
    }
}
pub(super) fn groups(services: &[McpService], calls: &[McpCall]) -> Vec<McpGroup> {
    let mut entries: BTreeMap<String, Vec<&McpService>> = BTreeMap::new();
    for service in services {
        entries.entry(group_id(service)).or_default().push(service);
    }
    let mut result = vec![];
    for (id, members) in entries {
        let rows: Vec<_> = calls.iter().filter(|row| row.group_id == id).collect();
        let known = !rows.is_empty() || members.iter().any(|s| s.call_count.is_some());
        let mut durations: Vec<_> = rows
            .iter()
            .filter(|r| matches!(r.call.status.as_str(), "returned" | "error"))
            .filter_map(|r| r.call.duration_ms)
            .collect();
        durations.sort_unstable();
        let returned_count = rows.iter().filter(|r| r.call.status == "returned").count();
        let error_count = rows.iter().filter(|r| r.call.status == "error").count();
        let usage = usage(rows.iter().copied(), rows.is_empty() && known);
        let first = members[0];
        result.push(McpGroup {
            id,
            name: first.name.clone(),
            identity: if first.name == "来源未知" && first.configuration_status == "unknown" {
                "unattributed"
            } else {
                "server_name"
            },
            service_ids: members.iter().map(|s| s.id.clone()).collect(),
            agent_ids: rows
                .iter()
                .map(|r| crate::agents::group(&r.call.agent_id, r.call.agent_product_id.as_deref()))
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect(),
            configured_agent_ids: members
                .iter()
                .flat_map(|s| s.applicable_agent_ids.clone())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect(),
            project_paths: members
                .iter()
                .filter_map(|s| s.project_path.clone())
                .chain(rows.iter().filter_map(|r| r.project_path.clone()))
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect(),
            configuration_count: members
                .iter()
                .filter(|s| s.configuration_status == "present")
                .count(),
            call_count: known.then_some(rows.len()),
            last_call_at: rows.iter().filter_map(|r| r.call.timestamp.clone()).max(),
            returned_count,
            error_count,
            unknown_count: rows.len() - returned_count - error_count,
            error_rate: (returned_count + error_count > 0)
                .then(|| error_count as f64 / (returned_count + error_count) as f64),
            duration_samples: durations.len(),
            p95_ms: durations
                .get((durations.len() * 95).div_ceil(100).saturating_sub(1))
                .copied(),
            session_count: rows
                .iter()
                .map(|r| &r.call.session_id)
                .collect::<BTreeSet<_>>()
                .len(),
            related_tokens: usage.related_tokens,
            related_responses: usage.related_responses,
            token_covered_calls: usage.token_covered_calls,
            token_missing_calls: usage.token_missing_calls,
        });
    }
    result.sort_by(|a, b| {
        (a.identity == "unattributed", &a.name).cmp(&(b.identity == "unattributed", &b.name))
    });
    result
}
