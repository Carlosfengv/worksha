use super::{Attempt, Selection};
use crate::observation::Observation;
use axum::http::StatusCode;
use local_agent_core::{
    model_services::policy::{
        Dispatch, RouteDecision, RouteDecisionReason as Reason, RouteTrace, RoutingPolicy,
    },
    observation::{ModelObservation, ROUTE_TRACE_ID_PREFIX},
};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

/// Metadata follows the logical request without opening an upstream attempt.
pub(in crate::observation) struct Journal {
    observation: Arc<Observation>,
    row: Option<ModelObservation>,
    trace: RouteTrace,
    uncertain: bool,
    sent: Arc<AtomicBool>,
}

impl Journal {
    pub(in crate::observation) fn new(
        observation: Arc<Observation>,
        mut row: ModelObservation,
    ) -> (Self, Attempt) {
        let logical_id = format!("model-service-{}", uuid::Uuid::new_v4());
        row.id = format!("{ROUTE_TRACE_ID_PREFIX}{logical_id}");
        row.logical_id = Some(logical_id.clone());
        (
            Self {
                observation,
                row: Some(row),
                trace: RouteTrace::default(),
                uncertain: false,
                sent: Arc::new(AtomicBool::new(false)),
            },
            Attempt { logical_id },
        )
    }

    pub(super) fn extend(&mut self, decisions: impl IntoIterator<Item = RouteDecision>) {
        self.trace.extend(decisions);
    }

    pub(super) fn candidate(&mut self, target: &Selection, policy: &RoutingPolicy, reason: Reason) {
        self.remaining(std::iter::once(target), policy, reason);
    }

    pub(super) fn note(&mut self, target: &Selection, reason: Reason) {
        if let Some(service) = &target.service {
            self.extend([RouteDecision::new(service, None, reason)]);
        }
    }

    pub(super) fn remaining<'a>(
        &mut self,
        candidates: impl IntoIterator<Item = &'a Selection>,
        policy: &RoutingPolicy,
        reason: Reason,
    ) {
        self.extend(candidates.into_iter().filter_map(|target| {
            target
                .service
                .as_ref()
                .map(|service| RouteDecision::candidate(service, policy, reason))
        }));
    }

    pub(super) fn uncertain(&mut self) {
        self.uncertain = true;
    }

    pub(in crate::observation) fn duplicate(&mut self, target: &Selection) {
        self.note(target, Reason::OutcomeUnknown);
        self.uncertain();
        self.finish(StatusCode::CONFLICT);
    }

    pub(super) fn send_evidence(
        &self,
        pending: Option<(Arc<Dispatch>, i64)>,
        check_access: bool,
    ) -> SendEvidence {
        SendEvidence {
            candidate: Arc::new(AtomicBool::new(false)),
            request: self.sent.clone(),
            pending,
            check_access,
        }
    }

    pub(in crate::observation) fn finish(&mut self, status: StatusCode) {
        self.trace.http_status = Some(status.as_u16());
        self.persist(if self.uncertain {
            "interrupted"
        } else if status.is_success() {
            "success"
        } else {
            "error"
        });
    }

    fn persist(&mut self, status: &str) {
        let Some(mut row) = self.row.take() else {
            return;
        };
        if self.trace.decisions.is_empty() {
            return;
        }
        let Some(routing) = &mut row.routing else {
            return;
        };
        self.trace.sent_upstream = self.sent.load(Ordering::Acquire);
        routing.trace = Some(std::mem::take(&mut self.trace));
        row.ended_at = Some(chrono::Utc::now().to_rfc3339());
        row.status = status.into();
        row.http_status = routing.trace.as_ref().and_then(|trace| trace.http_status);
        self.observation.hub.publish_route_trace(row);
    }
}

impl Drop for Journal {
    fn drop(&mut self) {
        self.persist("interrupted");
    }
}

#[derive(Clone)]
pub(in crate::observation) struct SendEvidence {
    candidate: Arc<AtomicBool>,
    request: Arc<AtomicBool>,
    pub pending: Option<(Arc<Dispatch>, i64)>,
    pub check_access: bool,
}

impl SendEvidence {
    pub(in crate::observation) fn mark_sent(&self) {
        self.request.store(true, Ordering::Release);
        self.candidate.store(true, Ordering::Release);
    }

    pub(super) fn was_sent(&self) -> bool {
        self.candidate.load(Ordering::Acquire)
    }
}
