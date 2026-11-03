//! Read-only native transcript adapters. Complete parsed bodies stay in memory;
//! bounded, redacted activity summaries use a separate private cache.
mod activity_history;
#[cfg(test)]
mod activity_history_tests;
pub mod activity_input;
pub mod activity_input_cache;
pub mod activity_performance;
mod antigravity;
mod antigravity_proto;
#[cfg(test)]
mod antigravity_tests;
mod calls;
mod codex_attribution;
#[cfg(test)]
mod codex_attribution_tests;
#[cfg(any(target_os = "macos", test))]
mod codex_runtime;
mod context;
mod domain_activity;
mod domain_catalog;
#[cfg(test)]
mod domain_fact_index_tests;
mod domain_facts;
mod domain_mcp;
mod domain_memory;
mod domain_object_summary;
mod domain_query;
mod domain_source;
mod failure;
mod jsonl;
pub mod live;
#[cfg(test)]
mod live_review_tests;
mod mcp;
mod mcp_metrics;
#[cfg(test)]
mod mcp_tests;
mod memory;
#[cfg(test)]
mod memory_context_tests;
mod opencode;
mod opencode_live;
mod opencode_log;
mod opencode_mcp;
mod opencode_messages;
#[cfg(test)]
mod opencode_tests;
mod parser;
#[cfg(test)]
mod scan_bench_tests;
mod source_plugins;
#[cfg(test)]
mod tests;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::UNIX_EPOCH,
};

pub use calls::CallCatalog;
pub(crate) use calls::CallSnapshotIdentity;
pub use context::{ContextUsage, SessionContext};
pub use domain_activity::Scope as ActivityAuthorization;
pub use failure::ModelFailure;
pub use mcp::{McpQuery, McpSnapshot};
pub use memory::MemorySnapshot;
pub use opencode_mcp::{install_reader_worker, read_config_worker};
pub use parser::{Event, SessionSummary, Usage};
pub use source_plugins::NativeSourcePlugins;

