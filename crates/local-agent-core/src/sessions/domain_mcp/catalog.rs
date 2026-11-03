//! Explicit native refresh publishes a bounded configuration observation through the common port.
//! Ordinary graph queries consume this cache without discovery, process launch or transcript I/O.
use super::*;
use crate::plugin::PluginLease;
use m::McpCatalogSource;
use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};

const MAX_AGE: Duration = Duration::from_secs(300);
const READ_BUDGET: Duration = Duration::from_secs(2);
type SourceBindings = BTreeMap<String, (u64, PluginLease)>;
#[cfg(test)]
mod tests;

struct Reader<'a> {
    expected: m::McpCatalogRequest,
    native: &'a NativeSessions,
    sources: Vec<super::super::SourceStatus>,
    files: Vec<super::super::mcp::ConfigFile>,
    projects: BTreeMap<String, String>,
    check: &'a dyn Fn() -> NativeResult<()>,
    deadline: Instant,
}
impl m::McpCatalogSource for Reader<'_> {
    fn read_catalog(&mut self, request: &m::McpCatalogRequest) -> d::Result<m::McpCatalogSnapshot> {
        if request != &self.expected {
            return Err(d::Failure::new(d::ErrorCode::StaleSnapshot));
        }
        let convert = |e: NativeError| super::super::domain_source::domain_failure(&e, false);
        (self.check)().map_err(convert)?;
        let (inventory, _) = self
            .native
            .mcp_configuration(&self.sources, &self.files, Some(self.deadline))
            .map_err(convert)?;
        (self.check)().map_err(convert)?;
        let snapshot = inventory.catalog(&request.source_ids, &self.projects);
        snapshot.validate(&request.source_ids)?;
        if snapshot.entries.len() as u64 > request.invocation.budget.max_records.value() {
            return Err(d::Failure::new(d::ErrorCode::ResourceLimit));
        }
        Ok(snapshot)
    }
}

