//! Read existing native indexes as a bounded fact window; never rescan or clone call history.
use super::{CallSummary, NativeError, NativeResult, NativeSessions};
use crate::domain::{
    self as d,
    facts::{
        query::{FactOrder, FactQuery, FactWindow},
        *,
    },
};
use std::{
    cmp::Reverse,
    collections::{BTreeSet, HashSet},
    path::Path,
    sync::Arc,
};

/// Immutable positions built once with a native file generation. Query lookups
/// borrow the original calls instead of rebuilding two history-sized hash maps.
#[derive(Clone)]
pub(super) struct CallIndex(Arc<IndexedCalls>);
struct IndexedCalls {
    ids: Box<[usize]>,
    models: Box<[usize]>,
    mcp: Box<[usize]>,
    mcp_count: usize,
    model_usage: d::Result<UnitUsage>,
}
impl CallIndex {
    pub(super) fn new(calls: &[CallSummary]) -> Self {
        let mut positions: Vec<_> = (0..calls.len()).collect();
        positions.sort_unstable_by(|a, b| calls[*a].id.cmp(&calls[*b].id).then(a.cmp(b)));
        let (models, _) = ordered(calls, "model");
        let (mcp, mcp_count) = ordered(calls, "mcp");
        let model_usage =
            UnitUsage::model_responses(calls.iter().filter(|c| c.kind == "model").map(|c| {
                c.usage
                    .as_ref()
                    .map(Into::into)
                    .unwrap_or_else(unknown_usage)
            }));
        Self(Arc::new(IndexedCalls {
            ids: positions.into(),
            models,
            mcp,
            mcp_count,
            model_usage,
        }))
    }
    fn end(&self, calls: &[CallSummary], id: &str) -> usize {
        self.0.ids.partition_point(|n| calls[*n].id.as_str() <= id)
    }
    fn call<'a>(&self, calls: &'a [CallSummary], id: &str) -> Option<&'a CallSummary> {
        self.end(calls, id)
            .checked_sub(1)
            .map(|n| &calls[self.0.ids[n]])
            .filter(|c| c.id == id)
    }
    fn response(&self, calls: &[CallSummary], id: &str) -> bool {
        self.0.ids[..self.end(calls, id)]
            .iter()
            .rev()
            .map(|n| &calls[*n])
            .take_while(|c| c.id == id)
            .any(|c| c.kind == "model")
    }
    fn window<'a>(
        &'a self,
        calls: &'a [CallSummary],
        source: &str,
        kind: Option<FactKind>,
        after: Option<&FactOrder>,
    ) -> impl Iterator<Item = &'a CallSummary> {
        let positions: &[usize] = match kind {
            Some(FactKind::Response) => &self.0.models,
            Some(FactKind::McpExecution) => &self.0.mcp,
            _ => &[],
        };
        let start = after.map_or(0, |after| {
            positions.partition_point(|n| {
                let c = &calls[*n];
                after
                    .cmp_metadata(source, &c.id, c.timestamp.as_deref())
                    .is_ge()
            })
        });
        positions[start..]
            .iter()
            .take(d::PAGE_SIZE + 1)
            .map(|n| &calls[*n])
    }
}
fn ordered(calls: &[CallSummary], kind: &str) -> (Box<[usize]>, usize) {
    let mut rows = calls
        .iter()
        .enumerate()
        .filter(|(_, c)| c.kind == kind)
        .map(|(n, c)| {
            (
                (
                    c.timestamp.as_deref().and_then(d::timestamp).map(Reverse),
                    c.id.as_str(),
                ),
                n,
            )
        })
        .collect::<Vec<_>>();
    let count = rows.len();
    rows.sort_unstable_by(|a, b| {
        (a.0.0.is_none(), &a.0, Reverse(a.1)).cmp(&(b.0.0.is_none(), &b.0, Reverse(b.1)))
    });
    // A page contains unique order keys. Match FactWindow's last-insertion wins
    // behavior before taking the lookahead, including duplicate native IDs.
    rows.dedup_by(|a, b| a.0 == b.0);
    (
        rows.into_iter()
            .map(|(_, n)| n)
            .collect::<Vec<_>>()
            .into_boxed_slice(),
        count,
    )
}

