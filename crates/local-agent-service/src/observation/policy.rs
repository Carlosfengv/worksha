//! Bounded service attempts before downstream commitment. Committed turns stay pinned.
use crate::AppState;
use axum::{
    body::{Body, Bytes},
    extract::Request,
    http::{StatusCode, request::Parts},
    response::{IntoResponse, Response},
};
use futures_util::StreamExt;
use local_agent_core::{
    Provider,
    model_services::policy::{Dispatch, ModelLimit},
    routing::{Selection, Turn},
};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

#[cfg(test)]
mod tests;

#[derive(Clone)]
pub(super) struct Selected(pub Selection);
#[derive(Clone)]
pub(super) struct Attempt {
    pub logical_id: String,
}
#[derive(Clone)]
pub(super) struct PermanentReject;
pub(super) fn permanent_error(bytes: &[u8]) -> bool {
    let value: serde_json::Value = serde_json::from_slice(bytes).unwrap_or_default();
    let code = value["error"]["code"]
        .as_str()
        .or_else(|| value["error"]["type"].as_str())
        .unwrap_or("");
    matches!(
        code,
        "insufficient_quota"
            | "insufficient_balance"
            | "billing_hard_limit_reached"
            | "content_policy_violation"
    )
}
#[derive(Default)]
pub(super) struct Runtime {
    turns: Mutex<HashSet<String>>,
    resources: Mutex<HashMap<String, Resource>>,
}
struct Resource {
    active: u32,
    started: VecDeque<Instant>,
}
struct TurnGuard {
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
struct Slot {
    runtime: Arc<Runtime>,
    key: String,
}
impl Drop for Slot {
    fn drop(&mut self) {
        if let Ok(mut map) = self.runtime.resources.lock()
            && let Some(r) = map.get_mut(&self.key)
        {
            r.active = r.active.saturating_sub(1);
        }
    }
}
impl Runtime {
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
    fn slot(self: &Arc<Self>, key: String, limit: &ModelLimit) -> Result<Slot, &'static str> {
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
            started: VecDeque::new(),
        });
        while r
            .started
            .front()
            .is_some_and(|t| now.duration_since(*t) >= Duration::from_secs(60))
        {
            r.started.pop_front();
        }
        if (limit.max_concurrent > 0 && r.active >= limit.max_concurrent)
            || (limit.requests_per_minute > 0
                && r.started.len() >= limit.requests_per_minute as usize)
        {
            return Err("来源已达到配置的并发或每分钟请求限制");
        }
        if r.started.len() >= 1_000_000 {
            return Err("来源请求频率计数已满");
        }
        r.active += 1;
        r.started.push_back(now);
        Ok(Slot {
            runtime: self.clone(),
            key,
        })
    }
}
fn scopes(provider: &Provider) -> (String, String) {
    let endpoint = url::Url::parse(&provider.endpoint)
        .ok()
        .map(|u| {
            format!(
                "{}://{}:{}",
                u.scheme(),
                u.host_str().unwrap_or(""),
                u.port_or_known_default().unwrap_or(0)
            )
        })
        .unwrap_or_default();
    let account = local_agent_core::providers::files::hash(
        format!(
            "{endpoint}|{}",
            provider.credential_ref.as_deref().unwrap_or("anonymous")
        )
        .as_bytes(),
    );
    let model = local_agent_core::providers::files::hash(
        format!("{account}|{}", provider.model).as_bytes(),
    );
    (format!("account:{account}"), format!("model:{model}"))
}
pub(crate) fn health(store: &local_agent_core::Store, provider: &Provider) -> serde_json::Value {
    let (account, model) = scopes(provider);
    match store
        .service_cooldown(&account)
        .and_then(|v| Ok(v.or(store.service_cooldown(&model)?)))
    {
        Ok(Some((until, reason))) => {
            serde_json::json!({"state":"cooling","untilMs":until,"reason":reason})
        }
        Ok(None) => serde_json::json!({"state":"ready","untilMs":null}),
        Err(_) => serde_json::json!({"state":"unknown","untilMs":null}),
    }
}
fn rejection(status: StatusCode, message: &str) -> Response {
    let mut r=(status,axum::Json(serde_json::json!({"type":"error","error":{"type":if status==StatusCode::TOO_MANY_REQUESTS{"rate_limit_error"}else{"api_error"},"message":message}}))).into_response();
    if status == StatusCode::TOO_MANY_REQUESTS {
        r.headers_mut()
            .insert("retry-after", axum::http::HeaderValue::from_static("30"));
    }
    r
}
fn held(response: Response, guard: impl Send + 'static) -> Response {
    let (parts, body) = response.into_parts();
    let mut stream = body.into_data_stream();
    let stream = async_stream::stream! {let _guard=guard;while let Some(item)=stream.next().await {yield item;}};
    Response::from_parts(parts, Body::from_stream(stream))
}
fn monitor(response: Response, state: AppState, model_scope: String, cooldown: u64) -> Response {
    let sse = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("text/event-stream"));
    let (parts, body) = response.into_parts();
    let mut input = body.into_data_stream();
    let stream = async_stream::stream! {
        let mut decoder=crate::gateway::stream::Decoder::new("messages");let mut marked=false;
        while let Some(chunk)=input.next().await {
            let failed=match &chunk {Ok(bytes)=>sse && decoder.feed(bytes).is_err(),Err(_)=>true};
            if failed && !marked {
                marked=true;
                let scope = model_scope.clone();
                let _ = crate::blocking::store(state.store.clone(), move |store| {
                    store.service_set_cooldown(&scope, chrono::Utc::now().timestamp_millis() + (cooldown * 1000) as i64, "stream_interrupted_after_commit")
                }).await;
            }
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
#[derive(Clone, Copy)]
enum Failure {
    Rate,
    Transient,
    Stop,
    Uncertain,
}
fn failure(status: StatusCode, response: &Response) -> Failure {
    if response.extensions().get::<PermanentReject>().is_some() {
        Failure::Stop
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
            .get::<super::proxy::UncertainSend>()
            .is_none()
        && response
            .extensions()
            .get::<super::proxy::LocalCapacity>()
            .is_none()
    {
        Failure::Transient
    } else if response
        .extensions()
        .get::<super::proxy::UncertainSend>()
        .is_some()
    {
        Failure::Uncertain
    } else {
        Failure::Stop
    }
}
fn retry_seconds(response: &Response, fallback: u64) -> u64 {
    response
        .headers()
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| {
            v.parse::<u64>().ok().or_else(|| {
                chrono::DateTime::parse_from_rfc2822(v)
                    .ok()
                    .map(|t| (t.timestamp() - chrono::Utc::now().timestamp()).max(1) as u64)
            })
        })
        .unwrap_or(fallback)
        .clamp(1, 3600)
}
fn stream_rejection(bytes: &[u8]) -> Response {
    for line in bytes.split(|b| *b == b'\n') {
        let Some(data) = line.strip_prefix(b"data:") else {
            continue;
        };
        let Ok(value) = serde_json::from_slice::<serde_json::Value>(data) else {
            continue;
        };
        if value["type"] != "error" && value.get("error").is_none() {
            continue;
        }
        let raw = serde_json::to_vec(&value).unwrap_or_default();
        if super::context_budget::is_context_error(&raw) {
            return super::context_budget::upstream_rejection();
        }
        let kind = value["error"]["type"].as_str().unwrap_or("");
        let permanent = permanent_error(&raw);
        let status = match kind {
            "rate_limit_error" => StatusCode::TOO_MANY_REQUESTS,
            "invalid_request_error" => StatusCode::BAD_REQUEST,
            "authentication_error" => StatusCode::UNAUTHORIZED,
            "permission_error" => StatusCode::FORBIDDEN,
            "not_found_error" => StatusCode::NOT_FOUND,
            _ if permanent => StatusCode::TOO_MANY_REQUESTS,
            _ => StatusCode::BAD_GATEWAY,
        };
        let mut response = rejection(status, "上游在模型输出前返回协议错误");
        if permanent {
            response.extensions_mut().insert(PermanentReject);
        }
        return response;
    }
    rejection(StatusCode::BAD_GATEWAY, "上游在模型输出前返回协议错误")
}
/// Consume only a bounded protocol prelude before committing headers/events.
async fn preflight(response: Response) -> Result<Response, Response> {
    if !response.status().is_success() {
        let (mut parts, body) = response.into_parts();
        let bytes = match axum::body::to_bytes(body, 64 * 1024).await {
            Ok(b) => b,
            Err(_) => {
                return Err(rejection(
                    StatusCode::BAD_GATEWAY,
                    "上游错误响应超限或读取失败",
                ));
            }
        };
        if permanent_error(&bytes) {
            parts.extensions.insert(PermanentReject);
        }
        return Err(Response::from_parts(parts, Body::from(bytes)));
    }
    let sse = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("text/event-stream"));
    let (parts, body) = response.into_parts();
    let mut input = body.into_data_stream();
    let mut chunks = vec![];
    let mut bytes = 0usize;
    let mut decoder = crate::gateway::stream::Decoder::new("messages");
    loop {
        match input.next().await {
            Some(Ok(chunk)) => {
                bytes = bytes.saturating_add(chunk.len());
                if bytes > if sse { 256 * 1024 } else { 8 * 1024 * 1024 } {
                    return Err(rejection(
                        StatusCode::BAD_GATEWAY,
                        "响应预检超过大小限制；未拼接其他模型输出",
                    ));
                }
                let ready = if sse {
                    match decoder.feed(&chunk) {
                        Ok(ds) => ds.iter().any(|d| {
                            matches!(d,crate::gateway::stream::Delta::Text(t) if !t.is_empty())
                                || matches!(
                                    d,
                                    crate::gateway::stream::Delta::Tool(..)
                                        | crate::gateway::stream::Delta::Finish(..)
                                        | crate::gateway::stream::Delta::End
                                )
                        }),
                        Err(_) => {
                            let mut raw: Vec<u8> = chunks
                                .iter()
                                .flat_map(|b: &Bytes| b.iter().copied())
                                .collect();
                            raw.extend_from_slice(&chunk);
                            return Err(stream_rejection(&raw));
                        }
                    }
                } else {
                    false
                };
                chunks.push(chunk);
                if ready {
                    break;
                }
            }
            Some(Err(_)) => return Err(rejection(StatusCode::BAD_GATEWAY, "上游在模型输出前中断")),
            None => {
                if sse {
                    return Err(rejection(StatusCode::BAD_GATEWAY, "上游未产生完整模型响应"));
                }
                let raw = chunks
                    .iter()
                    .flat_map(|b| b.iter().copied())
                    .collect::<Vec<_>>();
                if serde_json::from_slice::<serde_json::Value>(&raw)
                    .ok()
                    .is_none_or(|v| v.get("error").is_some() || v["type"] != "message")
                {
                    return Err(rejection(StatusCode::BAD_GATEWAY, "上游未返回有效模型响应"));
                }
                break;
            }
        }
    }
    let stream = async_stream::stream! {for chunk in chunks {yield Ok::<_,axum::Error>(chunk);}while let Some(chunk)=input.next().await {yield chunk;}};
    Ok(Response::from_parts(parts, Body::from_stream(stream)))
}

pub(super) async fn dispatch(
    state: AppState,
    binding: super::proxy::Forward,
    mut parts: Parts,
    body: Bytes,
    selected: Selection,
    turn: Turn,
) -> Response {
    let context = parts
        .extensions
        .get::<crate::gateway::dynamic::Context>()
        .expect("authenticated dynamic context");
    let binding_id = context.binding.id.clone();
    let key = format!(
        "{}|{}|{}",
        binding_id,
        turn.session.as_deref().unwrap_or("unknown"),
        turn.digest
    );
    let runtime = state.observation.policy.clone();
    let guard = match runtime.turn(key) {
        Ok(g) => g,
        Err(e) => return rejection(StatusCode::CONFLICT, e),
    };
    let started = {
        let binding = binding_id.clone();
        let turn = turn.clone();
        let selected = selected.clone();
        let plugins = state.plugins.clone();
        crate::blocking::store(state.store.clone(), move |store| {
            if selected.fresh
                && let Some(service) = &selected.service
                && plugins.policy_needed(&service.service_id)
            {
                let id = store.model_service_operation_store_id()?;
                let published = store.model_service_version(&service.service_version_id)?;
                plugins
                    .policy_service_enabled(
                        &id,
                        &service.service_id,
                        published.routing_policy.enabled,
                    )
                    .map_err(|e| {
                        local_agent_core::providers::ConfigError::new(
                            e.code,
                            "外部策略绑定仍需核查，未开始新的发送",
                        )
                    })?;

                plugins
                    .policy_publication_guard(&id, &service.service_id, None)
                    .map_err(|e| {
                        local_agent_core::providers::ConfigError::new(
                            e.code,
                            "策略发布结果尚未确认，未开始新的发送",
                        )
                    })?;
            }
            store.service_dispatch(&binding, &turn, &selected)
        })
        .await
    };
    let plan = match started {
        Ok(p) => p,
        Err(error) => {
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
    parts.extensions.insert(Attempt {
        logical_id: format!("model-service-{}", uuid::Uuid::new_v4()),
    });
    let Some(plan) = plan else {
        let policy = if let Some(service) = &selected.service {
            let version = service.service_version_id.clone();
            match crate::blocking::store(state.store.clone(), move |store| {
                store.model_service_version(&version)
            })
            .await
            {
                Ok(service) => Some(service.routing_policy),
                Err(_) => {
                    return rejection(
                        StatusCode::SERVICE_UNAVAILABLE,
                        "本轮路由策略未能读取，未发送请求",
                    );
                }
            }
        } else {
            None
        };
        let mut slot = None;
        if let Some(policy) = policy.filter(|p| p.enabled) {
            let (account, model) = scopes(&selected.version.provider);
            let scope = model.clone();
            let blocked = crate::blocking::store(state.store.clone(), move |store| {
                Ok(store
                    .service_cooldown(&account)?
                    .or(store.service_cooldown(&scope)?))
            })
            .await;
            if !matches!(blocked, Ok(None)) {
                return rejection(
                    StatusCode::TOO_MANY_REQUESTS,
                    "本轮原目标正在冷却，工具续调保留原路由，请稍后重试",
                );
            }
            let id = selected
                .service
                .as_ref()
                .map(|s| s.model_id.as_str())
                .unwrap_or("");
            slot = match runtime.slot(model, &policy.limit(id)) {
                Ok(s) => Some(s),
                Err(e) => return rejection(StatusCode::TOO_MANY_REQUESTS, e),
            };
        }
        let req = rebuild(&parts, &body, &selected);
        let response = Box::pin(super::proxy::forward(state.clone(), binding, req)).await;
        let response =
            super::route_notices::apply(response, state, binding_id, turn, selected, &body);
        return held(response, (guard, slot));
    };
    let mut plan = plan;
    let mut choice = None;
    let ranking_started = tokio::time::Instant::now();
    if let Some(service) = plan
        .candidates
        .iter()
        .find_map(|candidate| candidate.service.as_ref())
        .or(selected.service.as_ref())
        && state.plugins.policy_needed(&service.service_id)
    {
        let metadata = {
            let service = service.clone();
            let candidates = plan.candidates.clone();
            crate::blocking::store(state.store.clone(), move |store| {
                use local_agent_core::domain::routing_policy::{
                    Candidate, CandidateHealth, RankRequest,
                };
                let published = store.model_service_version(&service.service_version_id)?;
                let ids: Vec<_> = candidates
                    .iter()
                    .filter_map(|s| s.service.as_ref().map(|s| s.model_id.clone()))
                    .collect();
                if candidates.iter().any(|c| {
                    c.service.as_ref().is_none_or(|s| {
                        s.service_id != service.service_id
                            || s.service_version_id != service.service_version_id
                    })
                }) {
                    return Err(local_agent_core::providers::ConfigError::new(
                        "invalid_record",
                        "候选服务版本不一致，未发送模型请求",
                    ));
                }

                let request = RankRequest {
                    service_id: service.service_id,
                    service_version_id: service.service_version_id,
                    preferred_model_id: service.preferred_model_id.filter(|id| ids.contains(id)),
                    default_model_id: ids
                        .contains(&published.default_model_id)
                        .then_some(published.default_model_id),
                    configured_order: published
                        .routing_policy
                        .order
                        .iter()
                        .filter(|id| ids.contains(id))
                        .cloned()
                        .collect(),
                    candidates: published
                        .models
                        .iter()
                        .filter(|model| ids.contains(&model.id))
                        .map(|model| Candidate {
                            model_id: model.id.clone(),
                            capabilities: (&model.capabilities).into(),
                            health: match health(store, &model.provider)["state"].as_str() {
                                Some("cooling") => CandidateHealth::Cooling,
                                Some("ready") => CandidateHealth::Available,
                                _ => CandidateHealth::Unknown,
                            },
                        })
                        .collect(),
                };
                Ok((store.model_service_operation_store_id()?, request))
            })
            .await
        };
        let ranked = match metadata {
            Ok((id, request)) => tokio::time::timeout_at(
                ranking_started + Duration::from_secs(plan.policy.deadline_seconds),
                state.plugins.policy_rank(id, request),
            )
            .await
            .unwrap_or_else(|_| Err(crate::plugin_host::external::fail("deadline_exceeded"))),
            Err(_) => Err(crate::plugin_host::external::fail("unavailable")),
        };
        match ranked {
            Ok(ranked) => {
                if let Some(order) = &ranked.order {
                    let mut candidates = std::mem::take(&mut plan.candidates);
                    plan.candidates = order
                        .iter()
                        .filter_map(|id| {
                            candidates
                                .iter()
                                .position(|s| s.service.as_ref().is_some_and(|s| &s.model_id == id))
                                .map(|i| candidates.remove(i))
                        })
                        .collect();
                }
                choice = Some((ranked, ranking_started));
            }
            Err(error) => {
                let _ = crate::blocking::store(state.store.clone(), move |store| {
                    store.service_dispatch_finish(&plan, false)
                })
                .await;
                let message = match error.code {
                    "deadline_exceeded" => "外部策略处理超时，未发送模型请求。",
                    "resource_limit" => "外部策略达到执行资源限制，未发送模型请求。",
                    _ => {
                        "外部策略无法核查，未发送模型请求；请检查授权、版本绑定或已选的失败处理方式"
                    }
                };
                return (StatusCode::SERVICE_UNAVAILABLE,axum::Json(serde_json::json!({"type":"error","error":{"type":"api_error","code":error.code,"message":message}}))).into_response();
            }
        }
    }
    execute(state, binding, parts, body, guard, plan, choice).await
}
async fn execute(
    state: AppState,
    binding: super::proxy::Forward,
    parts: Parts,
    body: Bytes,
    guard: TurnGuard,
    plan: Dispatch,
    choice: Option<(
        crate::plugin_host::external::policy::Choice,
        tokio::time::Instant,
    )>,
) -> Response {
    let plan = Arc::new(plan);
    let deadline = choice
        .as_ref()
        .map(|(_, started)| *started)
        .unwrap_or_else(tokio::time::Instant::now)
        + Duration::from_secs(plan.policy.deadline_seconds);
    let runtime = state.observation.policy.clone();
    let mut tried = 0u8;
    let mut reason = if choice.as_ref().is_some_and(|(c, _)| c.order.is_some()) {
        Some("policy_order".into())
    } else {
        plan.candidates
            .first()
            .and_then(|s| s.service.as_ref())
            .filter(|s| {
                s.preferred_model_id
                    .as_deref()
                    .is_some_and(|id| id != s.model_id)
            })
            .map(|_| "preferred_unavailable".into())
    };
    let mut last = None;
    for mut selected in plan.candidates.iter().cloned() {
        if tried >= plan.policy.max_attempts || tokio::time::Instant::now() >= deadline {
            break;
        }
        let (account, model) = scopes(&selected.version.provider);
        let cooling = {
            let account = account.clone();
            let model = model.clone();
            crate::blocking::store(state.store.clone(), move |store| {
                let active = store
                    .service_cooldown(&account)?
                    .or(store.service_cooldown(&model)?);
                let recovering = store.service_cooldown_probe(&account)?
                    || store.service_cooldown_probe(&model)?;
                Ok((active, recovering))
            })
            .await
        };
        let recovering = match cooling {
            Ok((Some((_, cause)), _)) => {
                let rate = cause == "upstream_rate_limit";
                reason = Some(
                    if rate {
                        "rate_limit_cooldown"
                    } else {
                        "failure_cooldown"
                    }
                    .into(),
                );
                if !(if rate {
                    plan.policy.fallback_on_rate_limit
                } else {
                    plan.policy.fallback_on_failure
                }) {
                    break;
                }
                continue;
            }
            Err(_) => break,
            Ok((None, recovering)) => recovering,
        };
        let probe = if recovering {
            match runtime.turn(format!("probe:{account}")) {
                Ok(g) => Some(g),
                Err(_) => {
                    reason = Some("recovery_probe_in_progress".into());
                    continue;
                }
            }
        } else {
            None
        };
        let model_id = selected
            .service
            .as_ref()
            .map(|s| s.model_id.as_str())
            .unwrap_or("");
        let slot = match runtime.slot(model.clone(), &plan.policy.limit(model_id)) {
            Ok(s) => s,
            Err(_) => {
                reason = Some("configured_rate_limit".into());
                if !plan.policy.fallback_on_rate_limit {
                    break;
                }
                continue;
            }
        };
        if let Some((choice, _)) = &choice {
            let plugins = state.plugins.clone();
            // A revocation after the first attempt stops the chain; it never starts a new ranking.
            if plugins.policy_choice_alive(choice).await.is_err() {
                break;
            }
            if tokio::time::Instant::now() >= deadline {
                break;
            }
        }
        tried += 1;
        if let Some(service) = &mut selected.service {
            service.attempt = tried;
            service.fallback_reason = reason.clone();
        }
        let attempt = {
            let plan = plan.clone();
            let selected = selected.clone();
            crate::blocking::store(state.store.clone(), move |store| {
                store.service_attempt(&plan, &selected)
            })
            .await
        };
        let Ok(attempt) = attempt else { break };
        let req = rebuild(&parts, &body, &selected);
        let result = tokio::time::timeout_at(deadline, async {
            let response =
                Box::pin(super::proxy::forward(state.clone(), binding.clone(), req)).await;
            preflight(response).await
        })
        .await;
        let response = match result {
            Ok(Ok(response)) => {
                let status = response.status().as_u16();
                let saved = {
                    let plan = plan.clone();
                    crate::blocking::store(state.store.clone(), move |store| {
                        store.service_attempt_result(attempt, status, "committed")?;
                        store.service_dispatch_finish(&plan, true)
                    })
                    .await
                };
                if saved.is_err() {
                    return rejection(
                        StatusCode::SERVICE_UNAVAILABLE,
                        "最终路由未能持久保存，未提交模型输出",
                    );
                }
                let recovered_model = model.clone();
                let _ = crate::blocking::store(state.store.clone(), move |store| {
                    store.service_recovered(&account)?;
                    store.service_recovered(&recovered_model)
                })
                .await;
                let response =
                    monitor(response, state.clone(), model, plan.policy.cooldown_seconds);
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
                return held(response, (guard, Some(slot), probe));
            }
            Ok(Err(response)) => response,
            Err(_) => {
                let _ = crate::blocking::store(state.store.clone(), move |store| {
                    store.service_attempt_result(attempt, 504, "outcome_unknown")
                })
                .await;
                // A timeout can leave a charged generation upstream. Keep pending across restart.
                return rejection(
                    StatusCode::GATEWAY_TIMEOUT,
                    "请求等待超时，上游结果未确认；未自动重复发送",
                );
            }
        };
        let status = response.status();
        let kind = failure(status, &response);
        let retry = retry_seconds(&response, plan.policy.cooldown_seconds);
        let recorded = crate::blocking::store(state.store.clone(), move |store| {
            store.service_attempt_result(
                attempt,
                status.as_u16(),
                match kind {
                    Failure::Rate => "rate_limited",
                    Failure::Transient => "temporary_failure",
                    Failure::Uncertain => "outcome_unknown",
                    Failure::Stop => "rejected",
                },
            )
        })
        .await;
        if recorded.is_err() {
            return rejection(
                StatusCode::SERVICE_UNAVAILABLE,
                "本次尝试结果未能保存，未自动重复发送",
            );
        }
        if matches!(kind, Failure::Uncertain) {
            return held(response, (guard, Some(slot), probe));
        }
        let can_retry = match kind {
            Failure::Rate => plan.policy.fallback_on_rate_limit,
            Failure::Transient => plan.policy.fallback_on_failure,
            _ => false,
        };
        if matches!(kind, Failure::Rate | Failure::Transient) {
            let scope = if matches!(kind, Failure::Rate) {
                account
            } else {
                model
            };
            if crate::blocking::store(state.store.clone(), move |store| {
                store.service_set_cooldown(
                    &scope,
                    chrono::Utc::now().timestamp_millis() + (retry * 1000) as i64,
                    if matches!(kind, Failure::Rate) {
                        "upstream_rate_limit"
                    } else {
                        "upstream_failure"
                    },
                )
            })
            .await
            .is_err()
            {
                break;
            }
        }
        reason = Some(
            if matches!(kind, Failure::Rate) {
                "upstream_rate_limit"
            } else {
                "upstream_failure"
            }
            .into(),
        );
        if !can_retry {
            let plan = plan.clone();
            let _ = crate::blocking::store(state.store.clone(), move |store| {
                store.service_dispatch_finish(&plan, false)
            })
            .await;
            return held(response, (guard, Some(slot), probe));
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
