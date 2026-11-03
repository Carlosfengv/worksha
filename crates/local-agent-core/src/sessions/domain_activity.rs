//! All native activity origins share a public metadata consumer. File paths,
//! runtime correlation and measured performance remain in a private host frame.
use super::{
    NativeError, NativeResult, NativeSessions, live::NativeActivity, source_plugins::SourceContext,
};
use crate::{
    domain::{self as d, activity as a},
    plugin::PluginLease,
};
use a::ActivitySource;
use std::collections::HashMap;
#[cfg(test)]
mod tests;

#[derive(Clone)]
pub struct Scope {
    pub(super) source_id: String,
    pub(super) revision: u64,
    pub(super) lease: PluginLease,
}
impl Scope {
    pub fn source_id(&self) -> &str {
        &self.source_id
    }
    pub fn ensure_current(&self, sessions: &NativeSessions) -> NativeResult<()> {
        self.lease
            .ensure_active()
            .map_err(|e| NativeError::new(e.code, &e.message))?;
        if !sessions.sources()?.iter().any(|s| {
            s.agent_id == self.source_id && s.enabled && s.authorization_revision == self.revision
        }) {
            return Err(NativeError::new("unauthorized", "此活动来源已断开"));
        }
        Ok(())
    }
}
impl NativeSessions {
    /// Host-only leases, never serialized or usable as a guest resource grant.
    pub fn domain_activity_authorizations(&self) -> NativeResult<Vec<Scope>> {
        scopes(self)
    }
}
pub(super) fn scopes(sessions: &NativeSessions) -> NativeResult<Vec<Scope>> {
    Ok(sessions
        .active_sources()?
        .into_iter()
        .filter(|s| s.enabled)
        .filter_map(|s| {
            sessions
                .source_plugins
                .lease(&s.agent_id)
                .map(|lease| Scope {
                    source_id: s.agent_id,
                    revision: s.authorization_revision,
                    lease,
                })
        })
        .collect())
}

fn observation(row: &NativeActivity) -> a::ActivityObservation {
    a::ActivityObservation {
        id: row.id.clone(),
        source_id: row.agent_id.clone(),
        session_id: row.session_id.clone(),
        message_id: row.message_id().map(Into::into),
        product_id: row.agent_product_id.clone(),
        attribution: row.agent_attribution.clone(),
        title: row.title.clone(),
        model: row.model.clone(),
        phase: row.phase.clone(),
        step: row.step.clone(),
        user_input: row.user_input.as_ref().map(|i| a::ActivityInput {
            text: i.text.clone(),
            at: i.at.clone(),
        }),
        tool_names: row.tool_names.clone(),
        message_started_at: row.message_started_at.clone(),
        event_at: row.event_at.clone(),
        stale: row.stale,
    }
}

struct NativeRead<'a> {
    context: &'a SourceContext<'a>,
    expected: a::ActivityRequest,
    broker: &'a mut dyn FnMut() -> NativeResult<Vec<NativeActivity>>,
    private: HashMap<String, NativeActivity>,
    collection_micros: u64,
    limited: bool,
    error: Option<NativeError>,
}
impl NativeRead<'_> {
    fn check(&mut self) -> d::Result<()> {
        self.context.check().map_err(|e| {
            let failure = super::domain_source::domain_failure(&e, self.context.domain_cancelled());
            self.error = Some(e);
            failure
        })
    }
    fn consume(&mut self, snapshot: a::ActivitySnapshot) -> NativeResult<Vec<NativeActivity>> {
        snapshot.validate(&self.expected).map_err(|_| invalid())?;
        self.context.check()?;
        if snapshot.collection_micros.value() != self.collection_micros {
            return Err(invalid());
        }
        let mut result = Vec::with_capacity(snapshot.activities.len());
        for item in snapshot.activities {
            let mut row = self.private.remove(&item.id).ok_or_else(invalid)?;
            // Numeric/latency evidence and native paths cannot move to another
            // source, event or turn, even when a source reuses an activity ID.
            if item.source_id != row.agent_id
                || item.session_id != row.session_id
                || item.message_id.as_deref() != row.message_id()
                || item.message_started_at != row.message_started_at
                || item.event_at != row.event_at
                || item.product_id != row.agent_product_id
                || item.attribution != row.agent_attribution
            {
                return Err(invalid());
            }
            row.title = item.title;
            row.model = item.model;
            row.phase = item.phase;
            row.step = item.step;
            row.user_input = item.user_input.map(|i| super::activity_input::UserInput {
                text: i.text,
                at: i.at,
            });
            row.tool_names = item.tool_names;
            row.stale = item.stale;
            result.push(row);
        }
        self.context.check()?;
        Ok(result)
    }
}
impl ActivitySource for NativeRead<'_> {
    fn collect_activity(&mut self, request: &a::ActivityRequest) -> d::Result<a::ActivitySnapshot> {
        if request != &self.expected {
            return Err(d::Failure::new(d::ErrorCode::StaleSnapshot));
        }
        self.check()?;
        let rows = (self.broker)().map_err(|e| {
            let failure = super::domain_source::domain_failure(&e, self.context.domain_cancelled());
            self.error = Some(e);
            failure
        })?;
        self.check()?;
        let result = a::ActivitySnapshot {
            source_id: request.source_id.clone(),
            source_revision: request.invocation.invocation_id.clone(),
            observed_at: request.observed_at.clone(),
            collection_micros: self.collection_micros.into(),
            activities: rows.iter().map(observation).collect(),
            limited: self.limited,
        };
        result.validate(request)?;
        self.private = rows.into_iter().map(|r| (r.id.clone(), r)).collect();
        Ok(result)
    }
}
fn invalid() -> NativeError {
    NativeError::new("unavailable", "活动结果未通过领域契约核查")
}

