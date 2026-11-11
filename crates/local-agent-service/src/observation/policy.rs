//! Bounded service attempts before downstream commitment. Committed turns stay pinned.
use crate::AppState;
use axum::{
    body::{Body, Bytes},
    extract::Request,
    http::{Extensions, HeaderMap, HeaderValue, StatusCode, request::Parts},
    response::{IntoResponse, Response},
};
use futures_util::StreamExt;
use local_agent_core::{
    Provider, Store,
    model_services::policy::{
        AccountPlanningHint, BalanceRejection, Dispatch, FailedAttemptAdmission, HttpProbeClaim,
        IntentTurn, ModelLimit, OpenRouterBudgetRejection, QuotaProbeClaim, RouteDecision,
        RouteDecisionReason as Reason, RoutingPolicy, RuleFacts, access_scopes, scopes,
    },
    providers::{ConfigError, upstream_accounts::AccountRef},
    routing::{Selection, Turn},
};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

pub(super) mod balance_rejection;
mod health;
pub(super) mod intent;
mod nested;
pub(super) mod openrouter_budget;
mod quota_probe;
mod recovery;
#[cfg(test)]
mod tests;
pub(super) mod trace;
pub(crate) use health::{Cooldown, CooldownSnapshot, unknown_health};
pub(super) use quota_probe::CheckAccountQuota;
pub(crate) use quota_probe::{QuotaAdmission, status as quota_admission};

#[derive(Clone)]
pub(super) struct Selected(pub Selection);
#[derive(Clone)]
pub(super) struct Attempt {
    pub logical_id: String,
}
#[derive(Clone)]
struct PermanentReject;
#[derive(Clone)]
struct AccountQuotaBlocked(&'static str);
#[derive(Clone)]
pub(super) struct OpenRouterReply;

#[derive(Clone)]
pub(super) enum ConfirmedQuota {
    OpenRouter(OpenRouterBudgetRejection),
    Balance(BalanceRejection),
}

impl ConfirmedQuota {
    fn admission(&self) -> FailedAttemptAdmission<'_> {
        match self {
            Self::OpenRouter(rejection) => FailedAttemptAdmission::OpenRouterBudget(rejection),
            Self::Balance(rejection) => FailedAttemptAdmission::Balance(rejection),
        }
    }

    fn record(&self, store: &Store) -> Result<(), ConfigError> {
        match self {
            Self::OpenRouter(rejection) => store.record_openrouter_budget_rejection(rejection),
            Self::Balance(rejection) => {
                store.record_upstream_balance(&rejection.credential, rejection.observed_at, false)
            }
        }
    }
}

fn admission_reason(response: &Response) -> Reason {
    if let Some(reason) = response.extensions().get::<Reason>() {
        return *reason;
    }
    if let Some(access) = response.extensions().get::<AccessRejection>() {
        return access.trace_reason();
    }
    match response
        .extensions()
        .get::<AccountQuotaBlocked>()
        .map(|blocked| blocked.0)
    {
        Some("account_quota_exhausted") => Reason::AccountQuotaExhausted,
        Some("recovery_probe_in_progress") => Reason::RecoveryInProgress,
        Some("rate_limit_cooldown") => Reason::RateLimitCooldown,
        _ => Reason::AdmissionFailed,
    }
}

#[derive(Clone, Copy, serde::Deserialize, serde::Serialize)]
pub(super) enum AccessRejection {
    Credential,
    Model,
}

impl AccessRejection {
    fn trace_reason(self) -> Reason {
        match self {
            Self::Credential => Reason::AccessCredentialRejected,
            Self::Model => Reason::AccessModelDenied,
        }
    }

    fn reason(self) -> &'static str {
        match self {
            Self::Credential => "upstream_credential_rejected",
            Self::Model => "upstream_model_access_denied",
        }
    }

    pub(super) fn message(self) -> &'static str {
        match self {
            Self::Credential => "上游拒绝了此账号的 API Key",
            Self::Model => "上游拒绝此账号访问选定模型",
        }
    }

    pub(super) fn response(self) -> Response {
        let status = match self {
            Self::Credential => StatusCode::UNAUTHORIZED,
            Self::Model => StatusCode::FORBIDDEN,
        };
        let mut response = rejection(status, self.message());
        response.extensions_mut().insert(self);
        response
    }

    fn active(
        store: &local_agent_core::Store,
        account: &str,
        model: &str,
    ) -> Result<Option<Self>, local_agent_core::providers::ConfigError> {
        if store.service_cooldown(account)?.is_some() {
            return Ok(Some(Self::Credential));
        }
        Ok(store.service_cooldown(model)?.map(|_| Self::Model))
    }

    fn scope(self, account: String, model: String) -> String {
        match self {
            Self::Credential => account,
            Self::Model => model,
        }
    }

    async fn record(
        self,
        state: &AppState,
        scope: String,
        cooldown: u64,
    ) -> Result<(), local_agent_core::providers::ConfigError> {
        crate::blocking::store(state.store.clone(), move |store| {
            store.service_set_cooldown(
                &scope,
                chrono::Utc::now().timestamp_millis() + (cooldown * 1000) as i64,
                self.reason(),
            )
        })
        .await
    }
}

fn permanent_error(code: &str) -> bool {
    matches!(
        code,
        "insufficient_quota"
            | "insufficient_balance"
            | "exceeded_current_quota_error"
            | "billing_hard_limit_reached"
            | "content_policy_violation"
    )
}

pub(super) fn mark_upstream_rejection(
    status: StatusCode,
    bytes: &[u8],
    extensions: &mut Extensions,
) {
    // The typed envelope rejects duplicate evidence fields instead of choosing
    // whichever conflicting code happened to be last.
    #[derive(serde::Deserialize)]
    struct Envelope {
        error: Evidence,
    }
    #[derive(serde::Deserialize)]
    struct Evidence {
        code: Option<serde_json::Value>,
        #[serde(rename = "type")]
        kind: Option<serde_json::Value>,
    }
    let Ok(Envelope { error }) = serde_json::from_slice(bytes) else {
        return;
    };
    let code = error.code.as_ref().and_then(serde_json::Value::as_str);
    let kind = error.kind.as_ref().and_then(serde_json::Value::as_str);
    if code.is_some_and(permanent_error) || kind.is_some_and(permanent_error) {
        extensions.insert(PermanentReject);
    } else {
        if error.kind.is_some() && kind.is_none() {
            return;
        }
        // An optional error type must agree with the wire status and exact code.
        let access = match (status, code, kind) {
            (
                StatusCode::UNAUTHORIZED,
                Some("invalid_api_key"),
                None | Some("authentication_error" | "invalid_api_key" | "invalid_request_error"),
            ) => AccessRejection::Credential,
            (
                StatusCode::FORBIDDEN,
                Some("model_access_denied" | "model_not_found"),
                None
                | Some(
                    "permission_error"
                    | "permission_denied"
                    | "not_found_error"
                    | "invalid_request_error"
                    | "model_access_denied"
                    | "model_not_found",
                ),
            ) => AccessRejection::Model,
            _ => return,
        };
        extensions.insert(access);
    }
}
#[derive(Default)]
pub(super) struct Runtime {
    turns: Mutex<HashSet<String>>,
    resources: Mutex<HashMap<String, Resource>>,
}

const RECOVERY_PROBE_PREFIX: &str = "probe:";

struct Resource {
    active: u32,
    reserved: usize,
    started: VecDeque<Instant>,
}
impl Resource {
    fn at_limit(&self, limit: &ModelLimit, recent: usize) -> bool {
        (limit.max_concurrent > 0 && self.active >= limit.max_concurrent)
            || (limit.requests_per_minute > 0
                && recent + self.reserved >= limit.requests_per_minute as usize)
    }
}
pub(super) struct TurnGuard {
    runtime: Arc<Runtime>,
    key: String,
}
impl Drop for TurnGuard {
    fn drop(&mut self) {
        if let Ok(mut turns) = self.runtime.turns.lock() {
            turns.remove(&self.key);
        }
    }
}

pub(super) struct CooldownProbe {
    guard: Option<TurnGuard>,
    store: Arc<Mutex<Store>>,
    checkpointed: AtomicBool,
    owner: String,
    scopes: [String; 4],
    checked_at: i64,
    cooldown_ms: i64,
}

impl Drop for CooldownProbe {
    fn drop(&mut self) {
        if !self.checkpointed.load(Ordering::Acquire) {
            return;
        }
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let store = self.store.clone();
        let owner = self.owner.clone();
        let guard = self.guard.take();
        let now = chrono::Utc::now().timestamp_millis();
        // One worker per unsettled send. Keep the account reservation until
        // SQLite has converted the checkpoint to a cooldown.
        runtime.spawn_blocking(move || {
            let _guard = guard;
            if let Ok(store) = store.lock() {
                let _ = store.interrupt_service_cooldown_probe(&owner, now);
            }
        });
    }
}

enum ProbeAdmission {
    Ready(Option<Arc<CooldownProbe>>),
    Busy,
}

impl CooldownProbe {
    /// Called on the store worker so reading expiry and reserving a probe are
    /// ordered with recovery and all cooldown writes.
    fn admit(
        store: &Store,
        state: &AppState,
        scopes: [String; 4],
        cooldown: u64,
    ) -> Result<ProbeAdmission, ConfigError> {
        let checked_at = chrono::Utc::now().timestamp_millis();
        if !store.service_recovery_pending(scopes.each_ref().map(String::as_str), checked_at)? {
            return Ok(ProbeAdmission::Ready(None));
        }
        let Ok(guard) = state.observation.policy.probe(&scopes[0]) else {
            return Ok(ProbeAdmission::Busy);
        };
        Ok(ProbeAdmission::Ready(Some(Arc::new(Self {
            guard: Some(guard),
            store: state.store.clone(),
            checkpointed: AtomicBool::new(false),
            owner: uuid::Uuid::new_v4().to_string(),
            scopes,
            checked_at,
            cooldown_ms: (cooldown * 1000) as i64,
        }))))
    }

    pub(super) fn begin_send(
        store: &Store,
        probe: Option<&Self>,
        attempt: Option<(&Dispatch, i64)>,
        quota: Option<QuotaProbeClaim<'_>>,
    ) -> Result<(), ConfigError> {
        store.service_begin_send(
            attempt,
            probe.map(|probe| HttpProbeClaim {
                owner: &probe.owner,
                scopes: probe.scopes.each_ref().map(String::as_str),
                checked_at: probe.checked_at,
                cooldown_ms: probe.cooldown_ms,
            }),
            quota,
        )?;
        if let Some(probe) = probe {
            probe.checkpointed.store(true, Ordering::Release);
        }
        Ok(())
    }

