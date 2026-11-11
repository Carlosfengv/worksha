use super::{Observation, label, stream::decode};
use crate::AppState;
use axum::{
    body::{Body, to_bytes},
    extract::{Path, Request, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use futures_util::StreamExt;
use local_agent_core::Provider;
use local_agent_core::domain::protocol::{Finished, Open, Opened, Output, ProtocolAdapter};
use local_agent_core::{
    model_services::policy::QuotaProbeClaim, observation::ModelObservation, providers::ConfigError,
    sessions::Usage,
};
use std::{
    sync::{Arc, OnceLock},
    time::Duration,
};
use zeroize::Zeroizing;

struct Guard {
    observation: Arc<Observation>,
    id: String,
    done: bool,
    mismatched_reply: bool,
}
impl Guard {
    fn finish(&mut self, status: &str, issue: Option<&str>) {
        if !self.done {
            let (status, issue) = if self.mismatched_reply && status != "error" {
                ("interrupted", Some("上游响应的流式格式与请求不符"))
            } else {
                (status, issue)
            };
            self.observation.hub.finish(&self.id, status, issue);
            self.done = true;
        }
    }
}
impl Drop for Guard {
    fn drop(&mut self) {
        self.finish("cancelled", Some("客户端结束读取，未收到完整结束事件"));
    }
}
#[derive(Clone)]
pub(super) struct LocalCapacity;
#[derive(Clone)]
pub(super) struct UncertainSend;
#[derive(Clone)]
pub(super) struct ObservedHttpFailure;
#[derive(Clone, Default)]
pub(super) struct ConvertedStreamRejection(pub Arc<OnceLock<serde_json::Value>>);
fn reject(status: StatusCode, message: &str) -> Response {
    (
        status,
        axum::Json(
            serde_json::json!({"error":{"type":"local_observation_error","message":message}}),
        ),
    )
        .into_response()
}
pub(super) fn uncertain_response(message: &str) -> Response {
    let mut response = super::policy::rejection(
        StatusCode::BAD_GATEWAY,
        &format!("{message}，上游结果未确认"),
    );
    response.extensions_mut().insert(UncertainSend);
    response
}
fn secret_eq(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.bytes()
        .zip(b.bytes())
        .fold(0u8, |diff, (a, b)| diff | (a ^ b))
        == 0
}
#[derive(serde::Deserialize)]
struct ClaudeSessionMetadata {
    session_id: String,
}
fn native_session(agent: &str, headers: &HeaderMap, body: &serde_json::Value) -> Option<String> {
    let names: &[&str] = match agent {
        "opencode" => &[
            "x-opencode-session",
            "x-session-id",
            "x-session-affinity",
            "session_id",
            "x-local-agent-session-id",
        ],
        "claude" => &["session_id", "x-local-agent-session-id"],
        "codex" => &[
            // Codex uses session-id (with a hyphen) for cache affinity. Its
            // thread-id names the actual conversation, including for forks.
            "thread-id",
            "session_id",
            "x-codex-session-id",
            "x-local-agent-session-id",
        ],
        _ => return None,
    };
    let normalize = |raw: &str| -> Option<String> {
        if agent == "opencode" {
            let suffix = raw.strip_prefix("ses_")?;
            (!suffix.is_empty()
                && suffix.len() <= 128
                && suffix.bytes().all(|b| b.is_ascii_alphanumeric()))
            .then(|| format!("opencode:{raw}"))
        } else {
            uuid::Uuid::parse_str(raw)
                .ok()
                .map(|id| format!("{agent}:{id}"))
        }
    };
    // OpenCode 2.x sends the real session separately from cache affinity,
    // which can refer to a parent when the request is made in a fork.
    if agent == "opencode" && headers.contains_key("x-opencode-session-id") {
        let mut values = headers.get_all("x-opencode-session-id").iter();
        let session = normalize(values.next()?.to_str().ok()?)?;
        return values.next().is_none().then_some(session);
    }
    let mut session = None;
    for name in names {
        if headers.get_all(*name).iter().count() > 1 {
            return None;
        }
        if let Some(raw) = headers.get(*name) {
            let candidate = normalize(raw.to_str().ok()?)?;
            if session.as_ref().is_some_and(|old| old != &candidate) {
                return None;
            }
            session = Some(candidate);
        }
    }
    // Claude's structured metadata is a second explicit candidate, never a
    // fallback that overrides a conflicting header. Other families' metadata
    // and OpenCode parent / request IDs are not Session evidence.
    if agent == "claude"
        && let Some(user) = body["metadata"]["user_id"].as_str()
    {
        if user.len() > 4096 {
            return None;
        }
        if user.trim_start().starts_with('{') {
            let metadata = serde_json::from_str::<serde_json::Value>(user).ok()?;
            if metadata.get("session_id").is_none() {
                return session;
            }
            // Deriving a struct rejects duplicate Session fields in the embedded
            // JSON; a Value alone would silently select the final occurrence.
            let metadata = serde_json::from_str::<ClaudeSessionMetadata>(user).ok()?;
            let candidate = normalize(&metadata.session_id)?;
            if session.as_ref().is_some_and(|old| old != &candidate) {
                return None;
            }
            session = Some(candidate);
        }
    }
    session
}

pub async fn capture(
    State(state): State<AppState>,
    Path((agent, secret, path)): Path<(String, String, String)>,
    request: Request,
) -> Response {
    if request.headers().get("host").and_then(|v| v.to_str().ok()) != Some(state.host.as_str())
        || request.headers().contains_key("origin")
        || request.uri().query().is_some()
    {
        return reject(StatusCode::UNAUTHORIZED, "未授权的本地观测请求");
    }
    let binding = state
        .observation
        .bindings
        .lock()
        .ok()
        .and_then(|b| b.get(&agent).cloned())
        .filter(|b| b.enabled && secret_eq(&b.secret, &secret));
    let Some(binding) = binding else {
        return reject(
            StatusCode::UNAUTHORIZED,
            "观测入口未启用或已更新，请使用最新启动入口",
        );
    };
    let expected = if binding.provider.protocol == "messages" {
        "v1/messages"
    } else {
        "v1/responses"
    };
    if path != expected || request.method() != axum::http::Method::POST {
        return reject(StatusCode::NOT_FOUND, "此入口仅支持对应协议的模型请求");
    }
    let protocol = binding.provider.protocol.clone();
    forward(
        state,
        Forward {
            agent_product_id: Some(
                if agent == "claude" {
                    "claude-code-cli"
                } else {
                    "codex-cli"
                }
                .into(),
            ),
            provider: binding.provider,
            agent,
            revision: binding.revision,
            client_protocol: protocol,
            credential: None,
            managed: false,
            models: vec![],
        },
        request,
    )
    .await
}

#[derive(Clone)]
pub struct Forward {
    pub agent_product_id: Option<String>,
    pub provider: Provider,
    pub agent: String,
    pub revision: u64,
    pub client_protocol: String,
    pub credential: Option<Zeroizing<Vec<u8>>>,
    pub managed: bool,
    pub models: Vec<String>,
}
pub async fn forward(state: AppState, mut binding: Forward, request: Request) -> Response {
    let agent = binding.agent.clone();
    let admission = request.extensions().get::<crate::lan::Admission>().cloned();
    let local_permit = if admission.is_none() {
        match state.observation.capacity.clone().try_acquire_owned() {
            Ok(permit) => Some(permit),
            Err(_) => {
                let mut response = reject(
                    StatusCode::TOO_MANY_REQUESTS,
                    "当前本机请求过多，请稍后重试",
                );
                response.extensions_mut().insert(LocalCapacity);
                return response;
            }
        }
    } else {
        None
    };
    let client = admission.as_ref().map(|a| a.client.clone());
    let permit = (local_permit, admission);
    let (mut parts, body) = request.into_parts();
    let classifier = parts
        .extensions
        .get::<super::policy::intent::ClassifyRequest>()
        .cloned();
    let agent_product_id = if agent == "codex" {
        if ["originator", "user-agent"].into_iter().any(|name| {
            parts.headers.get_all(name).iter().count() > 1
                || parts.headers.get(name).is_some_and(|v| v.to_str().is_err())
        }) {
            None
        } else {
            local_agent_core::agents::codex_request_product(
                binding.agent_product_id.as_deref(),
                parts
                    .headers
                    .get("originator")
                    .and_then(|v| v.to_str().ok()),
                parts
                    .headers
                    .get("user-agent")
                    .and_then(|v| v.to_str().ok()),
            )
        }
    } else if agent == "opencode" {
        crate::plugin_host::opencode::request_product(&state.readonly.host, &parts.headers)
    } else {
        binding.agent_product_id.clone()
    };
    if parts.headers.get("content-encoding").is_some() {
        return reject(StatusCode::UNSUPPORTED_MEDIA_TYPE, "暂不支持压缩请求体");
    }
    let body = match tokio::time::timeout(Duration::from_secs(30), to_bytes(body, 32 * 1024 * 1024))
        .await
    {
        Ok(Ok(bytes)) => bytes,
        Err(_) => {
            return reject(
                StatusCode::REQUEST_TIMEOUT,
                "请求体读取超时，请重新发起请求",
            );
        }
        Ok(Err(_)) => {
            return reject(
                StatusCode::PAYLOAD_TOO_LARGE,
                "模型请求超出 32 MiB 读取限制",
            );
        }
    };
    let _ = process_clock();
    let clock = std::time::Instant::now();
    let started_at = chrono::Utc::now().to_rfc3339();
    let mut parsed: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => return reject(StatusCode::BAD_REQUEST, "模型请求必须为 JSON"),
    };
    if parsed["background"] == true {
        return reject(StatusCode::BAD_REQUEST, "当前观测入口不支持后台响应模式");
    }
    let user_input = classifier
        .is_none()
        .then(|| local_agent_core::sessions::activity_input::request(&parsed))
        .flatten()
        .map(
            |text| local_agent_core::sessions::activity_input::UserInput {
                text,
                at: started_at.clone(),
            },
        );
    let requested_model = parsed["model"].as_str().map(label);
    if binding.managed && !binding.models.is_empty() {
        let Some(model) = parsed["model"].as_str().and_then(|requested| {
            binding.models.iter().find(|model| {
                if binding.agent_product_id.as_deref() == Some("claude-code-desktop") {
                    local_agent_core::providers::claude_desktop_model_id(
                        &binding.provider.id,
                        model,
                    ) == requested
                } else {
                    model.as_str() == requested
                }
            })
        }) else {
            return reject(
                StatusCode::BAD_REQUEST,
                "请选择此 Provider 分组已发布的模型；未知或未启用模型不会转发",
            );
        };
        binding.provider.model = model.into();
    }
    let session_id = native_session(&agent, &parts.headers, &parsed).map(|session| {
        client
            .as_ref()
            .map_or_else(|| session.clone(), |c| format!("lan:{}:{session}", c.id))
    });
    let mut routing = None;
    if let Some(context) = parts
        .extensions
        .get::<crate::gateway::dynamic::Context>()
        .cloned()
    {
        let requested = parsed["model"].as_str().unwrap_or("");
        let alias = context.service.as_ref().map_or_else(
            || (requested == local_agent_core::routing::ALIAS).then(|| requested.to_owned()),
            |service| service.alias_for_client_model(&context.binding.agent_id, requested),
        );
        let Some(alias) = alias else {
            return reject(
                StatusCode::BAD_REQUEST,
                "动态入口只接受已发布的动态模型标识",
            );
        };
        if parsed
            .get("previous_response_id")
            .is_some_and(|v| !v.is_null())
        {
            return reject(
                StatusCode::BAD_REQUEST,
                "动态入口暂不接收服务端会话引用，请开启完整历史的新会话",
            );
        }
        let evidence = if context.service.is_some() {
            super::dynamic_turn::service_evidence
        } else {
            super::dynamic_turn::evidence
        };
        let responses = binding.client_protocol == "responses";
        let mut projection = if responses {
            match super::dynamic_turn::responses_projection(&parsed) {
                Ok(body) => Some(body),
                Err(message) => return reject(StatusCode::BAD_REQUEST, message),
            }
        } else {
            None
        };
        let (turn, compatible) = match evidence(
            &state.observation.hub,
            projection.as_ref().unwrap_or(&parsed),
            session_id.clone(),
            &agent,
            agent_product_id.as_deref(),
        ) {
            Ok(turn) => turn,
            Err(message) => return reject(StatusCode::BAD_REQUEST, message),
        };
        let dispatch_service =
            context.service.is_some() && parts.extensions.get::<super::policy::Attempt>().is_none();
        let selection = if let Some(overridden) = parts.extensions.get::<super::policy::Selected>()
        {
            Ok(Ok((overridden.0.clone(), None, turn, None)))
        } else {
            let state = state.clone();
            let id = context.binding.id.clone();
            let model = context.service.as_ref().map(|_| alias);
            let requested_output = (model.is_some() && !responses)
                .then(|| parsed["max_tokens"].as_u64().unwrap_or_default());
            let groups = dispatch_service
                .then(|| {
                    super::dynamic_turn::opencode_text_groups(
                        projection.as_ref().unwrap_or(&parsed),
                        &turn,
                    )
                })
                .flatten();
            crate::blocking::run(move || {
                let history = groups.as_ref().and_then(|groups| {
                    let coalesced = groups.iter().any(|group| group.len() > 1);
                    let needs_proof = coalesced
                        || (groups.len() > 1
                            && turn.session.as_deref().is_some_and(|session| {
                                state.store.lock().ok().is_some_and(|store| {
                                    store
                                        .opencode_boundary_needs_proof(&id, session)
                                        .unwrap_or(false)
                                })
                            }));
                    if !needs_proof {
                        return None;
                    }
                    super::dynamic_turn::opencode_history(
                        &state.observation.hub,
                        &state.sessions,
                        groups,
                        &turn,
                        &compatible,
                    )
                });
                let store = state
                    .store
                    .lock()
                    .map_err(|_| ConfigError::new("unavailable", "动态路由存储不可用"))?;
                let history = history.filter(|proof| {
                    state.sessions.opencode_routing_authorized(proof.authority)
                        && store
                            .native_sources()
                            .is_ok_and(|sources| sources.iter().any(|source| source == "opencode"))
                });
                let (selected, effective) = store.gateway_admit(
                    &id,
                    &turn,
                    &compatible,
                    history.as_ref().map(|h| &h.route),
                    model.as_deref(),
                    requested_output,
                )?;
                // Reserve the turn before releasing the store lock. A duplicate
                // cannot overtake admission while the first request checks its input.
                let guard =
                    dispatch_service.then(|| state.observation.policy.reserve_turn(&id, effective));
                Ok::<_, ConfigError>((
                    selected,
                    guard,
                    effective.clone(),
                    history.map(|h| h.intent),
                ))
            })
            .await
        };
        let (mut selected, guard, policy_turn, native_intent) = match selection {
            Ok(Ok(s)) => s,
            Ok(Err(error)) => {
                let status = if error.code == "invalid_output_limit" {
                    StatusCode::BAD_REQUEST
                } else {
                    StatusCode::CONFLICT
                };
                return reject(status, &error.message);
            }
            Err(_) => return reject(StatusCode::SERVICE_UNAVAILABLE, "动态路由选择未完成"),
        };
        let journal_for = |selected: &local_agent_core::routing::Selection| {
            let mut service = selected.service.clone();
            if let Some(service) = &mut service {
                service.upstream_account = None;
                service.upstream_account_label = None;
            }
            super::policy::trace::Journal::new(
                state.observation.clone(),
                ModelObservation {
                    agent_id: agent.clone(),
                    agent_product_id: agent_product_id.clone(),
                    session_id: session_id.clone(),
                    client: client.clone(),
                    model: service.as_ref().map(|s| s.model_name.clone()),
                    provider: service
                        .as_ref()
                        .map_or_else(|| "模型服务".into(), |s| label(&s.service_name)),
                    provider_id: service
                        .as_ref()
                        .map(|s| format!("model-service-{}", s.service_id)),
                    started_at: started_at.clone(),
                    routing: Some(local_agent_core::observation::RouteEvidence {
                        binding_id: context.binding.id.clone(),
                        route_version_id: selected.version.id.clone(),
                        provider_revision: selected.version.provider.revision,
                        boundary: selected.reason.clone(),
                        service,
                        trace: None,
                    }),
                    ..ModelObservation::default()
                },
            )
        };
        let turn_guard = match guard {
            Some(Ok(guard)) => Some(guard),
            Some(Err(message)) => {
                let (mut journal, _) = journal_for(&selected);
                journal.duplicate(&selected);
                return super::policy::rejection(StatusCode::CONFLICT, message);
            }
            None if dispatch_service => {
                return reject(StatusCode::SERVICE_UNAVAILABLE, "本轮路由保护不可用");
            }
            None => None,
        };
        if !responses
            && context.service.is_some()
            && let Some(session) = &policy_turn.session
        {
            let store = state.store.clone();
            let binding_id = context.binding.id.clone();
            let session = session.clone();
            let issued = crate::blocking::run(move || {
                store
                    .lock()
                    .ok()
                    .and_then(|s| s.model_service_notice_hashes(&binding_id, &session).ok())
            })
            .await;
            match issued {
                Ok(Some(issued)) => super::route_notices::strip_history(&mut parsed, &issued),
                Ok(None) => {}
                Err(_) => return reject(StatusCode::SERVICE_UNAVAILABLE, "路由通知读取繁忙"),
            }
        }
        let caps = selected
            .service
            .as_ref()
            .map(|s| s.capabilities.clone())
            .unwrap_or_default();
        let request_stream = parsed["stream"] == true;
        let analysis = projection.as_mut().unwrap_or(&mut parsed);
        if responses && analysis.get("max_tokens").is_none() {
            analysis["max_tokens"] = serde_json::json!(caps.max_output_tokens);
        }
        if let Err(message) = super::dynamic_turn::prepare(analysis, &caps) {
            return reject(StatusCode::BAD_REQUEST, message);
        }
        let request_tools = analysis["tools"].as_array().is_some_and(|t| !t.is_empty());
        let input = analysis.clone();
        if selected.fresh
            && client.is_none()
            && selected
                .service
                .as_ref()
                .is_some_and(|s| s.mode == "default")
            && let Some(text) = native_intent.unwrap_or_else(|| {
                local_agent_core::sessions::activity_input::intent_excerpt(&input)
            })
        {
            parts
                .extensions
                .insert(super::policy::intent::IntentInput(text));
        }
        let max_output_tokens = input["max_tokens"].clone();
        let provider = selected.version.provider.clone();
        let counter_caps = caps.clone();
        let previous = parts
            .extensions
            .get::<super::context_budget::Evaluation>()
            .cloned();
        let evaluation =
            match super::context_budget::evaluate(input, counter_caps, provider, previous).await {
                Ok(b) => b,
                Err(e) => return reject(StatusCode::SERVICE_UNAVAILABLE, e),
            };
        let budget = evaluation.budget.clone();
        parts.extensions.insert(evaluation);
        if budget.input_tokens.saturating_add(budget.output_tokens) > u64::from(caps.context_window)
        {
            return super::context_budget::rejection(&budget);
        }
        let input_tokens = budget.input_tokens;
        if let Some(service) = &mut selected.service {
            service.budget = Some(budget);
        }
        if let Some(turn_guard) = turn_guard {
            drop(permit);
            let (mut journal, attempt) = journal_for(&selected);
            parts.extensions.insert(attempt);
            let response = Box::pin(super::policy::dispatch(
                state,
                binding,
                parts,
                body,
                selected,
                super::policy::DispatchInputs {
                    turn: policy_turn,
                    turn_guard,
                    facts: local_agent_core::model_services::policy::RuleFacts {
                        agent: context.binding.agent_id,
                        agent_product_id,
                        stream: request_stream,
                        tools: request_tools,
                        input_tokens,
                        ..Default::default()
                    },
                    journal: &mut journal,
                },
            ))
            .await;
            journal.finish(response.status());
            return response;
        }
        if selected
            .service
            .as_ref()
            .is_some_and(|service| service.pending_child.is_some())
        {
            return reject(StatusCode::CONFLICT, "子服务路由尚未选定可发送的模型");
        }
        if responses {
            parsed["max_output_tokens"] = max_output_tokens;
            parsed["store"] = serde_json::json!(false);
            if !caps.tools
                && let Some(body) = parsed.as_object_mut()
            {
                for field in ["tools", "tool_choice", "parallel_tool_calls"] {
                    body.remove(field);
                }
            }
        }
        if !caps.thinking && (!responses || selected.version.provider.protocol != "responses") {
            parsed["thinking"] = serde_json::json!({"type":"disabled"});
        }
        let account = selected
            .service
            .as_ref()
            .and_then(|s| s.upstream_account.clone());
        binding.provider = if let Some(account) = account {
            let provider = selected.version.provider;
            match crate::blocking::store(state.store.clone(), move |store| {
                store
                    .upstream_account_target(&provider, &account)
                    .map(|(target, _)| target)
            })
            .await
            {
                Ok(provider) => provider,
                Err(error) => {
                    let mut response = reject(StatusCode::SERVICE_UNAVAILABLE, &error.message);
                    response.extensions_mut().insert(LocalCapacity);
                    if error.code == "account_unavailable" {
                        response.extensions_mut().insert(local_agent_core::model_services::policy::RouteDecisionReason::AccountUnavailable);
                    }
                    return response;
                }
            }
        } else {
            selected.version.provider
        };
        binding.revision = u64::from_str_radix(
            &local_agent_core::providers::files::hash(selected.version.id.as_bytes())[..13],
            16,
        )
        .unwrap_or(1);
        routing = Some(local_agent_core::observation::RouteEvidence {
            trace: None,
            binding_id: context.binding.id.clone(),
            route_version_id: selected.version.id,
            provider_revision: binding.provider.revision,
            boundary: selected.reason,
            service: selected.service,
        });
        binding.credential = match binding.provider.credential_ref.as_ref() {
            Some(reference) => {
                match crate::blocking::secret(state.credentials.clone(), reference.clone()).await {
                    Ok(secret) => Some(secret),
                    Err(error) => return reject(StatusCode::SERVICE_UNAVAILABLE, &error.message),
                }
            }
            None => None,
        };
    }
    let minimax_plan = binding.managed
        && local_agent_core::providers::subscription::minimax_plan(&binding.provider);
    if minimax_plan {
        if client.is_some() {
            return reject(
                StatusCode::FORBIDDEN,
                "MiniMax M Plan 订阅连接仅供本机 Agent 使用",
            );
        }
        if let Err(error) = local_agent_core::providers::subscription::validate_key(
            &binding.provider,
            binding.credential.as_deref().map(Vec::as_slice),
        ) {
            return reject(StatusCode::UNAUTHORIZED, &error.message);
        }
    }
    let tool_calls_allowed = classifier.is_none()
        && routing
            .as_ref()
            .and_then(|route| route.service.as_ref())
            .is_none_or(|service| service.capabilities.tools);
    let codex_client = agent == "codex";
    let hide_intrinsic_reasoning = (classifier.is_some()
        || routing
            .as_ref()
            .is_some_and(|route| route.service.is_some()))
        && local_agent_core::providers::subscription::intrinsic_reasoning(&binding.provider);
    // Decode text-only replies even on the same protocol: a client can still
    // have tools registered, so unexpected upstream tool calls must not reach it.
    let converted = binding.client_protocol != binding.provider.protocol
        || !tool_calls_allowed
        || hide_intrinsic_reasoning;
    let activity_message = classifier
        .is_none()
        .then(|| {
            state
                .observation
                .hub
                .message_identity(&parsed, session_id.as_deref(), agent_product_id.as_deref())
                .map(|id| local_agent_core::observation::ActivityMessage {
                    id,
                    started_at: started_at.clone(),
                })
        })
        .flatten();
    let (tools, parsed) = if converted {
        match crate::gateway::tools::Tools::prepare(&binding.client_protocol, &parsed) {
            Ok(v) => v,
            Err(message) => return reject(StatusCode::BAD_REQUEST, message),
        }
    } else {
        (crate::gateway::tools::Tools::default(), parsed)
    };
    let mut native_protocol = crate::gateway::adapter::Native::new(
        &binding.provider,
        &binding.client_protocol,
        tools,
        tool_calls_allowed,
        binding.managed,
    )
    .hide_intrinsic_reasoning(hide_intrinsic_reasoning);
    let parsed = match native_protocol
        .open(Open {
            stream: parsed["stream"] == true,
            request: parsed,
        })
        .await
    {
        Ok(Opened::Json(v)) => v,
        Ok(Opened::Bytes(_)) => {
            return reject(StatusCode::BAD_REQUEST, "协议转换返回了不匹配的请求");
        }
        Err(message) => return reject(StatusCode::BAD_REQUEST, message),
    };
    let streamed = parsed["stream"] == true;
    let model = parsed["model"].as_str().map(label);
    let logical_id = parts
        .extensions
        .get::<super::policy::Attempt>()
        .map(|a| a.logical_id.clone())
        .or_else(|| {
            parts
                .headers
                .get("x-local-agent-logical-request-id")
                .or_else(|| parts.headers.get("idempotency-key"))
                .and_then(|v| v.to_str().ok())
                .filter(|v| {
                    !v.is_empty()
                        && v.len() <= 128
                        && v.bytes()
                            .all(|c| c.is_ascii_alphanumeric() || b"-_.:".contains(&c))
                })
                .map(|id| {
                    client
                        .as_ref()
                        .map_or_else(|| id.to_owned(), |c| format!("lan:{}:{id}", c.id))
                })
        });
    let mut outgoing = if binding.managed {
        serde_json::to_vec(&parsed).unwrap_or_default()
    } else {
        body.to_vec()
    };
    drop(parsed);
    let upstream_wire =
        match crate::gateway::capabilities::builtin().resolve(&binding.provider.protocol) {
            Ok(wire) => wire,
            Err(message) => return reject(StatusCode::BAD_REQUEST, message),
        };
    let client_wire =
        match crate::gateway::capabilities::builtin().resolve(&binding.client_protocol) {
            Ok(wire) => wire,
            Err(message) => return reject(StatusCode::BAD_REQUEST, message),
        };
    let mut upstream = upstream_wire.endpoint(&binding.provider.endpoint);
    let mut external_protocol = if binding.managed {
        match crate::plugin_host::external::advanced::protocol::prepare(
            state.plugins.clone(),
            &binding.provider,
            &outgoing,
        )
        .await
        {
            Ok(p) => p,
            Err(_) => {
                return reject(
                    StatusCode::BAD_GATEWAY,
                    "外部协议未获授权、版本已变化或初始化失败；未向上游发送请求",
                );
            }
        }
    } else {
        None
    };
    if let Some(protocol) = &mut external_protocol {
        upstream = protocol.url.to_string();
        outgoing = std::mem::take(&mut protocol.body);
    }
    // Keychain and protocol workers can wait. Recheck the account and routing
    // authority after that work and before admitting the HTTP send.
    let account = routing
        .as_ref()
        .and_then(|r| r.service.as_ref())
        .and_then(|s| s.upstream_account.clone());
    let authority = parts.extensions.remove::<super::policy::SendAuthority>();
    let send_evidence = parts.extensions.get::<super::policy::trace::SendEvidence>();
    let check_http_cooldown = send_evidence.is_some();
    let check_access = send_evidence.is_some_and(|evidence| evidence.check_access);
    let pinned_attempt = send_evidence.is_some_and(|evidence| evidence.pending.is_none());
    let pending = send_evidence.and_then(|evidence| evidence.pending.clone());
    let recovery_probe = parts
        .extensions
        .get::<Arc<super::policy::CooldownProbe>>()
        .cloned();
    let check_quota = parts
        .extensions
        .get::<super::policy::CheckAccountQuota>()
        .cloned();
    if account.is_some()
        || pending.is_some()
        || authority.is_some()
        || recovery_probe.is_some()
        || check_quota.is_some()
        || check_http_cooldown
    {
        let provider = binding.provider.clone();
        let state_copy = state.clone();
        match crate::blocking::store(state.store.clone(), move |store| {
            if authority.is_some_and(|authority| !authority.valid(&state_copy.plugins)) {
                let mut response = super::policy::rejection(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "原路由策略授权已变更，本次候选未发送",
                );
                response.extensions_mut().insert(LocalCapacity);
                response.extensions_mut().insert(local_agent_core::model_services::policy::RouteDecisionReason::PolicyUnavailable);
                return Ok(Err(Box::new(response)));
            }
            let target = match &account {
                Some(account) => store.upstream_account_target(&provider, account)?.0,
                None => provider,
            };
            if check_http_cooldown
                && let Some(response) = super::policy::CooldownProbe::check_http(
                    store, &target, recovery_probe.as_deref(), check_access,
                )?
            {
                return Ok(Err(Box::new(response)));
            }
            let begin_send = |quota: Option<QuotaProbeClaim<'_>>| {
                super::policy::CooldownProbe::begin_send(
                    store, recovery_probe.as_deref(),
                    pending.as_ref().map(|(plan, id)| (plan.as_ref(), *id)),
                    quota,
                )
                .map_err(|_| {
                    let mut response = reject(
                        StatusCode::SERVICE_UNAVAILABLE,
                        "发送状态未能保存，本次候选未发送",
                    );
                    response.extensions_mut().insert(LocalCapacity);
                    Box::new(response)
                })
            };
            let admitted = if let Some(check) = check_quota {
                let budget = state_copy
                    .billing
                    .lock()
                    .ok()
                    .map_or_else(crate::billing::UpstreamBudget::unavailable, |billing| {
                        billing.route_budget(store, &target, chrono::Utc::now())
                    });
                check.admit(
                    store,
                    local_agent_core::model_services::policy::scopes(&target).0,
                    budget,
                    begin_send,
                )
            } else {
                begin_send(None)
            };
            Ok(admitted)
        })
        .await
        {
            Ok(Err(response)) => return *response,
            Ok(Ok(())) => (),
            Err(_) => {
                let mut response =
                    reject(StatusCode::SERVICE_UNAVAILABLE, "上游账号在发送前已变更");
                response.extensions_mut().insert(LocalCapacity);
                response.extensions_mut().insert(local_agent_core::model_services::policy::RouteDecisionReason::AccountUnavailable);
                return response;
            }
        }
    }
    if let Some(classifier) = classifier {
        let store = state.store.clone();
        let state = state.clone();
        let provider = binding.provider.clone();
        let admitted = crate::blocking::store(store, move |store| {
            classifier.admit(&state, store, &provider)
        })
        .await
        .is_ok_and(|admitted| admitted);
        if !admitted {
            let mut response = reject(StatusCode::SERVICE_UNAVAILABLE, "意图分类连接已变更");
            response.extensions_mut().insert(LocalCapacity);
            return response;
        }
    }
    let id = uuid::Uuid::new_v4().to_string();
    state.observation.hub.start(
        ModelObservation {
            last_rate_sample: None,
            routing,
            activity_message,
            client,
            sent_ms: None,
            upstream_request_id: None,
            response_id: None,
            client_response_id: None,
            upstream_response_id: None,
            stop_reason: None,
            downstream_status: None,
            requested_model,
            routed_model: model.clone(),
            reported_model: None,
            first_output_kind: None,
            clock_id: Some(process_clock().0.clone()),
            start_tick_ms: Some(
                clock
                    .saturating_duration_since(process_clock().1)
                    .as_secs_f64()
                    * 1000.0,
            ),
            reported_cost: None,
            provider_id: Some(binding.provider.id.clone()),
            agent_product_id,
            id: id.clone(),
            logical_id,
            agent_id: agent,
            session_id,
            model,
            provider: label(&binding.provider.name),
            endpoint: binding.provider.endpoint.clone(),
            binding_revision: binding.revision,
            started_at,
            ended_at: None,
            status: "in_progress".into(),
            http_status: None,
            ttft_ms: None,
            first_text_ms: None,
            last_text_ms: None,
            estimated_text_tokens: None,
            duration_ms: None,
            usage: Usage::default(),
            protocol: binding.provider.protocol.clone(),
            client_protocol: Some(binding.client_protocol.clone()),
            timing_issue: (!streamed)
                .then(|| "非流式响应不测 TTFT；仅保留端到端耗时与已报告用量".into()),
            revision: 0,
        },
        clock,
        user_input,
    );
    let mut guard = Guard {
        observation: state.observation.clone(),
        id: id.clone(),
        done: false,
        mismatched_reply: false,
    };
    let request_client = external_protocol
        .as_ref()
        .map(|p| &p.client)
        .unwrap_or(&state.observation.client);
    let builtin_upstream = external_protocol.is_none();
    let openrouter_reply =
        builtin_upstream && local_agent_core::provider_cost::openrouter_endpoint(&upstream);
    let mut call = request_client
        .post(upstream)
        .header("content-type", "application/json")
        .header("accept-encoding", "identity")
        .body(outgoing);
    for name in [
        "authorization",
        "x-api-key",
        "anthropic-version",
        "anthropic-beta",
        "openai-organization",
        "openai-project",
        "user-agent",
        "session_id",
        "x-codex-turn-metadata",
        "x-client-request-id",
        "idempotency-key",
        "traceparent",
        "tracestate",
    ] {
        if !binding.managed
            && let Some(value) = parts.headers.get(name)
        {
            call = call.header(name, value);
        }
    }
    if binding.managed {
        if let Some(key) = binding.credential.as_deref() {
            let Ok(key) = std::str::from_utf8(key) else {
                return reject(StatusCode::BAD_GATEWAY, "上游凭据不可读取");
            };
            call = call.bearer_auth(key);
            if upstream_wire.api_key_header() && !minimax_plan {
                call = call.header("x-api-key", key);
            }
        }
        for (name, value) in upstream_wire.headers() {
            call = call.header(*name, *value);
        }
    }
    if let Some(slots) = parts.extensions.get::<super::policy::SendSlots>()
        && let Err(error) = slots.mark_sent()
    {
        let mut response = reject(StatusCode::SERVICE_UNAVAILABLE, error);
        response.extensions_mut().insert(LocalCapacity);
        return response;
    }
    state.observation.hub.sent(&id);
    if let Some(evidence) = send_evidence {
        evidence.mark_sent();
    }
    let response = match tokio::time::timeout(Duration::from_secs(120), call.send()).await {
        Ok(Ok(r)) => r,
        Ok(Err(reason)) => {
            guard.finish(
                "error",
                Some(if reason.is_timeout() {
                    "上游连接或首包等待超时"
                } else {
                    "上游连接失败，未收到 HTTP 响应"
                }),
            );
            if reason.is_connect() {
                return reject(StatusCode::BAD_GATEWAY, "目标模型服务连接失败或超时");
            }
            return uncertain_response("未收到目标模型服务响应");
        }
        Err(_) => {
            guard.finish("error", Some("上游响应头等待超时"));
            return uncertain_response("目标模型服务响应超时");
        }
    };
    let status = response.status();
    let observed_at = chrono::Utc::now();
    let retry_after = super::policy::single_retry_after(response.headers()).cloned();
    state.observation.hub.upstream_id(
        &id,
        response
            .headers()
            .get("x-request-id")
            .or_else(|| response.headers().get("request-id"))
            .and_then(|v| v.to_str().ok()),
    );
    state.observation.hub.status(&id, status.as_u16());
    let response = if let Some(protocol) = external_protocol {
        match protocol
            .response(response, binding.credential.as_deref().map(Vec::as_slice))
            .await
        {
            Ok(response) => response,
            Err(_) => {
                let issue = "外部协议适配失败";
                guard.finish("error", Some(issue));
                return uncertain_response(issue);
            }
        }
    } else {
        response
    };
    let compressed = response
        .headers()
        .get("content-encoding")
        .is_some_and(|h| h != "identity");
    let sse = !compressed && super::stream::is_event_stream(response.headers());
    let recovery = Arc::new(super::delivery::Recovery::default());
    let delivery_recovery = recovery.clone();
    let codex_upstream_502 = codex_client && status == StatusCode::BAD_GATEWAY;
    let service_attempt = parts.extensions.get::<super::policy::Attempt>().is_some();
    let classifier_attempt = parts
        .extensions
        .get::<super::policy::intent::ClassifyRequest>()
        .is_some();
    let quota_request = service_attempt || classifier_attempt;
    let service_access_error =
        quota_request && matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN);
    let budget_credential = (quota_request
        && openrouter_reply
        && matches!(status, StatusCode::FORBIDDEN | StatusCode::PAYMENT_REQUIRED))
    .then(|| crate::billing::openrouter_scope(&binding.provider))
    .flatten();
    let balance_target = (quota_request
        && builtin_upstream
        && matches!(
            status,
            StatusCode::PAYMENT_REQUIRED | StatusCode::TOO_MANY_REQUESTS
        ))
    .then(|| crate::billing::balance_target(&binding.provider))
    .flatten()
    .filter(|(provider, _)| super::policy::balance_rejection::matches_status(*provider, status));
    let observed_http_failure = (pinned_attempt || classifier_attempt)
        && matches!(status.as_u16(), 429 | 500 | 502 | 503 | 504 | 529);
    if !status.is_success()
        && (converted
            || codex_upstream_502
            || service_access_error
            || budget_credential.is_some()
            || balance_target.is_some()
            || observed_http_failure)
    {
        let deadline = tokio::time::Instant::now() + super::delivery::RECOVERY_LIMIT;
        let mut input = response.bytes_stream();
        let mut bytes = Vec::new();
        let complete = loop {
            match tokio::time::timeout_at(deadline, input.next()).await {
                Ok(Some(Ok(chunk))) if bytes.len() + chunk.len() <= 64 * 1024 => {
                    bytes.extend_from_slice(&chunk);
                }
                Ok(None) => break true,
                _ => break false,
            }
        };
        // A bounded prefix can parse as JSON even when the response did not
        // finish. Only the complete error body is accounting evidence.
        if complete {
            let mut update = decode(&bytes, &binding.provider.protocol);
            update.generated = false;
            update.text = false;
            update.estimated_tokens = 0.0;
            state.observation.hub.update(&id, update);
        }
        if super::context_budget::is_context_error(&bytes) {
            guard.finish("error", Some("上游上下文超限"));
            return super::context_budget::upstream_rejection();
        }
        let budget = budget_credential
            .filter(|_| complete && !compressed)
            .and_then(|credential| {
                super::policy::openrouter_budget::parse(
                    status,
                    &bytes,
                    retry_after.as_ref(),
                    credential,
                    observed_at,
                )
            });
        let balance = balance_target
            .filter(|_| complete && !compressed)
            .and_then(|target| {
                super::policy::balance_rejection::parse(target, status, &bytes, observed_at)
            });
        let issue = if balance.is_some() {
            "上游确认此凭据的余额或调用额度不可用"
        } else if let Some(budget) = &budget {
            budget.message()
        } else if status == StatusCode::UNAUTHORIZED {
            "上游拒绝鉴权，请检查账号凭据"
        } else if status == StatusCode::FORBIDDEN {
            "上游拒绝访问，请检查账号权限"
        } else if codex_upstream_502 {
            "请求已到达 Local Agent，但上游模型接口返回 502。请在 Local Agent 的「请求」查看对应记录。"
        } else {
            upstream_rejection(&bytes)
        };
        let mut result = reject(status, issue);
        if observed_http_failure {
            result.extensions_mut().insert(ObservedHttpFailure);
        }
        if let Some(balance) = balance {
            result
                .extensions_mut()
                .insert(super::policy::ConfirmedQuota::Balance(balance));
        } else {
            super::policy::mark_upstream_rejection(
                if complete {
                    status
                } else {
                    StatusCode::BAD_GATEWAY
                },
                &bytes,
                result.extensions_mut(),
            );
        }
        if let Some(budget) = budget {
            budget.mark(result.extensions_mut());
        }
        if let Some(access) = result.extensions().get::<super::policy::AccessRejection>() {
            guard.finish("error", Some(access.message()));
            return access.response();
        }
        guard.finish("error", Some(issue));
        if let Some(value) = retry_after {
            result.headers_mut().insert("retry-after", value);
        }
        return result;
    }
    if converted {
        if compressed || (streamed && !sse) || (!streamed && sse) {
            guard.finish("error", Some("上游流式 / 编码类型与请求不符"));
            return uncertain_response("上游返回的流式或编码类型无法转换");
        }
        if !streamed {
            let mut response = response;
            let deadline = tokio::time::Instant::now() + super::delivery::REQUEST_LIMIT;
            loop {
                match tokio::time::timeout_at(
                    deadline.min(tokio::time::Instant::now() + Duration::from_secs(120)),
                    response.chunk(),
                )
                .await
                {
                    Ok(Ok(Some(chunk)))
                        if native_protocol.json_bytes().len() + chunk.len() <= 8 * 1024 * 1024 =>
                    {
                        if native_protocol.feed(&chunk).await.is_err() {
                            guard.finish("interrupted", Some("转换响应超限、中断或超时"));
                            return uncertain_response("上游响应超限、中断或超时");
                        }
                    }
                    Ok(Ok(None)) => break,
                    _ => {
                        guard.finish("interrupted", Some("转换响应超限、中断或超时"));
                        return uncertain_response("上游响应超限、中断或超时");
                    }
                }
            }
            // Accounting is independent of rendering: even an unsupported answer
            // can contain authoritative final usage that must survive the failure.
            let mut update = decode(native_protocol.json_bytes(), &binding.provider.protocol);
            update.generated = false;
            update.text = false;
            update.estimated_tokens = 0.0;
            let upstream_status = update.terminal;
            state.observation.hub.update(&id, update);
            let result = native_protocol.finish().await;
            return match result {
                Ok(Finished::Json(v)) => {
                    if let Some(client_response_id) = native_protocol.response_id() {
                        state
                            .observation
                            .hub
                            .client_response_id(&id, client_response_id);
                    }
                    guard.finish(
                        if upstream_status == Some("incomplete")
                            || native_protocol.end_status() == "incomplete"
                        {
                            "incomplete"
                        } else {
                            "success"
                        },
                        None,
                    );
                    axum::Json(v).into_response()
                }
                Ok(Finished::Stream(_)) => {
                    guard.finish("error", Some("协议转换返回了不匹配的结果"));
                    uncertain_response("协议转换返回了不匹配的结果")
                }
                Err(message) => {
                    guard.finish("error", Some(message));
                    serde_json::from_slice(native_protocol.json_bytes())
                        .ok()
                        .as_ref()
                        .and_then(|value| {
                            super::policy::reply_rejection(
                                value,
                                openrouter_reply,
                                retry_after.as_ref(),
                            )
                        })
                        .unwrap_or_else(|| uncertain_response(message))
                }
            };
        }
        let observation = state.observation.clone();
        let start = match native_protocol.start_stream(upstream_wire, client_wire) {
            Ok(start) => start,
            Err(message) => {
                guard.finish("error", Some(message));
                return uncertain_response(message);
            }
        };
        let delivery_id = id.clone();
        let delivery_observation = observation.clone();
        let stream_rejection = ConvertedStreamRejection::default();
        let recorded_rejection = stream_rejection.clone();
        let stream = async_stream::stream! {
            let _permit=permit;let mut guard=guard;
            let deadline=tokio::time::Instant::now()+super::delivery::REQUEST_LIMIT;
            let mut metrics=upstream_wire.metric_stream();let mut input=response.bytes_stream();
            if let Some(response_id)=native_protocol.response_id(){observation.hub.client_response_id(&id,response_id);}
            yield Ok::<_,std::io::Error>(axum::body::Bytes::from(start));
            loop {
                let wait=Duration::from_secs(120).min(deadline.saturating_duration_since(tokio::time::Instant::now()));
                match tokio::time::timeout(wait,input.next()).await {
                    Ok(Some(Ok(chunk)))=>{
                        if !recovery.allows(chunk.len()) { break; }
                        let received_at=std::time::Instant::now();observation.hub.touch(&id);for update in metrics.feed(&chunk){observation.hub.update_at(&id,update,received_at);}
                        let mut output=Output::default();
                        let converted=native_protocol.feed_stream(&chunk,&mut output);
                        if let Err(message)=converted {
                            guard.finish("error",Some(message));
                            if let Some(rejection)=native_protocol.take_rejection(){let _=recorded_rejection.0.set(rejection);}
                        } else if output.complete {
                            // A client may close as soon as it receives the terminal event.
                            guard.finish(native_protocol.end_status(),None);
                        }
                        for part in output.chunks {if !part.is_empty(){yield Ok(axum::body::Bytes::from(part));}}
                        if output.complete{break;}
                        if converted.is_err() {
                            yield Ok(axum::body::Bytes::from(native_protocol.error()));
                            let recovery_deadline=deadline.min(tokio::time::Instant::now()+super::delivery::RECOVERY_LIMIT);
                            let mut bytes=0usize;
                            loop {
                                match tokio::time::timeout_at(recovery_deadline,input.next()).await {
                                    Ok(Some(Ok(chunk)))=>{
                                        bytes=bytes.saturating_add(chunk.len());
                                        if bytes>super::delivery::RECOVERY_BYTES {break;}
                                        for update in metrics.feed(&chunk) {observation.hub.update(&id,update);}
                                    },
                                    Ok(None)=>{for update in metrics.end() {observation.hub.update(&id,update);}break;},
                                    _=>break,
                                }
                            }
                            break;
                        }
                    },
                    Ok(None)=>{
                        for update in metrics.end(){observation.hub.update(&id,update);}
                        let output=native_protocol.finish().await;
                        match output {
                            Ok(Finished::Stream(output)) if output.complete=>{
                                guard.finish(native_protocol.end_status(),None);
                                for part in output.chunks {if !part.is_empty(){yield Ok(axum::body::Bytes::from(part));}}
                            },
                            _=>{guard.finish("interrupted",Some("上游在完整结束事件前断开"));yield Ok(axum::body::Bytes::from(native_protocol.error()));}
                        }
                        break;
                    },
                    _=>{guard.finish("interrupted",Some("转换流中断或等待超时"));yield Ok(axum::body::Bytes::from(native_protocol.error()));break;}
                }
            }
        };
        let mut result = (
            [
                ("content-type", "text/event-stream"),
                ("cache-control", "no-cache"),
                ("x-accel-buffering", "no"),
            ],
            super::delivery::body(stream, delivery_observation, delivery_id, delivery_recovery),
        )
            .into_response();
        result.extensions_mut().insert(stream_rejection);
        if openrouter_reply {
            result
                .extensions_mut()
                .insert(super::policy::OpenRouterReply);
        }
        if let Some(value) = retry_after {
            result.headers_mut().insert("retry-after", value);
        }
        return result;
    }
    let mismatched_reply = service_attempt && status.is_success() && streamed != sse;
    guard.mismatched_reply = mismatched_reply;
    let mut headers = HeaderMap::new();
    for (name, value) in response.headers() {
        let key = name.as_str();
        if [
            "content-type",
            "content-encoding",
            "cache-control",
            "request-id",
            "x-request-id",
            "retry-after",
        ]
        .contains(&key)
            || key.starts_with("x-ratelimit-")
        {
            headers.append(name, value.clone());
        }
    }
    headers.insert(
        "x-accel-buffering",
        axum::http::HeaderValue::from_static("no"),
    );
    let protocol = binding.provider.protocol.clone();
    let observation = state.observation.clone();
    let delivery_id = id.clone();
    let delivery_observation = observation.clone();
    let stream = async_stream::try_stream! {
        let _permit=permit;let mut guard=guard;let mut decoder=upstream_wire.metric_stream();let mut input=response.bytes_stream();let mut terminal=None;let mut json=Vec::new();let mut total=0usize;
        let deadline=tokio::time::Instant::now()+super::delivery::REQUEST_LIMIT;
        loop {
            let wait=Duration::from_secs(120).min(deadline.saturating_duration_since(tokio::time::Instant::now()));
            let next=tokio::time::timeout(wait,input.next()).await;
            match next {
                Ok(Some(Ok(chunk)))=>{
                    if !recovery.allows(chunk.len()) { break; }
                    observation.hub.touch(&id);
                    if sse && status.is_success(){let received_at=std::time::Instant::now();for update in decoder.feed(&chunk){if let Some(state)=update.terminal{terminal=Some(state);}observation.hub.update_at(&id,update,received_at);}if let Some(state)=terminal{guard.finish(state,None);}}
                    else if !sse && !compressed {total=total.saturating_add(chunk.len());if total<=8*1024*1024{json.extend_from_slice(&chunk);}else{json.clear();}}
                    yield chunk;
                    if sse && terminal.is_some(){break;}
                },
                Ok(None)=>{
                    if sse && status.is_success(){for update in decoder.end(){if let Some(state)=update.terminal{terminal=Some(state);}observation.hub.update(&id,update);}}
                    if !sse && !compressed {let mut update=decode(&json,&protocol);update.generated=false;update.text=false;update.estimated_tokens=0.0;if let Some(state)=update.terminal{terminal=Some(state);}observation.hub.update(&id,update);}
                    let final_status=if !status.is_success(){"error"}else{terminal.unwrap_or("interrupted")};
                    guard.finish(final_status,if compressed{Some("上游返回压缩内容，已原样转发；流式计时和用量不可测")}else if sse&&terminal.is_none(){Some("连接在协议结束事件前关闭，计时样本不进入成功分布")}else{None});break;
                },
                Ok(Some(Err(_)))=>{guard.finish("interrupted",Some("上游流读取中断"));Err(std::io::Error::other("上游流读取中断"))?;},
                Err(_)=>{guard.finish("interrupted",Some("上游流超过 120 秒没有数据"));Err(std::io::Error::new(std::io::ErrorKind::TimedOut,"上游流等待超时"))?;}
            }
        }
    };
    let stream = stream.map(|item: Result<axum::body::Bytes, std::io::Error>| item);
    let mut result = Response::new(if mismatched_reply {
        // Nothing from this wire format can be delivered as a successful reply.
        // Preflight reads a bounded prefix directly for errors and accounting.
        Body::from_stream(stream)
    } else {
        super::delivery::body(stream, delivery_observation, delivery_id, delivery_recovery)
    });
    *result.status_mut() = status;
    *result.headers_mut() = headers;
    if openrouter_reply {
        result
            .extensions_mut()
            .insert(super::policy::OpenRouterReply);
    }
    if mismatched_reply {
        match super::policy::preflight(result, &binding.client_protocol, streamed).await {
            Ok(response) | Err(response) => response,
        }
    } else {
        result
    }
}

