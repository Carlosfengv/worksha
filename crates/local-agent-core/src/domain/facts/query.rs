//! Fixed-generation graph queries. Lists and summaries use the same filtered graph.
use super::*;
use crate::domain::{QueryScope, SourceState};
use std::{
    cmp::Reverse,
    collections::{BTreeMap, BTreeSet},
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FactQuery {
    pub scope: QueryScope,
    pub kind: FactKind,
    pub project_id: Option<String>,
    pub session_id: Option<String>,
    pub response_id: Option<String>,
    pub snapshot_token: Option<String>,
    pub cursor: Option<String>,
}
impl FactQuery {
    pub fn validate(&self) -> Result<()> {
        if [&self.project_id, &self.session_id, &self.response_id]
            .into_iter()
            .flatten()
            .any(|s| s.is_empty() || s.len() > 320)
            || self.snapshot_token.as_ref().is_some_and(|s| s.len() > 80)
            || self.cursor.as_ref().is_some_and(|s| s.len() > 4096)
        {
            return Err(Failure::new(ErrorCode::InvalidInput));
        }
        Ok(())
    }
    pub fn same_selection(&self, other: &Self) -> bool {
        self.scope == other.scope
            && self.kind == other.kind
            && self.project_id == other.project_id
            && self.session_id == other.session_id
            && self.response_id == other.response_id
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FactPage {
    pub items: Vec<FactRecord>,
    pub snapshot_token: String,
    pub next_cursor: Option<String>,
    pub as_of: String,
    pub source_states: Vec<SourceState>,
    pub source_summaries: Vec<FactSummary>,
    pub applied_query: FactQuery,
    /// Source claims never establish gateway correlation or cross-source deduplication.
    pub overlap_unknown: bool,
    pub hidden_projects: Vec<HiddenProject>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HiddenProject {
    pub source_id: String,
    pub id: String,
    pub name: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProjectVisibility {
    pub source_id: String,
    pub project_id: String,
    pub hidden: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FactOrder {
    unknown_time: bool,
    time: Option<Reverse<i64>>,
    source: String,
    id: String,
}
impl FactOrder {
    pub(crate) fn cmp_metadata(
        &self,
        source: &str,
        id: &str,
        at: Option<&str>,
    ) -> std::cmp::Ordering {
        let time = at.and_then(crate::domain::timestamp);
        (
            self.unknown_time,
            self.time,
            self.source.as_str(),
            self.id.as_str(),
        )
            .cmp(&(time.is_none(), time.map(Reverse), source, id))
    }
    fn of(record: &FactRecord) -> Self {
        let time = record
            .value
            .occurred_at()
            .and_then(crate::domain::timestamp);
        Self {
            unknown_time: time.is_none(),
            time: time.map(Reverse),
            source: record.identity.source_id.clone(),
            id: record.id.clone(),
        }
    }
}
/// Keeps at most one page plus a lookahead even for a million native calls.
pub struct FactWindow {
    after: Option<FactOrder>,
    rows: BTreeMap<FactOrder, FactRecord>,
}
impl FactWindow {
    pub fn new(after: Option<FactOrder>) -> Self {
        Self {
            after,
            rows: BTreeMap::new(),
        }
    }
    pub fn observe(&mut self, record: FactRecord) {
        let order = FactOrder::of(&record);
        if self.after.as_ref().is_some_and(|after| &order <= after) {
            return;
        }
        self.rows.insert(order, record);
        if self.rows.len() > crate::domain::PAGE_SIZE + 1 {
            self.rows.pop_last();
        }
    }
    pub fn finish(self) -> (Vec<FactRecord>, Option<FactOrder>) {
        let more = self.rows.len() > crate::domain::PAGE_SIZE;
        let mut last = None;
        let rows = self
            .rows
            .into_iter()
            .take(crate::domain::PAGE_SIZE)
            .map(|(key, value)| {
                last = Some(key);
                value
            })
            .collect();
        (rows, if more { last } else { None })
    }
    pub fn candidates(self) -> Vec<FactRecord> {
        self.rows.into_values().collect()
    }
}

impl FactSnapshot {
    /// Hidden projects lose their references; independently authorized sessions remain readable.
    /// No source path is interpreted here or forwarded to native project readers.
    pub fn filtered_graph(&self, query: &FactQuery, hidden: &[String]) -> Vec<FactRecord> {
        let constrained =
            query.project_id.is_some() || query.session_id.is_some() || query.response_id.is_some();
        let responses: BTreeSet<_> = self
            .records
            .iter()
            .filter_map(|f| match &f.value {
                FactCandidate::Response(r)
                    if query.response_id.as_ref().is_some_and(|id| id == &f.id) =>
                {
                    Some(r.session_ref.as_str())
                }
                _ => None,
            })
            .collect();
        let sessions: BTreeSet<_> = self
            .records
            .iter()
            .filter_map(|f| match &f.value {
                FactCandidate::Session(s)
                    if query.session_id.as_ref().is_none_or(|id| id == &f.id)
                        && query.project_id.as_ref().is_none_or(|id| {
                            !hidden.contains(id) && s.project_ref.as_ref() == Some(id)
                        })
                        && (query.response_id.is_none() || responses.contains(f.id.as_str())) =>
                {
                    Some(f.id.as_str())
                }
                _ => None,
            })
            .collect();
        let projects: BTreeSet<_> = self
            .records
            .iter()
            .filter_map(|f| match &f.value {
                FactCandidate::Session(s) if sessions.contains(f.id.as_str()) => {
                    s.project_ref.as_deref()
                }
                _ => None,
            })
            .collect();
        let included_response = |id: &str| query.response_id.as_ref().is_none_or(|r| r == id);
        let executions: BTreeSet<_> = self
            .records
            .iter()
            .filter_map(|f| match &f.value {
                FactCandidate::McpExecution(e)
                    if !constrained
                        || e.session_ref
                            .as_deref()
                            .is_some_and(|s| sessions.contains(s))
                            && (query.response_id.is_none()
                                || e.response_ref.as_deref().is_some_and(included_response)) =>
                {
                    e.server_ref.as_deref()
                }
                _ => None,
            })
            .collect();
        self.records
            .iter()
            .filter(|f| match &f.value {
                FactCandidate::Project(_) => {
                    !hidden.contains(&f.id)
                        && (!constrained
                            || projects.contains(f.id.as_str())
                            || query.project_id.as_ref() == Some(&f.id)
                                && query.session_id.is_none()
                                && query.response_id.is_none())
                }
                FactCandidate::Session(_) => sessions.contains(f.id.as_str()),
                FactCandidate::Response(r) => {
                    sessions.contains(r.session_ref.as_str()) && included_response(&f.id)
                }
                FactCandidate::Activity(a) => {
                    sessions.contains(a.session_ref.as_str())
                        && (query.response_id.is_none()
                            || a.response_ref.as_deref().is_some_and(included_response))
                }
                FactCandidate::Write(w) => {
                    !hidden.contains(&w.project_ref)
                        && sessions.contains(w.session_ref.as_str())
                        && (query.response_id.is_none()
                            || w.response_ref.as_deref().is_some_and(included_response))
                }
                FactCandidate::McpCatalog(_) => !constrained || executions.contains(f.id.as_str()),
                FactCandidate::McpExecution(e) => {
                    !constrained
                        || e.session_ref
                            .as_deref()
                            .is_some_and(|s| sessions.contains(s))
                            && (query.response_id.is_none()
                                || e.response_ref.as_deref().is_some_and(included_response))
                }
            })
            .cloned()
            .map(|mut f| {
                if let FactCandidate::Session(s) = &mut f.value
                    && s.project_ref.as_ref().is_some_and(|p| hidden.contains(p))
                {
                    s.project_ref = None;
                }
                f
            })
            .collect()
    }
}