    pub(super) fn check_http(
        store: &Store,
        target: &Provider,
        probe: Option<&Self>,
        check_access: bool,
    ) -> Result<Option<Response>, ConfigError> {
        let (account, model) = scopes(target);
        let (access_account, access_model) = if check_access {
            let (account, model) = access_scopes(target);
            if let Some(access) = AccessRejection::active(store, &account, &model)? {
                return Ok(Some(access.response()));
            }
            (account, model)
        } else {
            (String::new(), String::new())
        };
        if let Some((until, cause)) = store
            .service_cooldown(&account)?
            .or(store.service_cooldown(&model)?)
        {
            return Ok(Some(http_cooling(until, &cause)));
        }
        let scopes = [
            account.as_str(),
            model.as_str(),
            access_account.as_str(),
            access_model.as_str(),
        ];
        if !Self::ready_at_send(store, scopes, probe)? {
            return Ok(Some(http_recovery_wait()));
        }
        Ok(None)
    }

    fn ready_at_send(
        store: &Store,
        scopes: [&str; 4],
        probe: Option<&Self>,
    ) -> Result<bool, ConfigError> {
        if !store.service_recovery_pending(scopes, chrono::Utc::now().timestamp_millis())? {
            return Ok(true);
        }
        match probe {
            Some(probe) => store.service_recovery_pending(scopes, probe.checked_at),
            None => Ok(false),
        }
    }

    fn clear(&self, store: &Store) -> Result<(), ConfigError> {
        store.service_clear_cooldown_probe(&self.owner)?;
        self.checkpointed.store(false, Ordering::Release);
        Ok(())
    }

    async fn finished(self: Arc<Self>, state: &AppState) -> Result<(), ConfigError> {
        crate::blocking::store(state.store.clone(), move |store| self.clear(store)).await
    }

    async fn recovered(self: Arc<Self>, state: &AppState) {
        let _ = crate::blocking::store(state.store.clone(), move |store| {
            store.service_recover_cooldowns(
                &self.scopes.each_ref().map(String::as_str),
                self.checked_at,
            )?;
            self.clear(store)
        })
        .await;
    }
}

async fn settle_http_rejection(
    state: &AppState,
    probe: Option<Arc<CooldownProbe>>,
    [account, model]: [String; 2],
    failure: Failure,
    retry: u64,
) -> Result<(), ConfigError> {
    let (scope, reason) = match failure {
        Failure::Rate => (account, "upstream_rate_limit"),
        Failure::Transient => (model, "upstream_failure"),
        Failure::Uncertain => return Ok(()),
        _ => {
            return match probe {
                Some(probe) => probe.finished(state).await,
                None => Ok(()),
            };
        }
    };
    let until = chrono::Utc::now().timestamp_millis() + (retry * 1000) as i64;
    crate::blocking::store(state.store.clone(), move |store| {
        store.service_set_cooldown(&scope, until, reason)?;
        if let Some(probe) = probe {
            probe.clear(store)?;
        }
        Ok(())
    })
    .await
}
struct Slot {
    runtime: Arc<Runtime>,
    key: String,
    // The resources lock serializes this flag with the shared counters.
    reserved: AtomicBool,
}
impl Drop for Slot {
    fn drop(&mut self) {
        if let Ok(mut map) = self.runtime.resources.lock()
            && let Some(r) = map.get_mut(&self.key)
        {
            r.active = r.active.saturating_sub(1);
            if self.reserved.load(Ordering::Relaxed) {
                r.reserved -= 1;
            }
        }
    }
}

#[derive(Clone)]
pub(super) struct SendSlots(Vec<Arc<Slot>>);

impl SendSlots {
    pub(super) fn mark_sent(&self) -> Result<(), &'static str> {
        let Some(first) = self.0.first() else {
            return Ok(());
        };
        let mut resources = first
            .runtime
            .resources
            .lock()
            .map_err(|_| "路由状态不可用")?;
        let now = Instant::now();
        for slot in &self.0 {
            if slot.reserved.swap(false, Ordering::Relaxed) {
                let resource = resources.get_mut(&slot.key).expect("active route slot");
                resource.reserved -= 1;
                resource.started.push_back(now);
            }
        }
        Ok(())
    }
}

impl Runtime {
    fn account_has_capacity(&self, key: &str, limit: &ModelLimit) -> bool {
        if limit.max_concurrent == 0 && limit.requests_per_minute == 0 {
            return true;
        }
        self.resources.lock().is_ok_and(|resources| {
            resources.get(key).is_none_or(|resource| {
                let now = Instant::now();
                let expired = resource
                    .started
                    .partition_point(|time| now.duration_since(*time) >= Duration::from_secs(60));
                !resource.at_limit(limit, resource.started.len() - expired)
            })
        })
    }

    pub(super) fn reserve_turn(
        self: &Arc<Self>,
        binding: &str,
        turn: &Turn,
    ) -> Result<TurnGuard, &'static str> {
        self.turn(format!(
            "{}|{}|{}",
            binding,
            turn.session.as_deref().unwrap_or("unknown"),
            turn.digest
        ))
    }

    fn probe(self: &Arc<Self>, account_scope: &str) -> Result<TurnGuard, &'static str> {
        self.turn(format!("{RECOVERY_PROBE_PREFIX}{account_scope}"))
    }

    fn probe_active(&self, account_scope: &str) -> Option<bool> {
        let key = format!("{RECOVERY_PROBE_PREFIX}{account_scope}");
        Some(self.turns.lock().ok()?.contains(&key))
    }

    fn turn(self: &Arc<Self>, key: String) -> Result<TurnGuard, &'static str> {
        let mut turns = self.turns.lock().map_err(|_| "路由状态不可用")?;
        if turns.len() >= 4096 || !turns.insert(key.clone()) {
            return Err("同轮请求仍在执行，请等待完成；未重复发送");
        }
        Ok(TurnGuard {
            runtime: self.clone(),
            key,
        })
    }
    fn slot(self: &Arc<Self>, key: String, limit: &ModelLimit) -> Result<Arc<Slot>, &'static str> {
        let mut map = self.resources.lock().map_err(|_| "路由状态不可用")?;
        let now = Instant::now();
        map.retain(|_, r| {
            r.active > 0
                || r.started
                    .back()
                    .is_some_and(|t| now.duration_since(*t) < Duration::from_secs(60))
        });
        if map.len() >= 4096 && !map.contains_key(&key) {
            return Err("本机来源计数容量已满");
        }
        let r = map.entry(key.clone()).or_insert_with(|| Resource {
            active: 0,
            reserved: 0,
            started: VecDeque::new(),
        });
        while r
            .started
            .front()
            .is_some_and(|t| now.duration_since(*t) >= Duration::from_secs(60))
        {
            r.started.pop_front();
        }
        let used = r.started.len() + r.reserved;
        if r.at_limit(limit, r.started.len()) {
            return Err("来源已达到配置的并发或每分钟请求限制");
        }
        if used >= 1_000_000 {
            return Err("来源请求频率计数已满");
        }
        r.active += 1;
        r.reserved += 1;
        Ok(Arc::new(Slot {
            runtime: self.clone(),
            key,
            reserved: AtomicBool::new(true),
        }))
    }
}

fn account_planning_hint(
    store: &Store,
    billing: &Mutex<crate::billing::Billing>,
    runtime: &Runtime,
    target: &Provider,
    reference: &AccountRef,
    limit: &ModelLimit,
    now: chrono::DateTime<chrono::Utc>,
) -> AccountPlanningHint {
    let blocked = |reason| AccountPlanningHint {
        quota: None,
        blocked: Some(reason),
    };
    let Ok(billing) = billing.lock() else {
        return blocked(Reason::AdmissionFailed);
    };
    let budget = billing.route_budget(store, target, now);
    match quota_admission(store, target, &budget) {
        None => return blocked(Reason::AdmissionFailed),
        Some(QuotaAdmission::Exhausted) => return blocked(Reason::AccountQuotaExhausted),
        Some(QuotaAdmission::Cooling) => return blocked(Reason::RateLimitCooldown),
        Some(QuotaAdmission::Probing) => return blocked(Reason::RecoveryInProgress),
        _ => (),
    }
    let (account_scope, model_scope) = scopes(target);
    match runtime.probe_active(&account_scope) {
        None => return blocked(Reason::AdmissionFailed),
        Some(true) => {
            let (access_account, access_model) = access_scopes(target);
            let keys = [
                account_scope.as_str(),
                model_scope.as_str(),
                access_account.as_str(),
                access_model.as_str(),
            ];
            match store.service_recovery_pending(keys, chrono::Utc::now().timestamp_millis()) {
                Ok(true) => return blocked(Reason::RecoveryInProgress),
                Ok(false) => (),
                Err(_) => return blocked(Reason::AdmissionFailed),
            }
        }
        Some(false) => (),
    }
    if !runtime.account_has_capacity(&model_scope, limit) {
        return blocked(Reason::ConfiguredRateLimit);
    }
    AccountPlanningHint {
        quota: billing.upstream_quota(target, Some(reference), now),
        blocked: None,
    }
}
pub(super) fn rejection(status: StatusCode, message: &str) -> Response {
    let kind = match status {
        StatusCode::BAD_REQUEST => "invalid_request_error",
        StatusCode::TOO_MANY_REQUESTS => "rate_limit_error",
        StatusCode::UNAUTHORIZED => "authentication_error",
        StatusCode::FORBIDDEN => "permission_error",
        _ => "api_error",
    };
    let mut r = (
        status,
        axum::Json(serde_json::json!({"type":"error","error":{"type":kind,"message":message}})),
    )
        .into_response();
    if status == StatusCode::TOO_MANY_REQUESTS {
        r.headers_mut()
            .insert("retry-after", axum::http::HeaderValue::from_static("30"));
    }
    r
}

fn http_cooling(until_ms: i64, cause: &str) -> Response {
    let reason = if cause == "upstream_rate_limit" {
        Reason::RateLimitCooldown
    } else {
        Reason::FailureCooldown
    };
    let remaining = until_ms.saturating_sub(chrono::Utc::now().timestamp_millis());
    let seconds = (remaining.saturating_add(999) / 1000).clamp(1, 3600);
    let mut response = rejection(
        StatusCode::TOO_MANY_REQUESTS,
        "此上游连接正在冷却，请稍后重试",
    );
    response.extensions_mut().insert(reason);
    response
        .headers_mut()
        .insert("retry-after", HeaderValue::from(seconds));
    response
}

fn http_recovery_wait() -> Response {
    let mut response = rejection(
        StatusCode::TOO_MANY_REQUESTS,
        "此上游连接需要恢复探测，请稍后重试",
    );
    response.extensions_mut().insert(Reason::RecoveryInProgress);
    response
        .headers_mut()
        .insert("retry-after", HeaderValue::from_static("1"));
    response
}

