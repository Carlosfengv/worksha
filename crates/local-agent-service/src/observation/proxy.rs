use super::{Observation, label, stream::decode};
use crate::AppState;
use axum::{
    body::to_bytes,
    extract::{Path, Request, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use futures_util::StreamExt;
use local_agent_core::Provider;
use local_agent_core::domain::protocol::{Finished, Open, Opened, ProtocolAdapter};
use local_agent_core::{observation::ModelObservation, sessions::Usage};
use std::{sync::Arc, time::Duration};
use zeroize::Zeroizing;

struct Guard {
    observation: Arc<Observation>,
    id: String,
    done: bool,
}
impl Guard {
    fn finish(&mut self, status: &str, issue: Option<&str>) {
        if !self.done {
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
fn reject(status: StatusCode, message: &str) -> Response {
    (
        status,
        axum::Json(
            serde_json::json!({"error":{"type":"local_observation_error","message":message}}),
        ),
    )
        .into_response()
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
    let (parts, body) = request.into_parts();
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
    let user_input = local_agent_core::sessions::activity_input::request(&parsed).map(|text| {
        local_agent_core::sessions::activity_input::UserInput {
            text,
            at: started_at.clone(),
        }
    });
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
    if let Some(context) = parts.extensions.get::<crate::gateway::dynamic::Context>() {
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
        let turn = match evidence(
            &state.observation.hub,
            projection.as_ref().unwrap_or(&parsed),
            session_id.clone(),
            agent_product_id.as_deref(),
        ) {
            Ok(turn) => turn,
            Err(message) => return reject(StatusCode::BAD_REQUEST, message),
        };
        let policy_turn = turn.clone();
        let selection = if let Some(overridden) = parts.extensions.get::<super::policy::Selected>()
        {
            Ok(Ok(overridden.0.clone()))
        } else {
            let state = state.clone();
            let id = context.binding.id.clone();
            let model = context.service.as_ref().map(|_| alias);
            crate::blocking::run(move || {
                let store = state
                    .store
                    .lock()
                    .map_err(|_| "动态路由存储不可用".to_string())?;
                if let Some(model) = model {
                    store.model_service_admit(&id, &turn, &model)
                } else {
                    store.dynamic_admit(&id, &turn)
                }
                .map_err(|e| e.message)
            })
            .await
        };
        let mut selected = match selection {
            Ok(Ok(s)) => s,
            Ok(Err(message)) => return reject(StatusCode::CONFLICT, &message),
            Err(_) => return reject(StatusCode::SERVICE_UNAVAILABLE, "动态路由选择未完成"),
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
        let analysis = projection.as_mut().unwrap_or(&mut parsed);
        if responses && analysis.get("max_tokens").is_none() {
            analysis["max_tokens"] = serde_json::json!(caps.max_output_tokens);
        }
        if let Err(message) = super::dynamic_turn::prepare(analysis, &caps) {
            return reject(StatusCode::BAD_REQUEST, message);
        }
        let input = analysis.clone();
        let max_output_tokens = input["max_tokens"].clone();
        let provider = selected.version.provider.clone();
        let counter_caps = caps.clone();
        let budget = match super::context_budget::evaluate(input, counter_caps, provider).await {
            Ok(b) => b,
            Err(e) => return reject(StatusCode::SERVICE_UNAVAILABLE, e),
        };
        if budget.input_tokens.saturating_add(budget.output_tokens) > u64::from(caps.context_window)
        {
            return super::context_budget::rejection(&budget);
        }
        if let Some(service) = &mut selected.service {
            service.budget = Some(budget);
        }
        if context.service.is_some() && parts.extensions.get::<super::policy::Attempt>().is_none() {
            drop(permit);
            return Box::pin(super::policy::dispatch(
                state,
                binding,
                parts,
                body,
                selected,
                policy_turn,
            ))
            .await;
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
        binding.provider = selected.version.provider;
        binding.revision = u64::from_str_radix(
            &local_agent_core::providers::files::hash(selected.version.id.as_bytes())[..13],
            16,
        )
        .unwrap_or(1);
        routing = Some(local_agent_core::observation::RouteEvidence {
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
    let tool_calls_allowed = routing
        .as_ref()
        .and_then(|route| route.service.as_ref())
        .is_none_or(|service| service.capabilities.tools);
    let codex_service = agent == "codex"
        && routing
            .as_ref()
            .is_some_and(|route| route.service.is_some());
    // Decode text-only replies even on the same protocol: a client can still
    // have tools registered, so unexpected upstream tool calls must not reach it.
    let converted = binding.client_protocol != binding.provider.protocol || !tool_calls_allowed;
    let activity_message = state
        .observation
        .hub
        .message_identity(&parsed, session_id.as_deref(), agent_product_id.as_deref())
        .map(|id| local_agent_core::observation::ActivityMessage {
            id,
            started_at: started_at.clone(),
        });
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
    );
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
    };
    let request_client = external_protocol
        .as_ref()
        .map(|p| &p.client)
        .unwrap_or(&state.observation.client);
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
            if upstream_wire.api_key_header() {
                call = call.header("x-api-key", key);
            }
        }
        for (name, value) in upstream_wire.headers() {
            call = call.header(*name, *value);
        }
    }
    state.observation.hub.sent(&id);
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
            let mut response = reject(StatusCode::BAD_GATEWAY, "目标模型服务连接失败或超时");
            if !reason.is_connect() {
                response.extensions_mut().insert(UncertainSend);
            }
            return response;
        }
        Err(_) => {
            guard.finish("error", Some("上游响应头等待超时"));
            let mut response = reject(StatusCode::BAD_GATEWAY, "目标模型服务响应超时");
            response.extensions_mut().insert(UncertainSend);
            return response;
        }
    };
    let response = if let Some(protocol) = external_protocol {
        match protocol
            .response(response, binding.credential.as_deref().map(Vec::as_slice))
            .await
        {
            Ok(response) => response,
            Err(_) => {
                guard.finish("error", Some("外部协议适配失败"));
                return reject(
                    StatusCode::BAD_GATEWAY,
                    "外部协议适配失败，未重新发送模型请求",
                );
            }
        }
    } else {
        response
    };
    let status = response.status();
    let retry_after = response.headers().get("retry-after").cloned();
    state.observation.hub.upstream_id(
        &id,
        response
            .headers()
            .get("x-request-id")
            .or_else(|| response.headers().get("request-id"))
            .and_then(|v| v.to_str().ok()),
    );
    state.observation.hub.status(&id, status.as_u16());
    let compressed = response
        .headers()
        .get("content-encoding")
        .is_some_and(|h| h != "identity");
    let sse = !compressed
        && response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.starts_with("text/event-stream"));
    let recovery = Arc::new(super::delivery::Recovery::default());
    let delivery_recovery = recovery.clone();
    let codex_upstream_502 = codex_service && status == StatusCode::BAD_GATEWAY;
    if !status.is_success() && (converted || codex_upstream_502) {
        let deadline = tokio::time::Instant::now() + super::delivery::RECOVERY_LIMIT;
        let mut input = response.bytes_stream();
        let mut bytes = Vec::new();
        while let Ok(Some(Ok(chunk))) = tokio::time::timeout_at(deadline, input.next()).await {
            if bytes.len() + chunk.len() > 64 * 1024 {
                break;
            }
            bytes.extend_from_slice(&chunk);
        }
        let mut update = decode(&bytes, &binding.provider.protocol);
        update.generated = false;
        update.text = false;
        update.estimated_tokens = 0.0;
        state.observation.hub.update(&id, update);
        if super::context_budget::is_context_error(&bytes) {
            guard.finish("error", Some("上游上下文超限"));
            return super::context_budget::upstream_rejection();
        }
        let issue = if codex_upstream_502 && bytes.iter().all(u8::is_ascii_whitespace) {
            "请求已到达 Local Agent，但上游模型接口返回 502。请在 Local Agent 的「请求」查看对应记录。"
        } else {
            upstream_rejection(&bytes)
        };
        guard.finish("error", Some(issue));
        let mut result = reject(status, issue);
        if super::policy::permanent_error(&bytes) {
            result
                .extensions_mut()
                .insert(super::policy::PermanentReject);
        }
        if let Some(value) = retry_after {
            result.headers_mut().insert("retry-after", value);
        }
        return result;
    }
    if converted {
        if compressed || (streamed && !sse) || (!streamed && sse) {
            guard.finish("error", Some("上游流式 / 编码类型与请求不符"));
            return reject(StatusCode::BAD_GATEWAY, "上游返回的流式或编码类型无法转换");
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
                            return reject(StatusCode::BAD_GATEWAY, "上游响应超限、中断或超时");
                        }
                    }
                    Ok(Ok(None)) => break,
                    _ => {
                        guard.finish("interrupted", Some("转换响应超限、中断或超时"));
                        return reject(StatusCode::BAD_GATEWAY, "上游响应超限、中断或超时");
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
                    reject(StatusCode::BAD_GATEWAY, "协议转换返回了不匹配的结果")
                }
                Err(message) => {
                    guard.finish("error", Some(message));
                    reject(StatusCode::BAD_GATEWAY, message)
                }
            };
        }
        let observation = state.observation.clone();
        let start = match native_protocol.start_stream(upstream_wire, client_wire) {
            Ok(start) => start,
            Err(message) => {
                guard.finish("error", Some(message));
                return reject(StatusCode::BAD_GATEWAY, message);
            }
        };
        let delivery_id = id.clone();
        let delivery_observation = observation.clone();
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
                        let output=native_protocol.feed(&chunk).await;
                        match output {Ok(output)=>{
                            // A client may close immediately after the protocol terminal event.
                            // Finalize from validated upstream completion, before yielding it.
                            if output.complete{guard.finish(native_protocol.end_status(),None);}
                            for part in output.chunks {if !part.is_empty(){yield Ok(axum::body::Bytes::from(part));}}
                            if output.complete{break;}
                        },Err(message)=>{
                            guard.finish("error",Some(message));
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
                        }}
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
        return (
            [
                ("content-type", "text/event-stream"),
                ("cache-control", "no-cache"),
                ("x-accel-buffering", "no"),
            ],
            super::delivery::body(stream, delivery_observation, delivery_id, delivery_recovery),
        )
            .into_response();
    }
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
    let mut result = Response::new(super::delivery::body(
        stream,
        delivery_observation,
        delivery_id,
        delivery_recovery,
    ));
    *result.status_mut() = status;
    *result.headers_mut() = headers;
    result
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