struct Published {
    snapshot: m::McpCatalogSnapshot,
    bindings: SourceBindings,
    revision: String,
    checked_at: Instant,
}
#[derive(Default)]
pub(in crate::sessions) struct Cache {
    published: Option<Published>,
    problem: bool,
}
pub(in crate::sessions) struct View {
    pub entries: Vec<m::McpConfiguration>,
    pub observed_at: String,
    pub revision: String,
    pub coverage: d::Coverage,
}
impl Cache {
    fn publish(&mut self, mut snapshot: m::McpCatalogSnapshot, bindings: SourceBindings) {
        // Never replace a previously complete source with a partial prefix.
        // Retention is valid only within the exact same active authorization lease.
        if let Some(old) = &self.published {
            for source in &snapshot.partial_sources {
                if old.bindings.get(source).is_some_and(|(revision, lease)| {
                    lease.ensure_active().is_ok()
                        && bindings.get(source).is_some_and(|(r, _)| r == revision)
                }) {
                    snapshot.entries.retain(|e| &e.source_id != source);
                    snapshot.entries.extend(
                        old.snapshot
                            .entries
                            .iter()
                            .filter(|e| &e.source_id == source)
                            .cloned(),
                    );
                    snapshot.observed_at =
                        snapshot.observed_at.min(old.snapshot.observed_at.clone());
                }
            }
        }
        snapshot
            .entries
            .sort_by(|a, b| (&a.source_id, &a.id).cmp(&(&b.source_id, &b.id)));
        if snapshot.entries.len() > 4096 {
            self.problem = true;
            return;
        }
        let unchanged = self.published.as_ref().is_some_and(|old| {
            old.snapshot.entries == snapshot.entries
                && old.snapshot.configuration_revision == snapshot.configuration_revision
                && old.snapshot.partial_sources == snapshot.partial_sources
                && old.bindings.len() == bindings.len()
                && old.bindings.iter().all(|(source, (revision, lease))| {
                    lease.ensure_active().is_ok()
                        && bindings
                            .get(source)
                            .is_some_and(|(current, _)| current == revision)
                })
        });
        let revision = if unchanged {
            let old = self.published.as_ref().unwrap();
            snapshot.observed_at = old.snapshot.observed_at.clone();
            old.revision.clone()
        } else {
            uuid::Uuid::new_v4().to_string()
        };
        self.published = Some(Published {
            snapshot,
            bindings,
            revision,
            checked_at: Instant::now(),
        });
        self.problem = false;
    }
    pub(in crate::sessions) fn view(&self, source: &str, authorization: u64) -> Option<View> {
        let published = self.published.as_ref()?;
        let (revision, lease) = published.bindings.get(source)?;
        if *revision != authorization || lease.ensure_active().is_err() {
            return None;
        }
        Some(View {
            entries: published
                .snapshot
                .entries
                .iter()
                .filter(|e| e.source_id == source)
                .cloned()
                .collect(),
            observed_at: published.snapshot.observed_at.clone(),
            revision: published.revision.clone(),
            coverage: if self.problem
                || published.checked_at.elapsed() >= MAX_AGE
                || published
                    .snapshot
                    .partial_sources
                    .iter()
                    .any(|s| s == source)
            {
                d::Coverage::Partial
            } else {
                d::Coverage::Complete
            },
        })
    }
    fn vector(&self) -> String {
        serde_json::json!([
            self.problem,
            self.published.as_ref().map(|p| (
                &p.revision,
                p.checked_at.elapsed() >= MAX_AGE,
                p.bindings
                    .iter()
                    .map(|(s, (r, l))| (s, r, l.ensure_active().is_ok()))
                    .collect::<Vec<_>>()
            )),
        ])
        .to_string()
    }
}
impl NativeSessions {
    pub fn domain_fact_catalog_revision(&self) -> NativeResult<String> {
        Ok(match self.mcp_catalog.lock() {
            Ok(cache) => cache.vector(),
            Err(_) => "unavailable".into(),
        })
    }
    pub(in crate::sessions) fn refresh_domain_mcp_catalog(&self, proceed: &dyn Fn() -> bool) {
        let result = self.collect_domain_mcp_catalog(proceed);
        if let Ok(mut cache) = self.mcp_catalog.lock() {
            match result {
                Ok((snapshot, bindings)) => cache.publish(snapshot, bindings),
                Err(_) => cache.problem = true,
            }
        }
    }
    fn collect_domain_mcp_catalog(
        &self,
        proceed: &dyn Fn() -> bool,
    ) -> NativeResult<(m::McpCatalogSnapshot, SourceBindings)> {
        let (sources, files, projects, revision) = {
            let state = self.lock()?;
            let sources = state
                .sources
                .iter()
                .filter(|s| s.enabled && self.source_active(&s.agent_id))
                .cloned()
                .collect::<Vec<_>>();
            let files = super::super::unique_files(&state)
                .into_iter()
                .filter(|f| sources.iter().any(|s| s.agent_id == f.agent))
                .map(super::super::mcp::ConfigFile::from)
                .collect();
            (
                sources,
                files,
                normalize::project_ids(&state, &Default::default()),
                state.revision,
            )
        };
        let bindings = sources
            .iter()
            .map(|s| {
                let lease = self
                    .source_plugins
                    .lease(&s.agent_id)
                    .ok_or_else(|| NativeError::new("stale_snapshot", "MCP 来源已停用"))?;
                Ok((s.agent_id.clone(), (s.authorization_revision, lease)))
            })
            .collect::<NativeResult<BTreeMap<_, _>>>()?;
        let check = || {
            let state = self.lock()?;
            if !proceed()
                || state.revision != revision
                || bindings.iter().any(|(id, (revision, lease))| {
                    lease.ensure_active().is_err()
                        || !state.sources.iter().any(|s| {
                            s.agent_id == *id && s.enabled && s.authorization_revision == *revision
                        })
                })
            {
                return Err(NativeError::new("stale_snapshot", "MCP 目录读取范围已变化"));
            }
            Ok(())
        };
        let provider = self.domain_mcp_provider();
        let expected = m::McpCatalogRequest {
            invocation: d::Invocation {
                invocation_id: uuid::Uuid::new_v4().to_string(),
                provider_key: provider.provider_key,
                implementation_identity: provider.implementation_identity,
                epoch: 1u64.into(),
                grant_revision: revision.into(),
                catalog_revision: revision.into(),
                resource_ref: uuid::Uuid::new_v4().to_string(),
                deadline_ms: Some(2000u64.into()),
                budget: d::Budget {
                    batch_items: 40,
                    max_records: 4096u64.into(),
                    max_document_bytes: (6u64 * 1024 * 1024).into(),
                },
            },
            source_ids: sources.iter().map(|s| s.agent_id.clone()).collect(),
        };
        let mut reader = Reader {
            expected: expected.clone(),
            native: self,
            sources,
            files,
            projects,
            check: &check,
            deadline: Instant::now() + READ_BUDGET,
        };
        let snapshot = reader
            .read_catalog(&expected)
            .map_err(|_| NativeError::new("unavailable", "MCP 目录未完成领域核查"))?;
        check()?;
        Ok((snapshot, bindings))
    }
}
