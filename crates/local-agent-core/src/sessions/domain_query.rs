//! D2 query adaptation reads the existing index; it never scans a source.
use super::{NativeError, NativeResult, NativeSessions, SessionSummary};
use crate::domain as d;

pub(super) fn record(s: &SessionSummary) -> d::SessionRecord {
    d::SessionRecord {
        id: s.id.clone(),
        native_id: s.native_id.clone(),
        product_ids: s.agent_products.clone(),
        unknown_client: s.has_unknown_client,
        title: s.title.clone(),
        created_at: s.created_at.clone(),
        updated_at: s.updated_at.clone(),
        project_ref: s.project_id.clone(),
        usage: (&s.usage).into(),
        partial: s.partial,
    }
}
impl NativeSessions {
    /// Captures both index and authorization revisions without exposing a filesystem identity.
    pub fn domain_revision(&self) -> NativeResult<String> {
        let s = self.lock()?;
        serde_json::to_string(&(
            s.revision,
            s.sources
                .iter()
                .filter(|s| {
                    self.source_plugins
                        .sources
                        .get(s.agent_id.as_str())
                        .is_some_and(|p| p.domain_provider().is_some())
                })
                .map(|s| {
                    (
                        &s.agent_id,
                        s.authorization_revision,
                        self.source_active(&s.agent_id),
                    )
                })
                .collect::<Vec<_>>(),
        ))
        .map_err(|_| NativeError::new("unavailable", "领域代次不可用"))
    }
    pub fn domain_sessions(
        &self,
        filter: &d::SessionFilter,
        after: Option<d::SessionOrder>,
        cutoff: Option<i64>,
        hidden: &[String],
    ) -> NativeResult<(Vec<d::SessionItem>, d::SessionScopeSummary)> {
        self.domain_sessions_for_source("claude", filter, after, cutoff, hidden)
    }
    pub fn domain_sessions_for_source(
        &self,
        source: &str,
        filter: &d::SessionFilter,
        after: Option<d::SessionOrder>,
        cutoff: Option<i64>,
        hidden: &[String],
    ) -> NativeResult<(Vec<d::SessionItem>, d::SessionScopeSummary)> {
        if self
            .source_plugins
            .sources
            .get(source)
            .is_none_or(|p| p.domain_provider().is_none())
        {
            return Err(NativeError::new("unsupported", "来源尚未迁移"));
        }
        filter
            .validate()
            .map_err(|_| NativeError::new("invalid_input", "领域筛选无效"))?;
        let state = self.lock()?;
        let mut window = d::SessionWindow::new(source, after);
        if state
            .sources
            .iter()
            .any(|s| s.agent_id == source && s.enabled)
            && self.source_active(source)
        {
            for f in super::unique_files(&state)
                .into_iter()
                .filter(|f| f.agent == source)
            {
                let mut record = record(&f.summary);
                if record
                    .project_ref
                    .as_ref()
                    .is_some_and(|id| hidden.contains(id))
                {
                    record.project_ref = None;
                }
                if filter.matches(&record, cutoff) {
                    window
                        .observe(record)
                        .map_err(|_| NativeError::new("resource_limit", "领域结果超出预算"))?;
                }
            }
        }
        Ok(window.finish())
    }
    pub fn domain_conversation(
        &self,
        id: &str,
        cursor: Option<&str>,
        anchor: Option<&str>,
    ) -> NativeResult<d::ConversationPage> {
        if id.split_once(':').is_none_or(|(source, _)| {
            self.source_plugins
                .sources
                .get(source)
                .is_none_or(|p| p.domain_provider().is_none())
        }) {
            return Err(NativeError::new("unsupported", "来源尚未迁移"));
        }
        let c = self.conversation(id, cursor, anchor)?;
        Ok(d::ConversationPage {
            session_id: id.into(),
            events: c
                .items
                .iter()
                .map(|e| d::ConversationEvent {
                    id: e.id.clone(),
                    native_id: e.native_id.clone(),
                    turn_id: e.turn_id.clone(),
                    kind: e.kind.clone(),
                    role: e.role.clone(),
                    status: e.status.clone(),
                    text: e.text.clone(),
                    input: e.input.clone(),
                    output: e.output.clone(),
                    model: e.model.clone(),
                    response_id: e.model_response_id.clone(),
                    usage: e.usage.as_ref().map(Into::into),
                })
                .collect(),
            next_cursor: c.next_cursor.clone(),
            complete: c.next_cursor.is_none(),
        })
    }
    pub fn domain_session_matches(
        &self,
        id: &str,
        filter: &d::SessionFilter,
        cutoff: Option<i64>,
    ) -> NativeResult<bool> {
        let state = self.lock()?;
        Ok(super::unique_files(&state).into_iter().any(|f| {
            self.source_plugins
                .sources
                .get(f.agent.as_str())
                .is_some_and(|p| p.domain_provider().is_some())
                && f.summary.id == id
                && filter.matches(&record(&f.summary), cutoff)
        }))
    }
}
