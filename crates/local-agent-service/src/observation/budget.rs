use crate::AppState;
use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use chrono::{DateTime, Utc};
use local_agent_core::{
    Provider, Store, StoreWriter,
    model_services::ContextBudget,
    observation::ModelObservation,
    providers::ConfigError,
    routing::{
        Turn,
        budget::{Amounts, Attempt, Prepare, Quote, Tier},
    },
    sessions::Usage,
};
use serde_json::json;
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub(super) struct Scope {
    pub binding: String,
    pub session: Option<String>,
    pub turn: String,
    pub continuation: bool,
    pub request_id: String,
    pub tier: Tier,
    pub fallback: Option<(local_agent_core::routing::Selection, Amounts)>,
    pub quote_override: Option<Amounts>,
    pub monitor: Option<local_agent_core::routing::loop_guard::Scope>,
}

impl Scope {
    pub fn new(binding: String, turn: &Turn) -> Self {
        Self {
            binding,
            session: turn.session.clone(),
            turn: turn.digest.clone(),
            continuation: turn.continuation,
            request_id: uuid::Uuid::new_v4().to_string(),
            tier: Tier::Primary,
            fallback: None,
            quote_override: None,
            monitor: None,
        }
    }

    fn input<'a>(
        &'a self,
        provider: &'a Provider,
        id: &'a str,
        now: DateTime<Utc>,
        estimate: Amounts,
    ) -> Prepare<'a> {
        Prepare {
            binding_id: &self.binding,
            session: self.session.as_deref(),
            turn: &self.turn,
            request_id: &self.request_id,
            observation_id: id,
            provider_id: &provider.id,
            model: &provider.model,
            selected_tier: self.tier,
            continuation: self.continuation,
            now,
            estimate,
            fallback: self
                .fallback
                .as_ref()
                .map(|(selected, amounts)| (selected, *amounts)),
        }
    }

    pub fn quote(
        &self,
        store: &Store,
        provider: &Provider,
        estimate: Amounts,
        now: DateTime<Utc>,
    ) -> Result<Quote, ConfigError> {
        store.budget_quote(&self.input(provider, "", now, estimate))
    }
}

#[derive(Clone, Copy)]
pub(super) struct RetryWithFallback(pub Amounts);

pub(super) fn error(error: &ConfigError) -> Response {
    let status = match error.code.as_str() {
        "local_agent_budget_exceeded"
        | "budget_grace_exceeded"
        | "budget_fallback_exhausted"
        | "local_agent_loop_circuit_breaker"
        | "local_agent_loop_dedup_capacity"
        | "capacity" => StatusCode::TOO_MANY_REQUESTS,
        "budget_session_required" | "budget_pricing_unavailable" => StatusCode::BAD_REQUEST,
        "chat_key_changed" => StatusCode::UNAUTHORIZED,
        "dispatch_not_committed" | "dispatch_uncertain" | "route_turn_superseded" | "conflict" => {
            StatusCode::CONFLICT
        }
        _ => StatusCode::SERVICE_UNAVAILABLE,
    };
    let mut response = (
        status,
        Json(json!({"error":{"type":"local_observation_error",
        "code":error.code,"message":error.message}})),
    )
        .into_response();
    response
        .extensions_mut()
        .insert(super::proxy::LocalCapacity);
    response
}

pub(super) fn estimate(provider: &Provider, budget: &ContextBudget, now: DateTime<Utc>) -> Amounts {
    let input_tokens = budget.input_tokens;
    let output_tokens = budget.output_tokens;
    let tokens = input_tokens.checked_add(output_tokens);
    let time = now.to_rfc3339();
    let usage = Usage {
        input_tokens: Some(1),
        output_tokens: Some(1),
        cached_input_tokens: Some(0),
        ..Usage::default()
    };
    let price = local_agent_core::provider_cost::calculate(
        Some(&provider.endpoint),
        Some(&provider.model),
        Some(&time),
        "success",
        &usage,
        None,
        Some(&time),
    )
    .price;
    let usd_micros = price.map(|price| {
        // Charges can reach the peak rate during an in-flight attempt.
        let factor = if price.period == "peak" { 1.0 } else { 2.0 };
        ((price.input_per_million * input_tokens as f64
            + price.output_per_million * output_tokens as f64)
            * factor)
            .ceil() as u64
    });
    Amounts { tokens, usd_micros }
}

