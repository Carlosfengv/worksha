//! Reassemble the old IPC using common business data and operation-local native bindings.
use super::{McpSnapshot, NativeError, NativeResult, d, m, normalize};
use crate::sessions::mcp_metrics::{McpFacet, McpGroup, McpUsage};
use std::collections::BTreeMap;

fn invalid() -> NativeError {
    NativeError::new("unavailable", "MCP 结果未通过领域契约核查")
}
fn count(value: d::Decimal) -> NativeResult<usize> {
    usize::try_from(value.value()).map_err(|_| invalid())
}
fn status(value: &str, allowed: &'static [&'static str]) -> NativeResult<&'static str> {
    allowed
        .iter()
        .copied()
        .find(|s| *s == value)
        .ok_or_else(invalid)
}
fn usage(v: m::McpUsageSummary) -> NativeResult<McpUsage> {
    Ok(McpUsage {
        related_tokens: v.related_tokens.map(|n| n.value()),
        related_responses: count(v.related_responses)?,
        token_covered_calls: count(v.token_covered_calls)?,
        token_missing_calls: count(v.token_missing_calls)?,
    })
}
fn native_usage(v: d::Usage) -> crate::usage::Usage {
    crate::usage::Usage {
        input_tokens: v.input_tokens.map(|n| n.value()),
        output_tokens: v.output_tokens.map(|n| n.value()),
        cached_input_tokens: v.cached_input_tokens.map(|n| n.value()),
        cache_write_input_tokens: v.cache_write_input_tokens.map(|n| n.value()),
        reasoning_output_tokens: v.reasoning_output_tokens.map(|n| n.value()),
        total_tokens: v.total_tokens.map(|n| n.value()),
    }
}