pub const NATIVE_FACT_KINDS: [FactKind; 7] = [
    FactKind::Project,
    FactKind::Session,
    FactKind::Response,
    FactKind::Activity,
    FactKind::Write,
    FactKind::McpCatalog,
    FactKind::McpExecution,
];
fn error(_: d::Failure) -> NativeError {
    NativeError::new("resource_limit", "领域事实超出统计预算")
}
fn outcome(status: &str) -> Outcome {
    match status {
        "success" | "completed" => Outcome::Success,
        "error" | "failed" => Outcome::Failure,
        _ => Outcome::Unknown,
    }
}
impl NativeSessions {
    pub fn domain_facts_for_source(
        &self,
        source: &str,
        query: &FactQuery,
        after: Option<FactOrder>,
        hidden: &[String],
        coverage: d::Coverage,
    ) -> NativeResult<(Vec<FactRecord>, FactSummary)> {
        query
            .validate()
            .map_err(|_| NativeError::new("invalid_input", "领域筛选无效"))?;
        let provider = self
            .source_plugins
            .sources
            .get(source)
            .and_then(|p| p.domain_provider())
            .ok_or_else(|| NativeError::new("unsupported", "来源尚未迁移"))?;
        let state = self.lock()?;
        let authorization = state
            .sources
            .iter()
            .find(|s| s.agent_id == source && s.enabled)
            .filter(|_| self.source_active(source))
            .ok_or_else(|| NativeError::new("permission_denied", "来源尚未授权"))?;
        let catalog = self
            .mcp_catalog
            .lock()
            .ok()
            .and_then(|cache| cache.view(source, authorization.authorization_revision));
        let identity = FactIdentity {
            provider_key: provider.provider_key,
            source_id: source.into(),
            implementation_identity: provider.implementation_identity,
            epoch: authorization.authorization_revision.into(),
            grant_revision: authorization.authorization_revision.into(),
            observed_at: state.as_of.clone(),
            evidence: EvidenceLevel::BuiltinNative,
        };
        let revision = state.revision.to_string();
        let files = super::unique_files(&state)
            .into_iter()
            .filter(|f| f.agent == source)
            .collect::<Vec<_>>();
        let coverage = if coverage == d::Coverage::Complete
            && (files.iter().any(|f| f.summary.partial)
                || catalog
                    .as_ref()
                    .is_none_or(|c| c.coverage != d::Coverage::Complete))
        {
            d::Coverage::Partial
        } else {
            coverage
        };
        let mut window = FactWindow::new(after.clone());
        let available = NATIVE_FACT_KINDS
            .into_iter()
            .filter(|k| *k != FactKind::McpCatalog || catalog.is_some())
            .filter(|k| *k != FactKind::Activity || matches!(source, "claude" | "codex"))
            .collect::<Vec<_>>();
        let mut tally = FactTally::new(source, &available, coverage).map_err(error)?;
        // Current configuration does not establish a historical session/response association.
        // Keep scoped catalog totals empty unless there is actual relationship evidence.
        if query.project_id.is_none()
            && query.session_id.is_none()
            && query.response_id.is_none()
            && let Some(catalog) = catalog
        {
            let provider = self.domain_mcp_provider();
            let identity = FactIdentity {
                provider_key: provider.provider_key,
                implementation_identity: provider.implementation_identity,
                observed_at: catalog.observed_at,
                ..identity.clone()
            };
            for entry in catalog
                .entries
                .into_iter()
                .filter(|e| e.project_ref.as_ref().is_none_or(|id| !hidden.contains(id)))
            {
                let value = FactCandidate::McpCatalog(McpCatalogCandidate {
                    native_id: entry.id.clone(),
                    server_name: entry.name,
                    tool_names: None,
                });
                tally.observe(&value).map_err(error)?;
                if query.kind == FactKind::McpCatalog {
                    window.observe(FactRecord {
                        id: format!("{source}:mcp-config:{}", entry.id),
                        identity: identity.clone(),
                        source_revision: catalog.revision.clone(),
                        value,
                    });
                }
            }
        }
        let mut projects = BTreeSet::new();
        for file in files {
            if file.fact_calls.0.ids.len() != file.calls.len() {
                return Err(NativeError::new(
                    "unavailable",
                    "调用事实索引已变化，请重新读取",
                ));
            }
            let s = &file.summary;
            if query.session_id.as_ref().is_some_and(|id| id != &s.id)
                || query
                    .project_id
                    .as_ref()
                    .is_some_and(|id| hidden.contains(id) || s.project_id.as_ref() != Some(id))
                || query
                    .response_id
                    .as_ref()
                    .is_some_and(|id| !file.fact_calls.response(&file.calls, id))
            {
                continue;
            }
            let scoped_response = query.response_id.is_some();
            if !scoped_response {
                tally
                    .merge_responses(
                        file.fact_calls
                            .0
                            .model_usage
                            .as_ref()
                            .map_err(|failure| error(failure.clone()))?,
                    )
                    .map_err(error)?;
                tally
                    .observe_mcp_executions(file.fact_calls.0.mcp_count)
                    .map_err(error)?;
            }
            let mut emit = |id: String, value: FactCandidate| -> NativeResult<()> {
                // Unscoped call totals come from the immutable generation;
                // selected response graphs still count their exact filtered rows.
                if scoped_response
                    || !matches!(
                        value,
                        FactCandidate::Response(_) | FactCandidate::McpExecution(_)
                    )
                {
                    tally.observe(&value).map_err(error)?;
                }
                if value.fact_kind() == query.kind {
                    window.observe(FactRecord {
                        id,
                        identity: identity.clone(),
                        source_revision: revision.clone(),
                        value,
                    });
                }
                Ok(())
            };
            let project = s.project_id.as_ref().filter(|p| !hidden.contains(p));
            if let Some(id) = project
                && projects.insert(id.clone())
            {
                emit(
                    id.clone(),
                    FactCandidate::Project(ProjectCandidate {
                        native_id: id.clone(),
                        name: s
                            .project_name
                            .clone()
                            .unwrap_or_else(|| "项目名称未知".into()),
                        path_hint: s.project_root.clone().or_else(|| s.project_path.clone()),
                    }),
                )?;
            }
            emit(
                s.id.clone(),
                FactCandidate::Session(SessionCandidate {
                    native_id: s.native_id.clone(),
                    product_ids: s.agent_products.clone(),
                    unknown_client: s.has_unknown_client,
                    title: s.title.clone(),
                    created_at: s.created_at.clone(),
                    updated_at: s.updated_at.clone(),
                    project_ref: project.cloned(),
                    usage: (&s.usage).into(),
                    partial: s.partial,
                }),
            )?;
            let scoped_calls: &[CallSummary] = if scoped_response { &file.calls } else { &[] };
            for call in scoped_calls.iter().chain(file.fact_calls.window(
                &file.calls,
                source,
                (!scoped_response).then_some(query.kind),
                after.as_ref(),
            )) {
                if call.kind == "model"
                    && query.response_id.as_ref().is_none_or(|id| id == &call.id)
                {
                    emit(
                        call.id.clone(),
                        FactCandidate::Response(ResponseCandidate {
                            native_id: call.id.clone(),
                            session_ref: s.id.clone(),
                            product_id: call.agent_product_id.clone(),
                            occurred_at: call.timestamp.clone(),
                            model: call.model.clone(),
                            outcome: outcome(&call.status),
                            usage: call
                                .usage
                                .as_ref()
                                .map(Into::into)
                                .unwrap_or_else(unknown_usage),
                            reported_response_id: call.response_id.clone(),
                            duration_ms: call.duration_ms.map(Into::into),
                        }),
                    )?;
                }
                if call.kind == "mcp" {
                    let response = call
                        .model_response_id
                        .as_ref()
                        .filter(|r| file.fact_calls.response(&file.calls, r));
                    if query
                        .response_id
                        .as_ref()
                        .is_none_or(|id| response == Some(id))
                    {
                        emit(
                            call.id.clone(),
                            FactCandidate::McpExecution(McpExecutionCandidate {
                                native_id: call.id.clone(),
                                server_ref: None,
                                session_ref: Some(s.id.clone()),
                                response_ref: response.cloned(),
                                tool_name: call
                                    .tool_name
                                    .clone()
                                    .unwrap_or_else(|| call.name.clone()),
                                occurred_at: call.timestamp.clone(),
                                duration_ms: call.duration_ms.map(Into::into),
                                outcome: outcome(&call.status),
                            }),
                        )?;
                    }
                }
            }
            if query.response_id.is_none() && query.kind == FactKind::Activity {
                let mut activities = HashSet::new();
                for activity in &file.activity_messages {
                    let id = super::live::message_key(activity);
                    if activity.session_id == s.id && activities.insert(id.clone()) {
                        emit(
                            id.clone(),
                            FactCandidate::Activity(ActivityCandidate {
                                native_id: id,
                                session_ref: s.id.clone(),
                                response_ref: None,
                                occurred_at: Some(activity.event_at.clone()),
                                phase: match activity.phase.as_str() {
                                    "thinking" => ActivityPhase::Thinking,
                                    "generating" => ActivityPhase::Generating,
                                    "tool" => ActivityPhase::Tool,
                                    "idle" => ActivityPhase::Idle,
                                    _ => ActivityPhase::Unknown,
                                },
                                duration_ms: None,
                            }),
                        )?;
                    }
                }
            }
            if let Some(project_id) = project {
                let root = s.project_root.as_deref().or(s.project_path.as_deref());
                for write in &file.writes {
                    if &write.project_id != project_id || write.session_id != s.id {
                        continue;
                    }
                    // FileWrite.id is the exact confirmed tool event ID;
                    // call_id is its original source-local tool identifier.
                    let response = file
                        .fact_calls
                        .call(&file.calls, &write.id)
                        .and_then(|c| c.model_response_id.as_ref())
                        .filter(|r| file.fact_calls.response(&file.calls, r));
                    if query
                        .response_id
                        .as_ref()
                        .is_some_and(|id| response != Some(id))
                    {
                        continue;
                    }
                    for (n, file) in write.files.iter().enumerate() {
                        let Some(relative) = root
                            .and_then(|root| Path::new(&file.path).strip_prefix(root).ok())
                            .and_then(Path::to_str)
                            .filter(|p| !p.is_empty() && !p.split('/').any(|p| p == ".."))
                        else {
                            continue;
                        };
                        let id = format!("{}:{n}", write.id);
                        emit(
                            id.clone(),
                            FactCandidate::Write(WriteCandidate {
                                native_id: id,
                                session_ref: s.id.clone(),
                                project_ref: project_id.clone(),
                                response_ref: response.cloned(),
                                relative_path: relative.into(),
                                occurred_at: write.occurred_at.clone(),
                                outcome: Outcome::Success,
                            }),
                        )?;
                    }
                }
            }
        }
        let mut summary = tally.finish();
        // The existing native message projection keeps at most 128 recent
        // messages per file, and may retain only the latest older message.
        // Only an activity query can describe this bounded observation count.
        // Other selections leave its full-history total unknown, without
        // lowering complete coverage for an independent selected fact kind.
        if matches!(source, "claude" | "codex") {
            if query.kind == FactKind::Activity {
                if summary.coverage == d::Coverage::Complete {
                    summary.coverage = d::Coverage::Partial;
                }
            } else {
                summary.activities = None;
            }
        }
        Ok((window.candidates(), summary))
    }
    pub fn supports_domain_fact(kind: FactKind) -> bool {
        NATIVE_FACT_KINDS.contains(&kind)
    }
}
fn unknown_usage() -> d::Usage {
    d::Usage {
        input_tokens: None,
        output_tokens: None,
        cached_input_tokens: None,
        cache_write_input_tokens: None,
        reasoning_output_tokens: None,
        total_tokens: None,
    }
}
