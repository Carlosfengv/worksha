//! Keep request ownership through response completion, including streaming bodies.
use local_agent_core::plugin::PluginError;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Condvar, Mutex},
    time::{Duration, Instant},
};

struct State {
    accepting: bool,
    switching: Option<Scope>,
    in_flight: usize,
    requests: BTreeMap<Option<&'static str>, usize>,
}
pub(crate) enum Scope {
    All,
    Plugins(BTreeSet<String>),
}
impl Scope {
    fn blocks(&self, owner: Option<&str>) -> bool {
        match self {
            Self::All => true,
            Self::Plugins(plugins) if plugins.is_empty() => false,
            Self::Plugins(plugins) => owner.is_none_or(|id| plugins.contains(id)),
        }
    }
}
pub(crate) struct Intake {
    state: Mutex<State>,
    changed: Condvar,
}
pub(crate) struct Permit(Arc<Intake>, Option<&'static str>);
pub(crate) struct Transition {
    intake: Arc<Intake>,
    previous: bool,
    completed: bool,
}
impl Intake {
    pub(crate) fn new(accepting: bool) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(State {
                accepting,
                switching: None,
                in_flight: 0,
                requests: BTreeMap::new(),
            }),
            changed: Condvar::new(),
        })
    }
    #[cfg(test)]
    pub(crate) fn acquire(self: &Arc<Self>) -> Option<Permit> {
        self.acquire_for(None)
    }
    pub(crate) fn acquire_for(self: &Arc<Self>, owner: Option<&'static str>) -> Option<Permit> {
        let mut state = self.state.lock().ok()?;
        if !state.accepting
            || state
                .switching
                .as_ref()
                .is_some_and(|scope| scope.blocks(owner))
            || state.in_flight >= 4096
        {
            return None;
        }
        state.in_flight += 1;
        *state.requests.entry(owner).or_default() += 1;
        Some(Permit(self.clone(), owner))
    }
    #[cfg(test)]
    pub(crate) fn transition(
        self: &Arc<Self>,
        budget: Duration,
    ) -> Result<Transition, PluginError> {
        self.transition_scope(budget, Scope::All)
    }
    pub(crate) fn transition_scope(
        self: &Arc<Self>,
        budget: Duration,
        scope: Scope,
    ) -> Result<Transition, PluginError> {
        let mut state = self.state.lock().map_err(|_| unavailable())?;
        if state.switching.is_some() {
            return Err(PluginError::new(
                "busy",
                "另一个组合切换正在进行，请先核查其结果",
            ));
        }
        let previous = state.accepting;
        state.switching = Some(scope);
        let mut transition = Transition {
            intake: self.clone(),
            previous,
            completed: false,
        };
        let deadline = Instant::now() + budget;
        while state.requests.keys().any(|owner| {
            state
                .switching
                .as_ref()
                .expect("pending transition")
                .blocks(*owner)
        }) {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                state.switching = None;
                transition.completed = true;
                return Err(PluginError::new(
                    "busy",
                    "仍有业务请求或流式响应进行中，请等待结束后切换组合",
                ));
            }
            let (current, _) = self
                .changed
                .wait_timeout(state, remaining)
                .map_err(|_| unavailable())?;
            state = current;
        }
        drop(state);
        Ok(transition)
    }
}
fn unavailable() -> PluginError {
    PluginError::new("unavailable", "业务接入作用域暂不可用")
}
impl Drop for Permit {
    fn drop(&mut self) {
        let mut state = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
        state.in_flight = state.in_flight.saturating_sub(1);
        if let Some(count) = state.requests.get_mut(&self.1) {
            *count -= 1;
            if *count == 0 {
                state.requests.remove(&self.1);
            }
        }
        self.0.changed.notify_all();
    }
}
impl Transition {
    /// Durable changes cannot use the previous admission state on failure.
    pub(crate) fn keep_closed_on_failure(&mut self) {
        self.previous = false;
    }

