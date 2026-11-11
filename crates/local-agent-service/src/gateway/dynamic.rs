use crate::{ApiResult, AppState, json_value, providers};
use axum::{
    Json,
    extract::{Path, Request, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use local_agent_core::{
    Provider,
    model_services::{self, CONNECTION_AGENTS},
    providers::{BindingDraft, ConfigError, Routing, fingerprint, select_model},
    routing::{self, Binding},
};
use serde::Deserialize;
use serde_json::json;

#[derive(Clone)]
pub struct Context {
    pub binding: Binding,
    pub service: Option<local_agent_core::model_services::ServiceAttachment>,
}
fn unavailable() -> ConfigError {
    ConfigError::new("routing_unavailable", "动态路由暂不可用")
}
fn worker_error(error: crate::blocking::Error) -> (StatusCode, Json<serde_json::Value>) {
    let unknown = matches!(error, crate::blocking::Error::Unavailable);
    let (status, mut body) = providers::error(crate::blocking::config_error(error));
    body.0["outcomeUnknown"] = unknown.into();
    (status, body)
}

#[cfg(test)]
mod admission_tests {
    use super::*;

    #[test]
    fn catalog_query_preserves_defaults_decoding_and_rejects_ambiguous_parameters() {
        for raw in [None, Some("")] {
            let query = CatalogQuery::parse(raw).unwrap();
            assert_eq!(query.limit, 1000);
            assert_eq!(query.after_id, None);
        }
        for raw in [
            "limit=1&after_id=model%2F%E4%B8%80",
            "after_id=model%2F%E4%B8%80&limit=1",
        ] {
            let query = CatalogQuery::parse(Some(raw)).unwrap();
            assert_eq!(query.limit, 1);
            assert_eq!(query.after_id.as_deref(), Some("model/一"));
        }
        for raw in [
            "limit=0",
            "limit=1001",
            "limit=invalid",
            "limit=1&limit=2",
            "after_id=a&after_id=b",
            "after_id=",
            "beta=true",
            "unknown=1",
        ] {
            assert!(CatalogQuery::parse(Some(raw)).is_err(), "{raw}");
        }
    }

    #[test]
    fn rejected_admission_and_unknown_worker_outcome_are_distinct() {
        for (error, code, unknown) in [
            (crate::blocking::Error::Busy, "busy", false),
            (crate::blocking::Error::Unavailable, "unavailable", true),
        ] {
            let (_, Json(body)) = worker_error(error);
            assert_eq!(body["code"], code);
            assert_eq!(body["outcomeUnknown"], unknown);
        }
    }
}
pub fn endpoint(state: &AppState, b: &Binding) -> String {
    format!("http://{}/gateway/{}", state.gateway.host, b.id)
}
pub fn configured_endpoint(state: &AppState, b: &Binding) -> String {
    format!(
        "{}{}",
        endpoint(state, b),
        if ["opencode", "codex"].contains(&b.agent_id.as_str()) {
            "/v1"
        } else {
            ""
        }
    )
}
pub(crate) fn client(state: &AppState, b: &Binding) -> Provider {
    let mut p = b.target.provider.clone();
    if local_agent_core::providers::subscription::minimax_plan(&p) {
        p.vendor = "custom".into();
    }
    p.id = format!("dynamic-{}", b.id);
    p.name = "Local Agent · 动态模型".into();
    p.model = routing::ALIAS.into();
    p.models = vec![];
    p.disabled_models = vec![];
    p.endpoint = if b.agent_id == "codex" {
        configured_endpoint(state, b)
    } else {
        endpoint(state, b)
    };
    p.protocol = client_protocol(&b.agent_id).into();
    p.credential_ref = Some(b.credential_ref.clone());
    p.credential_label = Some("固定本机入口认证".into());
    p
}

fn client_protocol(agent: &str) -> &'static str {
    if agent == "codex" {
        "responses"
    } else {
        "messages"
    }
}
pub async fn connect(State(state): State<AppState>, Json(draft): Json<BindingDraft>) -> ApiResult {
    crate::blocking::run(move || {
        if !["claude", "opencode"].contains(&draft.agent_id.as_str()) {
            return Err(providers::error(ConfigError::new(
                "unsupported",
                "动态接入当前支持 Claude Code CLI 与 OpenCode",
            )));
        }
        providers::require_supported(&draft.agent_id).map_err(providers::error)?;
        let manager = state
            .providers
            .lock()
            .map_err(|_| providers::error(unavailable()))?;
        let store = state
            .store
            .lock()
            .map_err(|_| providers::error(unavailable()))?;
        if let Some(t) = crate::local_tokens::replay_draft(
            &manager,
            &store,
            &draft,
            local_agent_core::providers::local_tokens::Purpose::DynamicBinding,
        )
        .map_err(providers::error)?
        {
            return json_value(t);
        }
        let config = manager
            .inspect_configs()
            .into_iter()
            .find(|c| c.agent_id == draft.agent_id)
            .ok_or_else(|| providers::error(unavailable()))?;
        if config.error.is_some() || config.revision.as_deref() != Some(&draft.expected_revision) {
            return Err(providers::error(ConfigError::new(
                "conflict",
                "配置已变化，请重新读取",
            )));
        }
        if store
            .dynamic_bindings()
            .map_err(providers::error)?
            .iter()
            .any(|b| {
                b.agent_id == draft.agent_id
                    && config.endpoint.as_deref() == Some(&configured_endpoint(&state, b))
            })
        {
            return Err(providers::error(ConfigError::new(
                "conflict",
                "已接入固定入口，请使用动态切换",
            )));
        }
        let source = store
            .providers()
            .map_err(crate::error)?
            .into_iter()
            .find(|p| Some(&p.id) == draft.provider_id.as_ref())
            .ok_or_else(|| {
                providers::error(ConfigError::new("not_found", "请选择已保存的 Provider"))
            })?;
        if Some(source.revision) != draft.expected_provider_revision {
            return Err(providers::error(ConfigError::new(
                "conflict",
                "Provider 已变化",
            )));
        }
        let selected = select_model(&source, draft.model.as_deref()).map_err(providers::error)?;
        manager
            .require_verification(&selected)
            .map_err(providers::error)?;
        json_value(
            crate::local_tokens::dynamic(&state, &manager, &store, &draft, &selected)
                .map_err(providers::error)?,
        )
    })
    .await
    .map_err(worker_error)?
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Draft {
    pub operation_id: String,
    pub binding_id: String,
    pub expected_generation: String,
    pub provider_id: String,
    pub expected_provider_revision: u64,
    pub model: String,
}
fn attached(
    state: &AppState,
    m: &local_agent_core::providers::ProviderManager,
    b: &Binding,
) -> Result<(), ConfigError> {
    if !m.inspect_configs().iter().any(|c| {
        c.agent_id == b.agent_id
            && c.path == b.scope
            && c.error.is_none()
            && c.endpoint.as_deref() == Some(&configured_endpoint(state, b))
    }) {
        return Err(ConfigError::new(
            "not_attached",
            "原生配置未匹配此固定入口；请先完成接入或重新读取",
        ));
    }
    Ok(())
}
fn verified_target(
    store: &local_agent_core::Store,
    m: &local_agent_core::providers::ProviderManager,
    p: &Provider,
) -> Result<(), ConfigError> {
    let current = store
        .providers()
        .map_err(|_| unavailable())?
        .into_iter()
        .find(|s| s.id == p.id)
        .ok_or_else(|| ConfigError::new("conflict", "Provider 已删除"))?;
    let selected = select_model(&current, Some(&p.model))?;
    if fingerprint(&selected) != fingerprint(p) {
        return Err(ConfigError::new(
            "conflict",
            "Provider / 模型 / 凭据版本已变化，请重新预览",
        ));
    }
    m.require_verification(p)
}
pub async fn prepare(State(state): State<AppState>, Json(d): Json<Draft>) -> ApiResult {
    crate::blocking::run(move || {
        let m = state
            .providers
            .lock()
            .map_err(|_| providers::error(unavailable()))?;
        let store = state
            .store
            .lock()
            .map_err(|_| providers::error(unavailable()))?;
        let b = store
            .dynamic_binding(&d.binding_id)
            .map_err(providers::error)?
            .ok_or_else(|| providers::error(unavailable()))?;
        if store
            .model_service_attachment(&b.id)
            .map_err(providers::error)?
            .is_some()
        {
            return Err(providers::error(ConfigError::new(
                "service_managed",
                "此入口由模型服务管理，请修改服务的默认模型",
            )));
        }
        attached(&state, &m, &b).map_err(providers::error)?;
        let p = store
            .providers()
            .map_err(crate::error)?
            .into_iter()
            .find(|p| p.id == d.provider_id && p.revision == d.expected_provider_revision)
            .ok_or_else(|| providers::error(ConfigError::new("conflict", "Provider 已变化")))?;
        let p = select_model(&p, Some(&d.model)).map_err(providers::error)?;
        m.require_verification(&p).map_err(providers::error)?;
        json_value(
            store
                .dynamic_prepare(&d.operation_id, &b.id, &d.expected_generation, &p)
                .map_err(providers::error)?,
        )
    })
    .await
    .map_err(worker_error)?
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Action {
    pub action: String,
}

struct CatalogQuery {
    limit: usize,
    after_id: Option<String>,
}
impl CatalogQuery {
    fn parse(query: Option<&str>) -> Result<Self, ConfigError> {
        let mut limit = None;
        let mut after_id = None;
        for (key, value) in url::form_urlencoded::parse(query.unwrap_or("").as_bytes()) {
            match key.as_ref() {
                "limit" if limit.is_none() => {
                    let value = value.parse::<usize>().map_err(|_| unavailable())?;
                    if !(1..=1000).contains(&value) {
                        return Err(unavailable());
                    }
                    limit = Some(value);
                }
                "after_id" if after_id.is_none() && !value.is_empty() => {
                    after_id = Some(value.into_owned());
                }
                _ => return Err(unavailable()),
            }
        }
        Ok(Self {
            limit: limit.unwrap_or(1000),
            after_id,
        })
    }
}

pub async fn action(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(a): Json<Action>,
) -> ApiResult {
    crate::blocking::run(move || {
        let m = state
            .providers
            .lock()
            .map_err(|_| providers::error(unavailable()))?;
        let store = state
            .store
            .lock()
            .map_err(|_| providers::error(unavailable()))?;
        let c = store.dynamic_change(&id).map_err(providers::error)?;
        if a.action == "cancel" {
            return json_value(store.dynamic_cancel(&id).map_err(providers::error)?);
        }
        if a.action == "commit" && c.state == "applied" {
            return json_value(c);
        }
        let b = store
            .dynamic_binding(&c.binding_id)
            .map_err(providers::error)?
            .ok_or_else(|| providers::error(unavailable()))?;
        attached(&state, &m, &b).map_err(providers::error)?;
        if a.action == "commit" {
            verified_target(&store, &m, &c.target).map_err(providers::error)?;
            json_value(store.dynamic_commit(&id).map_err(providers::error)?)
        } else if a.action == "restore" && c.state == "applied" {
            verified_target(&store, &m, &c.before.provider).map_err(providers::error)?;
            json_value(
                store
                    .dynamic_prepare(
                        &uuid::Uuid::new_v4().to_string(),
                        &b.id,
                        &b.generation,
                        &c.before.provider,
                    )
                    .map_err(providers::error)?,
            )
        } else {
            Err(providers::error(ConfigError::new(
                "invalid_action",
                "不支持此动态路由操作",
            )))
        }
    })
    .await
    .map_err(worker_error)?
}
pub async fn capture(state: AppState, id: String, request: Request) -> Response {
    let store = state.store.clone();
    let plugins = state.plugins.clone();
    let credentials = state.credentials.clone();
    let result = crate::blocking::run(move || {
        let b = store
            .lock()
            .map_err(|_| unavailable())?
            .dynamic_binding(&id)?
            .ok_or_else(unavailable)?;
        let external = routing::agent_connection_digest(&b.agent_id, &b.scope);
        let plugin_active = external.is_some_and(|digest| {
            plugins
                .agent_connection_access(&b.agent_id, None)
                .is_ok_and(|access| access.digest == digest && access.grant.is_some())
        });
        if !CONNECTION_AGENTS.contains(&b.agent_id.as_str()) && !plugin_active {
            return Err(unavailable());
        }
        let path = request.uri().path();
        let prefix = format!("/gateway/{id}/v1/");
        let catalog =
            request.method() == axum::http::Method::GET && path == format!("{prefix}models");
        let codex_catalog = b.agent_id == "codex"
            && request.method() == axum::http::Method::GET
            && path == format!("{prefix}codex/models.json");
        let protocol = client_protocol(&b.agent_id);
        if !catalog
            && !codex_catalog
            && (request.method() != axum::http::Method::POST
                || path != format!("{prefix}{protocol}"))
        {
            return Err(unavailable());
        }
        let query = if catalog {
            Some(CatalogQuery::parse(request.uri().query())?)
        } else {
            if let Some(q) = request.uri().query()
                && !(if codex_catalog {
                    let mut pairs = url::form_urlencoded::parse(q.as_bytes());
                    pairs.next().is_some_and(|(key, version)| {
                        key == "client_version"
                            && super::codex_catalog::valid_client_version(&version)
                    }) && pairs.next().is_none()
                } else {
                    protocol == "messages" && q == "beta=true"
                })
            {
                return Err(unavailable());
            }
            None
        };
        let token = request
            .headers()
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.strip_prefix("Bearer "))
            .or_else(|| {
                request
                    .headers()
                    .get("x-api-key")
                    .and_then(|v| v.to_str().ok())
            })
            .unwrap_or("");
        if token.is_empty() {
            return Err(unavailable());
        }
        uuid::Uuid::parse_str(&b.credential_ref).map_err(|_| unavailable())?;
        let key = credentials.get(&b.credential_ref)?;
        if token.len() != key.len()
            || token
                .bytes()
                .zip(key.iter())
                .fold(0u8, |a, (b, c)| a | (b ^ c))
                != 0
        {
            return Err(unavailable());
        }
        let service = store
            .lock()
            .map_err(|_| unavailable())?
            .model_service_attachment(&b.id)?;
        if service.is_none()
            && !plugin_active
            && !["claude", "opencode"].contains(&b.agent_id.as_str())
        {
            return Err(unavailable());
        }
        Ok::<_, ConfigError>((request, b, query, codex_catalog, service))
    })
    .await;
    let result = match result {
        Ok(result) => result,
        Err(_) => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                "动态入口验证繁忙，请稍后重试",
            )
                .into_response();
        }
    };
    let Ok((mut request, b, query, codex_catalog, service)) = result else {
        return (StatusCode::UNAUTHORIZED, "未授权的动态入口").into_response();
    };
    if service.is_some() && !crate::plugin_host::features::active(&state, "builtin.model-services")
    {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "模型服务插件已停用或卸载，请在插件中心恢复",
        )
            .into_response();
    }
    if codex_catalog && let Some(service) = &service {
        return Json(super::codex_catalog::service_catalog(service)).into_response();
    }
    if let Some(query) = query {
        let entries = service.as_ref().map(|s| s.catalog()).unwrap_or_else(|| {
            vec![(
                routing::ALIAS.into(),
                "Local Agent · 动态模型".into(),
                Default::default(),
            )]
        });
        let after = query.after_id.as_deref();
        let start = after
            .and_then(|id| {
                entries.iter().position(|(entry, _, _)| {
                    model_services::client_model_id(&b.agent_id, entry) == id
                })
            })
            .map_or(0, |i| i + 1);
        if after.is_some() && start == 0 {
            return (StatusCode::BAD_REQUEST, "模型目录游标无效").into_response();
        }
        let data: Vec<_> = entries
            .iter()
            .skip(start)
            .take(query.limit)
            .map(|(id, name, capabilities)| {
                json!({
                    "id": model_services::client_model_id(&b.agent_id, id),
                    "display_name": name,
                    "type": "model",
                    "description": format!(
                        "文本{} · 上下文 {} · 输出 {} · 关闭思考",
                        if capabilities.tools { " / 标准工具" } else { "" },
                        capabilities.context_window,
                        capabilities.max_output_tokens,
                    ),
                    "input_token_limit": capabilities.context_window,
                    "max_tokens": capabilities.max_output_tokens,
                    "capabilities": capabilities,
                })
            })
            .collect();
        return Json(json!({"first_id":data.first().map(|v|&v["id"]),"last_id":data.last().map(|v|&v["id"]),"has_more":start+data.len()<entries.len(),"data":data})).into_response();
    }
    request.extensions_mut().insert(Context {
        binding: b.clone(),
        service,
    });
    let protocol = client_protocol(&b.agent_id);
    let (agent, product) = match b.agent_id.as_str() {
        "claude-desktop" => ("claude".into(), Some("claude-code-desktop".into())),
        "claude" => (b.agent_id, Some("claude-code-cli".into())),
        _ if routing::agent_connection_digest(&b.agent_id, &b.scope).is_some() => (
            b.agent_id
                .split_once(':')
                .expect("validated plugin product")
                .0
                .into(),
            Some(b.agent_id),
        ),
        _ => (b.agent_id, None),
    };
    crate::observation::proxy::forward(
        state,
        crate::observation::proxy::Forward {
            agent_product_id: product,
            provider: b.target.provider,
            agent,
            revision: 1,
            client_protocol: protocol.into(),
            credential: None,
            managed: true,
            models: vec![],
        },
        request,
    )
    .await
}

pub fn ensure(
    b: &Binding,
    route: &Routing,
    provider: &Provider,
    scope: &str,
) -> Result<(), ConfigError> {
    if b.id != route.id
        || b.scope != scope
        || fingerprint(&b.target.provider) != fingerprint(provider)
    {
        return Err(ConfigError::new(
            "conflict",
            "固定入口目标或配置作用域已变化，请重新准备接入",
        ));
    }
    Ok(())
}