fn upstream_rejection(bytes: &[u8]) -> &'static str {
    let value: serde_json::Value = serde_json::from_slice(bytes).unwrap_or_default();
    let message = value["error"]["message"]
        .as_str()
        .unwrap_or("")
        .to_ascii_lowercase();
    // Return fixed diagnostics only, never upstream bodies that may echo prompts or secrets.
    if message.contains("reasoning_content") {
        "上游拒绝请求：助手历史缺少有效思考内容，请开启新会话或使用原连接"
    } else if message.contains("tool_choice") {
        "上游拒绝请求：工具选择与当前思考模式不兼容"
    } else if message.contains("messages") || message.contains("content") {
        "上游拒绝请求：对话角色或内容格式不符合接口要求"
    } else if message.contains("max_tokens") {
        "上游拒绝请求：输出 Token 上限不符合模型要求"
    } else if message.contains("tools") || message.contains("schema") {
        "上游拒绝请求：工具定义或参数格式不符合接口要求"
    } else {
        "上游拒绝跨协议请求；请检查权限、额度与模型能力"
    }
}

fn process_clock() -> &'static (String, std::time::Instant) {
    static CLOCK: std::sync::OnceLock<(String, std::time::Instant)> = std::sync::OnceLock::new();
    CLOCK.get_or_init(|| (uuid::Uuid::new_v4().to_string(), std::time::Instant::now()))
}

#[cfg(test)]
#[path = "proxy_tests.rs"]
mod session_tests;
