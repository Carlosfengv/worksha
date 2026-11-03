use super::operations::{self, Action, Outcome, Request};
use crate::backend::ConnectionContext;
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::PathBuf;
#[cfg(test)]
mod tests;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Input {
    pub request: Request,
    pub compensation_operation_id: String,
}
impl Input {
    pub fn parse(value: Value) -> Result<Self, String> {
        let input: Self = serde_json::from_value(value).map_err(|_| {
            operations::failure(
                "invalid_input",
                false,
                "原客户端请求不可读取，未打开签发流程",
            )
        })?;
        input.request.validate()?;
        if !matches!(input.request.action, Action::Create { .. })
            || !operations::uuid4(&input.compensation_operation_id)
            || input.compensation_operation_id == input.request.operation_id
        {
            return Err(operations::failure(
                "invalid_input",
                false,
                "原请求或补偿编号无效，未签发客户端",
            ));
        }
        Ok(input)
    }
    pub fn compensation(&self) -> Request {
        Request {
            store_id: self.request.store_id.clone(),
            operation_id: self.compensation_operation_id.clone(),
            action: Action::Revoke {
                client_id: self.request.operation_id.clone(),
            },
        }
    }
}
fn result(context: &ConnectionContext, status: &str, out: Outcome) -> Value {
    json!({"status":status,"outcome":out,"sourceInstanceId":context.instance_id(),"sourceCurrent":context.ensure_current().is_ok()})
}
fn not_issued(reason: &str) -> Value {
    json!({"status":"notIssued","reason":reason})
}
pub(super) async fn create_at(
    context: &ConnectionContext,
    input: Input,
    target: Option<PathBuf>,
) -> Result<Value, String> {
    let Some(target) = target else {
        return Ok(json!({"status":"cancelled"}));
    };
    let checked = target.clone();
    let checked_result = tokio::task::spawn_blocking(move || {
        local_agent_core::providers::files::checked_path(&checked)
    })
    .await;
    if !matches!(checked_result, Ok(Ok(()))) {
        return Ok(not_issued("保存位置未通过安全检查，本次未发送签发。"));
    }
    let status = match context
        .request(reqwest::Method::GET, "/api/gateway-lan", None)
        .await
    {
        Ok(value) => value,
        Err(_) => return Ok(not_issued("原后台状态尚未确认，本次未发送签发。")),
    };
    if status["writeProtocol"]["version"] != 1
        || status["writeProtocol"]["storeId"] != input.request.store_id
        || status["writeProtocol"]["secretReplay"] != false
        || status["listen"].as_str().is_none_or(|listen| {
            listen.parse::<std::net::SocketAddr>().is_err()
                || reqwest::Url::parse(&format!("http://{listen}/v1")).is_err()
        })
    {
        return Ok(not_issued(
            "原后台或客户端回执协议尚未确认，本次未发送签发。",
        ));
    }
    match operations::query(context, &input.request, false).await {
        Ok(out) => return Ok(result(context, "issued", out)),
        Err(error) if operations::not_found(&error) => (),
        Err(error) => return Err(error),
    }
    let reply = context
        .request(
            reqwest::Method::POST,
            "/api/gateway-lan/operations",
            Some(serde_json::to_value(&input.request).map_err(|_| operations::unknown())?),
        )
        .await;
    let (out, key) = match reply {
        Ok(value) => operations::committed(value, &input.request)?,
        Err(error) => {
            return match operations::query(context, &input.request, true).await {
                Ok(out) => Ok(result(context, "issued", out)),
                Err(query_error)
                    if operations::not_found(&query_error)
                        && operations::known_rejected(&error) =>
                {
                    Ok(not_issued(
                        "原后台明确拒绝本次签发，请核对来源、模型授权或容量后再试。",
                    ))
                }
                Err(_) => Err(error),
            };
        }
    };
    let Some(key) = key else {
        return Ok(result(context, "issued", out));
    };
    let Action::Create { draft } = &input.request.action else {
        return Err(operations::unknown());
    };
    let contents = json!({"clientId":out.receipt.client_id,"baseUrl":format!("http://{}/v1",out.receipt.listen),"apiKey":key,"agent":draft.agent,"models":draft.models});
    let bytes = serde_json::to_vec_pretty(&contents).map_err(|_| operations::unknown())?;
    let filename = target.file_name().map(|n| n.to_string_lossy().into_owned());
    let saved = tokio::task::spawn_blocking(move || {
        local_agent_core::providers::files::atomic_write(&target, &bytes, 0o600)
            .map_err(|e| e.message)
    })
    .await
    .map_err(|_| "保存连接文件任务中断".to_owned())
    .and_then(|r| r);
    match saved {
        Ok(()) => {
            let mut value = result(context, "saved", out);
            value["fileName"] = json!(filename);
            Ok(value)
        }
        Err(reason) => match operations::revoke(context, &input.compensation(), true).await {
            Ok(compensation) => {
                let proof = match &compensation {
                    operations::Revocation::Committed { .. } => "原客户端撤销回执已确认",
                    operations::Revocation::AlreadyRevoked { .. } => {
                        "原客户端当前已撤销，原补偿编号回执未确认"
                    }
                };
                let mut value = result(context, "revoked", out);
                value["compensation"] = json!(compensation);
                value["reason"] = json!(format!("{reason}；{proof}，请核对目标文件"));
                Ok(value)
            }
            Err(_) => Err(operations::failure(
                "gateway_client_revoke_unknown",
                true,
                "连接文件状态和原客户端撤销结果尚未确认，请保留原签发与补偿编号核查",
            )),
        },
    }
}
