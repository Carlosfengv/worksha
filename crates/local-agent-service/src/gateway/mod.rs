pub(crate) mod adapter;
pub(crate) mod capabilities;
pub(crate) mod codex_catalog;
mod deepseek;
#[cfg(test)]
mod deepseek_tests;
pub(crate) mod dynamic;
pub mod protocol;
pub mod stream;
#[cfg(test)]
mod test_support;
#[cfg(test)]
mod tests;
pub mod tools;
use crate::{
    AppState,
    observation::proxy::{Forward, forward},
};
use axum::{
    extract::{Path, Request, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use local_agent_core::{
    Provider,
    providers::{ConfigError, Routing, files, fingerprint, validate_source},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    path::{Path as FilePath, PathBuf},
    sync::Mutex,
};

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Route {
    agent: String,
    provider: Provider,
    local_credential_ref: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    local_token_owner: Option<local_agent_core::providers::credential_scope::IdentityV3>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    models: Vec<String>,
}
pub struct Gateway {
    pub lan: crate::lan::LanGateway,
    directory: PathBuf,
    pub host: String,
    routes: Mutex<HashMap<String, Route>>,
    _lock: std::fs::File,
}
// Preserve the historical first 13 SHA-256 hex digits without formatting and reparsing them.
fn route_revision(id: &str) -> u64 {
    Sha256::digest(id.as_bytes())[..7]
        .iter()
        .fold(0, |value, byte| (value << 8) | u64::from(*byte))
        >> 4
}
impl Gateway {
    fn client_provider(&self, id: &str, route: &Route) -> Provider {
        let mut p = route.provider.clone();
        p.protocol = if route.agent == "codex" {
            "responses"
        } else {
            "messages"
        }
        .into();
        p.endpoint = format!(
            "http://{}/gateway/{id}{}",
            self.host,
            if route.agent == "codex" { "/v1" } else { "" }
        );
        p.credential_ref = Some(route.local_credential_ref.clone());
        p.credential_label = Some("本机路由认证".into());
        p
    }
    pub fn usage_evidence(
        &self,
    ) -> Result<Vec<local_agent_core::provider_usage::Evidence>, String> {
        let routes = self.routes.lock().map_err(|_| "路由归属暂不可用")?;
        Ok(routes
            .iter()
            .map(|(id, r)| {
                let native_provider = match r.agent.as_str() {
                    "opencode" => Some(format!(
                        "local_agent_{}",
                        &files::hash(r.provider.id.as_bytes())[..16]
                    )),
                    "codex" => Some(format!(
                        "local_agent_{}",
                        &fingerprint(&self.client_provider(id, r))[..24]
                    )),
                    _ => None,
                };
                local_agent_core::provider_usage::Evidence {
                    agent: if r.agent == "claude-desktop" {
                        "claude".into()
                    } else {
                        r.agent.clone()
                    },
                    provider_id: r.provider.id.clone(),
                    provider_name: r.provider.name.clone(),
                    endpoint: r.provider.endpoint.clone(),
                    protocol: r.provider.protocol.clone(),
                    revision: route_revision(id),
                    native_provider,
                    model_aliases: if r.agent == "claude-desktop" {
                        r.models
                            .iter()
                            .map(|m| {
                                (
                                    local_agent_core::providers::claude_desktop_model_id(
                                        &r.provider.id,
                                        m,
                                    ),
                                    m.clone(),
                                )
                            })
                            .collect()
                    } else {
                        vec![]
                    },
                }
            })
            .collect())
    }
    pub async fn bind(directory: &FilePath) -> Result<(Self, tokio::net::TcpListener), String> {
        let directory = directory.join("gateway");
        files::private_dir(&directory).map_err(|e| e.message)?;
        let lock =
            files::exclusive_lock(&directory.join("listener.lock")).map_err(|e| e.message)?;
        let saved = files::read_protected(&directory.join("listener.json"))
            .map_err(|e| e.message)?
            .bytes;
        let had_saved_port = saved.is_some();
        let port: u16 = match saved {
            Some(bytes) => serde_json::from_slice(&bytes).map_err(|_| "本地网关端口记录损坏")?,
            None => 0,
        };
        if had_saved_port && port == 0 {
            return Err("本地网关固定端口记录无效".into());
        }
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port))
            .await
            .map_err(|_| "本地网关固定端口被占用；为保留已写入的路由地址，未更换端口")?;
        let address = listener.local_addr().map_err(|_| "本地网关地址不可用")?;
        if port == 0 {
            files::atomic_write(
                &directory.join("listener.json"),
                &serde_json::to_vec(&address.port()).map_err(|_| "端口无效")?,
                0o600,
            )
            .map_err(|e| e.message)?;
        }
        let mut routes = HashMap::new();
        for file in std::fs::read_dir(&directory).map_err(|_| "路由目录不可读取")? {
            let file = file.map_err(|_| "路由记录不可读取")?;
            let name = file.file_name().to_string_lossy().into_owned();
            if let Some(id) = name
                .strip_prefix("route-")
                .and_then(|s| s.strip_suffix(".json"))
            {
                uuid::Uuid::parse_str(id).map_err(|_| "路由 ID 无效")?;
                let bytes = files::read_protected(&file.path())
                    .map_err(|e| e.message)?
                    .bytes
                    .ok_or("路由记录缺失")?;
                let route: Route = serde_json::from_slice(&bytes).map_err(|_| "路由记录损坏")?;
                validate_source(&route.provider).map_err(|e| e.message)?;
                if !route.models.is_empty()
                    && (!["opencode", "claude-desktop", "codex"].contains(&route.agent.as_str())
                        || route.models != route.provider.enabled_model_ids())
                {
                    return Err("路由模型分组与供应商快照不一致".into());
                }
                if !["claude", "codex", "opencode", "claude-desktop"]
                    .contains(&route.agent.as_str())
                    || uuid::Uuid::parse_str(&route.local_credential_ref).is_err()
                {
                    return Err("路由身份无效".into());
                }
                routes.insert(id.to_string(), route);
                if routes.len() > 500 {
                    return Err("本地路由记录超过当前容量上限".into());
                }
            }
        }
        Ok((
            Self {
                lan: crate::lan::LanGateway::open(directory.parent().ok_or("网关目录无效")?)?,
                directory,
                host: address.to_string(),
                routes: Mutex::new(routes),
                _lock: lock,
            },
            listener,
        ))
    }
    /// A loopback URL alone is not evidence that this app owns the route.
    pub fn configured_provider(
        &self,
        agent: &str,
        endpoint: &str,
    ) -> Result<Option<Provider>, ConfigError> {
        let routes = self
            .routes
            .lock()
            .map_err(|_| ConfigError::new("busy", "本地路由状态暂不可用"))?;
        Ok(routes.iter().find_map(|(id, route)| {
            let suffix = if ["codex", "opencode"].contains(&agent) {
                "/v1"
            } else {
                ""
            };
            (route.agent == agent
                && endpoint.trim_end_matches('/')
                    == format!("http://{}/gateway/{id}{suffix}", self.host))
            .then(|| route.provider.clone())
        }))
    }
    pub fn route_models(agent: &str, provider: &Provider) -> Result<Vec<String>, ConfigError> {
        validate_source(provider)?;
        let models = if ["opencode", "claude-desktop", "codex"].contains(&agent) {
            provider.enabled_model_ids()
        } else {
            vec![]
        };
        if agent == "claude-desktop" && models.len() > 200 {
            return Err(ConfigError::new(
                "capacity",
                "Claude 桌面最多支持 200 个已启用模型",
            ));
        }
        if agent == "codex"
            && serde_json::to_vec(&capabilities::codex_catalog(provider, &models))
                .map_or(true, |bytes| bytes.len() > 1024 * 1024)
        {
            return Err(ConfigError::new(
                "capacity",
                "Codex 模型目录超过 1 MiB，请先减少此 Provider 的已启用模型",
            ));
        }
        Ok(models)
    }
    fn rendered_route(&self, id: &str, route: &Route) -> (Provider, Routing) {
        let rendered = self.client_provider(id, route);
        let routing = Routing {
            id: id.into(),
            client_protocol: if route.agent == "codex" {
                "responses"
            } else {
                "messages"
            }
            .into(),
            upstream_protocol: route.provider.protocol.clone(),
            local_endpoint: rendered.endpoint.clone(),
            models: route.models.clone(),
        };
        (rendered, routing)
    }
    pub fn existing_prepared(
        &self,
        store: &local_agent_core::Store,
        agent: &str,
        provider: &Provider,
    ) -> Result<Option<(Provider, Routing)>, ConfigError> {
        let models = Self::route_models(agent, provider)?;
        let (existing, count) = {
            let routes = self
                .routes
                .lock()
                .map_err(|_| ConfigError::new("busy", "本机路由暂不可用"))?;
            (
                routes
                    .iter()
                    .find(|(_, r)| {
                        r.agent == agent
                            && r.models == models
                            && fingerprint(&r.provider) == fingerprint(provider)
                    })
                    .map(|(id, r)| (id.clone(), r.clone())),
                routes.len(),
            )
        };
        if let Some((id, route)) = existing {
            if let Some(identity) = &route.local_token_owner {
                let record = store.local_token_record(identity)?.ok_or_else(|| {
                    ConfigError::new(
                        "local_token_record_unavailable",
                        "原本机路由归属缺失，原文件保留",
                    )
                })?;
                if record.status == local_agent_core::providers::local_tokens::Status::Pending {
                    return Err(ConfigError::new(
                        "local_token_recovery_required",
                        "原本机入口仍待核查，请按原编号查询或明确继续",
                    ));
                }
            }
            let result = self.rendered_route(&id, &route);
            self.ensure(&result.1, provider)?;
            return Ok(Some(result));
        }
        if count >= 500 {
            return Err(ConfigError::new(
                "capacity",
                "已保留 500 条路由快照，请先核查备份与留存",
            ));
        }
        Ok(None)
    }
    pub fn prepare_owned_route(
        &self,
        session: &mut local_agent_core::providers::local_tokens::Session<'_>,
    ) -> Result<(Provider, Routing), ConfigError> {
        use local_agent_core::providers::local_tokens::{Purpose, Status};
        let operation = session.operation().clone();
        let request = &operation.request;
        if request.purpose != Purpose::FixedRoute || operation.status != Status::Pending {
            return Err(ConfigError::new(
                "local_token_cancelled",
                "原本机路由操作不可再创建入口",
            ));
        }
        let models = Self::route_models(&request.agent_id, &request.provider)?;
        {
            let routes = self
                .routes
                .lock()
                .map_err(|_| ConfigError::new("busy", "本机路由暂不可用"))?;
            if routes.len() >= 500 && !routes.contains_key(&operation.owner_id) {
                return Err(ConfigError::new(
                    "capacity",
                    "已保留 500 条路由快照，请先核查备份与留存",
                ));
            }
        }
        session.ensure_credential()?;
        let route = Route {
            agent: request.agent_id.clone(),
            provider: request.provider.clone(),
            local_credential_ref: operation.credential_ref.clone(),
            models,
            local_token_owner: Some(request.identity.clone()),
        };
        let file = self
            .directory
            .join(format!("route-{}.json", operation.owner_id));
        let bytes = serde_json::to_vec(&route)
            .map_err(|_| ConfigError::new("write_failed", "本机路由记录无效"))?;
        match files::read_protected(&file)?.bytes {
            Some(existing) if existing != bytes => {
                return Err(ConfigError::new(
                    "conflict",
                    "原本机路由文件已变化，未覆盖或创建第二份引用",
                ));
            }
            Some(_) => (),
            None => files::atomic_write(&file, &bytes, 0o600)?,
        }
        if files::read_protected(&file)?.bytes.as_deref() != Some(bytes.as_slice()) {
            return Err(ConfigError::new(
                "local_token_result_unknown",
                "本机路由持久结果尚未确认，请保留原编号核查",
            ));
        }
        session.mark_owner_saved()?;
        let mut routes = self
            .routes
            .lock()
            .map_err(|_| ConfigError::new("busy", "本机路由暂不可用"))?;
        if let Some(existing) = routes.get(&operation.owner_id) {
            if serde_json::to_vec(existing).ok() != Some(bytes) {
                return Err(ConfigError::new(
                    "conflict",
                    "原本机路由内存快照已变化，未覆盖",
                ));
            }
        } else {
            if routes.len() >= 500 {
                return Err(ConfigError::new(
                    "capacity",
                    "本机路由名额已变化，原归属保留待核查",
                ));
            }
            routes.insert(operation.owner_id.clone(), route.clone());
        }
        Ok(self.rendered_route(&operation.owner_id, &route))
    }
    pub fn validate_token_owners(&self, store: &local_agent_core::Store) -> Result<(), String> {
        let routes = self.routes.lock().map_err(|_| "本机路由归属暂不可用")?;
        for (id, route) in routes.iter() {
            let Some(identity) = &route.local_token_owner else {
                continue;
            };
            let operation = store
                .local_token_record(identity)
                .map_err(|e| e.message)?
                .ok_or("本机路由的原令牌归属缺失；未认领旧材料")?;
            if operation.owner_id != *id
                || operation.request.purpose
                    != local_agent_core::providers::local_tokens::Purpose::FixedRoute
                || operation.request.agent_id != route.agent
                || operation.credential_ref != route.local_credential_ref
                || fingerprint(&operation.request.provider) != fingerprint(&route.provider)
            {
                return Err("本机路由与原令牌归属不一致，原记录保留".into());
            }
        }
        Ok(())
    }
    pub fn ensure(&self, routing: &Routing, provider: &Provider) -> Result<(), ConfigError> {
        let routes = self
            .routes
            .lock()
            .map_err(|_| ConfigError::new("busy", "路由不可用"))?;
        if routes.get(&routing.id).is_some_and(|r| {
            r.models == routing.models && fingerprint(&r.provider) == fingerprint(provider)
        }) {
            uuid::Uuid::parse_str(&routing.id)
                .map_err(|_| ConfigError::new("conflict", "路由 ID 无效"))?;
            let bytes =
                files::read_protected(&self.directory.join(format!("route-{}.json", routing.id)))?
                    .bytes
                    .ok_or_else(|| ConfigError::new("conflict", "路由记录已被删除"))?;
            let saved: Route = serde_json::from_slice(&bytes)
                .map_err(|_| ConfigError::new("conflict", "路由记录已被修改"))?;
            let current = &routes[&routing.id];
            if fingerprint(&saved.provider) != fingerprint(provider)
                || saved.local_credential_ref != current.local_credential_ref
                || saved.agent != current.agent
                || saved.models != current.models
                || serde_json::to_vec(&saved.local_token_owner).ok()
                    != serde_json::to_vec(&current.local_token_owner).ok()
            {
                return Err(ConfigError::new("conflict", "路由快照已被外部修改"));
            }
            Ok(())
        } else {
            Err(ConfigError::new(
                "conflict",
                "本地转换路由已改变，请重新预览",
            ))
        }
    }
}
pub async fn capture(
    State(state): State<AppState>,
    Path((id, path)): Path<(String, String)>,
    request: Request,
) -> Response {
    let reject = || {
        (StatusCode::UNAUTHORIZED,axum::Json(serde_json::json!({"error":{"type":"authentication_error","message":"未授权的本机路由请求"}}))).into_response()
    };
    if request.headers().get("host").and_then(|v| v.to_str().ok())
        != Some(state.gateway.host.as_str())
        || request.headers().contains_key("origin")
    {
        return reject();
    }
    let route = state
        .gateway
        .routes
        .lock()
        .ok()
        .and_then(|r| r.get(&id).cloned());
    let Some(route) = route else {
        return dynamic::capture(state, id, request).await;
    };
    let codex_catalog = route.agent == "codex"
        && !route.models.is_empty()
        && path == "v1/codex/models.json"
        && request.method() == axum::http::Method::GET;
    if let Some(query) = request.uri().query() {
        if codex_catalog {
            let pairs: Vec<_> = url::form_urlencoded::parse(query.as_bytes()).collect();
            if pairs.len() != 1 || pairs[0].0 != "client_version" {
                return reject();
            }
            if pairs[0].1.len() > 80
                || !local_agent_core::providers::config::builtin()
                    .resolve("codex")
                    .is_ok_and(|a| a.version_supported(&pairs[0].1))
            {
                return (
                    StatusCode::BAD_REQUEST,
                    "此 Codex 客户端版本尚未通过模型目录适配验证",
                )
                    .into_response();
            }
        } else if query != "beta=true"
            || !["v1/messages", "v1/messages/count_tokens"].contains(&path.as_str())
        {
            return reject();
        }
    }
    let client = if route.agent != "codex" {
        "messages"
    } else {
        "responses"
    };
    let catalog = request.method() == axum::http::Method::GET && path == "v1/models";
    if !catalog
        && !codex_catalog
        && (request.method() != axum::http::Method::POST || path != format!("v1/{client}"))
    {
        return (StatusCode::NOT_FOUND, "此路由只接收指定 Agent 协议").into_response();
    }
    let local = match crate::blocking::secret(
        state.credentials.clone(),
        route.local_credential_ref.clone(),
    )
    .await
    {
        Ok(secret) => secret,
        Err(e) if e.code == "busy" => {
            return (StatusCode::SERVICE_UNAVAILABLE, e.message).into_response();
        }
        Err(_) => return reject(),
    };
    let auth = request
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .or_else(|| {
            request
                .headers()
                .get("x-api-key")
                .and_then(|v| v.to_str().ok())
        })
        .unwrap_or("");
    if auth.len() != local.len()
        || auth
            .bytes()
            .zip(local.iter())
            .fold(0u8, |a, (b, c)| a | (b ^ c))
            != 0
    {
        return reject();
    }
    let upstream = if let Some(reference) = &route.provider.credential_ref {
        match crate::blocking::secret(state.credentials.clone(), reference.clone()).await {
            Ok(secret) => Some(secret),
            Err(e) => return (StatusCode::SERVICE_UNAVAILABLE, e.message).into_response(),
        }
    } else {
        None
    };
    if codex_catalog {
        return axum::Json(capabilities::codex_catalog(&route.provider, &route.models))
            .into_response();
    }
    if catalog {
        let path = state.gateway.directory.join(format!("route-{id}.json"));
        let created = match crate::blocking::run(move || {
            std::fs::metadata(path)
                .and_then(|m| m.modified())
                .ok()
                .map(|t| chrono::DateTime::<chrono::Utc>::from(t).to_rfc3339())
        })
        .await
        {
            Ok(created) => created,
            Err(_) => return (StatusCode::SERVICE_UNAVAILABLE, "模型目录读取繁忙").into_response(),
        };
        let mut models = if route.models.is_empty() {
            vec![route.provider.model.clone()]
        } else {
            route.models.clone()
        };
        if route.agent == "claude-desktop" {
            models.sort_by_key(|model| model != &route.provider.model);
        }
        let data: Vec<_> = models.iter().map(|model| {
            let id = if route.agent == "claude-desktop" {
                if route.models.is_empty() { "claude-sonnet-5".into() }
                else { local_agent_core::providers::claude_desktop_model_id(&route.provider.id, model) }
            } else { model.clone() };
            serde_json::json!({"type":"model","id":id,"display_name":format!("{} · {}",route.provider.name,model),"created_at":created})
        }).collect();
        return axum::Json(
            serde_json::json!({"data":data,"has_more":false,"first_id":null,"last_id":null}),
        )
        .into_response();
    }
    forward(
        state,
        Forward {
            agent_product_id: match route.agent.as_str() {
                "claude-desktop" => Some("claude-code-desktop".into()),
                "claude" => Some("claude-code-cli".into()),
                "opencode" => Some("opencode".into()),
                _ => None,
            },
            provider: route.provider,
            agent: if route.agent == "claude-desktop" {
                "claude".into()
            } else {
                route.agent
            },
            revision: route_revision(&id),
            client_protocol: client.into(),
            credential: upstream,
            managed: true,
            models: route.models,
        },
        request,
    )
    .await
}
