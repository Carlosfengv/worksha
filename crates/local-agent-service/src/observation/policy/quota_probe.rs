use super::{quota_blocked, rejection};
use crate::billing::{BudgetReset, UpstreamBudget};
use axum::{http::StatusCode, response::Response};
use local_agent_core::{
    Provider, Store,
    model_services::policy::{QuotaProbeClaim, QuotaProbeSource, QuotaProbeState, scopes},
    providers::ConfigError,
};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{sync::Notify, time::Instant};

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum QuotaAdmission {
    Open,
    Recovered,
    WaitingForProbe,
    Cooling,
    Probing,
    Exhausted,
}

impl QuotaAdmission {
    pub(crate) fn display_key(self) -> Option<&'static str> {
        match self {
            Self::Open => None,
            Self::Exhausted => Some("exhausted"),
            Self::WaitingForProbe => Some("recovery"),
            Self::Probing => Some("recovering"),
            Self::Cooling => Some("recoveryCooling"),
            Self::Recovered => Some("recovered"),
        }
    }
}

type ScopeState<'a> = (&'a str, Option<&'a BudgetReset>, QuotaAdmission);

fn guard_scope(source: QuotaProbeSource, account: &str) -> String {
    match source {
        QuotaProbeSource::OpenRouter => format!("openrouter-probe:{account}"),
        QuotaProbeSource::MiniMax => format!("minimax-probe:{account}"),
        QuotaProbeSource::Balance => format!("balance-probe:{account}"),
    }
}

fn scope_states<'a>(
    store: &Store,
    account: &'a str,
    guard: &'a str,
    budget: &'a UpstreamBudget,
    now_ms: i64,
) -> Result<Vec<ScopeState<'a>>, ConfigError> {
    let scopes = || budget.scopes(account).chain(std::iter::once((guard, None)));
    let saved =
        store.service_quota_probes(budget.source, scopes().map(|(scope, _)| scope), now_ms)?;
    Ok(scopes()
        .zip(saved)
        .map(|((scope, reset), state)| {
            let admission = match (state, reset) {
                (Some(QuotaProbeState::Active), _) => QuotaAdmission::Probing,
                (Some(QuotaProbeState::Cooling), _) => QuotaAdmission::Cooling,
                (Some(QuotaProbeState::Recovered(Some(at))), Some(reset))
                    if at == reset.reset_at =>
                {
                    QuotaAdmission::Recovered
                }
                (_, Some(_)) => QuotaAdmission::WaitingForProbe,
                _ => QuotaAdmission::Open,
            };
            (scope, reset, admission)
        })
        .collect())
}