fn http_recovery_timeout(sent: bool) -> Response {
    let mut response = rejection(
        StatusCode::GATEWAY_TIMEOUT,
        if sent {
            "账号恢复探测等待超时，上游结果未确认"
        } else {
            "账号恢复探测准备超时，本次候选未发送"
        },
    );
    if sent {
        response
            .extensions_mut()
            .insert(super::proxy::UncertainSend);
    } else {
        response
            .extensions_mut()
            .insert(super::proxy::LocalCapacity);
        response.extensions_mut().insert(Reason::Deadline);
    }
    response
}

pub(super) fn quota_blocked() -> Response {
    let mut response = rejection(
        StatusCode::TOO_MANY_REQUESTS,
        "此上游账号的已知预算已用尽，请刷新账户额度或等待重置",
    );
    response
        .extensions_mut()
        .insert(AccountQuotaBlocked("account_quota_exhausted"));
    response
}
fn held(response: Response, guard: impl Send + 'static) -> Response {
    let (parts, body) = response.into_parts();
    let mut stream = body.into_data_stream();
    let stream = async_stream::stream! {let _guard=guard;while let Some(item)=stream.next().await {yield item;}};
    Response::from_parts(parts, Body::from_stream(stream))
}
fn monitor(
    response: Response,
    state: AppState,
    protocol: &str,
    model_scope: String,
    cooldown: u64,
    mut probe: Option<Arc<CooldownProbe>>,
    mut quota: Option<CheckAccountQuota>,
) -> Response {
    let mut decoder = super::stream::is_event_stream(response.headers())
        .then(|| super::stream::StreamDecoder::new(protocol));
    let (parts, body) = response.into_parts();
    let mut input = body.into_data_stream();
    let stream = async_stream::stream! {
        let mut complete = false;
        let mut marked = false;
        loop {
            let next = input.next().await;
            let mut failed = matches!(&next, Some(Err(_)));
            if !marked && let Some(decoder) = decoder.as_mut() {
                let updates = match &next {
                    Some(Ok(bytes)) => decoder.feed(bytes),
                    Some(Err(_)) => Vec::new(),
                    None => decoder.end(),
                };
                for update in updates {
                    failed |= update.issue || matches!(update.terminal, Some("error" | "interrupted"));
                    complete |= matches!(update.terminal, Some("success" | "incomplete"));
                }
                // Only an observed upstream EOF proves truncation. A downstream drop
                // stops this stream without reaching this check.
                failed |= next.is_none() && !complete;
            }
            if failed && !marked {
                marked = true;
                if let Some(quota) = quota.take() {
                    quota.settle(false, cooldown).await;
                }
                let scope = model_scope.clone();
                let failed_probe = probe.take();
                let _ = crate::blocking::store(state.store.clone(), move |store| {
                    store.service_set_cooldown(&scope, chrono::Utc::now().timestamp_millis() + (cooldown * 1000) as i64, "stream_interrupted_after_commit")?;
                    if let Some(probe) = failed_probe {
                        probe.clear(store)?;
                    }
                    Ok(())
                }).await;
            }
            if complete && !marked {
                if let Some(quota) = quota.take() {
                    quota.settle(true, 0).await;
                }
                if let Some(probe) = probe.take() {
                    probe.recovered(&state).await;
                }
            }
            let Some(chunk) = next else { break };
            yield chunk;
        }
    };
    Response::from_parts(parts, Body::from_stream(stream))
}
fn rebuild(parts: &Parts, body: &Bytes, selected: &Selection) -> Request {
    let mut request = Request::new(Body::from(body.clone()));
    *request.method_mut() = parts.method.clone();
    *request.uri_mut() = parts.uri.clone();
    *request.version_mut() = parts.version;
    *request.headers_mut() = parts.headers.clone();
    *request.extensions_mut() = parts.extensions.clone();
    request.extensions_mut().insert(Selected(selected.clone()));

    request
}
#[derive(Clone, Copy, serde::Deserialize, serde::Serialize)]
enum QuotaFailure {
    Exhausted,
    UpstreamExhausted,
    RecoveryInProgress,
    Cooling,
}

impl QuotaFailure {
    fn reason(self) -> &'static str {
        match self {
            Self::Exhausted | Self::UpstreamExhausted => "account_quota_exhausted",
            Self::RecoveryInProgress => "recovery_probe_in_progress",
            Self::Cooling => "rate_limit_cooldown",
        }
    }
}

#[derive(Clone, Copy, serde::Deserialize, serde::Serialize)]
enum Failure {
    Rate,
    Quota(QuotaFailure),
    Access(AccessRejection),
    Transient,
    Stop,
    Uncertain,
}
fn failure(status: StatusCode, response: &Response) -> Failure {
    if response
        .extensions()
        .get::<super::proxy::UncertainSend>()
        .is_some()
    {
        Failure::Uncertain
    } else if response.extensions().get::<PermanentReject>().is_some() {
        Failure::Stop
    } else if let Some(access) = response.extensions().get::<AccessRejection>() {
        Failure::Access(*access)
    } else if response.extensions().get::<ConfirmedQuota>().is_some() {
        Failure::Quota(QuotaFailure::UpstreamExhausted)
    } else if response
        .extensions()
        .get::<openrouter_budget::InFlightBudget>()
        .is_some()
    {
        Failure::Rate
    } else if let Some(blocked) = response.extensions().get::<AccountQuotaBlocked>() {
        Failure::Quota(match blocked.0 {
            "recovery_probe_in_progress" => QuotaFailure::RecoveryInProgress,
            "rate_limit_cooldown" => QuotaFailure::Cooling,
            _ => QuotaFailure::Exhausted,
        })
    } else if response.extensions().get::<Reason>() == Some(&Reason::RateLimitCooldown) {
        Failure::Rate
    } else if matches!(
        response.extensions().get::<Reason>(),
        Some(Reason::FailureCooldown | Reason::RecoveryInProgress)
    ) {
        Failure::Transient
    } else if status == StatusCode::TOO_MANY_REQUESTS
        && response
            .extensions()
            .get::<super::proxy::LocalCapacity>()
            .is_none()
    {
        Failure::Rate
    } else if [500, 502, 503, 504, 529].contains(&status.as_u16())
        && response
            .extensions()
            .get::<super::proxy::LocalCapacity>()
            .is_none()
    {
        Failure::Transient
    } else {
        Failure::Stop
    }
}
fn retry_seconds(response: &Response, fallback: u64) -> u64 {
    single_retry_after(response.headers())
        .and_then(|v| retry_after_seconds(v, chrono::Utc::now()))
        .unwrap_or(fallback)
        .clamp(1, 3600)
}

pub(super) fn single_retry_after(headers: &HeaderMap) -> Option<&HeaderValue> {
    let mut values = headers.get_all("retry-after").iter();
    let value = values.next()?;
    values.next().is_none().then_some(value)
}

fn retry_after_seconds(value: &HeaderValue, now: chrono::DateTime<chrono::Utc>) -> Option<u64> {
    let value = value.to_str().ok()?;
    value.parse::<u64>().ok().or_else(|| {
        chrono::DateTime::parse_from_rfc2822(value)
            .ok()
            .map(|time| time.timestamp().saturating_sub(now.timestamp()).max(0) as u64)
    })
}
fn openrouter_reply_status(error: &serde_json::Value, kind: Option<&str>) -> Option<StatusCode> {
    let wire = error["code"]
        .as_u64()
        .and_then(|code| u16::try_from(code).ok())
        .and_then(|code| StatusCode::from_u16(code).ok())
        .filter(|status| status.is_client_error() || status.is_server_error());
    let status = match kind {
        Some("authentication") => StatusCode::UNAUTHORIZED,
        Some("payment_required") => StatusCode::PAYMENT_REQUIRED,
        Some("permission_denied" | "content_policy_violation" | "refusal") => StatusCode::FORBIDDEN,
        Some("not_found" | "image_not_found") => StatusCode::NOT_FOUND,
        Some("rate_limit_exceeded") => StatusCode::TOO_MANY_REQUESTS,
        Some("precondition_failed") => StatusCode::PRECONDITION_FAILED,
        Some("payload_too_large") => StatusCode::PAYLOAD_TOO_LARGE,
        Some("unprocessable") => StatusCode::UNPROCESSABLE_ENTITY,
        Some(
            "invalid_request"
            | "invalid_prompt"
            | "max_tokens_exceeded"
            | "token_limit_exceeded"
            | "string_too_long"
            | "invalid_image"
            | "image_too_large"
            | "unsupported_image_format"
            | "image_download_failed",
        ) => StatusCode::BAD_REQUEST,
        Some("provider_overloaded") => StatusCode::SERVICE_UNAVAILABLE,
        Some("provider_unavailable") => StatusCode::BAD_GATEWAY,
        Some("server") => StatusCode::INTERNAL_SERVER_ERROR,
        Some("timeout") => StatusCode::GATEWAY_TIMEOUT,
        _ => return wire,
    };
    // Conflicting wire evidence cannot promote a permanent rejection to a retry.
    if let Some(wire) = wire
        && wire.is_client_error()
        && wire != StatusCode::TOO_MANY_REQUESTS
        && (status.is_server_error() || status == StatusCode::TOO_MANY_REQUESTS)
    {
        return Some(wire);
    }
    Some(status)
}

