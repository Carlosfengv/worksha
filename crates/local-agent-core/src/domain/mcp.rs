//! Catalog observation and execution observation are distinct facts and units.
use super::*;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpFilter {
    pub agent: Option<String>,
    pub days: Option<u16>,
    pub service: Option<String>,
    pub model: Option<String>,
    /// Existing MCP facet selector; distinct from a host project object identity.
    pub project: Option<String>,
    pub call: Option<String>,
    pub cursor: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpRequest {
    pub invocation: Invocation,
    pub filter: McpFilter,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpCatalogFact {
    pub id: String,
    pub source_id: String,
    pub name: String,
    pub applicable_product_ids: Vec<String>,
    pub project_ref: Option<String>,
    pub project_facet_ref: Option<String>,
    pub configuration_status: String,
    pub connection_status: String,
    pub observation_status: String,
    pub tool_catalog_count: Option<Decimal>,
    pub execution_count: Option<Decimal>,
    pub returned_count: Decimal,
    pub error_count: Decimal,
    pub unknown_count: Decimal,
    pub duration_samples: Decimal,
    pub p95_ms: Option<Decimal>,
    pub error_rate: Option<f64>,
    pub recorded_tool_count: Decimal,
    pub session_count: Decimal,
    pub last_execution_at: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpExecutionFact {
    pub id: String,
    pub service_id: String,
    pub group_id: String,
    pub source_id: String,
    pub session_id: String,
    pub product_id: Option<String>,
    pub response_id: Option<String>,
    pub name: String,
    pub status: String,
    pub occurred_at: Option<String>,
    pub duration_ms: Option<Decimal>,
    pub associated_response_usage: Option<Usage>,
    pub associated_product_id: Option<String>,
    pub project_ref: Option<String>,
    pub project_facet_ref: Option<String>,
    pub association: String,
    pub associated_usage_state: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpUsageSummary {
    pub related_tokens: Option<Decimal>,
    pub related_responses: Decimal,
    pub token_covered_calls: Decimal,
    pub token_missing_calls: Decimal,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpGroup {
    pub id: String,
    pub name: String,
    pub identity: String,
    pub service_ids: Vec<String>,
    pub observed_client_ids: Vec<String>,
    pub configured_product_ids: Vec<String>,
    pub project_refs: Vec<String>,
    pub project_facet_refs: Vec<String>,
    pub configuration_count: Decimal,
    pub execution_count: Option<Decimal>,
    pub last_execution_at: Option<String>,
    pub returned_count: Decimal,
    pub error_count: Decimal,
    pub unknown_count: Decimal,
    pub error_rate: Option<f64>,
    pub duration_samples: Decimal,
    pub p95_ms: Option<Decimal>,
    pub session_count: Decimal,
    pub usage: McpUsageSummary,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpFacet {
    pub id: String,
    pub label: String,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpFacts {
    pub catalogs: Vec<McpCatalogFact>,
    pub executions: Vec<McpExecutionFact>,
    /// Full filtered range, independently of the execution page.
    pub groups: Vec<McpGroup>,
    pub usage_summary: McpUsageSummary,
    pub model_facets: Vec<McpFacet>,
    /// Opaque host bindings. Native paths and their labels are not a guest API.
    pub project_facets: Vec<String>,
    pub total_executions: Decimal,
    pub next_cursor: Option<String>,
    pub selected_service_id: Option<String>,
    pub selected_group_id: Option<String>,
    pub source_as_of: String,
    pub history_as_of: String,
    pub coverage: String,
}
pub trait McpSource {
    fn read_mcp(&mut self, request: &McpRequest) -> Result<McpFacts>;
}

/// Configuration observation has no execution counts or connection proof.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpCatalogRequest {
    pub invocation: Invocation,
    pub source_ids: Vec<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpConfiguration {
    pub id: String,
    pub source_id: String,
    pub name: String,
    pub applicable_product_ids: Vec<String>,
    pub project_ref: Option<String>,
    pub transport: Option<String>,
    pub configured_enabled: Option<bool>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpCatalogSnapshot {
    pub entries: Vec<McpConfiguration>,
    pub partial_sources: Vec<String>,
    pub observed_at: String,
    /// Opaque metadata version, without configuration values or credentials.
    pub configuration_revision: String,
}
pub trait McpCatalogSource {
    fn read_catalog(&mut self, request: &McpCatalogRequest) -> Result<McpCatalogSnapshot>;
}
impl McpCatalogSnapshot {
    pub fn validate(&self, sources: &[String]) -> Result<()> {
        let mut ids = std::collections::HashSet::new();
        if self.entries.len() > 4096
            || sources.len() > 16
            || self.partial_sources.iter().any(|s| !sources.contains(s))
            || super::timestamp(&self.observed_at).is_none()
            || self.configuration_revision.is_empty()
            || self.configuration_revision.len() > 256
            || self.entries.iter().any(|e| {
                e.id.is_empty()
                    || e.id.len() > 256
                    || !ids.insert((&e.source_id, &e.id))
                    || !sources.contains(&e.source_id)
                    || e.name.is_empty()
                    || e.name.len() > 2048
                    || e.applicable_product_ids.len() > 16
                    || e.project_ref
                        .as_ref()
                        .is_some_and(|p| p.is_empty() || p.len() > 320)
                    || e.transport.as_ref().is_some_and(|s| s.len() > 64)
            })
        {
            return Err(Failure::new(ErrorCode::InvalidInput));
        }
        Ok(())
    }
}
impl McpFacts {
    pub fn validate(&self) -> Result<()> {
        use std::collections::HashSet;
        let invalid = || Failure::new(ErrorCode::InvalidInput);
        let catalogs = self
            .catalogs
            .iter()
            .map(|c| c.id.as_str())
            .collect::<HashSet<_>>();
        let groups = self
            .groups
            .iter()
            .map(|g| g.id.as_str())
            .collect::<HashSet<_>>();
        let rate = |v: Option<f64>| v.is_none_or(|n| n.is_finite() && (0.0..=1.0).contains(&n));
        if catalogs.len() != self.catalogs.len()
            || groups.len() != self.groups.len()
            || catalogs.contains("")
            || groups.contains("")
            || chrono::DateTime::parse_from_rfc3339(&self.source_as_of).is_err()
            || chrono::DateTime::parse_from_rfc3339(&self.history_as_of).is_err()
            || !["complete", "partial", "unavailable"].contains(&self.coverage.as_str())
            || self.executions.len() as u64 > self.total_executions.value()
            || self.catalogs.iter().any(|c| !rate(c.error_rate))
            || self.groups.iter().any(|g| {
                !rate(g.error_rate)
                    || g.service_ids
                        .iter()
                        .any(|id| !catalogs.contains(id.as_str()))
            })
            || self.executions.iter().any(|e| {
                !catalogs.contains(e.service_id.as_str()) || !groups.contains(e.group_id.as_str())
            })
            || self
                .selected_service_id
                .as_ref()
                .is_some_and(|id| !catalogs.contains(id.as_str()))
            || self
                .selected_group_id
                .as_ref()
                .is_some_and(|id| !groups.contains(id.as_str()))
        {
            return Err(invalid());
        }
        Ok(())
    }
}
