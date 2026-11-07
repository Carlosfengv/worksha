//! Keep request ownership through response completion, including streaming bodies.
use local_agent_core::plugin::PluginError;
use std::{
    sync::{Arc, Condvar, Mutex},
    time::{Duration, Instant},
};

struct State {
    accepting: bool,
    switching: bool,
    in_flight: usize,
}
pub(crate) struct Intake {
    state: Mutex<State>,
    changed: Condvar,
}
pub(crate) struct Permit(Arc<Intake>);
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
                switching: false,
                in_flight: 0,
            }),
            changed: Condvar::new(),
        })
    }
    pub(crate) fn acquire(self: &Arc<Self>) -> Option<Permit> {
        let mut state = self.state.lock().ok()?;
        if !state.accepting || state.switching || state.in_flight >= 4096 {
            return None;
        }
        state.in_flight += 1;
        Some(Permit(self.clone()))
    }
    pub(crate) fn transition(
        self: &Arc<Self>,
        budget: Duration,
    ) -> Result<Transition, PluginError> {
        let mut state = self.state.lock().map_err(|_| unavailable())?;
        if state.switching {
            return Err(PluginError::new(
                "busy",
                "另一个组合切换正在进行，请先核查其结果",
            ));
        }
        let previous = state.accepting;
        state.switching = true;
        state.accepting = false;
        let mut transition = Transition {
            intake: self.clone(),
            previous,
            completed: false,
        };
        let deadline = Instant::now() + budget;
        while state.in_flight > 0 {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                state.switching = false;
                state.accepting = previous;
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
        self.0.changed.notify_all();
    }
}
impl Transition {
    pub(crate) fn commit(mut self, accepting: bool) {
        let mut state = self.intake.state.lock().unwrap_or_else(|e| e.into_inner());
        state.accepting = accepting;
        state.switching = false;
        self.completed = true;
        self.intake.changed.notify_all();
    }
}
impl Drop for Transition {
    fn drop(&mut self) {
        if !self.completed {
            let mut state = self.intake.state.lock().unwrap_or_else(|e| e.into_inner());
            state.accepting = self.previous;
            state.switching = false;
            self.intake.changed.notify_all();
        }
    }
}

pub(crate) struct ScopedBody {
    body: axum::body::Body,
    _permit: Permit,
}
impl ScopedBody {
    pub(crate) fn new(body: axum::body::Body, permit: Permit) -> Self {
        Self {
            body,
            _permit: permit,
        }
    }
}
impl axum::body::HttpBody for ScopedBody {
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