pub(super) fn reply_rejection(
    value: &serde_json::Value,
    openrouter: bool,
    retry_after: Option<&HeaderValue>,
) -> Option<Response> {
    let error = crate::gateway::protocol::reply_error(value)?;
    let code = error["code"].as_str();
    let kind = error["type"].as_str().filter(|kind| *kind != "error");
    let provider_kind = openrouter
        .then(|| {
            error["metadata"]["error_type"]
                .as_str()
                .or_else(|| error["error_type"].as_str())
                .or_else(|| {
                    if value["type"] == "response.failed" {
                        value["response"]["error_type"].as_str()
                    } else {
                        value["error_type"].as_str()
                    }
                })
        })
        .flatten();
    if !error.is_object() {
        return None;
    }
    if provider_kind == Some("context_length_exceeded")
        || super::context_budget::is_context_rejection(error)
    {
        return Some(super::context_budget::upstream_rejection());
    }
    let official_status = openrouter
        .then(|| openrouter_reply_status(error, provider_kind))
        .flatten();
    if official_status.is_none()
        && kind.is_none()
        && code.is_none()
        && !error["message"].is_string()
    {
        return None;
    }
    let kind = kind.or(code).unwrap_or("");
    let permanent = code.is_some_and(permanent_error) || permanent_error(kind);
    // An access error embedded in HTTP 200 cannot authorize a second send.
    let native_status = match (kind, code) {
        (_, Some("invalid_api_key")) => StatusCode::UNAUTHORIZED,
        (_, Some("model_access_denied" | "model_not_found")) => StatusCode::FORBIDDEN,
        ("rate_limit_error" | "rate_limit_exceeded", _) => StatusCode::TOO_MANY_REQUESTS,
        ("invalid_request_error", _) => StatusCode::BAD_REQUEST,
        ("authentication_error", _) => StatusCode::UNAUTHORIZED,
        ("permission_error", _) => StatusCode::FORBIDDEN,
        ("not_found_error", _) => StatusCode::NOT_FOUND,
        _ if permanent => StatusCode::TOO_MANY_REQUESTS,
        _ => StatusCode::BAD_GATEWAY,
    };
    let status = match official_status {
        Some(status)
            if native_status.is_client_error()
                && native_status != StatusCode::TOO_MANY_REQUESTS
                && (status.is_server_error() || status == StatusCode::TOO_MANY_REQUESTS) =>
        {
            native_status
        }
        Some(status) => status,
        None => native_status,
    };
    let mut response = rejection(status, "上游在模型输出前返回协议错误");
    if let Some(value) = retry_after {
        response.headers_mut().insert("retry-after", value.clone());
    } else {
        response.headers_mut().remove("retry-after");
    }
    if permanent {
        response.extensions_mut().insert(PermanentReject);
    }
    Some(response)
}
/// Consume only a bounded protocol prelude before committing headers/events.
pub(super) async fn preflight(
    response: Response,
    protocol: &str,
    streamed: bool,
) -> Result<Response, Response> {
    if !response.status().is_success() {
        let (mut parts, body) = response.into_parts();
        let bytes = match axum::body::to_bytes(body, 64 * 1024).await {
            Ok(b) => b,
            Err(_) => {
                let mut response = rejection(parts.status, "上游错误响应超限或读取失败");
                response.headers_mut().remove("retry-after");
                if let Some(retry) = single_retry_after(&parts.headers) {
                    response.headers_mut().insert("retry-after", retry.clone());
                }
                response.extensions_mut().extend(parts.extensions);
                return Err(response);
            }
        };
        mark_upstream_rejection(parts.status, &bytes, &mut parts.extensions);
        if let Some(access) = parts.extensions.get::<AccessRejection>() {
            return Err(access.response());
        }
        return Err(Response::from_parts(parts, Body::from(bytes)));
    }
    let sse = super::stream::is_event_stream(response.headers());
    let (parts, body) = response.into_parts();
    let openrouter = parts.extensions.get::<OpenRouterReply>().is_some();
    let classify = |value| reply_rejection(value, openrouter, single_retry_after(&parts.headers));
    let mut input = body.into_data_stream();
    let mut chunks = vec![];
    let mut bytes = 0usize;
    let limit = if sse { 256 * 1024 } else { 8 * 1024 * 1024 };
    let mut decoder = crate::gateway::stream::Decoder::new(protocol);
    loop {
        match input.next().await {
            Some(Ok(chunk)) => {
                let mut ready = false;
                if sse {
                    use crate::gateway::stream::Delta;
                    let prefix = &chunk[..chunk.len().min(limit - bytes)];
                    let decoded = decoder.feed_each(prefix, |delta| {
                        ready |= match delta {
                            Delta::Text(text) | Delta::Refusal(text) => !text.is_empty(),
                            Delta::Tool(..) | Delta::End => true,
                            _ => false,
                        };
                        Ok(())
                    });
                    if decoded.is_err() && !ready {
                        // A converted stream may contain a synthetic error event.
                        // Only the original upstream event can authorize another send.
                        let response = match parts
                            .extensions
                            .get::<super::proxy::ConvertedStreamRejection>()
                        {
                            Some(rejection) => rejection.0.get().and_then(classify),
                            None => decoder.take_rejection().as_ref().and_then(classify),
                        };
                        return Err(response.unwrap_or_else(|| {
                            super::proxy::uncertain_response("上游在模型输出前返回无效事件")
                        }));
                    }
                }
                bytes = bytes.saturating_add(chunk.len());
                if !ready && bytes > limit {
                    return Err(super::proxy::uncertain_response(
                        "响应预检超过大小限制；未拼接其他模型输出",
                    ));
                }
                chunks.push(chunk);
                if ready {
                    break;
                }
            }
            Some(Err(_)) => {
                return Err(super::proxy::uncertain_response("上游在模型输出前中断"));
            }
            None => {
                if sse {
                    return Err(super::proxy::uncertain_response("上游未产生完整模型响应"));
                }
                let raw = chunks
                    .iter()
                    .flat_map(|b| b.iter().copied())
                    .collect::<Vec<_>>();
                let value = serde_json::from_slice::<serde_json::Value>(&raw).ok();
                if let Some(error) = value.as_ref().and_then(classify) {
                    return Err(error);
                }
                if !value.is_some_and(|v| {
                    if protocol == "responses" {
                        v.get("error").is_none_or(|error| error.is_null())
                            && v["object"] == "response"
                            && matches!(v["status"].as_str(), Some("completed" | "incomplete"))
                            && v["output"].is_array()
                    } else {
                        v.get("error").is_none() && v["type"] == "message"
                    }
                }) {
                    return Err(super::proxy::uncertain_response("上游未返回有效模型响应"));
                }
                break;
            }
        }
    }
    if sse != streamed {
        return Err(super::proxy::uncertain_response(
            "上游响应的流式格式与请求不符",
        ));
    }
    let stream = async_stream::stream! {for chunk in chunks {yield Ok::<_,axum::Error>(chunk);}while let Some(chunk)=input.next().await {yield chunk;}};
    Ok(Response::from_parts(parts, Body::from_stream(stream)))
}

fn guard_publication(
    store: &local_agent_core::Store,
    plugins: &crate::plugin_host::external::Manager,
    service_id: &str,
    enabled: bool,
) -> Result<(), local_agent_core::providers::ConfigError> {
    if !plugins.policy_needed(service_id) {
        return Ok(());
    }
    let id = store.model_service_operation_store_id()?;
    plugins
        .policy_service_enabled(&id, service_id, enabled)
        .map_err(|e| {
            local_agent_core::providers::ConfigError::new(
                e.code,
                "外部策略绑定仍需核查，未开始新的发送",
            )
        })?;
    plugins
        .policy_publication_guard(&id, service_id, None)
        .map_err(|e| {
            local_agent_core::providers::ConfigError::new(
                e.code,
                "策略发布结果尚未确认，未开始新的发送",
            )
        })
}

fn rule_project(
    store: &local_agent_core::Store,
    sessions: &local_agent_core::sessions::NativeSessions,
    session: Option<&str>,
    agent: &str,
) -> Option<String> {
    session
        .and_then(|id| id.split_once(':'))
        .filter(|(family, _)| *family == agent)
        .and_then(|(family, id)| sessions.cached_route_project(family, id))
        .filter(|id| {
            store
                .native_sources()
                .is_ok_and(|sources| sources.iter().any(|source| source == agent))
                && store.hidden_projects().is_ok_and(|hidden| {
                    local_agent_core::projects::visibility::ensure_visible(id, &hidden).is_ok()
                })
        })
}

pub(super) struct DispatchInputs<'a> {
    pub turn: Turn,
    pub turn_guard: TurnGuard,
    pub facts: RuleFacts,
    pub journal: &'a mut trace::Journal,
}