/// Pure consumption after the existing bounded sampler: no source refresh,
/// file access or guest execution is added here. Each origin has its own lease.
pub(super) fn publish(
    sessions: &NativeSessions,
    scopes: &[Scope],
    rows: Vec<NativeActivity>,
    observed_at: &str,
    collection_micros: u64,
    limited: bool,
) -> (Vec<NativeActivity>, bool) {
    let order = rows
        .iter()
        .map(|r| (r.agent_id.clone(), r.id.clone()))
        .collect::<Vec<_>>();
    let mut grouped: HashMap<_, Vec<_>> = HashMap::new();
    for row in rows {
        grouped.entry(row.agent_id.clone()).or_default().push(row);
    }
    let mut published = HashMap::new();
    let mut failed = false;
    for scope in scopes {
        let mut rows = Some(grouped.remove(&scope.source_id).unwrap_or_default());
        let result = sessions.with_source_context(
            &scope.source_id,
            Some(scope.revision),
            true,
            |source, context| {
                scope.lease.ensure_active().map_err(|_| invalid())?;
                let mut provider = source.domain_provider().ok_or_else(invalid)?;
                provider.provider_key =
                    format!("builtin:domain.activity-source@1:{}", scope.source_id);
                provider.capability.name = "domain.activity-source".into();
                let mut invocation = context.domain_invocation(&provider);
                invocation.budget.max_records = (a::MAX_ACTIVITIES as u64).into();
                let request = a::ActivityRequest {
                    invocation,
                    source_id: scope.source_id.clone(),
                    observed_at: observed_at.into(),
                };
                let mut broker = || rows.take().ok_or_else(invalid);
                let mut reader = NativeRead {
                    context,
                    expected: request.clone(),
                    broker: &mut broker,
                    private: HashMap::new(),
                    collection_micros,
                    limited,
                    error: None,
                };
                let result = reader
                    .collect_activity(&request)
                    .map_err(|_| reader.error.take().unwrap_or_else(invalid))?;
                let result = reader.consume(result)?;
                scope.lease.ensure_active().map_err(|_| invalid())?;
                Ok(result)
            },
        );
        match result {
            Ok(rows) => {
                for row in rows {
                    published.insert((row.agent_id.clone(), row.id.clone()), row);
                }
            }
            Err(_) => failed = true,
        }
    }
    failed |= !grouped.is_empty();
    // A previous source may have been stopped while another source was consumed.
    let latest = sessions.active_sources().unwrap_or_default();
    let rows = order
        .into_iter()
        .filter_map(|key| {
            let valid = scopes.iter().any(|scope| {
                scope.source_id == key.0
                    && scope.lease.ensure_active().is_ok()
                    && latest.iter().any(|s| {
                        s.enabled
                            && s.agent_id == key.0
                            && s.authorization_revision == scope.revision
                    })
            });
            if !valid {
                failed = true;
                return None;
            }
            published.remove(&key)
        })
        .collect();
    (rows, failed)
}
