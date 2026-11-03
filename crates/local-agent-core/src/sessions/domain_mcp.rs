//! The compatibility consumer uses the common MCP operation while retaining its full DTO.
use super::{McpQuery, McpSnapshot, NativeError, NativeResult, NativeSessions};
use crate::domain::{self as d, mcp as m};
use m::McpSource;
pub(super) mod catalog;
mod consume;
#[cfg(test)]
mod consumer_tests;
mod normalize;

impl From<&McpQuery> for m::McpFilter {
    fn from(v: &McpQuery) -> Self {
        Self {
            agent: v.agent.clone(),
            days: v.days,
            service: v.service.clone(),
            model: v.model.clone(),
            project: v.project.clone(),
            call: v.call.clone(),
            cursor: v.cursor.clone(),
        }
    }
}

struct NativeRead<'a> {
    broker: &'a dyn Fn() -> NativeResult<McpSnapshot>,
    check: &'a dyn Fn() -> NativeResult<()>,
    expected: m::McpRequest,
    legacy: Option<McpSnapshot>,
    error: Option<NativeError>,
    project_ids: std::collections::BTreeMap<String, String>,
}
impl NativeRead<'_> {
    fn capture<T>(&mut self, result: NativeResult<T>) -> d::Result<T> {
        result.map_err(|error| {
            let failure = super::domain_source::domain_failure(&error, false);
            self.error = Some(error);
            failure
        })
    }
}