pub(super) async fn dispatch(
    state: AppState,
    binding: super::proxy::Forward,
    mut parts: Parts,
    body: Bytes,
    selected: Selection,
    inputs: DispatchInputs<'_>,
) -> Response {
    let DispatchInputs {
        turn,
        turn_guard: guard,
        mut facts,
        journal,
    } = inputs;
    let streamed = facts.stream;
    let guard = Arc::new(guard);
    let routing_started = tokio::time::Instant::now();
    let context = parts
        .extensions
        .get::<crate::gateway::dynamic::Context>()
        .expect("authenticated dynamic context");
    let binding_id = context.binding.id.clone();
    let runtime = state.observation.policy.clone();
    let mut inputs = nested::PlanningFacts {
        facts: facts.clone(),
        excerpt: parts
            .extensions
            .remove::<intent::IntentInput>()
            .map(|input| input.0),
    };
    if selected.fresh
        && let Some(service) = selected.service.as_ref().filter(|s| s.mode == "default")
        && let Some(session) = turn.session.as_ref()
        && inputs.excerpt.is_some()
    {
        let prepared = {
            let state = state.clone();
            let service = service.clone();
            let turn = turn.clone();
            let mut metadata = RuleFacts {
                agent: facts.agent.clone(),
                agent_product_id: facts.agent_product_id.clone(),
                tools: facts.tools,
                stream: facts.stream,
                input_tokens: facts.input_tokens,
                ..Default::default()
            };
            crate::blocking::store(state.store.clone(), move |store| {
                let published = store.model_service_version(&service.service_version_id)?;
                if published.routing_policy.intent_classifier.is_none() {
                    return Ok(None);
                }
                if published
                    .routing_policy
                    .rules
                    .iter()
                    .any(|rule| rule.project_id.is_some())
                {
                    metadata.project_id = rule_project(
                        store,
                        &state.sessions,
                        turn.session.as_deref(),
                        &metadata.agent,
                    );
                }
                Ok(intent::prepare(&published, &service, &metadata))
            })
            .await
        };
        if let Ok(Some((classifier, labels))) = prepared
            && let Some(input) = inputs.excerpt.take()
        {
            let key = IntentTurn {
                binding: binding_id.clone(),
                session: session.clone(),
                digest: turn.digest.clone(),
            };
            facts.intent =
                intent::classify(&state, &runtime, classifier, labels, Some(input), key).await;
        }
    }
    let started = {
        let binding = binding_id.clone();
        let turn = turn.clone();
        let selected = selected.clone();
        let plugins = state.plugins.clone();
        let billing = state.billing.clone();
        let runtime = state.observation.policy.clone();
        let sessions = state.sessions.clone();
        let guard = guard.clone();
        crate::blocking::store(state.store.clone(), move |store| {
            // This task can outlive the caller while it waits for the store.
            let _guard = guard;
            if selected.fresh
                && let Some(service) = &selected.service
                && plugins.policy_needed(&service.service_id)
            {
                let published = store.model_service_version(&service.service_version_id)?;
                guard_publication(
                    store,
                    &plugins,
                    &service.service_id,
                    published.routing_policy.enabled,
                )?;
            }
            let now = chrono::Utc::now();
            store.service_dispatch(
                &binding,
                &turn,
                &selected,
                |needs_project| {
                    if needs_project {
                        facts.project_id =
                            rule_project(store, &sessions, turn.session.as_deref(), &facts.agent);
                    }
                    facts
                },
                |target, reference, limit| {
                    account_planning_hint(store, &billing, &runtime, target, reference, limit, now)
                },
            )
        })
        .await
    };
    let plan = match started {
        Ok(p) => p,
        Err(error) if error.code == "account_affinity_unavailable" => {
            journal.note(&selected, Reason::StrictAffinity);
            return rejection(StatusCode::CONFLICT, &error.message);
        }
        Err(error) if error.code == "dispatch_not_committed" => {
            journal.note(&selected, Reason::ConfirmedTurnRequired);
            return rejection(StatusCode::CONFLICT, &error.message);
        }
        Err(error) if error.code == "dispatch_already_committed" => {
            journal.note(&selected, Reason::EarlierCandidateAccepted);
            return rejection(StatusCode::CONFLICT, &error.message);
        }
        Err(error) => {
            if error.code == "dispatch_uncertain" {
                journal.note(&selected, Reason::OutcomeUnknown);
                journal.uncertain();
            } else {
                journal.note(&selected, Reason::AdmissionFailed);
            }
            return rejection(
                if [
                    "busy",
                    "unavailable",
                    "service_unavailable",
                    "routing_unavailable",
                    "result_pending",
                    "invalid_record",
                    "grant_revoked",
                ]
                .contains(&error.code.as_str())
                {
                    StatusCode::SERVICE_UNAVAILABLE
                } else {
                    StatusCode::CONFLICT
                },
                "原请求尚未确认或候选正在冷却；未重复发送，请稍后重试或开始新轮次",
            );
        }
    };
    let Some(plan) = plan else {
        if selected
            .service
            .as_ref()
            .is_some_and(|service| service.pending_child.is_some())
        {
            journal.note(&selected, Reason::ConfirmedTurnRequired);
            return rejection(StatusCode::CONFLICT, "请在可识别的新轮次开始使用子服务");
        }
        let path = if let Some(service) = &selected.service {
            let service = service.clone();
            match crate::blocking::store(state.store.clone(), move |store| {
                nested::pinned_scopes(store, &service)
            })
            .await
            {
                Ok(scopes) => scopes,
                Err(_) => {
                    journal.note(&selected, Reason::AdmissionFailed);
                    return rejection(
                        StatusCode::SERVICE_UNAVAILABLE,
                        "本轮路由策略未能读取，未发送请求",
                    );
                }
            }
        } else {
            Vec::new()
        };
        let mut slots = Vec::new();
        let mut quota = None;
        let mut probe_deadline = None;
        let mut pinned_access = None;
        let mut pinned_health = None;
        let mut pinned_probe = None;
        if let Some(((_, id, policy), parents)) = path.split_last() {
            for (scope, id, policy) in parents {
                match group_slot(&runtime, scope, id, policy) {
                    Ok(slot) => slots.extend(slot),
                    Err(e) => {
                        journal.candidate(&selected, policy, Reason::ConfiguredRateLimit);
                        return rejection(StatusCode::TOO_MANY_REQUESTS, e);
                    }
                }
            }
            let has_account = selected
                .service
                .as_ref()
                .is_some_and(|s| s.upstream_account.is_some());
            if (policy.enabled && has_account)
                || crate::billing::route_budget_supported(&selected.version.provider)
            {
                let deadline =
                    tokio::time::Instant::now() + Duration::from_secs(policy.deadline_seconds);
                quota = Some(
                    CheckAccountQuota::new(policy.cooldown_seconds, state.store.clone(), streamed)
                        .with_deadline(deadline),
                );
                probe_deadline = Some(deadline);
            }
            if policy.enabled && !policy.accounts(id).is_empty() && !has_account {
                journal.candidate(&selected, policy, Reason::StrictAffinity);
                return rejection(
                    StatusCode::CONFLICT,
                    "无法确定本轮的账号，请开启可识别的新会话",
                );
            }
            let (target, _, exhausted) = match account_target(&state, &selected).await {
                Ok(target) => target,
                Err(_) => {
                    journal.candidate(&selected, policy, Reason::AccountUnavailable);
                    return rejection(
                        StatusCode::SERVICE_UNAVAILABLE,
                        "本轮原账号不可用，请开启新轮次",
                    );
                }
            };
            let (account, model) = scopes(&target);
            let (access_account, access_model) = if policy.enabled {
                let (access_account, access_model) = access_scopes(&target);
                pinned_access = Some((
                    access_account.clone(),
                    access_model.clone(),
                    policy.cooldown_seconds,
                ));
                (access_account, access_model)
            } else {
                (String::new(), String::new())
            };
            let blocked = {
                let (account, model) = (account.clone(), model.clone());
                let state = state.clone();
                let cooldown = policy.cooldown_seconds;
                let check_access = policy.enabled;
                crate::blocking::store(state.store.clone(), move |store| {
                    let access = if check_access {
                        AccessRejection::active(store, &access_account, &access_model)?
                    } else {
                        None
                    };
                    let cooling = store
                        .service_cooldown(&account)?
                        .or(store.service_cooldown(&model)?);
                    let probe = if access.is_none() && cooling.is_none() && !exhausted {
                        CooldownProbe::admit(
                            store,
                            &state,
                            [account, model, access_account, access_model],
                            cooldown,
                        )?
                    } else {
                        ProbeAdmission::Ready(None)
                    };
                    Ok((access, cooling, probe))
                })
                .await
            };
            match blocked {
                Ok((Some(access), _, _)) => {
                    journal.candidate(&selected, policy, access.trace_reason());
                    return access.response();
                }
                Ok((None, _, _)) if exhausted => {
                    journal.candidate(&selected, policy, Reason::AccountQuotaExhausted);
                    return quota_blocked();
                }
                Ok((None, Some((until, cause)), _)) => {
                    let response = http_cooling(until, &cause);
                    journal.candidate(&selected, policy, admission_reason(&response));
                    return response;
                }
                Ok((None, None, ProbeAdmission::Ready(probe))) => {
                    if probe.is_some() && probe_deadline.is_none() {
                        probe_deadline = Some(
                            tokio::time::Instant::now()
                                + Duration::from_secs(policy.deadline_seconds),
                        );
                    }
                    pinned_probe = probe;
                }
                Ok((None, None, ProbeAdmission::Busy)) => {
                    journal.candidate(&selected, policy, Reason::RecoveryInProgress);
                    return http_recovery_wait();
                }
                Err(_) => {
                    journal.candidate(&selected, policy, Reason::AdmissionFailed);
                    return rejection(
                        StatusCode::SERVICE_UNAVAILABLE,
                        "上游冷却状态无法核查，本轮未发送",
                    );
                }
            }
            pinned_health = Some((account, model.clone(), policy.cooldown_seconds));
            let limit = if policy.enabled {
                policy.limit(id)
            } else {
                ModelLimit::default()
            };
            match runtime.slot(model, &limit) {
                Ok(slot) => slots.push(slot),
                Err(e) => {
                    journal.candidate(&selected, policy, Reason::ConfiguredRateLimit);
                    return rejection(StatusCode::TOO_MANY_REQUESTS, e);
                }
            };
        }
        let mut req = rebuild(&parts, &body, &selected);
        let sent = journal.send_evidence(None, pinned_access.is_some());
        req.extensions_mut().insert(sent.clone());
        req.extensions_mut().insert(SendSlots(slots.clone()));
        if let Some(probe) = &pinned_probe {
            req.extensions_mut().insert(probe.clone());
        }
        if let Some(check) = &quota {
            req.extensions_mut().insert(check.clone());
        }
        let protocol = binding.client_protocol.clone();
        let mut forward = Box::pin(super::proxy::forward(state.clone(), binding, req));
        let timeout = || http_recovery_timeout(sent.was_sent());
        let mut response = match (probe_deadline, pinned_probe.is_some(), quota.as_ref()) {
            (Some(deadline), true, _) => tokio::time::timeout_at(deadline, forward)
                .await
                .unwrap_or_else(|_| timeout()),
            (Some(deadline), false, Some(check)) => tokio::select! {
                biased;
                () = check.wait_for_probe() => {
                    if tokio::time::Instant::now() >= deadline {
                        drop(forward);
                        timeout()
                    } else {
                        tokio::time::timeout_at(deadline, forward)
                            .await
                            .unwrap_or_else(|_| timeout())
                    }
                }
                response = &mut forward => response,
            },
            _ => forward.await,
        };
        if let Some(rejection) = response.extensions().get::<ConfirmedQuota>().cloned()
            && crate::blocking::store(state.store.clone(), move |store| rejection.record(store))
                .await
                .is_err()
        {
            return self::rejection(StatusCode::SERVICE_UNAVAILABLE, "上游预算状态未能保存");
        }
        if !sent.was_sent() && !response.status().is_success() {
            let reason = admission_reason(&response);
            if let Some((_, _, policy)) = path.last() {
                journal.candidate(&selected, policy, reason);
            } else {
                journal.note(&selected, reason);
            }
        }
        if sent.was_sent()
            && let Some((account, model, cooldown)) = pinned_access
            && let Some(access) = response.extensions().get::<AccessRejection>().copied()
        {
            let _ = access
                .record(&state, access.scope(account, model), cooldown)
                .await;
        }
        let quota_probe = quota.as_ref().filter(|check| check.is_probing());
        let mut streaming_quota = None;
        if let Some(deadline) = probe_deadline
            && (quota_probe.is_some() || pinned_probe.is_some())
        {
            let outcome =
                tokio::time::timeout_at(deadline, preflight(response, &protocol, streamed))
                    .await
                    .unwrap_or_else(|_| Err(timeout()));
            let committed = outcome.is_ok();
            response = outcome.unwrap_or_else(|rejection| rejection);
            if let Some(check) = quota_probe {
                if committed && streamed {
                    streaming_quota = Some(check.clone());
                } else {
                    let retry = if sent.was_sent() {
                        check.retry_delay(&response)
                    } else {
                        0
                    };
                    check.settle(committed, retry).await;
                }
            }
        }
        if let Some((account, model, cooldown)) = pinned_health {
            if response.status().is_success() {
                if streamed
                    && (pinned_probe.is_some()
                        || streaming_quota.is_some()
                        || path.last().is_some_and(|(_, _, policy)| policy.enabled))
                {
                    response = monitor(
                        response,
                        state.clone(),
                        &protocol,
                        model,
                        cooldown,
                        pinned_probe.take(),
                        streaming_quota,
                    );
                } else if let Some(probe) = pinned_probe.take() {
                    probe.recovered(&state).await;
                }
            } else {
                let kind = failure(response.status(), &response);
                let retry = retry_seconds(&response, cooldown);
                let probe = pinned_probe.take();
                let observed = response
                    .extensions()
                    .get::<super::proxy::ObservedHttpFailure>()
                    .is_some()
                    || response
                        .extensions()
                        .get::<openrouter_budget::InFlightBudget>()
                        .is_some();
                let result = if sent.was_sent()
                    && (probe.is_some()
                        || (observed && matches!(kind, Failure::Rate | Failure::Transient)))
                {
                    settle_http_rejection(&state, probe, [account, model], kind, retry).await
                } else if let Some(probe) = probe {
                    probe.finished(&state).await
                } else {
                    Ok(())
                };
                if result.is_err() {
                    return rejection(StatusCode::SERVICE_UNAVAILABLE, "上游冷却状态未能保存");
                }
            }
        }
        let response =
            super::route_notices::apply(response, state, binding_id, turn, selected, &body);
        return held(response, (guard, slots, pinned_probe));
    };
    let mut candidates = plan.candidates.clone();
    journal.extend(plan.skipped.iter().cloned());
    let scope = plan
        .candidates
        .first()
        .and_then(|candidate| candidate.service.as_ref())
        .or(selected.service.as_ref())
        .map(nested::Scope::root);
    let choice = if plan.fallback.is_none()
        && let Some(scope) = &scope
    {
        match nested::rank(
            &state,
            scope,
            &mut candidates,
            &plan.policy,
            routing_started + Duration::from_secs(plan.policy.deadline_seconds),
            journal,
        )
        .await
        {
            Ok(choice) => choice,
            Err(error) => {
                journal.remaining(&candidates, &plan.policy, Reason::PolicyUnavailable);
                let _ = crate::blocking::store(state.store.clone(), move |store| {
                    store.service_dispatch_finish(&plan, false)
                })
                .await;
                let message = match error.code {
                    "deadline_exceeded" => "外部策略处理超时，未发送答复模型请求。",
                    "resource_limit" => "外部策略达到执行资源限制，未发送答复模型请求。",
                    _ => {
                        "外部策略无法核查，未发送答复模型请求；请检查授权、版本绑定或已选的失败处理方式"
                    }
                };
                return (StatusCode::SERVICE_UNAVAILABLE,axum::Json(serde_json::json!({"type":"error","error":{"type":"api_error","code":error.code,"message":message}}))).into_response();
            }
        }
    } else {
        None
    };
    execute(
        state,
        binding,
        parts,
        body,
        guard,
        plan,
        Execution {
            scope,
            choice,
            candidates,
            started: routing_started,
            inputs,
            journal,
        },
    )
    .await
}

