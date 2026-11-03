use super::{CreateClient, Entry, registry};
use crate::{ApiResult, AppState};
use axum::{
    Json,
    extract::rejection::{JsonRejection, QueryRejection},
    extract::{Path, Query, State},
    http::StatusCode,
};
use local_agent_core::providers::{ConfigError, files};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{net::SocketAddr, sync::Arc};
use tokio::sync::Semaphore;

#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Request {
    pub(super) store_id: String,
    pub(super) operation_id: String,
    pub(super) action: Action,
}
#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub(super) enum Action {
    Create { draft: CreateClient },
    Revoke { client_id: String },
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Receipt {
    pub api_version: u32,
    pub store_id: String,
    pub operation_id: String,
    pub action: String,
    pub client_id: String,
    pub listen: String,
    pub completed_at: String,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Outcome {
    pub request: Request,
    pub receipt: Receipt,
}
pub(super) fn uuid4(id: &str) -> bool {
    uuid::Uuid::parse_str(id).is_ok_and(|u| u.get_version_num() == 4 && u.to_string() == id)
}
fn fail(code: &str, message: &str) -> ConfigError {
    ConfigError::new(code, message)
}
impl CreateClient {
    pub(super) fn matches(&self, c: &super::Client) -> bool {
        self.name.trim() == c.name
            && self.agent == c.agent
            && self.provider_id == c.provider_id
            && self.models == c.models
            && self.concurrency == c.concurrency
    }
}
impl Request {
    fn validate(&self) -> Result<(), ConfigError> {
        if !uuid4(&self.store_id)
            || !uuid4(&self.operation_id)
            || serde_json::to_vec(self)
                .map_err(|_| registry::invalid())?
                .len()
                > 65_536
        {
            return Err(fail("invalid_input", "客户端操作编号、来源或请求大小无效"));
        }
        match &self.action {
            Action::Create { draft } => {
                super::validate_client(&draft.name, &draft.agent, &draft.models, draft.concurrency)
                    .map_err(|_| fail("invalid_input", "客户端名称、Agent、模型或并发数无效"))
            }
            Action::Revoke { client_id }
                if uuid::Uuid::parse_str(client_id).is_ok_and(|u| u.to_string() == *client_id) =>
            {
                Ok(())
            }
            _ => Err(fail("invalid_input", "客户端身份无效")),
        }
    }
}
impl Outcome {
    pub(super) fn validate(&self) -> Result<(), ConfigError> {
        self.request.validate()?;
        let r = &self.receipt;
        let (kind, id) = match &self.request.action {
            Action::Create { .. } => ("create", self.request.operation_id.as_str()),
            Action::Revoke { client_id } => ("revoke", client_id.as_str()),
        };
        let listen = r
            .listen
            .parse::<SocketAddr>()
            .map_err(|_| registry::invalid())?;
        super::Config {
            listen,
            concurrency: 1,
        }
        .validate()
        .map_err(|_| registry::invalid())?;
        if r.api_version != 1
            || r.store_id != self.request.store_id
            || r.operation_id != self.request.operation_id
            || r.action != kind
            || r.client_id != id
            || chrono::DateTime::parse_from_rfc3339(&r.completed_at).is_err()
        {
            return Err(registry::invalid());
        }
        Ok(())
    }
}
pub(super) fn api_error(e: ConfigError) -> (StatusCode, Json<Value>) {
    if e.code == "gateway_result_unknown" {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"code":e.code,"message":e.message,"outcomeUnknown":true})),
        )
    } else {
        crate::providers::error(e)
    }
}
fn worker_error(e: crate::blocking::Error) -> (StatusCode, Json<Value>) {
    match e {
        crate::blocking::Error::Busy => crate::providers::error(crate::blocking::config_error(e)),
        crate::blocking::Error::Unavailable => api_error(fail(
            "gateway_result_unknown",
            "客户端操作结果尚未确认，请核查原编号",
        )),
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Scope {
    store_id: String,
}
pub(crate) async fn operation(
    State(state): State<AppState>,
    Path(id): Path<String>,
    query: Result<Query<Scope>, QueryRejection>,
) -> ApiResult {
    let Query(scope) = query.map_err(|_| {
        api_error(fail(
            "invalid_input",
            "客户端操作来源不可读取，未查询其他数据库",
        ))
    })?;
    crate::blocking::run(move || {
        if !uuid4(&id) || !uuid4(&scope.store_id) {
            return Err(api_error(fail("invalid_input", "客户端操作编号或来源无效")));
        }
        let store = state
            .store
            .lock()
            .map_err(|_| api_error(registry::invalid()))?;
        if store.database_id().map_err(api_error)? != scope.store_id {
            return Err(api_error(fail(
                "operation_store_changed",
                "请连接原数据库核查客户端操作",
            )));
        }
        let clients = state
            .gateway
            .lan
            .clients
            .lock()
            .map_err(|_| api_error(registry::invalid()))?;
        clients.ensure_store(&scope.store_id).map_err(api_error)?;
        let outcome = clients
            .operations
            .iter()
            .find(|o| o.request.operation_id == id)
            .ok_or_else(|| {
                api_error(fail(
                    "operation_not_found",
                    "尚未查到原客户端操作；未自动重试或签发密钥",
                ))
            })?;
        crate::json_value(outcome)
    })
    .await
    .map_err(worker_error)?
}
pub(crate) async fn commit_operation(
    State(state): State<AppState>,
    input: Result<Json<Request>, JsonRejection>,
) -> ApiResult {
    let Json(request) = input.map_err(|_| {
        api_error(fail(
            "invalid_input",
            "客户端操作请求不可读取，未签发或撤销",
        ))
    })?;
    crate::blocking::run(move || {
        request.validate().map_err(api_error)?;
        let store = state
            .store
            .lock()
            .map_err(|_| api_error(registry::invalid()))?;
        let id = store.database_id().map_err(api_error)?;
        if request.store_id != id {
            return Err(api_error(fail(
                "operation_store_changed",
                "客户端草稿不属于当前数据库，未签发或撤销",
            )));
        }
        let lan = &state.gateway.lan;
        let mut clients = lan
            .clients
            .lock()
            .map_err(|_| api_error(registry::invalid()))?;
        clients.ensure_store(&id).map_err(api_error)?;
        // Receipt lookup precedes provider checks, key generation and all writes.
        if let Some(old) = clients
            .operations
            .iter()
            .find(|o| o.request.operation_id == request.operation_id)
        {
            if old.request != request {
                return Err(api_error(fail(
                    "operation_conflict",
                    "原编号的客户端请求不同，请保留原请求核查",
                )));
            }
            return crate::json_value(json!({"outcome":old,"key":null}));
        }
        let listen = lan
            .config
            .as_ref()
            .ok_or_else(|| api_error(fail("unavailable", "此服务未配置局域网入口")))?
            .listen
            .to_string();
        let mut next = clients.clone();
        let mut secret = None;
        let (action, client_id) = match &request.action {
            Action::Create { draft } => {
                if next.entries.len() >= 128 {
                    return Err(api_error(fail("capacity", "局域网客户端已达 128 个")));
                }
                if next
                    .entries
                    .iter()
                    .any(|e| e.client.id == request.operation_id)
                {
                    return Err(api_error(fail(
                        "operation_conflict",
                        "操作编号与既有客户端冲突",
                    )));
                }
                let provider = store
                    .providers()
                    .map_err(crate::error)?
                    .into_iter()
                    .find(|p| p.id == draft.provider_id)
                    .ok_or_else(|| api_error(fail("invalid_input", "找不到已保存的 Provider")))?;
                local_agent_core::providers::validate_source(&provider).map_err(api_error)?;
                if draft
                    .models
                    .iter()
                    .any(|m| !provider.enabled_model_ids().contains(m))
                {
                    return Err(api_error(fail(
                        "invalid_input",
                        "只能授权此 Provider 已启用的模型",
                    )));
                }
                let key = zeroize::Zeroizing::new(format!(
                    "la_{}{}",
                    uuid::Uuid::new_v4().simple(),
                    uuid::Uuid::new_v4().simple()
                ));
                next.entries.push(Entry {
                    capacity: Arc::new(Semaphore::new(draft.concurrency)),
                    client: super::Client {
                        id: request.operation_id.clone(),
                        name: draft.name.trim().into(),
                        agent: draft.agent.clone(),
                        provider_id: draft.provider_id.clone(),
                        models: draft.models.clone(),
                        concurrency: draft.concurrency,
                        enabled: true,
                        created_at: chrono::Utc::now().to_rfc3339(),
                        key_hash: files::hash(key.as_bytes()),
                    },
                });
                secret = Some(key);
                ("create", request.operation_id.clone())
            }
            Action::Revoke { client_id } => {
                // Each client has at most one persistent revocation receipt. Replays use its
                // original ID; a new ID cannot accumulate unbounded no-op history.
                if clients
                    .operations
                    .iter()
                    .any(|o| o.receipt.action == "revoke" && o.receipt.client_id == *client_id)
                {
                    return Err(api_error(fail(
                        "conflict",
                        "此客户端已有撤销回执，请核查原操作；未创建新操作",
                    )));
                }
                let entry = next
                    .entries
                    .iter_mut()
                    .find(|e| e.client.id == *client_id)
                    .ok_or_else(|| api_error(fail("not_found", "客户端不存在")))?;
                entry.client.enabled = false;
                ("revoke", client_id.clone())
            }
        };
        let outcome = Outcome {
            receipt: Receipt {
                api_version: 1,
                store_id: id.clone(),
                operation_id: request.operation_id.clone(),
                action: action.into(),
                client_id,
                listen,
                completed_at: chrono::Utc::now().to_rfc3339(),
            },
            request,
        };
        outcome.validate().map_err(api_error)?;
        next.store_id = Some(id);
        next.operations.push(outcome.clone());
        registry::commit(&lan.directory, &mut clients, next).map_err(api_error)?;
        crate::json_value(json!({"outcome":outcome,"key":secret.as_ref().map(|s| s.as_str())}))
    })
    .await
    .map_err(worker_error)?
}