pub fn now() -> String {
    Utc::now().to_rfc3339()
}
pub fn sanitize_label(value: &str, limit: usize) -> String {
    parser::safe_text(value, limit).0
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeError {
    pub code: &'static str,
    pub message: String,
}
impl NativeError {
    pub(crate) fn new(code: &'static str, message: &str) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}
pub type NativeResult<T> = Result<T, NativeError>;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceStatus {
    pub authorization_revision: u64,
    pub client_attribution: Option<codex_attribution::AttributionStatus>,
    pub agent_id: String,
    pub enabled: bool,
    pub locations: Vec<String>,
    pub status: String,
    pub session_count: usize,
    pub scanned_files: usize,
    pub total_files: usize,
    pub last_scan: Option<String>,
    pub last_successful_scan: Option<String>,
    pub error: Option<String>,
    pub versions: Vec<String>,
    pub capabilities: Vec<Capability>,
    pub skipped_files: usize,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Capability {
    pub name: &'static str,
    pub status: &'static str,
    pub reason: &'static str,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionQuery {
    pub agent: Option<String>,
    pub query: Option<String>,
    pub days: Option<u16>,
    pub status: Option<String>,
    pub cursor: Option<String>,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CallQuery {
    pub agent: Option<String>,
    pub kind: Option<String>,
    pub project: Option<String>,
    pub days: Option<u16>,
    pub cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CallSummary {
    pub response_id: Option<String>,
    #[serde(skip)]
    pub(crate) native_agent_product_id: Option<String>,
    pub agent_attribution: Option<&'static str>,
    pub agent_product_id: Option<String>,
    pub id: String,
    pub session_id: String,
    pub session_title: String,
    pub agent_id: String,
    pub turn_id: String,
    pub kind: String,
    pub name: String,
    pub status: String,
    pub timestamp: Option<String>,
    pub model: Option<String>,
    pub provider: Option<String>,
    pub model_response_id: Option<String>,
    pub usage: Option<Usage>,
    pub server: Option<String>,
    pub tool_name: Option<String>,
    pub duration_ms: Option<u64>,
    pub tool_source: Option<opencode_mcp::ToolSource>,
    pub failure: Option<ModelFailure>,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Page<T: Serialize, Q: Serialize> {
    #[serde(skip)]
    pub selected_session_ids: Vec<String>,
    pub items: Vec<T>,
    pub total: usize,
    pub next_cursor: Option<String>,
    pub as_of: String,
    pub coverage: &'static str,
    pub applied_filter: Q,
    pub missing_counts: usize,
    pub summary: Option<SessionTotals>,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionTotals {
    pub usage_scope: &'static str,
    pub model_calls: usize,
    pub tool_calls: usize,
    pub mcp_calls: usize,
    pub skill_calls: usize,
    pub usage: Usage,
    pub usage_unknown_sessions: usize,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Conversation {
    pub context: SessionContext,
    pub session: SessionSummary,
    pub items: Vec<Event>,
    pub total: usize,
    pub next_cursor: Option<String>,
    pub as_of: String,
    pub coverage: &'static str,
    pub warnings: Vec<String>,
}

#[derive(Clone)]
struct IndexedFile {
    fact_calls: domain_facts::CallIndex,
    activity_messages: Vec<live::NativeActivity>,
    context: ContextUsage,
    native_products: Vec<String>,
    native_unknown_non_calls: bool,
    agent: String,
    path: PathBuf,
    stamp: (u64, u128, u64, u64, u128),
    database_revision: Option<String>,
    summary: SessionSummary,
    calls: Arc<Vec<CallSummary>>,
    writes: Vec<crate::projects::writes::FileWrite>,
}
struct Catalog {
    sources: Vec<SourceStatus>,
    files: HashMap<PathBuf, IndexedFile>,
    revision: u64,
    as_of: String,
}
pub struct NativeSessions {
    source_plugins: NativeSourcePlugins,
    index_cache: Mutex<parser::IndexCache>,
    codex_evidence: Mutex<codex_attribution::EvidenceCache>,
    mcp_catalog: Mutex<domain_mcp::catalog::Cache>,
    home: PathBuf,
    catalog: Mutex<Catalog>,
    scanning: AtomicBool,
}

impl NativeSessions {
    pub fn home(&self) -> &Path {
        &self.home
    }
    pub(super) fn validate_agent(&self, agent: Option<&str>) -> NativeResult<()> {
        if agent.is_some_and(|id| !self.directory().valid_filter(id)) {
            return Err(NativeError::new("invalid_input", "未知 Agent"));
        }
        Ok(())
    }
    pub fn usage_status(&self, agent: &str) -> NativeResult<(String, String)> {
        self.validate_agent(Some(agent))?;
        let state = self.lock()?;
        let family = self.directory().family(agent);
        let status = if agent == "unknown" && state.sources.iter().any(|s| s.enabled) {
            "partial"
        } else {
            state
                .sources
                .iter()
                .find(|s| Some(s.agent_id.as_str()) == family)
                .map(|s| {
                    if s.enabled {
                        s.status.as_str()
                    } else {
                        "disabled"
                    }
                })
                .unwrap_or("disabled")
        };
        Ok((state.as_of.clone(), status.into()))
    }
    pub fn usage_records(
        &self,
        agent: &str,
    ) -> NativeResult<(Vec<crate::model_usage::UsageRecord>, String, String)> {
        self.validate_agent(Some(agent))?;
        let state = self.lock()?;
        let family = self.directory().family(agent);
        let source = state
            .sources
            .iter()
            .find(|source| Some(source.agent_id.as_str()) == family);
        let status = source
            .map(|source| {
                if source.enabled {
                    source.status.as_str()
                } else {
                    "disabled"
                }
            })
            .unwrap_or("disabled");
        let status = if agent == "unknown" && state.sources.iter().any(|s| s.enabled) {
            "partial"
        } else {
            status
        };
        let rows = unique_files(&state)
            .into_iter()
            .filter(|file| {
                (agent == "unknown" || Some(file.agent.as_str()) == family)
                    && state
                        .sources
                        .iter()
                        .any(|s| s.enabled && s.agent_id == file.agent)
            })
            .flat_map(|file| {
                file.calls
                    .iter()
                    .filter(|call| {
                        call.kind == "model"
                            && self.directory().matches(
                                Some(agent),
                                &call.agent_id,
                                call.agent_product_id.as_deref(),
                            )
                    })
                    .map(usage_record)
            })
            .collect();
        Ok((rows, state.as_of.clone(), status.into()))
    }
    pub fn project_catalog(&self) -> NativeResult<crate::projects::ProjectCatalog> {
        let state = self.lock()?;
        Ok(self.project_catalog_locked(&state))
    }
    fn project_catalog_locked(&self, state: &Catalog) -> crate::projects::ProjectCatalog {
        let files = unique_files(state);
        crate::projects::ProjectCatalog {
            sessions: files.iter().map(|f| f.summary.clone()).collect(),
            writes: files.iter().flat_map(|f| f.writes.clone()).collect(),
            calls: CallCatalog::shared(files.iter().map(|f| f.calls.clone()))
                .with_directory(self.source_plugins.directory.clone()),
            revision: state.revision,
            as_of: state.as_of.clone(),
            coverage: coverage(state).into(),
        }
    }
    pub fn revision(&self) -> NativeResult<u64> {
        Ok(self.lock()?.revision)
    }
    pub fn catalog_status(&self) -> NativeResult<(u64, String)> {
        let s = self.lock()?;
        Ok((s.revision, s.as_of.clone()))
    }
    pub fn analysis_rows(&self) -> NativeResult<Vec<crate::observation::AnalysisRow>> {
        let state = self.lock()?;
        Ok(unique_files(&state)
            .into_iter()
            .flat_map(|f| {
                f.calls.iter().filter(|c| c.kind == "model").map(|c| {
                    crate::observation::AnalysisRow {
                        cost: crate::observation::AnalysisCost {
                            unknown_attempts: 1,
                            ..Default::default()
                        },
                        agent: self
                            .directory()
                            .group(&c.agent_id, c.agent_product_id.as_deref()),
                        model: c.model.clone(),
                        time: c.timestamp.clone(),
                        tokens: c.usage.as_ref().and_then(|u| u.total_tokens),
                        provisional: false,
                        partial: f.summary.partial,
                    }
                })
            })
            .collect())
    }
    pub fn new(home: PathBuf, enabled: &[String]) -> Self {
        // Fixed built-in descriptors and preparation have no source I/O or user input.
        let plugins = NativeSourcePlugins::builtin().expect("valid built-in source descriptors");
        Self::with_source_plugins(home, enabled, plugins)
    }

    pub fn with_source_plugins(
        home: PathBuf,
        enabled: &[String],
        plugins: NativeSourcePlugins,
    ) -> Self {
        let sources = plugins
            .directory
            .sources()
            .iter()
            .map(|descriptor| descriptor.id.as_str())
            .map(|agent| SourceStatus {
                authorization_revision: 1,
                client_attribution: None,
                agent_id: agent.into(),
                enabled: enabled.iter().any(|s| s == agent),
                locations: plugins
                    .sources
                    .get(agent)
                    .map(|p| p.locations(&home))
                    .unwrap_or_default()
                    .iter()
                    .map(|p| p.to_string_lossy().into())
                    .collect(),
                status: if enabled.iter().any(|s| s == agent) {
                    "pending"
                } else {
                    "disabled"
                }
                .into(),
                session_count: 0,
                scanned_files: 0,
                total_files: 0,
                last_scan: None,
                last_successful_scan: None,
                error: None,
                versions: vec![],
                skipped_files: 0,
                capabilities: plugins
                    .sources
                    .get(agent)
                    .map(|p| p.capabilities())
                    .unwrap_or_default(),
            })
            .collect();
        Self {
            source_plugins: plugins,
            index_cache: Mutex::new(parser::IndexCache::default()),
            codex_evidence: Mutex::new(codex_attribution::EvidenceCache::default()),
            mcp_catalog: Mutex::new(domain_mcp::catalog::Cache::default()),
            home,
            catalog: Mutex::new(Catalog {
                sources,
                files: HashMap::new(),
                revision: 1,
                as_of: now(),
            }),
            scanning: AtomicBool::new(false),
        }
    }

    fn lock(&self) -> NativeResult<std::sync::MutexGuard<'_, Catalog>> {
        self.catalog
            .lock()
            .map_err(|_| NativeError::new("unavailable", "会话索引暂不可用"))
    }
    pub fn sources(&self) -> NativeResult<Vec<SourceStatus>> {
        Ok(self.lock()?.sources.clone())
    }
    pub fn set_enabled(&self, agent: &str, enabled: bool) -> NativeResult<()> {
        let mut state = self.lock()?;
        let source = state
            .sources
            .iter_mut()
            .find(|s| s.agent_id == agent)
            .ok_or_else(|| NativeError::new("unsupported", "暂不支持此会话来源"))?;
        if source.enabled != enabled {
            source.authorization_revision += 1;
            source.enabled = enabled;
            source.status = if enabled { "pending" } else { "disabled" }.into();
            source.error = None;
            if !enabled {
                source.session_count = 0;
                source.client_attribution = None;
                state.files.retain(|_, f| f.agent != agent);
            }
            state.revision += 1;
            state.as_of = now();
        }
        drop(state);
        // Revocation never waits for a file parser. A parser in flight checks
        // the authorization revision before publishing or retaining its result.
        if !enabled && let Ok(mut cache) = self.index_cache.try_lock() {
            cache.revoke(agent);
        }
        Ok(())
    }

    /// A single background scan may run at a time. Parsing never holds the catalog lock.
    pub fn refresh(&self) -> NativeResult<()> {
        self.refresh_guarded(&|| true)
    }
    pub fn refresh_guarded(&self, proceed: &dyn Fn() -> bool) -> NativeResult<()> {
        if !proceed() {
            return Ok(());
        }
        if self.scanning.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        struct ScanGuard<'a>(&'a AtomicBool);
        impl Drop for ScanGuard<'_> {
            fn drop(&mut self) {
                self.0.store(false, Ordering::Release);
            }
        }
        let _guard = ScanGuard(&self.scanning);
        self.refresh_source_plugins(proceed)?;
        if !proceed() {
            return Ok(());
        }
        let enabled = self
            .sources()?
            .iter()
            .any(|s| s.agent_id == "codex" && s.enabled && self.source_active("codex"));
        let mut evidence_cache = self
            .codex_evidence
            .lock()
            .map_err(|_| NativeError::new("unavailable", "客户端来源索引暂不可用"))?;
        let evidence = evidence_cache.snapshot(&self.home, enabled);
        let mut catalog = self.lock()?;
        if proceed()
            && catalog
                .sources
                .iter()
                .any(|s| s.agent_id == "codex" && s.enabled && self.source_active("codex"))
        {
            evidence_cache.apply(&mut catalog, evidence);
        }
        drop(catalog);
        drop(evidence_cache);
        // Publish configuration separately; ordinary fact queries never open configuration files.
        // A configuration failure must not change the existing history refresh outcome.
        self.refresh_domain_mcp_catalog(proceed);
        Ok(())
    }

    pub fn query(&self, query: SessionQuery) -> NativeResult<Page<SessionSummary, SessionQuery>> {
        let state = self.lock()?;
        self.query_locked(query, &state)
    }
    /// The page and fee evidence share the same immutable file batches, even if
    /// the scanner publishes a new generation before the response is serialized.
    pub fn query_with_catalog(
        &self,
        query: SessionQuery,
    ) -> NativeResult<(
        Page<SessionSummary, SessionQuery>,
        crate::projects::ProjectCatalog,
    )> {
        let state = self.lock()?;
        let page = self.query_locked(query, &state)?;
        Ok((page, self.project_catalog_locked(&state)))
    }
    fn query_locked(
        &self,
        query: SessionQuery,
        state: &Catalog,
    ) -> NativeResult<Page<SessionSummary, SessionQuery>> {
        self.validate_agent(query.agent.as_deref())?;
        if query
            .query
            .as_ref()
            .is_some_and(|q| q.chars().count() > 300)
            || query.days.is_some_and(|d| ![1, 7, 30].contains(&d))
            || query.status.as_deref().is_some_and(|s| s != "imported")
        {
            return Err(NativeError::new("invalid_input", "会话筛选条件无效"));
        }
        let needle = query.query.as_deref().unwrap_or("").trim().to_lowercase();
        let start = query.days.map(|d| {
            if d == 1 {
                Utc::now()
                    .with_timezone(&chrono::FixedOffset::east_opt(8 * 3600).unwrap())
                    .date_naive()
                    .and_hms_opt(0, 0, 0)
                    .unwrap()
                    .and_local_timezone(chrono::FixedOffset::east_opt(8 * 3600).unwrap())
                    .unwrap()
                    .timestamp()
            } else {
                Utc::now().timestamp() - i64::from(d) * 86400
            }
        });
        let indexed = unique_files(state);
        let by_id: HashMap<_, _> = indexed
            .iter()
            .map(|file| (file.summary.id.as_str(), *file))
            .collect();
        let mut items: Vec<_> = indexed
            .iter()
            .filter(|f| {
                session_matches(query.agent.as_deref(), &f.summary)
                    && (needle.is_empty()
                        || format!(
                            "{} {} {} {}",
                            f.summary.title,
                            f.summary.project_path.as_deref().unwrap_or(""),
                            f.summary.git_remote.as_deref().unwrap_or(""),
                            f.summary.native_id
                        )
                        .to_lowercase()
                        .contains(&needle))
                    && start.is_none_or(|s| {
                        f.summary
                            .created_at
                            .as_deref()
                            .and_then(|t| DateTime::parse_from_rfc3339(t).ok())
                            .is_some_and(|t| t.timestamp() >= s)
                    })
            })
            .map(|f| f.summary.clone())
            .collect();
        items.sort_by(|a, b| b.updated_at.cmp(&a.updated_at).then(a.id.cmp(&b.id)));
        let offset = cursor(query.cursor.as_deref(), &state.revision.to_string())?;
        let total = items.len();
        let missing_counts = items.iter().filter(|s| s.partial).count();
        let mut summary = SessionTotals {
            usage_scope: if query
                .agent
                .as_deref()
                .is_some_and(|id| !self.directory().source(id).is_some_and(|s| s.shared))
            {
                "matched_calls"
            } else {
                "session_cumulative"
            },
            model_calls: 0,
            tool_calls: 0,
            mcp_calls: 0,
            skill_calls: 0,
            usage: Usage::default(),
            usage_unknown_sessions: 0,
        };
        for s in &items {
            if summary.usage_scope == "matched_calls" {
                let mut missing = false;
                // Shared session cumulative usage cannot be allocated to participating clients.
                if let Some(file) = by_id.get(s.id.as_str()) {
                    if query
                        .agent
                        .as_deref()
                        .is_some_and(|id| !id.ends_with("-unknown") && id != "unknown")
                        && file
                            .calls
                            .iter()
                            .any(|call| call.kind == "model" && call.agent_product_id.is_none())
                    {
                        missing = true;
                    }
                    for call in file.calls.iter().filter(|c| {
                        self.directory().matches(
                            query.agent.as_deref(),
                            &c.agent_id,
                            c.agent_product_id.as_deref(),
                        )
                    }) {
                        match call.kind.as_str() {
                            "model" => {
                                summary.model_calls += 1;
                                if let Some(usage) = &call.usage {
                                    summary.usage.add(usage);
                                }
                                if call.usage.as_ref().is_none_or(|u| u.total_tokens.is_none()) {
                                    missing = true;
                                }
                            }
                            "mcp" => summary.mcp_calls += 1,
                            "skill" => summary.skill_calls += 1,
                            "tool" => summary.tool_calls += 1,
                            _ => {}
                        }
                    }
                }
                summary.usage_unknown_sessions += usize::from(missing);
                continue;
            }
            summary.model_calls += s.model_calls;
            summary.tool_calls += s.tool_calls;
            summary.mcp_calls += s.mcp_calls;
            summary.skill_calls += s.skill_calls;
            summary.usage.add(&s.usage);
            if ["unknown", "partial"].contains(&s.usage_quality) {
                summary.usage_unknown_sessions += 1;
            }
        }
        let selected_session_ids = items.iter().map(|s| s.id.clone()).collect();
        let page = items.into_iter().skip(offset).take(40).collect();
        Ok(Page {
            selected_session_ids,
            items: page,
            total,
            next_cursor: (offset + 40 < total)
                .then(|| format!("{}:{}", state.revision, offset + 40)),
            as_of: state.as_of.clone(),
            coverage: coverage(state),
            applied_filter: query,
            missing_counts,
            summary: Some(summary),
        })
    }

    pub fn calls(&self, query: CallQuery) -> NativeResult<Page<CallSummary, CallQuery>> {
        self.validate_agent(query.agent.as_deref())?;
        crate::projects::validate(&crate::projects::ProjectQuery {
            days: query.days,
            ..Default::default()
        })?;
        if query.project.as_ref().is_some_and(|id| {
            !id.strip_prefix("project:")
                .is_some_and(|v| v.len() == 64 && v.bytes().all(|b| b.is_ascii_hexdigit()))
        }) {
            return Err(NativeError::new("invalid_input", "项目标识无效"));
        }
        if query
            .kind
            .as_deref()
            .is_some_and(|s| !["model", "tool", "mcp", "skill", "diagnostic"].contains(&s))
        {
            return Err(NativeError::new("invalid_input", "调用类型无效"));
        }
        let state = self.lock()?;
        let mut items: Vec<_> = unique_files(&state)
            .into_iter()
            .filter(|f| {
                query.project.as_ref().is_none_or(|id| {
                    crate::projects::session_identity(&f.summary).is_some_and(|p| &p.id == id)
                }) && crate::projects::in_range(f.summary.created_at.as_deref(), query.days)
            })
            .flat_map(|f| f.calls.iter())
            .filter(|c| {
                self.directory().matches(
                    query.agent.as_deref(),
                    &c.agent_id,
                    c.agent_product_id.as_deref(),
                )
            })
            .filter(|c| query.kind.as_ref().is_none_or(|k| k == &c.kind))
            .collect();
        items.sort_by(|a, b| b.timestamp.cmp(&a.timestamp).then(a.id.cmp(&b.id)));
        let total = items.len();
        let revision = crate::projects::hash(
            &serde_json::json!([
                state.revision,
                query.agent,
                query.kind,
                query.project,
                query.days
            ])
            .to_string(),
        );
        let offset = cursor(query.cursor.as_deref(), &revision)?;
        let missing_counts = items
            .iter()
            .filter(|call| {
                call.status == "missing_result"
                    || (call.kind == "model"
                        && call.usage.as_ref().is_none_or(|u| u.total_tokens.is_none()))
            })
            .count();
        Ok(Page {
            selected_session_ids: vec![],
            items: items.into_iter().skip(offset).take(40).cloned().collect(),
            total,
            next_cursor: (offset + 40 < total).then(|| format!("{revision}:{}", offset + 40)),
            as_of: state.as_of.clone(),
            coverage: coverage(&state),
            applied_filter: query,
            missing_counts,
            summary: None,
        })
    }

    pub fn conversation(
        &self,
        id: &str,
        page_cursor: Option<&str>,
        anchor: Option<&str>,
    ) -> NativeResult<Conversation> {
        let (indexed, authorization_revision) = {
            let state = self.lock()?;
            let indexed = unique_files(&state)
                .into_iter()
                .find(|f| f.summary.id == id)
                .cloned()
                .ok_or_else(|| {
                    NativeError::new("not_found", "会话不存在、来源已断开或尚未完成索引")
                })?;
            let revision = state
                .sources
                .iter()
                .find(|source| source.agent_id == indexed.agent)
                .map(|source| source.authorization_revision)
                .ok_or_else(|| NativeError::new("unauthorized", "此会话来源已断开"))?;
            (indexed, revision)
        };
        let mut result =
            self.plugin_conversation(&indexed, authorization_revision, page_cursor, anchor)?;
        result.session.project_id = indexed.summary.project_id;
        result.session.project_root = indexed.summary.project_root;
        Ok(result)
    }
}

fn coverage(state: &Catalog) -> &'static str {
    if !state.sources.iter().any(|s| s.enabled) {
        "not_connected"
    } else if state
        .sources
        .iter()
        .any(|s| s.enabled && ["pending", "scanning", "partial"].contains(&s.status.as_str()))
    {
        "partial"
    } else {
        "native_history"
    }
}
fn unique_files(state: &Catalog) -> Vec<&IndexedFile> {
    let mut items: HashMap<&str, &IndexedFile> = HashMap::new();
    for f in state.files.values() {
        if items.get(f.summary.id.as_str()).is_none_or(|old| {
            (f.stamp.1, f.stamp.0, &f.path) > (old.stamp.1, old.stamp.0, &old.path)
        }) {
            items.insert(&f.summary.id, f);
        }
    }
    let mut files: Vec<_> = items.into_values().collect();
    files.sort_by(|a, b| a.summary.id.cmp(&b.summary.id));
    files
}
pub(crate) fn usage_record(call: &CallSummary) -> crate::model_usage::UsageRecord {
    crate::model_usage::UsageRecord {
        cost: crate::provider_cost::Cost::unknown("native_price_unconfirmed"),
        id: call.id.clone(),
        model: call.model.clone(),
        provider: call.provider.clone(),
        endpoint: None,
        route_revision: None,
        time: call.timestamp.clone(),
        usage: call.usage.clone().unwrap_or_default(),
        session_id: Some(call.session_id.clone()),
        request_id: None,
    }
}
pub(crate) fn session_matches(filter: Option<&str>, session: &SessionSummary) -> bool {
    match filter {
        Some("unknown") => session.has_unknown_client,
        Some(id) if id.strip_suffix("-unknown") == Some(session.agent_id.as_str()) => {
            session.has_unknown_client
        }
        Some(id) if id == session.agent_id => true,
        Some(id) => session.agent_products.iter().any(|product| product == id),
        None => true,
    }
}

fn cursor(value: Option<&str>, revision: &str) -> NativeResult<usize> {
    let Some(value) = value else {
        return Ok(0);
    };
    let Some((rev, offset)) = value.rsplit_once(':') else {
        return Err(NativeError::new("invalid_input", "分页位置无效"));
    };
    if rev != revision {
        return Err(NativeError::new(
            "stale_snapshot",
            "会话记录已更新，请刷新后继续阅读",
        ));
    }
    offset
        .parse()
        .ok()
        .filter(|v| *v <= 1_000_000)
        .ok_or_else(|| NativeError::new("invalid_input", "分页位置无效"))
}
fn roots(home: &Path, agent: &str) -> Vec<PathBuf> {
    match agent {
        "claude" => vec![home.join(".claude/projects")],
        "codex" => vec![
            home.join(".codex/sessions"),
            home.join(".codex/archived_sessions"),
        ],
        _ => vec![],
    }
}
fn source_locations(home: &Path, agent: &str) -> Vec<PathBuf> {
    if agent == "antigravity" {
        return vec![
            antigravity::root(home),
            home.join(".gemini/antigravity/mcp_config.json"),
            home.join(".gemini/config"),
        ];
    }
    if agent == "opencode" {
        let mut paths = vec![opencode::path(home), opencode_log::path(home)];
        paths.extend(opencode_mcp::global_paths(home));
        return paths;
    }
    let mut paths = roots(home, agent);
    if agent == "codex" {
        paths.push(home.join(".codex/session_index.jsonl"));
        paths.push(home.join(".codex/logs_2.sqlite"));
        paths.push(home.join("Library/Logs/com.openai.codex"));
    }
    paths
}
fn native_titles(home: &Path, agent: &str, paths: &[PathBuf]) -> (HashMap<String, String>, bool) {
    use std::io::Read;
    let files: HashSet<PathBuf> = if agent == "codex" {
        [home.join(".codex/session_index.jsonl")]
            .into_iter()
            .collect()
    } else {
        paths
            .iter()
            .filter_map(|p| p.parent().map(|p| p.join("sessions-index.json")))
            .collect()
    };
    let mut titles = HashMap::new();
    let mut failed = false;
    for path in files {
        match fs::symlink_metadata(&path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => {
                failed = true;
                continue;
            }
            _ => {}
        }
        if !safe_path(home, &path) {
            failed = true;
            continue;
        }
        let mut options = fs::OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW);
        }
        let Ok(file) = options.open(path) else {
            failed = true;
            continue;
        };
        let Ok(meta) = file.metadata() else {
            failed = true;
            continue;
        };
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if meta.nlink() > 1 {
                failed = true;
                continue;
            }
        }
        if !meta.is_file() || meta.len() > 16 * 1024 * 1024 {
            failed = true;
            continue;
        }
        let mut text = String::new();
        if file
            .take(16 * 1024 * 1024 + 1)
            .read_to_string(&mut text)
            .is_err()
            || text.len() > 16 * 1024 * 1024
        {
            failed = true;
            continue;
        }
        if agent == "codex" {
            for line in text.lines() {
                if let Ok(value) = serde_json::from_str::<serde_json::Value>(line)
                    && let (Some(id), Some(title)) =
                        (value["id"].as_str(), value["thread_name"].as_str())
                    && !title.trim().is_empty()
                {
                    titles.insert(id.into(), parser::safe_text(title, 200).0);
                }
            }
        } else if let Ok(value) = serde_json::from_str::<serde_json::Value>(&text)
            && let Some(entries) = value["entries"].as_array()
        {
            for entry in entries {
                let title = entry["customTitle"]
                    .as_str()
                    .or_else(|| entry["summary"].as_str())
                    .or_else(|| entry["title"].as_str());
                if let (Some(id), Some(title)) = (entry["sessionId"].as_str(), title)
                    && !title.trim().is_empty()
                {
                    titles.insert(id.into(), parser::safe_text(title, 200).0);
                }
            }
        }
    }
    (titles, failed)
}
fn safe_path(home: &Path, path: &Path) -> bool {
    let Ok(relative) = path.strip_prefix(home) else {
        return false;
    };
    let mut current = home.to_path_buf();
    for part in relative.components() {
        if !matches!(part, std::path::Component::Normal(_)) {
            return false;
        }
        current.push(part);
        if fs::symlink_metadata(&current).map_or(true, |m| m.file_type().is_symlink()) {
            return false;
        }
    }
    true
}
fn stamp(path: &Path) -> Option<(u64, u128, u64, u64, u128)> {
    let m = fs::symlink_metadata(path).ok()?;
    if !m.is_file() || m.file_type().is_symlink() {
        return None;
    }
    #[cfg(unix)]
    let (device, inode, changed) = {
        use std::os::unix::fs::MetadataExt;
        (
            m.dev(),
            m.ino(),
            (i128::from(m.ctime()) * 1_000_000_000 + i128::from(m.ctime_nsec())) as u128,
        )
    };
    #[cfg(not(unix))]
    let (device, inode, changed) = (0, 0, 0);
    Some((
        m.len(),
        m.modified()
            .ok()?
            .duration_since(UNIX_EPOCH)
            .ok()?
            .as_nanos(),
        device,
        inode,
        changed,
    ))
}
fn collect(
    root: &Path,
    depth: u8,
    paths: &mut Vec<PathBuf>,
    skipped: &mut usize,
    error: &mut bool,
) {
    let Ok(entries) = fs::read_dir(root) else {
        *error = true;
        return;
    };
    for entry in entries {
        let Ok(entry) = entry else {
            *error = true;
            continue;
        };
        let Ok(kind) = entry.file_type() else {
            *error = true;
            continue;
        };
        if kind.is_symlink() {
            *skipped += 1;
            continue;
        }
        let path = entry.path();
        if kind.is_dir() && depth > 0 && entry.file_name() != "subagents" {
            collect(&path, depth - 1, paths, skipped, error);
        } else if kind.is_file()
            && path.extension().is_some_and(|s| s == "jsonl")
            && !entry.file_name().to_string_lossy().starts_with("agent-")
        {
            if paths.len() < 10_000 {
                paths.push(path);
            } else {
                *skipped += 1;
                *error = true;
            }
        }
    }
}

fn source_capabilities(agent: &str) -> Vec<Capability> {
    if agent == "antigravity" {
        antigravity::capabilities()
    } else {
        vec![
            Capability {
                name: "Session / 消息",
                status: "supported",
                reason: "只读原生 JSONL / OpenCode SQLite；正文按需读取，不另存副本",
            },
            Capability {
                name: "工具 / MCP",
                status: "partial",
                reason: "按原生 call_id 关联；OpenCode 支持当前本机 / 项目配置关联，歧义保留未知",
            },
            Capability {
                name: "Skill",
                status: "partial",
                reason: "仅展示明确 Skill 工具调用；不从工具名或指令正文猜测",
            },
            Capability {
                name: "Token",
                status: "partial",
                reason: "仅使用原生 usage；累计与单次记录互斥去重",
            },
            Capability {
                name: "模型异常",
                status: "partial",
                reason: "读取明确原生错误；OpenCode 补充当前日志末尾的 Session 关联错误。未落盘 / 未经入口的异常可能缺失，不把错误日志条数当 HTTP 请求数",
            },
            Capability {
                name: "实时 TTFT / 速度 / 费用",
                status: "unsupported",
                reason: "本通道读取历史会话，不等于网关流式计时或 API 账单",
            },
        ]
    }
}