async fn account_target(
    state: &AppState,
    selected: &Selection,
) -> Result<(Provider, Option<String>, bool), local_agent_core::providers::ConfigError> {
    let provider = selected.version.provider.clone();
    let account = selected
        .service
        .as_ref()
        .and_then(|s| s.upstream_account.clone());
    if account.is_none() && !crate::billing::route_budget_supported(&provider) {
        return Ok((provider, None, false));
    }
    let state = state.clone();
    crate::blocking::store(state.store.clone(), move |store| {
        let (target, label) = if let Some(account) = &account {
            let (target, saved) = store.upstream_account_target(&provider, account)?;
            (target, Some(saved.label))
        } else {
            (provider, None)
        };
        let exhausted = state.billing.lock().is_ok_and(|billing| {
            billing
                .route_budget(store, &target, chrono::Utc::now())
                .exhausted
        });
        Ok((target, label, exhausted))
    })
    .await
}

type PolicyChoice = crate::plugin_host::external::policy::Choice;

struct Execution<'a> {
    scope: Option<nested::Scope>,
    choice: Option<PolicyChoice>,
    candidates: Vec<Selection>,
    started: tokio::time::Instant,
    inputs: nested::PlanningFacts,
    journal: &'a mut trace::Journal,
}

struct RouteFrame {
    scope: Option<nested::Scope>,
    policy: RoutingPolicy,
    candidates: std::vec::IntoIter<Selection>,
    deprioritized: Vec<RouteDecision>,
    deadline: tokio::time::Instant,
    attempts: u8,
    failure: Option<Failure>,
    ended: bool,
    choice: Option<PolicyChoice>,
    group_slot: Option<Arc<Slot>>,
    group: Option<recovery::CandidateKey>,
}

impl RouteFrame {
    fn expired(&self) -> bool {
        self.limit_reason().is_some()
    }

    fn limit_reason(&self) -> Option<Reason> {
        if self.attempts >= self.policy.max_attempts {
            Some(Reason::AttemptLimit)
        } else if tokio::time::Instant::now() >= self.deadline {
            Some(Reason::Deadline)
        } else {
            None
        }
    }

    fn record_remaining(&self, journal: &mut trace::Journal, reason: Reason) {
        if reason != Reason::EarlierCandidateAccepted || self.deprioritized.is_empty() {
            journal.remaining(self.candidates.as_slice(), &self.policy, reason);
            return;
        }
        journal.extend(self.candidates.as_slice().iter().filter_map(|target| {
            let service = target.service.as_ref()?;
            let mut decision = RouteDecision::candidate(service, &self.policy, reason);
            if let Some(planning) = self.deprioritized.iter().find(|candidate| {
                candidate.account_position == decision.account_position
                    && candidate.path == decision.path
            }) {
                decision.reason = planning.reason;
            }
            Some(decision)
        }));
    }

    fn fail(&mut self, kind: Failure) {
        self.failure = Some(kind);
        self.ended = !self.policy.enabled
            || !match kind {
                Failure::Rate | Failure::Quota(_) => self.policy.fallback_on_rate_limit,
                Failure::Access(_) => self.policy.fallback_on_access_denied,
                Failure::Transient => self.policy.fallback_on_failure,
                Failure::Stop | Failure::Uncertain => false,
            };
    }

    fn next(&mut self) -> Option<Selection> {
        if self.ended || self.expired() {
            return None;
        }
        self.candidates.next()
    }
}

#[derive(Clone, Default)]
pub(super) struct SendAuthority {
    external: Vec<PolicyChoice>,
    native_scopes: Vec<String>,
}

impl SendAuthority {
    fn from_frames(frames: &[RouteFrame]) -> Self {
        let mut authority = Self::default();
        for frame in frames {
            if let Some(choice) = &frame.choice {
                authority.external.push(choice.clone());
            } else if let Some(scope) = &frame.scope {
                authority
                    .native_scopes
                    .push(scope.reference.service_id.clone());
            }
        }
        authority
    }

    pub(super) fn valid(&self, plugins: &crate::plugin_host::external::Manager) -> bool {
        self.external
            .iter()
            .all(|choice| plugins.policy_choice_check(choice).is_ok())
            && self
                .native_scopes
                .iter()
                .all(|id| !plugins.policy_needed(id))
    }
}

async fn frames_authorized(state: &AppState, frames: &[RouteFrame]) -> bool {
    for frame in frames {
        if let Some(choice) = &frame.choice {
            if state.plugins.policy_choice_alive(choice).await.is_err() {
                return false;
            }
        } else if frame
            .scope
            .as_ref()
            .is_some_and(|scope| state.plugins.policy_needed(&scope.reference.service_id))
        {
            return false;
        }
    }
    true
}

fn group_slot(
    runtime: &Arc<Runtime>,
    scope: &nested::Scope,
    model_id: &str,
    policy: &RoutingPolicy,
) -> Result<Option<Arc<Slot>>, &'static str> {
    let limit = policy.limit(model_id);
    if !policy.enabled || (limit.max_concurrent == 0 && limit.requests_per_minute == 0) {
        return Ok(None);
    }
    runtime
        .slot(
            format!("group:{}:{model_id}", scope.reference.service_id),
            &limit,
        )
        .map(Some)
}

