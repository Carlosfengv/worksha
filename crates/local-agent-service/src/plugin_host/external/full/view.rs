//! Immutable read views; metadata queries never start a worker or restore a resource grant.
use super::super::{Checked, Manager, Result, fail};
use super::{
    runtime::{self, Context, Job, Output},
    store::{Lease, Projection},
};
use local_agent_core::{domain as d, plugin::full as f};
use std::sync::Arc;

#[derive(Clone)]
pub(crate) struct SourceView {
    pub plugin_id: String,
    pub digest: String,
    pub provider: d::Provider,
    pub source: d::Source,
    pub products: Vec<d::Product>,
    pub persist_metadata: bool,
    pub hidden_projects: Vec<String>,
    signing_key: String,
    manifest: f::Manifest,
    lease: Option<Lease>,
    projection: Option<Projection>,
    problem: Option<String>,
}
impl SourceView {
    pub fn live_report_window(&self) -> Option<u32> {
        self.manifest.source.as_ref()?.max_age_seconds
    }
    pub fn vector(&self) -> String {
        serde_json::json!([
            self.digest,
            self.source.status,
            self.lease
                .as_ref()
                .map(|l| (l.epoch, l.revision, l.publisher_revision)),
            self.projection
                .as_ref()
                .map(|p| (&p.facts.source_revision, &p.facts.identity)),
            self.problem,
            self.hidden_projects
        ])
        .to_string()
    }
    pub fn snapshot(&self) -> Option<&d::facts::FactSnapshot> {
        (self.source.status.authorization == d::Authorization::Authorized)
            .then_some(self.projection.as_ref())
            .flatten()
            .map(|p| p.facts.as_ref())
    }
    pub fn state(&self) -> d::SourceState {
        let snapshot = self.snapshot();
        let authorized = self.source.status.authorization == d::Authorization::Authorized;
        d::SourceState {
            source_id: self.source.id.clone(),
            authorization: self.source.status.authorization.clone(),
            freshness: self.source.status.freshness.clone(),
            coverage: if !authorized || snapshot.is_none() {
                d::Coverage::Unavailable
            } else if self.source.status.freshness != d::Freshness::Current
                || self.problem.is_some()
                || snapshot.is_some_and(|s| s.coverage != d::Coverage::Complete)
            {
                d::Coverage::Partial
            } else {
                d::Coverage::Complete
            },
            problem: self.problem.clone().or_else(|| {
                if !authorized {
                    Some("permission_denied".into())
                } else if snapshot.is_none() {
                    Some("needs_refresh".into())
                } else {
                    None
                }
            }),
        }
    }
    pub fn capabilities(&self) -> Vec<d::CapabilityEntry> {
        let Some(source) = self.manifest.source.as_ref() else {
            return vec![];
        };
        self.manifest
            .capabilities
            .iter()
            .filter_map(|capability| f::Capability::parse(capability))
            .map(|capability| {
                use d::ReadOperation::*;
                let name = capability.name();
                let mut provider = self.provider.clone();
                provider.capability.name = name.into();
                provider.provider_key = format!("{}:{name}@1:{}", self.plugin_id, source.id);
                let operations = match capability {
                    f::Capability::AgentDirectory => vec![Describe],
                    f::Capability::SessionSource => vec![Scan, ReadConversation],
                    f::Capability::CallSource => vec![ScanCalls],
                    f::Capability::ProjectSource => vec![CollectProjectEvidence],
                    f::Capability::ActivitySource => vec![CollectActivity],
                    f::Capability::McpSource => vec![CatalogFacts, ExecutionFacts],
                };
                d::CapabilityEntry {
                    provider,
                    status: self.source.status.clone(),
                    operations,
                }
            })
            .collect()
    }
    pub fn sessions(
        &self,
        filter: &d::SessionFilter,
        after: Option<d::SessionOrder>,
        cutoff: Option<i64>,
        hidden: &[String],
    ) -> Result<(Vec<d::SessionItem>, d::SessionScopeSummary)> {
        let mut window = d::SessionWindow::new(&self.source.id, after);
        if let Some(snapshot) = self.snapshot() {
            for fact in &snapshot.records {
                if let Some(mut record) = session_record(fact) {
                    if record
                        .project_ref
                        .as_ref()
                        .is_some_and(|p| hidden.contains(p))
                    {
                        record.project_ref = None;
                    }
                    if filter.matches(&record, cutoff) {
                        window.observe(record).map_err(|_| fail("resource_limit"))?;
                    }
                }
            }
        }
        Ok(window.finish())
    }
    pub fn matches_session(
        &self,
        id: &str,
        filter: &d::SessionFilter,
        cutoff: Option<i64>,
    ) -> bool {
        self.snapshot().is_some_and(|s| {
            s.records
                .iter()
                .filter_map(session_record)
                .any(|s| s.id == id && filter.matches(&s, cutoff))
        })
    }
}
pub(super) fn session_record(fact: &d::facts::FactRecord) -> Option<d::SessionRecord> {
    if let d::facts::FactCandidate::Session(s) = &fact.value {
        Some(d::SessionRecord {
            id: fact.id.clone(),
            native_id: s.native_id.clone(),
            product_ids: s.product_ids.clone(),
            unknown_client: s.unknown_client,
            title: s.title.clone(),
            created_at: s.created_at.clone(),
            updated_at: s.updated_at.clone(),
            project_ref: s.project_ref.clone(),
            usage: s.usage.clone(),
            partial: s.partial,
        })
    } else {
        None
    }
}
impl Manager {
    pub(crate) fn full_source_views(&self) -> Result<Vec<SourceView>> {
        let statuses = self.full_list()?;
        let mut views = Vec::new();
        for status in statuses {
            let (lease, projection, hidden_projects) = {
                let host = self.full.lock().checked("unavailable")?;
                let selected = host.selection(&status.manifest.id)?;
                if selected.digest != status.digest {
                    continue;
                }
                let lease = host
                    .active
                    .get(&status.manifest.id)
                    .filter(|l| {
                        l.digest == status.digest
                            && l.epoch == status.activation_epoch.value()
                            && l.revision == status.grant_revision.value()
                    })
                    .cloned();
                let projection = host
                    .projections
                    .get(&status.manifest.id)
                    .filter(|p| Some(&p.lease) == lease.as_ref())
                    .cloned();
                let mut statement = host
                    .db()?
                    .prepare("SELECT id FROM full_hidden_projects ORDER BY id LIMIT 4097")
                    .checked("unavailable")?;
                let hidden = statement
                    .query_map([], |r| r.get::<_, String>(0))
                    .checked("unavailable")?
                    .collect::<std::result::Result<Vec<_>, _>>()
                    .checked("invalid_record")?;
                if hidden.len() > 4096 {
                    return Err(fail("resource_limit"));
                }
                (lease, projection, hidden)
            };
            let manifest = status.manifest;
            let source = manifest.source.as_ref().ok_or(fail("invalid_record"))?;
            let id = manifest.source_id().ok_or(fail("invalid_record"))?;
            let key = format!("{}:domain.session-source@1:{}", manifest.id, source.id);
            let provider = d::Provider {
                provider_key: key.clone(),
                plugin_id: manifest.id.clone(),
                implementation_version: manifest.version.clone(),
                implementation_identity: status.digest.clone(),
                capability: d::Capability {
                    name: "domain.session-source".into(),
                    major: 1,
                    instance_id: id.clone(),
                },
                execution_kind: d::ExecutionKind::WasmComponent,
            };
            let products = source
                .products
                .iter()
                .map(|p| d::Product {
                    id: format!("{}:{}", manifest.id, p.id),
                    display_name: p.display_name.clone(),
                    source_id: id.clone(),
                    config_id: None,
                })
                .collect::<Vec<_>>();
            let source = d::Source {
                id,
                provider_key: key,
                product_ids: products.iter().map(|p| p.id.clone()).collect(),
                status: d::ProviderStatus {
                    enabled: status.enabled,
                    authorization: if status.unified_authorized {
                        d::Authorization::Authorized
                    } else {
                        d::Authorization::Required
                    },
                    grant_revision: status.grant_revision,
                    runtime_active: status.runtime_active,
                    freshness: status.freshness,
                },
            };
            views.push(SourceView {
                plugin_id: manifest.id.clone(),
                digest: status.digest,
                provider,
                source,
                products,
                persist_metadata: status.persist_metadata,
                hidden_projects,
                signing_key: status.signing_key,
                manifest,
                lease,
                projection,
                problem: status.problem,
            });
        }
        Ok(views)
    }
    pub(crate) fn full_project_visibility(
        &self,
        input: &d::facts::query::ProjectVisibility,
    ) -> Result<()> {
        if input.project_id.len() > 320 || input.source_id.len() > 240 {
            return Err(fail("invalid_input"));
        }
        let view = self
            .full_source_views()?
            .into_iter()
            .find(|v| v.source.id == input.source_id)
            .ok_or(fail("not_found"))?;
        if !view.snapshot().is_some_and(|s| {
            s.records.iter().any(|r| {
                r.id == input.project_id && r.value.fact_kind() == d::facts::FactKind::Project
            })
        }) {
            return Err(fail("permission_denied"));
        }
        let lease = view.lease.as_ref().ok_or(fail("grant_revoked"))?;
        if self.full_publisher_revision(&lease.publisher)? != lease.publisher_revision
            || self.full_publisher_key(&lease.publisher)?.as_ref() != Some(&view.signing_key)
        {
            return Err(fail("grant_revoked"));
        }
        let host = self.full.lock().checked("unavailable")?;
        if host.active.get(&view.plugin_id) != Some(lease)
            || !host
                .grants
                .get(&view.plugin_id)
                .is_some_and(|g| g.unified && &g.lease == lease)
        {
            return Err(fail("grant_revoked"));
        }
        if input.hidden {
            let count: i64 = host
                .db()?
                .query_row("SELECT count(*) FROM full_hidden_projects", [], |r| {
                    r.get(0)
                })
                .checked("unavailable")?;
            if count >= 4096 && !view.hidden_projects.contains(&input.project_id) {
                return Err(fail("resource_limit"));
            }
            host.db()?
                .execute(
                    "INSERT OR IGNORE INTO full_hidden_projects VALUES(?1)",
                    [&input.project_id],
                )
                .checked("unavailable")?;
        } else {
            host.db()?
                .execute(
                    "DELETE FROM full_hidden_projects WHERE id=?1",
                    [&input.project_id],
                )
                .checked("unavailable")?;
        }
        Ok(())
    }
    pub(crate) async fn full_read_conversation(
        self: &Arc<Self>,
        view: &SourceView,
        request: &d::UnifiedConversationRequest,
    ) -> Result<d::ConversationPage> {
        let snapshot = view.snapshot().ok_or(fail("permission_denied"))?;
        let session = snapshot
            .records
            .iter()
            .find_map(|fact| {
                (fact.id == request.session_id)
                    .then(|| session_record(fact))
                    .flatten()
            })
            .ok_or(fail("not_found"))?;
        let expected = view.lease.clone().ok_or(fail("grant_revoked"))?;
        let source_revision = snapshot.source_revision.clone();
        let captured = view
            .projection
            .as_ref()
            .and_then(|p| p.captured_source.clone());
        let gate = self.gate(&view.plugin_id)?;
        let _gate = gate.try_lock_owned().checked("busy")?;
        let manager = self.clone();
        let query = f::PackageQuery {
            plugin_id: view.plugin_id.clone(),
            digest: view.digest.clone(),
        };
        let context = super::super::run_sync(move || {
            let (p, path) = manager.full_verified(&query)?;
            let (before, lease, grant) = {
                let host = manager.full.lock().checked("unavailable")?;
                (
                    host.selection(&query.plugin_id)?,
                    host.active
                        .get(&query.plugin_id)
                        .cloned()
                        .ok_or(fail("plugin_disabled"))?,
                    host.grants
                        .get(&query.plugin_id)
                        .cloned()
                        .ok_or(fail("permission_denied"))?,
                )
            };
            if lease != expected || !grant.unified {
                return Err(fail("grant_revoked"));
            }
            let mut c = Context {
                package: p,
                path,
                before,
                lease,
                preflight: false,
                id: uuid::Uuid::new_v4().to_string(),
                resource: uuid::Uuid::new_v4().to_string(),
                source: None,
            };
            manager.full_check(&c)?;
            c.source = Some(match captured {
                Some(source) if source.grant == grant => source,
                Some(_) => return Err(fail("grant_revoked")),
                None => runtime::load_source(&grant)?,
            });
            manager.full_check(&c)?;
            Ok(c)
        })
        .await?;
        let Output::Conversation(mut page) = self
            .full_run(
                context,
                Job::Conversation {
                    native_id: session.native_id,
                    cursor: request.cursor.clone(),
                    anchor: request.anchor.clone(),
                    source_revision,
                },
            )
            .await?
        else {
            return Err(fail("invalid_record"));
        };
        page.session_id = request.session_id.clone();
        for event in &mut page.events {
            event.id = d::facts::publication::object_id(
                &view.source.id,
                "conversation-event",
                &serde_json::to_string(&(&request.session_id, &event.native_id))
                    .checked("invalid_record")?,
            );
            event.turn_id = d::facts::publication::object_id(
                &view.source.id,
                "conversation-turn",
                &serde_json::to_string(&(&request.session_id, &event.turn_id))
                    .checked("invalid_record")?,
            );
            event.response_id = event.response_id.as_ref().and_then(|native| {
                snapshot.records.iter().find_map(|fact| match &fact.value {
                    d::facts::FactCandidate::Response(r)
                        if r.native_id == *native && r.session_ref == request.session_id =>
                    {
                        Some(fact.id.clone())
                    }
                    _ => None,
                })
            });
        }
        Ok(page)
    }
}