pub(super) fn snapshot(facts: m::McpFacts, mut host: McpSnapshot) -> NativeResult<McpSnapshot> {
    facts.validate().map_err(|_| invalid())?;
    if facts.catalogs.len() != host.services.len()
        || facts.executions.len() != host.calls.len()
        || facts.groups.len() != host.groups.len()
    {
        return Err(invalid());
    }
    let mut paths = BTreeMap::new();
    for path in host
        .groups
        .iter()
        .flat_map(|g| &g.project_paths)
        .chain(host.services.iter().filter_map(|s| s.project_path.as_ref()))
        .chain(host.calls.iter().filter_map(|c| c.project_path.as_ref()))
    {
        paths.insert(normalize::facet_ref(path), path.clone());
    }
    let path = |id: Option<&str>| -> NativeResult<Option<String>> {
        id.map(|id| paths.get(id).cloned().ok_or_else(invalid))
            .transpose()
    };
    for (s, f) in host.services.iter_mut().zip(facts.catalogs) {
        if s.id != f.id
            || s.agent_id != f.source_id
            || s.applicable_agent_ids != f.applicable_product_ids
            || s.project_path.as_deref().map(normalize::facet_ref) != f.project_facet_ref
            || s.configuration_status != f.configuration_status
            || s.connection_status != f.connection_status
            || s.observation_status != f.observation_status
        {
            return Err(invalid());
        }
        s.name = f.name;
        s.applicable_agent_ids = f.applicable_product_ids;
        s.project_path = path(f.project_facet_ref.as_deref())?;
        s.configuration_status = status(&f.configuration_status, &["present", "unknown"])?;
        // This native inventory has no client-connection proof. A guest cannot add it.
        s.connection_status = status(&f.connection_status, &["unknown"])?;
        s.observation_status = status(
            &f.observation_status,
            &["unknown", "native_history", "not_enabled"],
        )?;
        s.tool_catalog_count = f.tool_catalog_count.map(count).transpose()?;
        s.call_count = f.execution_count.map(count).transpose()?;
        s.returned_count = count(f.returned_count)?;
        s.error_count = count(f.error_count)?;
        s.unknown_count = count(f.unknown_count)?;
        s.duration_samples = count(f.duration_samples)?;
        s.p95_ms = f.p95_ms.map(|n| n.value());
        s.error_rate = f.error_rate;
        s.recorded_tool_count = count(f.recorded_tool_count)?;
        s.session_count = count(f.session_count)?;
        s.last_call_at = f.last_execution_at;
    }
    for (c, f) in host.calls.iter_mut().zip(facts.executions) {
        if c.call.id != f.id
            || c.call.agent_id != f.source_id
            || c.call.session_id != f.session_id
            || c.service_id != f.service_id
            || c.group_id != f.group_id
            || c.call.agent_product_id != f.product_id
            || c.call.response_id != f.response_id
            || c.related_agent_product_id != f.associated_product_id
            || c.project_path.as_deref().map(normalize::facet_ref) != f.project_facet_ref
            || c.association != f.association
            || c.related_usage_state != f.associated_usage_state
        {
            return Err(invalid());
        }
        c.service_id = f.service_id;
        c.group_id = f.group_id;
        c.call.agent_product_id = f.product_id;
        c.call.response_id = f.response_id;
        c.call.name = f.name;
        c.call.status = f.status;
        c.call.timestamp = f.occurred_at;
        c.call.duration_ms = f.duration_ms.map(|n| n.value());
        c.related_usage = f.associated_response_usage.map(native_usage);
        c.related_agent_product_id = f.associated_product_id;
        c.project_path = path(f.project_facet_ref.as_deref())?;
        c.association = status(&f.association, &["unknown", "current_configuration"])?;
        c.related_usage_state = status(
            &f.associated_usage_state,
            &["unknown", "recorded", "outside_agent_scope"],
        )?;
    }
    let mut groups = Vec::with_capacity(facts.groups.len());
    for (original, f) in host.groups.iter().zip(facts.groups) {
        if original.id != f.id
            || original.identity != f.identity
            || original.service_ids != f.service_ids
            || original
                .project_paths
                .iter()
                .map(|p| normalize::facet_ref(p))
                .collect::<Vec<_>>()
                != f.project_facet_refs
        {
            return Err(invalid());
        }
        let u = usage(f.usage)?;
        groups.push(McpGroup {
            id: f.id,
            name: f.name,
            identity: status(&f.identity, &["server_name", "unattributed"])?,
            service_ids: f.service_ids,
            agent_ids: f.observed_client_ids,
            configured_agent_ids: f.configured_product_ids,
            project_paths: f
                .project_facet_refs
                .iter()
                .map(|id| path(Some(id)).and_then(|p| p.ok_or_else(invalid)))
                .collect::<NativeResult<_>>()?,
            configuration_count: count(f.configuration_count)?,
            call_count: f.execution_count.map(count).transpose()?,
            last_call_at: f.last_execution_at,
            returned_count: count(f.returned_count)?,
            error_count: count(f.error_count)?,
            unknown_count: count(f.unknown_count)?,
            error_rate: f.error_rate,
            duration_samples: count(f.duration_samples)?,
            p95_ms: f.p95_ms.map(|n| n.value()),
            session_count: count(f.session_count)?,
            related_tokens: u.related_tokens,
            related_responses: u.related_responses,
            token_covered_calls: u.token_covered_calls,
            token_missing_calls: u.token_missing_calls,
        });
    }
    let mut project_facets = host
        .facets
        .projects
        .into_iter()
        .map(|f| (f.id.clone(), f))
        .collect::<BTreeMap<_, _>>();
    host.facets.projects = facts
        .project_facets
        .into_iter()
        .map(|id| project_facets.remove(&id).ok_or_else(invalid))
        .collect::<NativeResult<_>>()?;
    host.facets.models = facts
        .model_facets
        .into_iter()
        .map(|f| McpFacet {
            id: f.id,
            label: f.label,
        })
        .collect();
    host.groups = groups;
    host.usage_summary = usage(facts.usage_summary)?;
    host.total_calls = count(facts.total_executions)?;
    host.next_cursor = facts.next_cursor;
    host.selected_service_id = facts.selected_service_id;
    host.selected_group_id = facts.selected_group_id;
    host.as_of = facts.source_as_of;
    host.history_as_of = facts.history_as_of;
    host.coverage = status(&facts.coverage, &["partial", "complete", "unavailable"])?;
    Ok(host)
}