fn highest(states: &[ScopeState<'_>]) -> QuotaAdmission {
    states
        .iter()
        .map(|(_, _, admission)| *admission)
        .max()
        .unwrap_or(QuotaAdmission::Open)
}

pub(crate) fn status(
    store: &Store,
    target: &Provider,
    budget: &UpstreamBudget,
) -> Option<QuotaAdmission> {
    if budget.unavailable {
        return None;
    }
    if budget.exhausted {
        return Some(QuotaAdmission::Exhausted);
    }
    if !budget.track_recovery {
        return Some(QuotaAdmission::Open);
    }
    let account = scopes(target).0;
    let guard = guard_scope(budget.source, &account);
    let states = scope_states(
        store,
        &account,
        &guard,
        budget,
        chrono::Utc::now().timestamp_millis(),
    );
    states.ok().map(|states| highest(&states))
}

struct Probe {
    store: Arc<Mutex<Store>>,
    source: QuotaProbeSource,
    owner: String,
    cooldown: u64,
    settled: bool,
}

fn settle(
    store: &Store,
    source: QuotaProbeSource,
    owner: &str,
    committed: bool,
    seconds: u64,
) -> Result<(), ConfigError> {
    store.settle_service_quota_probe(
        source,
        owner,
        committed,
        chrono::Utc::now().timestamp_millis(),
        milliseconds(seconds),
    )
}

fn milliseconds(seconds: u64) -> i64 {
    seconds.min(3600) as i64 * 1000
}

impl Drop for Probe {
    fn drop(&mut self) {
        if !self.settled
            && let Ok(runtime) = tokio::runtime::Handle::try_current()
        {
            let store = self.store.clone();
            let owner = self.owner.clone();
            let source = self.source;
            let cooldown = self.cooldown;
            runtime.spawn_blocking(move || {
                if let Ok(store) = store.lock() {
                    let _ = settle(&store, source, &owner, false, cooldown);
                }
            });
        }
    }
}

#[derive(Default)]
struct Reservation {
    probe: Mutex<Option<Probe>>,
    started: Notify,
}

#[derive(Clone)]
pub(in crate::observation) struct CheckAccountQuota {
    reservation: Arc<Reservation>,
    store: Arc<Mutex<Store>>,
    cooldown: u64,
    deadline: Option<Instant>,
    stream_window: Duration,
}

impl CheckAccountQuota {
    pub(super) fn new(cooldown: u64, store: Arc<Mutex<Store>>, streamed: bool) -> Self {
        Self {
            reservation: Arc::default(),
            store,
            cooldown,
            deadline: None,
            stream_window: if streamed {
                super::super::delivery::REQUEST_LIMIT
            } else {
                Duration::ZERO
            },
        }
    }

    pub(super) fn with_deadline(mut self, deadline: Instant) -> Self {
        self.deadline = Some(deadline);
        self
    }

    pub(super) async fn wait_for_probe(&self) {
        self.reservation.started.notified().await;
    }

    pub(in crate::observation) fn admit(
        &self,
        store: &Store,
        account: String,
        budget: UpstreamBudget,
        begin_send: impl FnOnce(Option<QuotaProbeClaim<'_>>) -> Result<(), Box<Response>>,
    ) -> Result<(), Box<Response>> {
        if budget.unavailable {
            return Err(unavailable());
        }
        if budget.exhausted {
            return Err(Box::new(quota_blocked()));
        }
        if !budget.track_recovery {
            return begin_send(None);
        }
        let mut probe = self.reservation.probe.lock().map_err(|_| unavailable())?;
        let now = chrono::Utc::now().timestamp_millis();
        let guard = guard_scope(budget.source, &account);
        let states =
            scope_states(store, &account, &guard, &budget, now).map_err(|_| unavailable())?;
        match highest(&states) {
            QuotaAdmission::Probing => {
                return Err(recovery_blocked(
                    "账号额度正在探测恢复，请稍后重试",
                    "recovery_probe_in_progress",
                ));
            }
            QuotaAdmission::Cooling => {
                return Err(recovery_blocked(
                    "账号额度恢复探测正在冷却，请稍后重试",
                    "rate_limit_cooldown",
                ));
            }
            _ => (),
        }
        let mut waiting = Vec::new();
        for (scope, reset, state) in states {
            let Some(reset) = reset else { continue };
            let until = reset.valid_until.saturating_mul(1000);
            match state {
                QuotaAdmission::Recovered => store
                    .retain_service_quota_recovery(budget.source, scope, reset.reset_at, until)
                    .map_err(|_| unavailable())?,
                QuotaAdmission::WaitingForProbe => {
                    waiting.push((scope, Some(reset.reset_at), until))
                }
                _ => (),
            }
        }
        if waiting.is_empty() {
            return begin_send(None);
        }
        if self
            .deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            return Err(Box::new(super::http_recovery_timeout(false)));
        }
        // Keep the initiating account single-flight even if its shared scope
        // disappears while the request is outstanding.
        if !waiting.iter().any(|(scope, _, _)| *scope == account) {
            let until = waiting
                .iter()
                .map(|(_, _, until)| *until)
                .max()
                .unwrap_or(now);
            waiting.push((&guard, None, until));
        }
        let owner = uuid::Uuid::new_v4().to_string();
        begin_send(Some(QuotaProbeClaim {
            source: budget.source,
            owner: &owner,
            scopes: &waiting,
            now_ms: now,
            cooldown_ms: milliseconds(self.cooldown),
            stream_window: self.stream_window,
        }))?;
        *probe = Some(Probe {
            store: self.store.clone(),
            source: budget.source,
            owner,
            cooldown: self.cooldown,
            settled: false,
        });
        if self.deadline.is_some() {
            self.reservation.started.notify_one();
        }
        Ok(())
    }

    pub(super) fn is_probing(&self) -> bool {
        self.reservation
            .probe
            .lock()
            .is_ok_and(|probe| probe.is_some())
    }

    pub(super) fn retry_delay(&self, response: &Response) -> u64 {
        super::retry_seconds(response, self.cooldown)
    }

    pub(super) async fn settle(&self, committed: bool, seconds: u64) {
        let Some(mut probe) = self
            .reservation
            .probe
            .lock()
            .ok()
            .and_then(|mut probe| probe.take())
        else {
            return;
        };
        let owner = probe.owner.clone();
        let source = probe.source;
        probe.settled = crate::blocking::store(self.store.clone(), move |store| {
            settle(store, source, &owner, committed, seconds)
        })
        .await
        .is_ok();
    }
}

fn unavailable() -> Box<Response> {
    let mut response = rejection(StatusCode::SERVICE_UNAVAILABLE, "账号额度恢复状态不可用");
    response
        .extensions_mut()
        .insert(crate::observation::proxy::LocalCapacity);
    Box::new(response)
}

fn recovery_blocked(message: &str, reason: &'static str) -> Box<Response> {
    let mut response = rejection(StatusCode::TOO_MANY_REQUESTS, message);
    response
        .extensions_mut()
        .insert(super::AccountQuotaBlocked(reason));
    Box::new(response)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_expired_fixed_deadline_never_records_a_quota_probe() {
        let directory = tempfile::tempdir().unwrap();
        let store = Arc::new(Mutex::new(Store::open(directory.path()).unwrap()));
        let check = CheckAccountQuota::new(30, store.clone(), false).with_deadline(Instant::now());
        let now = chrono::Utc::now().timestamp();
        let budget = UpstreamBudget::with_key_reset(BudgetReset {
            reset_at: now - 10,
            valid_until: now + 300,
        });
        let result = check
            .admit(&store.lock().unwrap(), "fixed".into(), budget, |_| {
                panic!("an expired probe must not reach the send checkpoint")
            })
            .unwrap_err();
        assert_eq!(result.status(), StatusCode::GATEWAY_TIMEOUT);
        assert_eq!(
            super::super::admission_reason(&result),
            super::super::Reason::Deadline
        );
        assert!(!check.is_probing());
    }

    #[tokio::test]
    async fn cancellation_cools_the_reserved_account_without_blocking_others() {
        let directory = tempfile::tempdir().unwrap();
        let store = Arc::new(Mutex::new(Store::open(directory.path()).unwrap()));
        let budget = || {
            let now = chrono::Utc::now().timestamp();
            UpstreamBudget::with_key_reset(BudgetReset {
                reset_at: now - 10,
                valid_until: now + 300,
            })
        };
        let first = CheckAccountQuota::new(30, store.clone(), false);
        let second = CheckAccountQuota::new(30, store.clone(), false);
        let concurrent = CheckAccountQuota::new(30, store.clone(), false);
        {
            let store = store.lock().unwrap();
            let begin_send = |quota: Option<QuotaProbeClaim<'_>>| {
                store
                    .service_begin_send(None, None, quota)
                    .map_err(|_| unavailable())
            };
            first
                .admit(&store, "first".into(), budget(), begin_send)
                .unwrap();
            second
                .admit(&store, "second".into(), budget(), begin_send)
                .unwrap();
            let mut missing_snapshot = budget();
            missing_snapshot.key_reset = None;
            let blocked = concurrent
                .admit(&store, "first".into(), missing_snapshot, begin_send)
                .unwrap_err();
            assert_eq!(
                blocked
                    .extensions()
                    .get::<super::super::AccountQuotaBlocked>()
                    .unwrap()
                    .0,
                "recovery_probe_in_progress"
            );
        }
        drop(first);
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let state = store
                    .lock()
                    .unwrap()
                    .service_quota_probes(
                        QuotaProbeSource::OpenRouter,
                        ["first"].into_iter(),
                        chrono::Utc::now().timestamp_millis(),
                    )
                    .unwrap()[0];
                if state == Some(QuotaProbeState::Cooling) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        second.settle(true, 0).await;
        let store = store.lock().unwrap();
        assert!(matches!(
            store
                .service_quota_probes(
                    QuotaProbeSource::OpenRouter,
                    ["second"].into_iter(),
                    chrono::Utc::now().timestamp_millis()
                )
                .unwrap()[0],
            Some(QuotaProbeState::Recovered(Some(_)))
        ));
    }
}