impl McpSource for NativeRead<'_> {
    fn read_mcp(&mut self, request: &m::McpRequest) -> d::Result<m::McpFacts> {
        if request != &self.expected {
            return Err(d::Failure::new(d::ErrorCode::StaleSnapshot));
        }
        self.capture((self.check)())?;
        let legacy = self.capture((self.broker)())?;
        self.capture((self.check)())?;
        let facts = normalize::snapshot(&legacy, &self.project_ids);
        facts.validate()?;
        let count = [
            facts.catalogs.len(),
            facts.groups.len(),
            facts.executions.len(),
        ]
        .into_iter()
        .try_fold(0u64, |sum, n| sum.checked_add(n as u64))
        .ok_or_else(|| d::Failure::new(d::ErrorCode::ResourceLimit))?;
        if count > request.invocation.budget.max_records.value() {
            return Err(d::Failure::new(d::ErrorCode::ResourceLimit));
        }
        self.legacy = Some(legacy);
        Ok(facts)
    }
}
impl NativeSessions {
    pub(super) fn domain_mcp_provider(&self) -> d::Provider {
        d::Provider {
            provider_key: "builtin:domain.mcp-source@1:native".into(),
            plugin_id: "builtin.mcp".into(),
            implementation_version: env!("CARGO_PKG_VERSION").into(),
            implementation_identity: format!("local-agent-core@{}:mcp", env!("CARGO_PKG_VERSION")),
            capability: d::Capability {
                name: "domain.mcp-source".into(),
                major: 1,
                instance_id: "native".into(),
            },
            execution_kind: d::ExecutionKind::Builtin,
        }
    }
    pub fn domain_mcp_with_hidden_projects(
        &self,
        query: McpQuery,
        hidden: &std::collections::HashSet<String>,
    ) -> NativeResult<McpSnapshot> {
        self.read_domain_mcp(query, hidden, consume::snapshot)
    }
    pub fn domain_mcp_facts(
        &self,
        query: McpQuery,
        hidden: &std::collections::HashSet<String>,
    ) -> NativeResult<m::McpFacts> {
        self.read_domain_mcp(query, hidden, |facts, _private| Ok(facts))
    }
    fn read_domain_mcp<T>(
        &self,
        query: McpQuery,
        hidden: &std::collections::HashSet<String>,
        use_result: impl FnOnce(m::McpFacts, McpSnapshot) -> NativeResult<T>,
    ) -> NativeResult<T> {
        let provider = self.domain_mcp_provider();
        let (revision, sources, project_ids) = {
            let state = self.lock()?;
            (
                state.revision,
                state.sources.clone(),
                normalize::project_ids(&state, hidden),
            )
        };
        let active = sources
            .iter()
            .map(|s| (s.agent_id.clone(), self.source_plugins.lease(&s.agent_id)))
            .collect::<Vec<_>>();
        let check = || {
            let state = self.lock()?;
            if state.revision != revision
                || sources.iter().any(|before| {
                    state
                        .sources
                        .iter()
                        .find(|s| s.agent_id == before.agent_id)
                        .is_none_or(|s| {
                            s.enabled != before.enabled
                                || s.authorization_revision != before.authorization_revision
                        })
                })
                || active.iter().any(|(id, before)| match before {
                    Some(lease) => lease.ensure_active().is_err(),
                    None => self.source_plugins.lease(id).is_some(),
                })
            {
                return Err(NativeError::new(
                    "stale_snapshot",
                    "MCP 来源代次已变化，请重新读取",
                ));
            }
            Ok(())
        };
        let expected = m::McpRequest {
            invocation: d::Invocation {
                invocation_id: uuid::Uuid::new_v4().to_string(),
                provider_key: provider.provider_key,
                implementation_identity: provider.implementation_identity,
                epoch: 1u64.into(),
                grant_revision: revision.into(),
                catalog_revision: revision.into(),
                resource_ref: uuid::Uuid::new_v4().to_string(),
                deadline_ms: None,
                budget: d::Budget {
                    batch_items: 40,
                    max_records: u64::MAX.into(),
                    max_document_bytes: u64::MAX.into(),
                },
            },
            filter: (&query).into(),
        };
        // Hidden paths and configuration/native call brokers remain in the host.
        let broker = || self.read_native_mcp(query.clone(), hidden);
        let mut reader = NativeRead {
            broker: &broker,
            check: &check,
            expected: expected.clone(),
            legacy: None,
            error: None,
            project_ids,
        };
        let facts = reader.read_mcp(&expected).map_err(|_| {
            reader
                .error
                .take()
                .unwrap_or_else(|| NativeError::new("unavailable", "MCP 结果未通过领域契约核查"))
        })?;
        let private = reader
            .legacy
            .take()
            .ok_or_else(|| NativeError::new("unavailable", "MCP 来源未完成读取"))?;
        let result = use_result(facts, private)?;
        check()?;
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_mcp_contract_keeps_the_full_response_without_executing_discovered_commands() {
        let home = tempfile::tempdir().unwrap();
        let marker = home.path().join("must-not-exist");
        std::fs::write(home.path().join(".claude.json"), serde_json::to_vec(&serde_json::json!({
            "mcpServers": {"fixture":{"command":"/usr/bin/touch","args":[marker],"env":{"TOKEN":"private-fixture-token"}}}
        })).unwrap()).unwrap();
        let native = NativeSessions::new(home.path().to_owned(), &["claude".into()]);
        native.refresh().unwrap();
        let old = native
            .read_native_mcp(McpQuery::default(), &Default::default())
            .unwrap();
        let migrated = native
            .domain_mcp_with_hidden_projects(McpQuery::default(), &Default::default())
            .unwrap();
        let stable = |v: &McpSnapshot| {
            let mut v = serde_json::to_value(v).unwrap();
            v.as_object_mut().unwrap().remove("asOf");
            v
        };
        assert_eq!(stable(&old), stable(&migrated));
        let facts = normalize::snapshot(&migrated, &Default::default());
        assert_eq!(facts.total_executions.value(), migrated.total_calls as u64);
        assert_eq!(facts.catalogs.len(), migrated.services.len());
        for (fact, service) in facts.catalogs.iter().zip(&migrated.services) {
            assert_eq!(
                fact.execution_count.as_ref().map(d::Decimal::value),
                service.call_count.map(|n| n as u64)
            );
            assert_eq!(
                fact.tool_catalog_count.as_ref().map(d::Decimal::value),
                service.tool_catalog_count.map(|n| n as u64)
            );
        }
        assert!(
            !serde_json::to_string(&facts)
                .unwrap()
                .contains("private-fixture-token")
        );
        assert!(!marker.exists());
        let invalid = McpQuery {
            days: Some(1),
            ..Default::default()
        };
        let expected = native
            .read_native_mcp(invalid.clone(), &Default::default())
            .unwrap_err();
        let actual = native
            .domain_mcp_with_hidden_projects(invalid, &Default::default())
            .unwrap_err();
        assert_eq!(actual.code, expected.code);
        assert_eq!(actual.message, expected.message);
    }
}
