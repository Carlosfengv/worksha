use crate::backend::ConnectionContext;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{net::SocketAddr, path::PathBuf};
#[cfg(test)]
mod tests;

#[derive(Deserialize)]
struct Issued {
    id: String,
    key: String,
}
fn failure(code: &str, unknown: bool, message: &str) -> String {
    json!({"code":code,"outcomeUnknown":unknown,"message":message}).to_string()
}
fn issued(value: Value) -> Result<Issued, String> {
    let result = serde_json::from_value::<Issued>(value).ok().filter(|v| {
        uuid::Uuid::parse_str(&v.id).is_ok_and(|id| id.to_string() == v.id)
            && v.key
                .strip_prefix("la_")
                .is_some_and(|key| key.len() == 64 && key.bytes().all(|c| c.is_ascii_hexdigit()))
    });
    result.ok_or_else(|| {
        failure(
            "gateway_client_result_unknown",
            true,
            "客户端签发回复不可核对，请在原后台核查；未重新签发密钥",
        )
    })
}
pub(super) async fn create_at(
    context: &ConnectionContext,
    input: Value,
    target: Option<PathBuf>,
) -> Result<Value, String> {
    let Some(target) = target else {
        return Ok(json!({"status":"cancelled"}));
    };
    local_agent_core::providers::files::checked_path(&target).map_err(|e| e.message)?;
    let status = context
        .request(reqwest::Method::GET, "/api/gateway-lan", None)
        .await?;
    let listen = status["listen"]
        .as_str()
        .and_then(|s| s.parse::<SocketAddr>().ok())
        .filter(|a| a.port() != 0)
        .ok_or_else(|| {
            failure(
                "gateway_unavailable",
                false,
                "局域网监听地址未确认，未签发客户端",
            )
        })?;
    let issued = issued(
        context
            .request(
                reqwest::Method::POST,
                "/api/gateway-lan/clients",
                Some(input.clone()),
            )
            .await?,
    )?;
    let id = issued.id;
    let contents = json!({"clientId":id,"baseUrl":format!("http://{listen}/v1"),"apiKey":issued.key,"agent":input["agent"],"models":input["models"]});
    let bytes = serde_json::to_vec_pretty(&contents).map_err(|_| {
        failure(
            "gateway_client_result_unknown",
            true,
            "连接文件无法生成，请在原后台核查本次客户端",
        )
    })?;
    let filename = target.file_name().map(|n| n.to_string_lossy().into_owned());
    let saved = tokio::task::spawn_blocking(move || {
        local_agent_core::providers::files::atomic_write(&target, &bytes, 0o600)
            .map_err(|e| e.message)
    })
    .await
    .map_err(|_| "保存连接文件中断".to_owned())
    .and_then(|r| r);
    if let Err(reason) = saved {
        let revoked = context
            .origin_request(
                reqwest::Method::POST,
                &format!("/api/gateway-lan/clients/{id}/revoke"),
                None,
            )
            .await
            .is_ok_and(|v| v["revoked"].as_bool() == Some(true));
        return Err(if revoked {
            failure(
                "gateway_client_not_saved",
                false,
                &format!("{reason}；原后台本次新密钥已确认撤销，请核对目标文件"),
            )
        } else {
            failure(
                "gateway_client_revoke_unknown",
                true,
                &format!("{reason}；请在原后台核查并撤销客户端 {id}，尚未确认撤销结果"),
            )
        });
    }
    // Only non-secret file metadata crosses the WebView boundary.
    Ok(
        json!({"status":"saved","id":id,"fileName":filename,"sourceInstanceId":context.instance_id(),"sourceCurrent":context.ensure_current().is_ok()}),
    )
}