async fn execute(
    state: AppState,
    binding: super::proxy::Forward,
    parts: Parts,
    body: Bytes,
    guard: Arc<TurnGuard>,
    plan: Dispatch,
    routing: Execution<'_>,
) -> Response {
    let plan = Arc::new(plan);
    let Execution {
        scope,
        choice,
        candidates,
        started,
        mut inputs,
        journal,
    } = routing;
    let runtime = state.observation.policy.clone();
    let first = candidates.first().and_then(|s| s.service.as_ref());
    let initial_reason = if first.is_some_and(|s| {
        s.routing_rule
            .as_ref()
            .is_some_and(|rule| rule.model_id == s.model_id)
    }) {
        None
    } else if choice.as_ref().is_some_and(|c| c.order.is_some()) {
        Some("policy_order".into())
    } else {
        first
            .filter(|s| {
                s.preferred_model_id
                    .as_deref()
                    .is_some_and(|id| id != s.model_id)
            })
            .map(|_| "preferred_unavailable".into())
    };
    let recovery::Running {
        mut frames,
        mut tried,
        mut reason,
        mut last,
        mut skipped_targets,
        mut rejected_access,
    } = if let Some(checkpoint) = &plan.fallback {
        match recovery::restore(&state, &plan, checkpoint, scope).await {
            Ok(running) => running,
            Err(error) => {
                let (status, reason) = match error.code.as_str() {
                    "route_capacity" => {
                        (StatusCode::TOO_MANY_REQUESTS, Reason::ConfiguredRateLimit)
                    }
                    "grant_revoked" => (StatusCode::SERVICE_UNAVAILABLE, Reason::PolicyUnavailable),
                    _ => (StatusCode::SERVICE_UNAVAILABLE, Reason::AdmissionFailed),
                };
                journal.remaining(&candidates, &plan.policy, reason);
                return rejection(status, &error.message);
            }
        }
    } else {
        recovery::Running {
            frames: vec![RouteFrame {
                scope,
                policy: plan.policy.clone(),
                candidates: candidates.into_iter(),
                deprioritized: plan.deprioritized.clone(),
                deadline: started + Duration::from_secs(plan.policy.deadline_seconds),
                attempts: 0,
                failure: None,
                ended: false,
                choice,
                group_slot: None,
                group: None,
            }],
            tried: 0,
            reason: initial_reason,
            last: None,
            skipped_targets: HashMap::new(),
            rejected_access: HashSet::new(),
        }
    };
    let mut stop_reason = None;
    while !frames.is_empty() {
        if let Some(index) = frames.iter().position(RouteFrame::expired)
            && index + 1 < frames.len()
        {
            let limit = frames[index].limit_reason().unwrap_or(Reason::Deadline);
            for frame in frames[index + 1..].iter().rev() {
                frame.record_remaining(journal, limit);
            }
            let failure = frames[index + 1..]
                .iter()
                .rev()
                .find_map(|frame| frame.failure);
            frames.truncate(index + 1);
            if failure.is_some() {
                frames[index].failure = failure;
            }
        }
        let frame = frames.last_mut().expect("active route scope");
        let Some(mut selected) = frame.next() else {
            let finished = frames.pop().expect("active route scope");
            finished.record_remaining(
                journal,
                finished.limit_reason().unwrap_or(Reason::FallbackStopped),
            );
            if let Some(parent) = frames.last_mut() {
                parent.fail(finished.failure.unwrap_or(Failure::Transient));
            }
            continue;
        };
        let deadline = frame.deadline;
        let model_id = if let Some(scope) = &frame.scope {
            let Some(id) = scope.model_id(&selected) else {
                journal.note(&selected, Reason::AdmissionFailed);
                break;
            };
            id
        } else {
            ""
        };
        if selected
            .service
            .as_ref()
            .is_some_and(|s| s.pending_child.is_some())
        {
            let Some(scope) = &frame.scope else { break };
            let slot = match group_slot(&runtime, scope, model_id, &frame.policy) {
                Ok(slot) => slot,
                Err(_) => {
                    journal.candidate(&selected, &frame.policy, Reason::ConfiguredRateLimit);
                    reason = Some("configured_rate_limit".into());
                    frame.fail(Failure::Rate);
                    continue;
                }
            };
            if !frames_authorized(&state, &frames).await {
                journal.note(&selected, Reason::PolicyUnavailable);
                return last.unwrap_or_else(|| {
                    rejection(
                        StatusCode::SERVICE_UNAVAILABLE,
                        "原路由策略授权已变更，未发送备用请求",
                    )
                });
            }
            match tokio::time::timeout_at(
                deadline,
                nested::open(
                    &state,
                    plan.clone(),
                    selected.clone(),
                    &mut inputs,
                    deadline,
                    SendAuthority::from_frames(&frames),
                    journal,
                ),
            )
            .await
            {
                Ok(Ok((scope, child, deadline, choice))) => frames.push(RouteFrame {
                    scope: Some(scope),
                    policy: child.policy,
                    candidates: child.candidates.into_iter(),
                    deprioritized: child.deprioritized,
                    deadline,
                    attempts: 0,
                    failure: None,
                    ended: false,
                    choice,
                    group_slot: slot,
                    group: Some(recovery::CandidateKey::of(&selected)),
                }),
                Ok(Err(error)) if error.code == "account_affinity_unavailable" => {
                    journal.note(&selected, Reason::StrictAffinity);
                    stop_reason = Some(Reason::StrictAffinity);
                    last = Some(rejection(StatusCode::CONFLICT, &error.message));
                    break;
                }
                outcome => {
                    journal.note(
                        &selected,
                        if outcome.is_err() {
                            Reason::Deadline
                        } else {
                            Reason::ChildUnavailable
                        },
                    );
                    last = Some(rejection(
                        StatusCode::SERVICE_UNAVAILABLE,
                        "子服务没有可用的路由",
                    ));
                    reason = Some("child_service_unavailable".into());
                    frames
                        .last_mut()
                        .expect("active parent scope")
                        .fail(Failure::Transient);
                }
            }
            continue;
        }
        let strict = selected
            .service
            .as_ref()
            .is_some_and(local_agent_core::model_services::ServiceSelection::has_strict_account);
        if strict {
            stop_reason = Some(Reason::StrictAffinity);
        }
        let limit = frame.policy.limit(model_id);
        let cooldown = frame.policy.cooldown_seconds;
        let (target, label, exhausted) = match account_target(&state, &selected).await {
            Ok(target) => target,
            Err(e) if e.code == "account_unavailable" => {
                journal.candidate(&selected, &frame.policy, Reason::AccountUnavailable);
                if strict {
                    last = Some(rejection(
                        StatusCode::SERVICE_UNAVAILABLE,
                        "此会话固定的上游账号不可用，请恢复账号或开启新会话",
                    ));
                    break;
                }
                frame.failure = Some(Failure::Transient);
                reason = Some("account_unavailable".into());
                continue;
            }
            Err(_) => {
                journal.candidate(&selected, &frame.policy, Reason::AdmissionFailed);
                break;
            }
        };
        let (account, model) = scopes(&target);
        let (access_account, access_model) = access_scopes(&target);
        let known_access = if rejected_access.contains(&access_account) {
            Some(AccessRejection::Credential)
        } else if rejected_access.contains(&access_model) {
            Some(AccessRejection::Model)
        } else {
            None
        };
        let prior = skipped_targets.get(&model).copied();
        let availability = {
            let access_account = access_account.clone();
            let access_model = access_model.clone();
            let account = account.clone();
            let model = model.clone();
            let state = state.clone();
            crate::blocking::store(state.store.clone(), move |store| {
                let access = match known_access {
                    Some(access) => Some(access),
                    None => AccessRejection::active(store, &access_account, &access_model)?,
                };
                if access.is_some() || exhausted || prior.is_some() {
                    return Ok((access, None, ProbeAdmission::Ready(None)));
                }
                let cooling = store
                    .service_cooldown(&account)?
                    .or(store.service_cooldown(&model)?);
                let probe = if cooling.is_none() {
                    CooldownProbe::admit(
                        store,
                        &state,
                        [account, model, access_account, access_model],
                        cooldown,
                    )?
                } else {
                    ProbeAdmission::Ready(None)
                };
                Ok((None, cooling, probe))
            })
            .await
        };
        let Ok((blocked, cooling, probe)) = availability else {
            journal.candidate(&selected, &frame.policy, Reason::AdmissionFailed);
            break;
        };
        if let Some(access) = blocked {
            journal.candidate(&selected, &frame.policy, access.trace_reason());
            last = Some(access.response());
            if strict {
                break;
            }
            reason = Some(access.reason().into());
            frame.fail(Failure::Access(access));
            continue;
        }
        // Reuse the first failure category when another branch names this target.
        if let Some(kind) = prior {
            journal.candidate(&selected, &frame.policy, Reason::DuplicateTarget);
            if strict {
                break;
            }
            frame.fail(kind);
            continue;
        }
        if exhausted {
            journal.candidate(&selected, &frame.policy, Reason::AccountQuotaExhausted);
            last = Some(quota_blocked());
            if strict {
                break;
            }
            let kind = Failure::Quota(QuotaFailure::Exhausted);
            skipped_targets.insert(model, kind);
            frame.fail(kind);
            reason = Some("account_quota_exhausted".into());
            continue;
        }
        if let Some((_, cause)) = cooling {
            let rate = cause == "upstream_rate_limit";
            journal.candidate(
                &selected,
                &frame.policy,
                if rate {
                    Reason::RateLimitCooldown
                } else {
                    Reason::FailureCooldown
                },
            );
            if strict {
                last = Some(rejection(
                    StatusCode::TOO_MANY_REQUESTS,
                    "此会话固定的上游账号正在冷却，请稍后重试",
                ));
                break;
            }
            reason = Some(
                if rate {
                    "rate_limit_cooldown"
                } else {
                    "failure_cooldown"
                }
                .into(),
            );
            let kind = if rate {
                Failure::Rate
            } else {
                Failure::Transient
            };
            skipped_targets.insert(model, kind);
            frame.fail(kind);
            continue;
        }
        let mut probe = match probe {
            ProbeAdmission::Ready(probe) => probe,
            ProbeAdmission::Busy => {
                journal.candidate(&selected, &frame.policy, Reason::RecoveryInProgress);
                if strict {
                    last = Some(rejection(
                        StatusCode::TOO_MANY_REQUESTS,
                        "此会话固定的上游账号正在探测恢复，请稍后重试",
                    ));
                    break;
                }
                skipped_targets.insert(model, Failure::Transient);
                frame.failure = Some(Failure::Transient);
                reason = Some("recovery_probe_in_progress".into());
                continue;
            }
        };
        let slot = match runtime.slot(model.clone(), &limit) {
            Ok(s) => s,
            Err(_) => {
                journal.candidate(&selected, &frame.policy, Reason::ConfiguredRateLimit);
                if strict {
                    last = Some(rejection(
                        StatusCode::TOO_MANY_REQUESTS,
                        "此会话固定的上游账号已达到本机限额，请稍后重试",
                    ));
                    break;
                }
                skipped_targets.insert(model, Failure::Rate);
                reason = Some("configured_rate_limit".into());
                frame.fail(Failure::Rate);
                continue;
            }
        };
        // Keep the original checkpoint when a grant is revoked between candidates.
        if !frames_authorized(&state, &frames).await {
            journal.note(&selected, Reason::PolicyUnavailable);
            return last.unwrap_or_else(|| {
                rejection(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "原路由策略授权已变更，未发送备用请求",
                )
            });
        }
        if tokio::time::Instant::now() >= deadline {
            journal.note(&selected, Reason::Deadline);
            if strict {
                last = Some(rejection(
                    StatusCode::GATEWAY_TIMEOUT,
                    "此会话固定的上游账号已超过等待时间，请稍后重试",
                ));
                break;
            }
            continue;
        }
        tried += 1;
        for frame in &mut frames {
            frame.attempts += 1;
        }
        if let Some(service) = &mut selected.service {
            service.attempt = tried;
            service.fallback_reason = reason.clone();
            service.upstream_account_label = label;
        }
        let attempt = {
            let plan = plan.clone();
            let selected = selected.clone();
            crate::blocking::store(state.store.clone(), move |store| {
                store.service_attempt(&plan, &selected)
            })
            .await
        };
        let Ok(attempt) = attempt else {
            journal.note(&selected, Reason::AdmissionFailed);
            break;
        };
        let mut req = rebuild(&parts, &body, &selected);
        let sent = journal.send_evidence(Some((plan.clone(), attempt)), true);
        req.extensions_mut().insert(sent.clone());
        if let Some(probe) = &probe {
            req.extensions_mut().insert(probe.clone());
        }
        req.extensions_mut()
            .insert(SendAuthority::from_frames(&frames));
        req.extensions_mut().insert(SendSlots(
            frames
                .iter()
                .filter_map(|frame| frame.group_slot.clone())
                .chain(std::iter::once(slot.clone()))
                .collect(),
        ));
        let quota = selected
            .service
            .as_ref()
            .filter(|service| {
                service.upstream_account.is_some()
                    || crate::billing::route_budget_supported(&target)
            })
            .map(|_| CheckAccountQuota::new(cooldown, state.store.clone(), inputs.facts.stream));
        if let Some(check) = &quota {
            req.extensions_mut().insert(check.clone());
        }
        let result = tokio::time::timeout_at(deadline, async {
            let response =
                Box::pin(super::proxy::forward(state.clone(), binding.clone(), req)).await;
            preflight(response, &binding.client_protocol, inputs.facts.stream).await
        })
        .await;
        let response = match result {
            Ok(Ok(response)) => {
                let status = response.status().as_u16();
                if !inputs.facts.stream
                    && let Some(check) = &quota
                {
                    check.settle(true, 0).await;
                }
                let saved = {
                    let plan = plan.clone();
                    crate::blocking::store(state.store.clone(), move |store| {
                        store.service_attempt_result(attempt, status, "committed")?;
                        store.service_dispatch_finish(&plan, true)
                    })
                    .await
                };
                if saved.is_err() {
                    for frame in frames.iter().rev() {
                        frame.record_remaining(journal, Reason::FallbackStopped);
                    }
                    return rejection(
                        StatusCode::SERVICE_UNAVAILABLE,
                        "最终路由未能持久保存，未提交模型输出",
                    );
                }
                if !inputs.facts.stream
                    && let Some(probe) = probe.take()
                {
                    probe.recovered(&state).await;
                }
                let response = monitor(
                    response,
                    state.clone(),
                    &binding.client_protocol,
                    model,
                    cooldown,
                    probe,
                    quota.filter(|check| inputs.facts.stream && check.is_probing()),
                );
                for frame in frames.iter().rev() {
                    frame.record_remaining(journal, Reason::EarlierCandidateAccepted);
                }
                // Candidates suppress recursive admission; this committed dispatch is
                // nevertheless the first response of a confirmed new user turn.
                selected.fresh = true;
                let response = super::route_notices::apply(
                    response,
                    state.clone(),
                    plan.binding.clone(),
                    Turn {
                        session: Some(plan.session.clone()),
                        digest: plan.turn.clone(),
                        chain: vec![],
                        continuation: false,
                        portable: true,
                    },
                    selected,
                    &body,
                );
                let groups: Vec<_> = frames
                    .into_iter()
                    .filter_map(|frame| frame.group_slot)
                    .collect();
                return held(response, (guard, slot, groups));
            }
            Ok(Err(response)) => response,
            Err(_) => {
                if let Some(check) = &quota {
                    check.settle(false, cooldown).await;
                }
                let was_sent = sent.was_sent();
                let reason = if was_sent {
                    Reason::UpstreamOutcomeUnconfirmed
                } else {
                    Reason::Deadline
                };
                let frame = frames.last().expect("active leaf scope");
                journal.candidate(&selected, &frame.policy, reason);
                for frame in frames.iter().rev() {
                    frame.record_remaining(
                        journal,
                        if was_sent {
                            Reason::OutcomeUnknown
                        } else {
                            Reason::Deadline
                        },
                    );
                }
                let saved = crate::blocking::store(state.store.clone(), move |store| {
                    if was_sent {
                        store.service_attempt_result(attempt, 504, "outcome_unknown")
                    } else {
                        store.service_deadline_before_send(&plan, attempt)
                    }
                })
                .await;
                if was_sent {
                    // A charged generation may still exist upstream. Keep pending across restart.
                    journal.uncertain();
                    return rejection(
                        StatusCode::GATEWAY_TIMEOUT,
                        "请求等待超时，上游结果未确认；未自动重复发送",
                    );
                }
                return if saved.is_ok() {
                    rejection(
                        StatusCode::GATEWAY_TIMEOUT,
                        "准备上游请求超时，本次候选未发送，请稍后重试",
                    )
                } else {
                    rejection(
                        StatusCode::SERVICE_UNAVAILABLE,
                        "本次候选未发送，路由恢复状态未能保存，请开启新轮次",
                    )
                };
            }
        };
        let status = response.status();
        let kind = failure(status, &response);
        let was_sent = sent.was_sent();
        if !was_sent {
            let frame = frames.last().expect("active leaf scope");
            journal.candidate(&selected, &frame.policy, admission_reason(&response));
        }
        let retry = retry_seconds(&response, cooldown);
        if let Some(check) = &quota {
            check.settle(false, retry).await;
        }
        let resumable = was_sent
            && matches!(
                kind,
                Failure::Rate
                    | Failure::Transient
                    | Failure::Access(_)
                    | Failure::Quota(QuotaFailure::UpstreamExhausted)
            );
        let outcome = if !was_sent {
            "not_sent"
        } else {
            match kind {
                Failure::Rate => "rate_limited",
                Failure::Quota(QuotaFailure::Exhausted) => "local_quota_exhausted",
                Failure::Quota(QuotaFailure::UpstreamExhausted) => "upstream_quota_exhausted",
                Failure::Quota(reason) => reason.reason(),
                Failure::Access(access) => access.reason(),
                Failure::Transient => "temporary_failure",
                Failure::Uncertain => "outcome_unknown",
                Failure::Stop => "rejected",
            }
        };
        if !resumable
            && crate::blocking::store(state.store.clone(), move |store| {
                store.service_attempt_result(attempt, status.as_u16(), outcome)
            })
            .await
            .is_err()
        {
            for frame in frames.iter().rev() {
                frame.record_remaining(journal, Reason::FallbackStopped);
            }
            return rejection(
                StatusCode::SERVICE_UNAVAILABLE,
                "本次尝试结果未能保存，未自动重复发送",
            );
        }
        if !was_sent && response.extensions().get::<Reason>() == Some(&Reason::PolicyUnavailable) {
            // The candidate was never sent. Keep any original failure checkpoint
            // so a new policy grant cannot take over this fallback.
            for frame in frames.iter().rev() {
                frame.record_remaining(journal, Reason::PolicyUnavailable);
            }
            return last.unwrap_or(response);
        }
        if matches!(kind, Failure::Uncertain) {
            journal.uncertain();
            let frame = frames.last().expect("active leaf scope");
            journal.candidate(&selected, &frame.policy, Reason::UpstreamOutcomeUnconfirmed);
            for frame in frames.iter().rev() {
                frame.record_remaining(journal, Reason::OutcomeUnknown);
            }
            let groups: Vec<_> = frames
                .into_iter()
                .filter_map(|frame| frame.group_slot)
                .collect();
            return held(response, (guard, slot, probe, groups));
        }
        if !was_sent {
            tried -= 1;
            for frame in &mut frames {
                frame.attempts -= 1;
            }
        }
        if matches!(kind, Failure::Rate | Failure::Transient | Failure::Quota(_)) {
            skipped_targets.insert(model.clone(), kind);
        }
        let cooling = match kind {
            Failure::Access(access) => {
                let scope = access.scope(access_account, access_model);
                rejected_access.insert(scope.clone());
                Some((scope, cooldown, access.reason()))
            }
            Failure::Rate => Some((account, retry, "upstream_rate_limit")),
            Failure::Transient => Some((model, retry, "upstream_failure")),
            _ => None,
        };
        reason = Some(
            match response.extensions().get::<Reason>() {
                Some(Reason::RateLimitCooldown) => "rate_limit_cooldown",
                Some(Reason::FailureCooldown) => "failure_cooldown",
                Some(Reason::RecoveryInProgress) => "recovery_probe_in_progress",
                _ => match kind {
                    Failure::Rate => "upstream_rate_limit",
                    Failure::Quota(reason) => reason.reason(),
                    Failure::Access(access) => access.reason(),
                    _ => "upstream_failure",
                },
            }
            .into(),
        );
        let root = frames.len() == 1;
        let frame = frames.last_mut().expect("active leaf scope");
        frame.fail(kind);
        let stopped = strict || matches!(kind, Failure::Stop) || (root && frame.ended);
        if resumable {
            let budget = response.extensions().get::<ConfirmedQuota>().cloned();
            let plan = plan.clone();
            let checkpoint = if stopped {
                None
            } else {
                let Ok(progress) = recovery::capture(
                    &frames,
                    tried,
                    reason.as_deref(),
                    status,
                    retry,
                    &skipped_targets,
                    &rejected_access,
                ) else {
                    return rejection(
                        StatusCode::SERVICE_UNAVAILABLE,
                        "路由恢复进度未能保存，未发送备用请求",
                    );
                };
                Some(
                    local_agent_core::model_services::policy::FallbackCheckpoint {
                        through: attempt,
                        progress,
                    },
                )
            };
            let saved = crate::blocking::store(state.store.clone(), move |store| {
                let admission = if let Some(budget) = &budget {
                    budget.admission()
                } else if let Some((scope, delay, cause)) = &cooling {
                    let until = chrono::Utc::now().timestamp_millis() + (delay * 1000) as i64;
                    FailedAttemptAdmission::Cooldown(scope, until, cause)
                } else {
                    return Err(ConfigError::new(
                        "admission_missing",
                        "路由失败缺少约束证据",
                    ));
                };
                store.service_record_failed_attempt(
                    &plan,
                    attempt,
                    status.as_u16(),
                    outcome,
                    checkpoint.as_ref(),
                    admission,
                )
            })
            .await;
            if saved.is_err() {
                for frame in frames.iter().rev() {
                    frame.record_remaining(journal, Reason::FallbackStopped);
                }
                return rejection(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "本次尝试结果未能保存，未自动重复发送",
                );
            }
        }
        if let Some(probe) = probe.take()
            && probe.finished(&state).await.is_err()
        {
            return rejection(StatusCode::SERVICE_UNAVAILABLE, "上游恢复探测结果未能保存");
        }
        if stopped {
            for frame in frames.iter().rev() {
                frame.record_remaining(journal, stop_reason.unwrap_or(Reason::FallbackStopped));
            }
            let plan = plan.clone();
            let _ = crate::blocking::store(state.store.clone(), move |store| {
                store.service_dispatch_finish(&plan, false)
            })
            .await;
            let groups: Vec<_> = frames
                .into_iter()
                .filter_map(|frame| frame.group_slot)
                .collect();
            return held(response, (guard, slot, probe, groups));
        }
        // Drain bounded error bodies so attempt accounting is finalized before another send.
        let (response_parts, response_body) = response.into_parts();
        let bytes =
            tokio::time::timeout_at(deadline, axum::body::to_bytes(response_body, 64 * 1024)).await;
        last = Some(Response::from_parts(
            response_parts,
            Body::from(bytes.ok().and_then(Result::ok).unwrap_or_default()),
        ));
        drop(slot);
    }
    for frame in frames.iter().rev() {
        frame.record_remaining(
            journal,
            stop_reason
                .or_else(|| frame.limit_reason())
                .unwrap_or(Reason::FallbackStopped),
        );
    }
    let _ = crate::blocking::store(state.store.clone(), move |store| {
        store.service_dispatch_finish(&plan, false)
    })
    .await;
    last.unwrap_or_else(|| {
        rejection(
            StatusCode::TOO_MANY_REQUESTS,
            "没有满足能力与限流条件的可用候选，请稍后重试",
        )
    })
}
