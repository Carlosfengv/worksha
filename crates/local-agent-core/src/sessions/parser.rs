use super::{CallSummary, NativeError, NativeResult};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
mod antigravity;
mod builder;
mod claude;
mod codex;
mod index;
mod opencode;
mod text;
mod usage;
pub use crate::usage::Usage;
pub(super) use antigravity::model as antigravity_model;
pub(super) use antigravity::parse as antigravity;
pub(super) use index::IndexCache;
#[cfg(test)]
pub use index::parse;
pub use index::parse_snapshot;
pub(super) use opencode::{opencode, opencode_index};
pub(super) use text::{content_text, is_context, safe_text};
const PAGE: usize = 40;
const MAX_LINE: u64 = 8 * 1024 * 1024;
const MAX_FILE: u64 = 512 * 1024 * 1024;
const MAX_TEXT: usize = 64 * 1024;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSummary {
    pub project_hidden: bool,
    pub id: String,
    pub native_id: String,
    pub agent_id: String,
    pub agent_product_id: Option<String>,
    pub agent_products: Vec<String>,
    pub has_unknown_client: bool,
    pub agent_version: Option<String>,
    pub title: String,
    pub title_source: String,
    pub project_path: Option<String>,
    pub project_name: Option<String>,
    pub project_id: Option<String>,
    pub project_root: Option<String>,
    pub git_remote: Option<String>,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    pub status: &'static str,
    pub message_count: usize,
    pub turn_count: Option<usize>,
    pub model_calls: usize,
    pub model_error_count: usize,
    pub latest_failure_id: Option<String>,
    pub tool_calls: usize,
    pub mcp_calls: usize,
    pub skill_calls: usize,
    pub usage: Usage,
    pub usage_quality: &'static str,
    pub disk_bytes: u64,
    pub disk_allocated_bytes: Option<u64>,
    pub disk_scope: &'static str,
    pub source_path: String,
    pub partial: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Event {
    pub agent_attribution: Option<&'static str>,
    pub agent_product_id: Option<String>,
    pub id: String,
    pub native_id: String,
    pub turn_id: String,
    pub kind: String,
    pub role: Option<String>,
    pub name: String,
    pub timestamp: Option<String>,
    pub end_time: Option<String>,
    pub status: String,
    pub text: Option<String>,
    pub input: Option<String>,
    pub output: Option<String>,
    pub model: Option<String>,
    pub model_response_id: Option<String>,
    pub provider: Option<String>,
    pub usage: Option<Usage>,
    pub raw_usage: Option<String>,
    pub call_id: Option<String>,
    pub server: Option<String>,
    pub tool_source: Option<super::opencode_mcp::ToolSource>,
    pub duration_ms: Option<u64>,
    pub truncated: bool,
    pub source_line: usize,
    pub failure: Option<super::ModelFailure>,
}
pub struct Window<'a> {
    pub offset: usize,
    pub anchor: Option<&'a str>,
}
pub struct Parsed {
    pub(super) activity_messages: Vec<super::live::NativeActivity>,
    pub context: super::context::SessionContext,
    pub unknown_non_calls: bool,
    pub summary: SessionSummary,
    pub calls: Vec<CallSummary>,
    pub events: Vec<Event>,
    pub total: usize,
    pub end: usize,
    pub warnings: Vec<String>,
    pub snapshot_bytes: u64,
    pub fingerprint: String,
    pub writes: Vec<crate::projects::writes::FileWrite>,
}

/// One refresh only: avoid repeated filesystem ancestry probes for shared cwd.
/// Never reuse a project identity across scans or detail reads.
#[derive(Default)]
pub(super) struct ProjectCache(HashMap<String, Option<crate::projects::ProjectIdentity>>);
impl ProjectCache {
    fn identity(&mut self, path: &str) -> Option<crate::projects::ProjectIdentity> {
        if let Some(identity) = self.0.get(path) {
            return identity.clone();
        }
        let identity = crate::projects::identity(path);
        if self.0.len() < 4096 {
            self.0.insert(path.into(), identity.clone());
        }
        identity
    }
}

#[derive(Clone)]
struct Builder {
    activity_history: super::activity_history::Collector,
    context: super::context::Collector,
    agent: String,
    product: Option<String>,
    product_evidence: Option<&'static str>,
    known_products: HashSet<String>,
    native_id: Option<String>,
    fallback: String,
    version: Option<String>,
    title: Option<String>,
    title_source: String,
    project: Option<String>,
    remote: Option<String>,
    model: Option<String>,
    provider: Option<String>,
    created: Option<String>,
    updated: Option<String>,
    turn: String,
    events: Vec<Event>,
    lookup: HashMap<String, usize>,
    seen_rows: HashSet<String>,
    window: Option<usize>,
    total_usage: Option<Usage>,
    usage_priority: u8,
    foreign_records: usize,
    proposed_writes: HashMap<String, Vec<crate::projects::writes::WriteFile>>,
    confirmed_writes: HashSet<String>,
}