fn actual(row: &ModelObservation) -> Amounts {
    let usage = &row.usage;
    let tokens = usage
        .input_tokens
        .zip(usage.output_tokens)
        .and_then(|(input, output)| input.checked_add(output))
        .filter(|total| usage.total_tokens.is_none_or(|reported| reported == *total))
        .or_else(|| {
            (usage.input_tokens.is_none() && usage.output_tokens.is_none())
                .then_some(usage.total_tokens)
                .flatten()
        });
    let cost = local_agent_core::provider_cost::calculate(
        Some(&row.endpoint),
        row.routed_model.as_deref(),
        Some(&row.started_at),
        &row.status,
        usage,
        row.reported_cost.as_ref(),
        row.ended_at.as_deref(),
    );
    let usd_micros = if cost.currency == Some("USD")
        && (cost.kind == "actual"
            || row
                .reported_cost
                .as_ref()
                .is_none_or(|cost| cost.currency == "USD"))
    {
        cost.amount
            .filter(|amount| amount.is_finite() && (0.0..=1e9).contains(amount))
            .map(|amount| (amount * 1_000_000.0).round() as u64)
    } else {
        None
    };
    Amounts { tokens, usd_micros }
}

#[derive(Clone)]
pub(super) struct Accounting(Arc<Active>);
struct Active {
    store: StoreWriter,
    attempt: Mutex<Option<String>>,
}
impl Drop for Active {
    fn drop(&mut self) {
        let Ok(attempt) = self.attempt.get_mut() else {
            return;
        };
        if let Some(key) = attempt.take() {
            let _ = persist(self.store.clone(), key, Amounts::default(), None, false);
        }
    }
}

fn persist(
    store: StoreWriter,
    key: String,
    evidence: Amounts,
    upstream: Option<String>,
    confirmed: bool,
) -> Option<tokio::task::JoinHandle<()>> {
    if let Ok(runtime) = tokio::runtime::Handle::try_current() {
        Some(runtime.spawn(async move {
            for retry in 0..3 {
                let key = key.clone();
                let upstream = upstream.clone();
                let done = crate::blocking::store(store.clone(), move |store| {
                    let item = store.budget_complete(&key, evidence, upstream.as_deref())?;
                    if item.sent && confirmed {
                        store.budget_record_response(&key, upstream.as_deref(), true)?;
                    }
                    Ok(())
                })
                .await;
                match done {
                    Ok(()) => return,
                    Err(error)
                        if retry < 2
                            && matches!(
                                error.code.as_str(),
                                "busy" | "unavailable" | "routing_unavailable"
                            ) =>
                    {
                        tokio::time::sleep(std::time::Duration::from_millis(50)).await
                    }
                    Err(error) => {
                        eprintln!("本机预算结算待恢复（{}）", error.code);
                        return;
                    }
                }
            }
        }))
    } else {
        None
    }
}

impl Accounting {
    pub fn new(state: &AppState) -> Self {
        Self(Arc::new(Active {
            store: state.store.clone(),
            attempt: Mutex::new(None),
        }))
    }

    pub fn prepare(
        &self,
        store: &Store,
        scope: &Scope,
        provider: &Provider,
        size: &ContextBudget,
        id: &str,
    ) -> Result<Option<Attempt>, ConfigError> {
        let now = Utc::now();
        let attempt = store.budget_prepare_monitored(
            &scope.input(provider, id, now, estimate(provider, size, now)),
            scope.monitor,
        )?;
        if let Some(attempt) = &attempt {
            *self
                .0
                .attempt
                .lock()
                .map_err(|_| ConfigError::new("unavailable", "预算发送状态暂不可用"))? =
                Some(attempt.attempt_key.clone());
        }
        Ok(attempt)
    }

    pub async fn record(&self, upstream: Option<String>, confirmed: bool) {
        let key = self
            .0
            .attempt
            .lock()
            .ok()
            .and_then(|attempt| attempt.clone());
        if let Some(key) = key {
            let _ = crate::blocking::store(self.0.store.clone(), move |store| {
                store.budget_record_response(&key, upstream.as_deref(), confirmed)
            })
            .await;
        }
    }

    pub fn finish(
        &self,
        row: Option<&ModelObservation>,
        verified: bool,
    ) -> Option<tokio::task::JoinHandle<()>> {
        let mut attempt = self.0.attempt.lock().ok()?;
        if let Some(key) = attempt.take() {
            let evidence = row
                .filter(|_| verified)
                .map_or_else(Amounts::default, actual);
            let upstream = row.and_then(|row| row.upstream_request_id.clone());
            let confirmed = verified
                && row.is_some_and(|row| matches!(row.status.as_str(), "success" | "incomplete"));
            persist(self.0.store.clone(), key, evidence, upstream, confirmed)
        } else {
            None
        }
    }
}
