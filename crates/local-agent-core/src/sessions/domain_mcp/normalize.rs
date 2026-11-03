use super::{McpSnapshot, m};
use crate::sessions::mcp_metrics;
use std::collections::{BTreeMap, BTreeSet, HashSet};

pub(super) fn facet_ref(path: &str) -> String {
    mcp_metrics::facet_id("project", Some(path))
}
pub(super) fn project_ids(
    catalog: &crate::sessions::Catalog,
    hidden: &HashSet<String>,
) -> BTreeMap<String, String> {
    let mut candidates: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for f in crate::sessions::unique_files(catalog) {
        if !catalog
            .sources
            .iter()
            .any(|s| s.agent_id == f.agent && s.enabled)
        {
            continue;
        }
        let s = &f.summary;
        if let Some(id) = &s.project_id {
            let paths = s
                .project_path
                .iter()
                .chain(s.project_root.iter())
                .collect::<Vec<_>>();
            if paths.iter().any(|p| hidden.contains(*p)) {
                continue;
            }
            for path in paths {
                candidates
                    .entry(path.clone())
                    .or_default()
                    .insert(id.clone());
            }
        }
    }
    candidates
        .into_iter()
        .filter_map(|(path, mut ids)| {
            if ids.len() == 1 {
                ids.pop_first().map(|id| (path, id))
            } else {
                None
            }
        })
        .collect()
}
pub(super) fn snapshot(v: &McpSnapshot, project_ids: &BTreeMap<String, String>) -> m::McpFacts {
    m::McpFacts {
        catalogs: v
            .services
            .iter()
            .map(|s| m::McpCatalogFact {
                id: s.id.clone(),
                source_id: s.agent_id.clone(),
                name: s.name.clone(),
                applicable_product_ids: s.applicable_agent_ids.clone(),
                project_ref: s
                    .project_path
                    .as_ref()
                    .and_then(|p| project_ids.get(p))
                    .cloned(),
                project_facet_ref: s.project_path.as_deref().map(facet_ref),
                configuration_status: s.configuration_status.into(),
                connection_status: s.connection_status.into(),
                observation_status: s.observation_status.into(),
                tool_catalog_count: s.tool_catalog_count.map(|n| (n as u64).into()),
                execution_count: s.call_count.map(|n| (n as u64).into()),
                returned_count: (s.returned_count as u64).into(),
                error_count: (s.error_count as u64).into(),
                unknown_count: (s.unknown_count as u64).into(),
                duration_samples: (s.duration_samples as u64).into(),
                p95_ms: s.p95_ms.map(Into::into),
                error_rate: s.error_rate,
                recorded_tool_count: (s.recorded_tool_count as u64).into(),
                session_count: (s.session_count as u64).into(),
                last_execution_at: s.last_call_at.clone(),
            })
            .collect(),
        executions: v
            .calls
            .iter()
            .map(|v| m::McpExecutionFact {
                id: v.call.id.clone(),
                service_id: v.service_id.clone(),
                group_id: v.group_id.clone(),
                source_id: v.call.agent_id.clone(),
                session_id: v.call.session_id.clone(),
                product_id: v.call.agent_product_id.clone(),
                response_id: v.call.response_id.clone(),
                name: v.call.name.clone(),
                status: v.call.status.clone(),
                occurred_at: v.call.timestamp.clone(),
                duration_ms: v.call.duration_ms.map(Into::into),
                associated_response_usage: v.related_usage.as_ref().map(Into::into),
                associated_product_id: v.related_agent_product_id.clone(),
                project_ref: v
                    .project_path
                    .as_ref()
                    .and_then(|p| project_ids.get(p))
                    .cloned(),
                project_facet_ref: v.project_path.as_deref().map(facet_ref),
                association: v.association.into(),
                associated_usage_state: v.related_usage_state.into(),
            })
            .collect(),
        groups: v
            .groups
            .iter()
            .map(|g| m::McpGroup {
                id: g.id.clone(),
                name: g.name.clone(),
                identity: g.identity.into(),
                service_ids: g.service_ids.clone(),
                observed_client_ids: g.agent_ids.clone(),
                configured_product_ids: g.configured_agent_ids.clone(),
                project_refs: g
                    .project_paths
                    .iter()
                    .filter_map(|p| project_ids.get(p))
                    .cloned()
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect(),
                project_facet_refs: g.project_paths.iter().map(|p| facet_ref(p)).collect(),
                configuration_count: (g.configuration_count as u64).into(),
                execution_count: g.call_count.map(|n| (n as u64).into()),
                last_execution_at: g.last_call_at.clone(),
                returned_count: (g.returned_count as u64).into(),
                error_count: (g.error_count as u64).into(),
                unknown_count: (g.unknown_count as u64).into(),
                error_rate: g.error_rate,
                duration_samples: (g.duration_samples as u64).into(),
                p95_ms: g.p95_ms.map(Into::into),
                session_count: (g.session_count as u64).into(),
                usage: usage(
                    g.related_tokens,
                    g.related_responses,
                    g.token_covered_calls,
                    g.token_missing_calls,
                ),
            })
            .collect(),
        usage_summary: usage(
            v.usage_summary.related_tokens,
            v.usage_summary.related_responses,
            v.usage_summary.token_covered_calls,
            v.usage_summary.token_missing_calls,
        ),
        model_facets: v
            .facets
            .models
            .iter()
            .map(|f| m::McpFacet {
                id: f.id.clone(),
                label: f.label.clone(),
            })
            .collect(),
        project_facets: v.facets.projects.iter().map(|f| f.id.clone()).collect(),
        total_executions: (v.total_calls as u64).into(),
        next_cursor: v.next_cursor.clone(),
        selected_service_id: v.selected_service_id.clone(),
        selected_group_id: v.selected_group_id.clone(),
        source_as_of: v.as_of.clone(),
        history_as_of: v.history_as_of.clone(),
        coverage: v.coverage.into(),
    }
}
fn usage(
    tokens: Option<u64>,
    responses: usize,
    covered: usize,
    missing: usize,
) -> m::McpUsageSummary {
    m::McpUsageSummary {
        related_tokens: tokens.map(Into::into),
        related_responses: (responses as u64).into(),
        token_covered_calls: (covered as u64).into(),
        token_missing_calls: (missing as u64).into(),
    }
}