    pub(crate) fn block_all(&self) {
        self.intake
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .switching = Some(Scope::All);
    }

    pub(crate) fn commit(mut self, accepting: bool) {
        let mut state = self.intake.state.lock().unwrap_or_else(|e| e.into_inner());
        state.accepting = accepting;
        state.switching = None;
        self.completed = true;
        self.intake.changed.notify_all();
    }
}
impl Drop for Transition {
    fn drop(&mut self) {
        if !self.completed {
            let mut state = self.intake.state.lock().unwrap_or_else(|e| e.into_inner());
            state.accepting = self.previous;
            state.switching = None;
            self.intake.changed.notify_all();
        }
    }
}

pub(crate) struct ScopedBody<P = Permit> {
    body: axum::body::Body,
    _permit: P,
}
impl<P> ScopedBody<P> {
    pub(crate) fn new(body: axum::body::Body, permit: P) -> Self {
        Self {
            body,
            _permit: permit,
        }
    }
}
impl<P: Unpin> axum::body::HttpBody for ScopedBody<P> {
    type Data = axum::body::Bytes;
    type Error = axum::Error;
    fn poll_frame(
        mut self: std::pin::Pin<&mut Self>,
        context: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Result<http_body::Frame<Self::Data>, Self::Error>>> {
        std::pin::Pin::new(&mut self.body).poll_frame(context)
    }
    fn is_end_stream(&self) -> bool {
        self.body.is_end_stream()
    }
    fn size_hint(&self) -> http_body::SizeHint {
        self.body.size_hint()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn draining_rejects_new_requests_and_timeout_restores_the_old_intake() {
        let intake = Intake::new(true);
        let permit = intake.acquire().unwrap();
        assert!(intake.transition(Duration::from_millis(1)).is_err());
        drop(permit);
        let next = intake.acquire().unwrap();
        drop(next);
        let transition = intake.transition(Duration::from_secs(1)).unwrap();
        assert!(intake.acquire().is_none());
        drop(transition);
        assert!(intake.acquire().is_some());
    }
    #[test]
    fn a_scoped_drain_rejects_affected_and_unclassified_requests_only() {
        let intake = Intake::new(true);
        let unrelated = intake.acquire_for(Some("gateway")).unwrap();
        let affected = intake.acquire_for(Some("builtin.projects")).unwrap();
        let scope = || Scope::Plugins(BTreeSet::from(["builtin.projects".into()]));
        assert!(intake.transition_scope(Duration::ZERO, scope()).is_err());
        drop(affected);
        let transition = intake.transition_scope(Duration::ZERO, scope()).unwrap();
        assert!(intake.acquire_for(Some("builtin.projects")).is_none());
        assert!(intake.acquire().is_none());
        assert!(intake.acquire_for(Some("gateway")).is_some());
        transition.commit(true);
        drop(unrelated);
    }
    #[test]
    fn recovery_keeps_new_calls_closed_and_can_reopen_with_a_new_transition() {
        let intake = Intake::new(true);
        intake
            .transition(Duration::from_secs(1))
            .unwrap()
            .commit(false);
        assert!(intake.acquire().is_none());
        intake
            .transition(Duration::from_secs(1))
            .unwrap()
            .commit(true);
        assert!(intake.acquire().is_some());
    }
    #[tokio::test]
    async fn a_stream_body_owns_the_permit_until_completion_or_drop() {
        let intake = Intake::new(true);
        let body = ScopedBody::new(axum::body::Body::from("fixture"), intake.acquire().unwrap());
        assert!(intake.transition(Duration::ZERO).is_err());
        let data = axum::body::to_bytes(axum::body::Body::new(body), 100)
            .await
            .unwrap();
        assert_eq!(data, "fixture");
        intake.transition(Duration::ZERO).unwrap().commit(true);
        let body = ScopedBody::new(axum::body::Body::empty(), intake.acquire().unwrap());
        drop(body);
        assert!(intake.transition(Duration::ZERO).is_ok());
    }
}
