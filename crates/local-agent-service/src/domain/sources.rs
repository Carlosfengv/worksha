//! Execution formats share metadata and query semantics, not private stores or grant state.
use crate::{
    AppState,
    plugin_host::external::{self, Result},
};
use local_agent_core::domain as d;

pub(super) enum SourceView {
    Legacy(Box<external::domain::SourceView>),
    Full(Box<external::full::SourceView>),
}
impl SourceView {
    pub fn has_agent_connection(&self) -> bool {
        matches!(self, Self::Full(view) if view.has_agent_connection())
    }
    pub fn source(&self) -> &d::Source {
        match self {
            Self::Legacy(v) => &v.source,
            Self::Full(v) => &v.source,
        }
    }
    pub fn provider(&self) -> &d::Provider {
        match self {
            Self::Legacy(v) => &v.provider,
            Self::Full(v) => &v.provider,
        }
    }
    pub fn products(&self) -> &[d::Product] {
        match self {
            Self::Legacy(v) => std::slice::from_ref(&v.product),
            Self::Full(v) => &v.products,
        }
    }
    pub fn plugin_id(&self) -> &str {
        match self {
            Self::Legacy(v) => &v.plugin_id,
            Self::Full(v) => &v.plugin_id,
        }
    }
    pub fn digest(&self) -> &str {
        match self {
            Self::Legacy(v) => &v.digest,
            Self::Full(v) => &v.digest,
        }
    }
    pub fn persist_metadata(&self) -> bool {
        match self {
            Self::Legacy(v) => v.persist_metadata,
            Self::Full(v) => v.persist_metadata,
        }
    }
    pub fn state(&self) -> d::SourceState {
        match self {
            Self::Legacy(v) => v.state(),
            Self::Full(v) => v.state(),
        }
    }
    pub fn vector(&self) -> String {
        match self {
            Self::Legacy(v) => v.vector(),
            Self::Full(v) => v.vector(),
        }
    }
    pub fn capabilities(&self) -> Vec<d::CapabilityEntry> {
        match self {
            Self::Legacy(v) => vec![d::CapabilityEntry {
                provider: v.provider.clone(),
                status: v.source.status.clone(),
                operations: vec![d::ReadOperation::Scan, d::ReadOperation::ReadConversation],
            }],
            Self::Full(v) => v.capabilities(),
        }
    }
    pub fn sessions(
        &self,
        filter: &d::SessionFilter,
        after: Option<d::SessionOrder>,
        cutoff: Option<i64>,
        hidden: &[String],
    ) -> Result<(Vec<d::SessionItem>, d::SessionScopeSummary)> {
        match self {
            Self::Legacy(v) => v.sessions(filter, after, cutoff),
            Self::Full(v) => v.sessions(filter, after, cutoff, hidden),
        }
    }
    pub fn matches_session(
        &self,
        id: &str,
        filter: &d::SessionFilter,
        cutoff: Option<i64>,
    ) -> bool {
        if self.source().status.authorization != d::Authorization::Authorized {
            return false;
        }
        match self {
            Self::Legacy(v) => v.cache.as_ref().is_some_and(|c| {
                c.sessions.iter().any(|s| {
                    format!("{}:{}", v.plugin_id, s.native_id) == id
                        && external::domain::legacy_record(&v.plugin_id, &v.product.id, s)
                            .is_ok_and(|s| filter.matches(&s, cutoff))
                })
            }),
            Self::Full(v) => v.matches_session(id, filter, cutoff),
        }
    }
    /// Resolve one retained metadata record; never invoke read_conversation or a guest.
    pub fn session_metadata(&self, id: &str) -> Result<Option<d::SessionRecord>> {
        if self.source().status.authorization != d::Authorization::Authorized {
            return Ok(None);
        }
        match self {
            Self::Legacy(v) => v
                .cache
                .as_ref()
                .and_then(|cache| {
                    cache
                        .sessions
                        .iter()
                        .find(|s| format!("{}:{}", v.plugin_id, s.native_id) == id)
                })
                .map(|s| external::domain::legacy_record(&v.plugin_id, &v.product.id, s))
                .transpose(),
            Self::Full(v) => Ok(v
                .snapshot()
                .and_then(|snapshot| snapshot.records.iter().find(|r| r.id == id))
                .and_then(|r| {
                    let d::facts::FactCandidate::Session(s) = &r.value else {
                        return None;
                    };
                    Some(d::SessionRecord {
                        id: r.id.clone(),
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
                })),
        }
    }
    pub async fn read_conversation(
        &self,
        state: &AppState,
        request: &d::UnifiedConversationRequest,
    ) -> Result<d::ConversationPage> {
        match self {
            Self::Legacy(v) => state.plugins.domain_read_legacy(v, request).await,
            Self::Full(v) => state.plugins.full_read_conversation(v, request).await,
        }
    }
}
pub(super) fn external_sources(state: &AppState) -> (Vec<SourceView>, Option<String>) {
    let mut views = Vec::new();
    let mut problem = None;
    match state.plugins.domain_sources() {
        Ok(v) => views.extend(v.into_iter().map(|v| SourceView::Legacy(Box::new(v)))),
        Err(e) => problem = Some(e.code.into()),
    }
    match state.plugins.full_source_views() {
        Ok(v) => {
            for full in v {
                let legacy = views.iter().position(|v| v.plugin_id() == full.plugin_id);
                if let Some(index) = legacy {
                    if full.source.status.enabled {
                        views.remove(index);
                        views.push(SourceView::Full(Box::new(full)));
                    }
                } else {
                    views.push(SourceView::Full(Box::new(full)));
                }
            }
        }
        Err(e) => {
            problem.get_or_insert_with(|| e.code.into());
        }
    }
    views.sort_by(|a, b| a.source().id.cmp(&b.source().id));
    (views, problem)
}
