//! Explicit LAN model plane. Management remains on its authenticated loopback listener.
use crate::{
    ApiResult, AppState, error, json_value,
    observation::proxy::{Forward, forward},
};
use axum::{
    Json,
    extract::{ConnectInfo, Path, Request, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use local_agent_core::{
    observation::GatewayClient,
    providers::{files, validate_source},
};
use serde::{Deserialize, Serialize};
use std::{
    net::{IpAddr, SocketAddr},
    path::{Path as FilePath, PathBuf},
    sync::{Arc, Mutex},
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
mod operations;
mod registry;
pub(crate) use operations::{commit_operation, operation};

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Config {
    pub listen: SocketAddr,
    pub concurrency: usize,
}
impl Config {
    pub fn validate(&self) -> Result<(), String> {
        let private = match self.listen.ip() {
            IpAddr::V4(ip) => ip.is_private() || ip.is_loopback(),
            IpAddr::V6(ip) => ip.is_loopback() || (ip.segments()[0] & 0xfe00) == 0xfc00,
        };
        if !private || self.listen.port() == 0 || !(1..=64).contains(&self.concurrency) {
            return Err("请指定本机回环或局域网私有地址、固定端口及 1–64 的局域网并发数".into());
        }
        Ok(())
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Client {
    id: String,
    name: String,
    agent: String,
    provider_id: String,
    models: Vec<String>,
    concurrency: usize,
    enabled: bool,
    created_at: String,
    key_hash: String,
}
#[derive(Clone)]
struct Entry {
    client: Client,
    capacity: Arc<Semaphore>,
}
pub struct LanGateway {
    directory: PathBuf,
    pub config: Option<Config>,
    capacity: Arc<Semaphore>,
    clients: Mutex<registry::Registry>,
}
#[derive(Clone)]
pub struct Admission {
    pub client: GatewayClient,
    // The response stream, including cancellation and backpressure, owns both slots.
    _permits: Arc<(OwnedSemaphorePermit, OwnedSemaphorePermit)>,
}

impl LanGateway {
    pub fn open(data: &FilePath) -> Result<Self, String> {
        let directory = data.join("gateway-lan");
        files::private_dir(&directory).map_err(|e| e.message)?;
        let config: Option<Config> = files::read_protected(&directory.join("config.json"))
            .map_err(|e| e.message)?
            .bytes
            .map(|bytes| serde_json::from_slice(&bytes))
            .transpose()
            .map_err(|_| "局域网网关配置损坏")?;
        if let Some(config) = &config {
            config.validate()?;
        }
        let clients = registry::load(&directory).map_err(|e| e.message)?;
        Ok(Self {
            capacity: Arc::new(Semaphore::new(config.as_ref().map_or(1, |c| c.concurrency))),
            config,
            directory,
            clients: Mutex::new(clients),
        })
    }
    pub async fn bind(&self) -> Result<Option<tokio::net::TcpListener>, String> {
        match &self.config {
            Some(config) => tokio::net::TcpListener::bind(config.listen)
                .await
                .map(Some)
                .map_err(|_| "局域网固定端口被占用，未更换端口或停止其他服务".into()),
            None => Ok(None),
        }
    }
    pub(crate) fn ensure_store(&self, store_id: &str) -> Result<(), String> {
        self.clients
            .lock()
            .map_err(|_| "客户端记录暂不可用".to_owned())?
            .ensure_store(store_id)
            .map_err(|e| e.message)
    }
}
fn validate_client(
    name: &str,
    agent: &str,
    models: &[String],
    concurrency: usize,
) -> Result<(), String> {
    if name.trim().is_empty()
        || name.chars().count() > 80
        || name.chars().any(char::is_control)
        || !["claude", "codex", "opencode"].contains(&agent)
        || models.is_empty()
        || models.len() > 200
        || models.iter().any(|m| m.is_empty() || m.len() > 256)
        || !(1..=16).contains(&concurrency)
    {
        return Err("客户端名称、Agent、模型或并发数无效（每客户端 1–16）".into());
    }
    Ok(())
}
pub async fn status(State(state): State<AppState>) -> ApiResult {
    crate::blocking::run(move || {
    let lan = &state.gateway.lan;
    let store_id = state.store.lock().map_err(|_| error("数据库身份暂不可读".into()))?
        .database_id().map_err(crate::providers::error)?;
    let clients = lan
        .clients
        .lock()
        .map_err(|_| error("客户端记录暂不可用".into()))?;
    clients.ensure_store(&store_id).map_err(crate::providers::error)?;
    json_value(serde_json::json!({
        "listen": lan.config.as_ref().map(|c| c.listen.to_string()),
        "concurrency": lan.config.as_ref().map(|c| c.concurrency),
        "active": lan.config.as_ref().map(|c| c.concurrency - lan.capacity.available_permits()),
        "writeProtocol": {"version":1,"storeId":store_id,"maxClients":128,"secretReplay":false},
        "clients": clients.entries.iter().map(|entry| {
            let c = &entry.client;
            serde_json::json!({"id":c.id,"name":c.name,"agent":c.agent,"providerId":c.provider_id,
                "models":c.models,"enabled":c.enabled,"concurrency":c.concurrency,
                "active":c.concurrency-entry.capacity.available_permits(),"createdAt":c.created_at})
        }).collect::<Vec<_>>()
    }))
    })
    .await
    .map_err(|e| crate::providers::error(crate::blocking::config_error(e)))?
}
#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateClient {
    name: String,
    agent: String,
    provider_id: String,
    models: Vec<String>,
    concurrency: usize,
}
pub async fn create(State(state): State<AppState>, Json(input): Json<CreateClient>) -> ApiResult {
    crate::blocking::run(move || {
        validate_client(&input.name, &input.agent, &input.models, input.concurrency)
            .map_err(error)?;
        if state.gateway.lan.config.is_none() {
            return Err(error("此服务未配置局域网入口".into()));
        }
        let provider = state
            .store
            .lock()
            .map_err(|_| error("Provider 暂不可用".into()))?
            .providers()
            .map_err(error)?
            .into_iter()
            .find(|p| p.id == input.provider_id)
            .ok_or_else(|| error("找不到已保存的 Provider".into()))?;
        validate_source(&provider).map_err(|e| error(e.message))?;
        if input
            .models
            .iter()
            .any(|m| !provider.enabled_model_ids().contains(m))
        {
            return Err(error("只能授权此 Provider 已启用的模型".into()));
        }
        let key = zeroize::Zeroizing::new(format!(
            "la_{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        ));
        let client = Client {
            id: uuid::Uuid::new_v4().to_string(),
            name: input.name.trim().into(),
            agent: input.agent,
            provider_id: input.provider_id,
            models: input.models,
            concurrency: input.concurrency,
            enabled: true,
            created_at: chrono::Utc::now().to_rfc3339(),
            key_hash: files::hash(key.as_bytes()),
        };
        let mut clients = state
            .gateway
            .lan
            .clients
            .lock()
            .map_err(|_| error("客户端记录暂不可用".into()))?;
        clients.healthy().map_err(crate::providers::error)?;
        if clients.entries.len() >= 128 {
            return Err(error("局域网客户端已达 128 个".into()));
        }
        let id = client.id.clone();
        let mut next = clients.clone();
        next.entries.push(Entry {
            capacity: Arc::new(Semaphore::new(client.concurrency)),
            client,
        });
        registry::commit(&state.gateway.lan.directory, &mut clients, next)
            .map_err(operations::api_error)?;
        // Only this authenticated creation response contains the new downstream credential.
        json_value(serde_json::json!({"id":id,"key":key.as_str()}))
    })
    .await
    .map_err(|e| crate::providers::error(crate::blocking::config_error(e)))?
}
pub async fn revoke(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult {
    crate::blocking::run(move || {
        let lan = &state.gateway.lan;
        let mut entries = lan
            .clients
            .lock()
            .map_err(|_| error("客户端记录暂不可用".into()))?;
        entries.healthy().map_err(crate::providers::error)?;
        let mut next = entries.clone();
        let entry = next
            .entries
            .iter_mut()
            .find(|e| e.client.id == id)
            .ok_or_else(|| error("客户端不存在".into()))?;
        entry.client.enabled = false;
        registry::commit(&lan.directory, &mut entries, next).map_err(operations::api_error)?;
        json_value(serde_json::json!({"revoked":true}))
    })
    .await
    .map_err(|e| crate::providers::error(crate::blocking::config_error(e)))?
}
fn reject(code: StatusCode, message: &str) -> Response {
    (
        code,
        Json(serde_json::json!({"error":{"type":"gateway_error","message":message}})),
    )
        .into_response()
}

pub async fn capture(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    mut request: Request,
) -> Response {
    let lan = &state.gateway.lan;
    if request.headers().get_all("authorization").iter().count() > 1
        || request.headers().get_all("x-api-key").iter().count() > 1
    {
        return reject(StatusCode::UNAUTHORIZED, "客户端认证头重复");
    }
    if request.headers().contains_key("origin")
        || request.headers().get("host").and_then(|v| v.to_str().ok())
            != lan.config.as_ref().map(|c| c.listen.to_string()).as_deref()
    {
        return reject(StatusCode::UNAUTHORIZED, "局域网入口地址或请求来源无效");
    }
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
    if auth.len() != 67 || !auth.starts_with("la_") {
        return reject(StatusCode::UNAUTHORIZED, "客户端密钥无效或已撤销");
    }
    let hash = files::hash(auth.as_bytes());
    let registry = state.gateway.clone();
    let authenticated = crate::blocking::run(move || {
        let entries = registry
            .lan
            .clients
            .lock()
            .map_err(|_| reject(StatusCode::SERVICE_UNAVAILABLE, "客户端授权暂不可用"))?;
        entries
            .healthy()
            .map_err(|_| reject(StatusCode::SERVICE_UNAVAILABLE, "客户端授权暂不可用"))?;
        let Some(entry) = entries
            .entries
            .iter()
            .find(|e| e.client.enabled && e.client.key_hash == hash)
        else {
            return Err(reject(StatusCode::UNAUTHORIZED, "客户端密钥无效或已撤销"));
        };
        Ok((entry.client.clone(), entry.capacity.clone()))
    })
    .await;
    let (client, client_capacity) = match authenticated {
        Ok(Ok(client)) => client,
        Ok(Err(response)) => return response,
        Err(_) => {
            return reject(
                StatusCode::SERVICE_UNAVAILABLE,
                "客户端验证繁忙，请稍后重试",
            );
        }
    };
    let path = request.uri().path();
    let codex_catalog = path == "/v1/codex/models.json"
        && request.method() == axum::http::Method::GET
        && client.agent == "codex";
    let catalog =
        (path == "/v1/models" && request.method() == axum::http::Method::GET) || codex_catalog;
    if let Some(query) = request.uri().query() {
        let valid = if codex_catalog {
            let pairs: Vec<_> = url::form_urlencoded::parse(query.as_bytes()).collect();
            pairs.len() == 1
                && pairs[0].0 == "client_version"
                && crate::gateway::codex_catalog::valid_client_version(&pairs[0].1)
        } else {
            path == "/v1/messages" && query == "beta=true"
        };
        if !valid {
            return reject(StatusCode::BAD_REQUEST, "此接口的查询参数无效");
        }
    }
    let protocol = match path {
        "/v1/messages" if request.method() == axum::http::Method::POST => "messages",
        "/v1/responses" if request.method() == axum::http::Method::POST => "responses",
        _ if catalog => "messages",
        _ => return reject(StatusCode::NOT_FOUND, "此端口仅提供模型接口"),
    };
    let expected = if client.agent == "codex" {
        "responses"
    } else {
        "messages"
    };
    if !catalog && protocol != expected {
        return reject(StatusCode::FORBIDDEN, "客户端未授权使用此协议");
    }
    let (Ok(total), Ok(own)) = (
        lan.capacity.clone().try_acquire_owned(),
        client_capacity.try_acquire_owned(),
    ) else {
        return reject(
            StatusCode::TOO_MANY_REQUESTS,
            "局域网或此客户端并发已满，请稍后重试",
        );
    };
    let lookup = state.clone();
    let provider_id = client.provider_id.clone();
    let resolved = crate::blocking::run(move || -> Result<_, String> {
        let provider = lookup
            .store
            .lock()
            .map_err(|_| "Provider 暂不可用")?
            .providers()?
            .into_iter()
            .find(|p| p.id == provider_id)
            .ok_or("Provider 已删除")?;
        validate_source(&provider).map_err(|e| e.message)?;
        let credential = provider
            .credential_ref
            .as_ref()
            .map(|id| {
                uuid::Uuid::parse_str(id).map_err(|_| "凭据引用无效".to_owned())?;
                lookup.credentials.get(id).map_err(|e| e.message)
            })
            .transpose()?;
        Ok((provider, credential))
    })
    .await;
    let Ok(Ok((provider, credential))) = resolved else {
        return reject(StatusCode::SERVICE_UNAVAILABLE, "Provider 或其凭据暂不可用");
    };
    let models: Vec<_> = client
        .models
        .iter()
        .filter(|m| provider.enabled_model_ids().contains(m))
        .cloned()
        .collect();
    if models.is_empty() {
        return reject(StatusCode::FORBIDDEN, "此客户端已无可用的授权模型");
    }
    if catalog {
        if codex_catalog {
            return Json(crate::gateway::codex_catalog::catalog(&provider, &models))
                .into_response();
        }
        return Json(serde_json::json!({"object":"list","data":models.iter().map(|id| serde_json::json!({"id":id,"object":"model","owned_by":"local-agent"})).collect::<Vec<_>>()})).into_response();
    }
    let identity = GatewayClient {
        id: client.id.clone(),
        name: client.name,
        peer: peer.ip().to_string(),
    };
    request.extensions_mut().insert(Admission {
        client: identity,
        _permits: Arc::new((total, own)),
    });
    forward(
        state,
        Forward {
            agent_product_id: Some(
                match client.agent.as_str() {
                    "claude" => "claude-code-cli",
                    "codex" => "codex-cli",
                    _ => "opencode",
                }
                .into(),
            ),
            provider,
            agent: client.agent,
            revision: u64::from_str_radix(&files::hash(client.id.as_bytes())[..13], 16)
                .unwrap_or(1),
            client_protocol: protocol.into(),
            credential,
            managed: true,
            models,
        },
        request,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::Config;
    #[test]
    fn oversized_registry_preserves_the_last_committed_clients() {
        let temp = tempfile::tempdir().unwrap();
        let data = temp.path().canonicalize().unwrap();
        let lan = super::LanGateway::open(&data).unwrap();
        let mut registry = lan.clients.lock().unwrap();
        let next = registry.clone();
        super::registry::commit(&lan.directory, &mut registry, next).unwrap();
        let before = std::fs::read(lan.directory.join("clients.json")).unwrap();
        let models: Vec<_> = (0..200)
            .map(|i| format!("{i:03}{}", "x".repeat(252)))
            .collect();
        let entries: Vec<_> = (0..24)
            .map(|_| super::Entry {
                capacity: std::sync::Arc::new(tokio::sync::Semaphore::new(1)),
                client: super::Client {
                    id: uuid::Uuid::new_v4().to_string(),
                    name: "fixture".into(),
                    agent: "claude".into(),
                    provider_id: "fixture".into(),
                    models: models.clone(),
                    concurrency: 1,
                    enabled: true,
                    created_at: "2026-10-22T00:00:00Z".into(),
                    key_hash: "a".repeat(64),
                },
            })
            .collect();
        let mut next = registry.clone();
        next.entries = entries;
        assert!(super::registry::commit(&lan.directory, &mut registry, next).is_err());
        assert_eq!(
            std::fs::read(lan.directory.join("clients.json")).unwrap(),
            before
        );
    }
    #[test]
    fn listener_requires_an_explicit_private_address_fixed_port_and_bounded_capacity() {
        for address in [
            "127.0.0.1:43210",
            "192.168.1.10:43210",
            "10.0.0.2:43210",
            "[::1]:43210",
            "[fd00::1]:43210",
        ] {
            assert!(
                Config {
                    listen: address.parse().unwrap(),
                    concurrency: 16
                }
                .validate()
                .is_ok()
            );
        }
        for address in [
            "0.0.0.0:43210",
            "[::]:43210",
            "8.8.8.8:43210",
            "127.0.0.1:0",
            "224.0.0.1:43210",
        ] {
            assert!(
                Config {
                    listen: address.parse().unwrap(),
                    concurrency: 16
                }
                .validate()
                .is_err()
            );
        }
        for concurrency in [0, 65, usize::MAX] {
            assert!(
                Config {
                    listen: "127.0.0.1:43210".parse().unwrap(),
                    concurrency
                }
                .validate()
                .is_err()
            );
        }
    }
}
