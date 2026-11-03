//! Scoped operation metadata only. Downstream credentials never cross this IPC boundary.
use crate::backend::ConnectionContext;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::net::SocketAddr;

#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Request {
    pub store_id: String,
    pub operation_id: String,
    pub action: Action,
}
#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub(super) enum Action {
    Create { draft: Draft },
    Revoke { client_id: String },
}
#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Draft {
    pub name: String,
    pub agent: String,
    pub provider_id: String,
    pub models: Vec<String>,
    pub concurrency: usize,
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
#[derive(Serialize)]
#[serde(tag = "status", rename_all = "camelCase")]
pub(super) enum Revocation {
    Committed { outcome: Outcome },
    AlreadyRevoked { request: Request },
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Commit {
    outcome: Outcome,
    key: Option<String>,
}
pub(super) fn failure(code: &str, unknown: bool, message: &str) -> String {
    json!({"code":code,"outcomeUnknown":unknown,"message":message}).to_string()
}
pub(super) fn unknown() -> String {
    failure(
        "gateway_client_result_unknown",
        true,
        "原客户端操作回复不可核对；请保留原编号核查，未重新签发密钥",
    )
}
pub(super) fn uuid4(id: &str) -> bool {
    uuid::Uuid::parse_str(id).is_ok_and(|u| u.get_version_num() == 4 && u.to_string() == id)
}
impl Request {
    pub fn validate(&self) -> Result<(), String> {
        let valid = uuid4(&self.store_id)
            && uuid4(&self.operation_id)
            && serde_json::to_vec(self).is_ok_and(|bytes| bytes.len() <= 65_536)
            && match &self.action {
                Action::Create { draft } => {
                    !draft.name.trim().is_empty()
                        && draft.name.chars().count() <= 80
                        && !draft.name.chars().any(char::is_control)
                        && ["claude", "codex", "opencode"].contains(&draft.agent.as_str())
                        && !draft.provider_id.is_empty()
                        && !draft.models.is_empty()
                        && draft.models.len() <= 200
                        && draft.models.iter().all(|m| !m.is_empty() && m.len() <= 256)
                        && (1..=16).contains(&draft.concurrency)
                }
                Action::Revoke { client_id } => {
                    uuid::Uuid::parse_str(client_id).is_ok_and(|id| id.to_string() == *client_id)
                }
            };
        if valid {
            Ok(())
        } else {
            Err(failure(
                "invalid_input",
                false,
                "客户端原请求或来源无效，未签发或撤销",
            ))
        }
    }
    pub fn path(&self) -> String {
        query_path(&self.operation_id, &self.store_id)
    }
}
fn query_path(operation: &str, store: &str) -> String {
    format!("/api/gateway-lan/operations/{operation}?storeId={store}")
}
pub(super) fn outcome(value: Value, expected: Option<&Request>) -> Result<Outcome, String> {
    let out: Outcome = serde_json::from_value(value).map_err(|_| unknown())?;
    out.request.validate().map_err(|_| unknown())?;
    let r = &out.receipt;
    let (action, id) = match &out.request.action {
        Action::Create { .. } => ("create", out.request.operation_id.as_str()),
        Action::Revoke { client_id } => ("revoke", client_id.as_str()),
    };
    let listen = r.listen.parse::<SocketAddr>().ok();
    if r.api_version != 1
        || r.store_id != out.request.store_id
        || r.operation_id != out.request.operation_id
        || r.action != action
        || r.client_id != id
        || listen.is_none_or(|a| a.port() == 0)
        || reqwest::Url::parse(&format!("http://{}/v1", r.listen)).is_err()
        || r.completed_at.is_empty()
        || r.completed_at.len() > 64
        || r.completed_at.chars().any(char::is_control)
        || expected.is_some_and(|request| *request != out.request)
    {
        return Err(unknown());
    }
    Ok(out)
}
pub(super) fn committed(
    value: Value,
    request: &Request,
) -> Result<(Outcome, Option<String>), String> {
    let reply: Commit = serde_json::from_value(value).map_err(|_| unknown())?;
    let out = outcome(
        serde_json::to_value(reply.outcome).map_err(|_| unknown())?,
        Some(request),
    )?;
    if let Some(key) = &reply.key
        && (!matches!(request.action, Action::Create { .. })
            || !key
                .strip_prefix("la_")
                .is_some_and(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())))
    {
        return Err(unknown());
    }
    Ok((out, reply.key))
}
pub(super) fn not_found(error: &str) -> bool {
    serde_json::from_str::<Value>(error)
        .is_ok_and(|v| v["code"] == "operation_not_found" && v["outcomeUnknown"] != true)
}
pub(super) fn known_rejected(error: &str) -> bool {
    serde_json::from_str::<Value>(error).is_ok_and(|v| {
        v["outcomeUnknown"] != true
            && matches!(
                v["code"].as_str(),
                Some(
                    "invalid_input"
                        | "operation_store_changed"
                        | "operation_conflict"
                        | "not_found"
                        | "conflict"
                        | "capacity"
                        | "busy"
                )
            )
    })
}
async fn already_disabled(
    context: &ConnectionContext,
    request: &Request,
    origin: bool,
) -> Result<bool, String> {
    let Action::Revoke { client_id } = &request.action else {
        return Err(unknown());
    };
    let value = if origin {
        context
            .origin_request(reqwest::Method::GET, "/api/gateway-lan", None)
            .await?
    } else {
        context
            .request(reqwest::Method::GET, "/api/gateway-lan", None)
            .await?
    };
    if value["writeProtocol"]["version"] != 1
        || value["writeProtocol"]["storeId"] != request.store_id
        || value["writeProtocol"]["secretReplay"] != false
    {
        return Err(failure(
            "operation_store_changed",
            false,
            "原客户端来源未确认，未向其他后台发送撤销",
        ));
    }
    let clients = value["clients"]
        .as_array()
        .filter(|v| v.len() <= 128)
        .ok_or_else(unknown)?;
    let matching: Vec<_> = clients.iter().filter(|c| c["id"] == *client_id).collect();
    if matching.is_empty() {
        return Err(failure(
            "not_found",
            false,
            "原后台尚未查到此客户端，请保留原请求核查",
        ));
    }
    if matching.len() != 1 {
        return Err(unknown());
    }
    matching[0]["enabled"]
        .as_bool()
        .map(|enabled| !enabled)
        .ok_or_else(unknown)
}
async fn reconcile_revocation(
    context: &ConnectionContext,
    request: &Request,
    error: String,
) -> Result<Revocation, String> {
    if let Ok(outcome) = query(context, request, true).await {
        return Ok(Revocation::Committed { outcome });
    }
    if already_disabled(context, request, true).await == Ok(true) {
        return Ok(Revocation::AlreadyRevoked {
            request: request.clone(),
        });
    }
    Err(error)
}
pub(super) async fn query(
    context: &ConnectionContext,
    request: &Request,
    origin: bool,
) -> Result<Outcome, String> {
    let value = if origin {
        context
            .origin_request(reqwest::Method::GET, &request.path(), None)
            .await?
    } else {
        context
            .request(reqwest::Method::GET, &request.path(), None)
            .await?
    };
    outcome(value, Some(request))
}
pub(super) async fn revoke(
    context: &ConnectionContext,
    request: &Request,
    origin: bool,
) -> Result<Revocation, String> {
    request.validate()?;
    if !matches!(request.action, Action::Revoke { .. }) {
        return Err(failure(
            "invalid_input",
            false,
            "此命令仅允许核对与撤销客户端，不返回签发密钥",
        ));
    }
    match query(context, request, origin).await {
        Ok(outcome) => return Ok(Revocation::Committed { outcome }),
        Err(e) if not_found(&e) => (),
        Err(e) => return Err(e),
    }
    if already_disabled(context, request, origin).await? {
        return Ok(Revocation::AlreadyRevoked {
            request: request.clone(),
        });
    }
    let body = Some(serde_json::to_value(request).map_err(|_| unknown())?);
    let reply = if origin {
        context
            .origin_request(reqwest::Method::POST, "/api/gateway-lan/operations", body)
            .await
    } else {
        context
            .request(reqwest::Method::POST, "/api/gateway-lan/operations", body)
            .await
    };
    match reply {
        Ok(value) => match committed(value, request) {
            Ok((outcome, _)) => Ok(Revocation::Committed { outcome }),
            Err(error) => reconcile_revocation(context, request, error).await,
        },
        Err(error) => reconcile_revocation(context, request, error).await,
    }
}
pub(super) async fn lookup(
    context: &ConnectionContext,
    operation_id: &str,
    store_id: &str,
) -> Result<Value, String> {
    if !uuid4(operation_id) || !uuid4(store_id) {
        return Err(failure(
            "invalid_input",
            false,
            "请保留原客户端操作编号与数据库身份",
        ));
    }
    let out = outcome(
        context
            .request(
                reqwest::Method::GET,
                &query_path(operation_id, store_id),
                None,
            )
            .await?,
        None,
    )?;
    if out.request.operation_id != operation_id || out.request.store_id != store_id {
        return Err(unknown());
    }
    serde_json::to_value(out).map_err(|_| unknown())
}
