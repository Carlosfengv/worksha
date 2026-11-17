use crate::backend::{Backend, ConnectionContext};
use local_agent_core::{
    providers::files,
    routing::chat::{Action, Operation, OperationQuery, OperationState, Receipt},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use tauri::Manager;
use tauri_plugin_dialog::DialogExt;

const ROOT: &str = "/api/agents/local-connections";

#[derive(Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum Input {
    List,
    Create {
        query: OperationQuery,
        name: String,
        service_id: String,
        service_version: String,
    },
    Rotate {
        query: OperationQuery,
        binding_id: String,
        expected_key_version: u32,
    },
    Revoke {
        query: OperationQuery,
        binding_id: String,
        expected_key_version: u32,
    },
    Resume {
        query: OperationQuery,
    },
    Cancel {
        query: OperationQuery,
    },
    BudgetBindings,
    BudgetWarnings,
    ReadBudget {
        binding_id: String,
    },
    SaveBudget {
        binding_id: String,
        store_id: String,
        policy: local_agent_core::routing::budget::Policy,
    },
    ReviewBudget {
        review: local_agent_core::routing::budget::Review,
    },
    RequestBudget {
        request_id: String,
    },
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ConnectionFile {
    base_url: String,
    binding_id: String,
    key_version: u32,
    api_key: String,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Reply {
    receipt: Receipt,
    base_url: String,
}

fn failure(error: local_agent_core::providers::ConfigError) -> String {
    serde_json::to_string(&error).unwrap_or(error.message)
}

async fn request(
    context: &ConnectionContext,
    endpoint: &str,
    input: Value,
) -> Result<Reply, String> {
    let value = context
        .request(
            reqwest::Method::POST,
            &format!("{ROOT}/{endpoint}"),
            Some(input),
        )
        .await?;
    serde_json::from_value(value).map_err(|_| "本机连接回执无效，请重新核查操作".into())
}

async fn operation(context: &ConnectionContext, query: &OperationQuery) -> Result<Reply, String> {
    request(context, "operation", json!(query)).await
}

async fn finish(
    context: &ConnectionContext,
    query: &OperationQuery,
    digest: Option<&str>,
) -> Result<Reply, String> {
    let result = request(
        context,
        "finish",
        json!({"query":query,"verifiedDigest":digest}),
    )
    .await;
    match result {
        Ok(reply) => Ok(reply),
        Err(error) => operation(context, query).await.or(Err(error)),
    }
}

fn validate_reply(reply: &Reply, query: &OperationQuery) -> Result<(), String> {
    let url = reqwest::Url::parse(&reply.base_url).map_err(|_| "本机网关连接地址无效")?;
    if reply.receipt.operation_id != query.operation_id
        || url.scheme() != "http"
        || url.host_str() != Some("127.0.0.1")
        || url.port().is_none()
        || url.path() != format!("/gateway/{}/v1", reply.receipt.binding_id)
        || url.query().is_some()
        || url.fragment().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
        || uuid::Uuid::parse_str(&reply.receipt.binding_id).is_err()
        || reply.receipt.key_version == 0
    {
        return Err("本机连接回执身份无效".into());
    }
    Ok(())
}

fn verify_file(path: &Path, reply: &Reply) -> Result<String, String> {
    let file = files::read(path).map_err(failure)?;
    let bytes = file
        .bytes
        .filter(|bytes| bytes.len() <= 4096)
        .ok_or("原连接文件不可读取")?;
    let contents: ConnectionFile =
        serde_json::from_slice(&bytes).map_err(|_| "原连接文件格式无效")?;
    if file.stamp.mode != 0o600
        || contents.base_url != reply.base_url
        || contents.binding_id != reply.receipt.binding_id
        || contents.key_version != reply.receipt.key_version
        || contents.api_key.is_empty()
        || contents.api_key.len() > 256
    {
        return Err("原连接文件身份或权限与本次操作不一致".into());
    }
    Ok(files::hash(contents.api_key.as_bytes()))
}

async fn choose_file(app: tauri::AppHandle, resume: bool) -> Result<Option<PathBuf>, String> {
    let window = app.get_webview_window("main").ok_or("应用窗口不可用")?;
    tokio::task::spawn_blocking(move || {
        let dialog = app
            .dialog()
            .file()
            .set_parent(&window)
            .add_filter("JSON 文件", &["json"]);
        if resume {
            dialog.set_title("选择原本机连接文件").blocking_pick_file()
        } else {
            dialog
                .set_title("保存新的本机连接文件（含 Key）")
                .set_file_name("local-chat-connection.json")
                .blocking_save_file()
        }
        .map(|file| {
            file.into_path()
                .map_err(|_| "请选择本机文件位置".to_owned())
        })
        .transpose()
    })
    .await
    .map_err(|_| "本机文件选择中断".to_owned())?
}

async fn prepare(
    context: &ConnectionContext,
    query: &OperationQuery,
    action: Action,
) -> Result<Reply, String> {
    let request_body = Operation {
        store_id: query.store_id.clone(),
        operation_id: query.operation_id.clone(),
        action,
    };
    let reply = request(context, "prepare", json!(request_body)).await?;
    validate_reply(&reply, query)?;
    Ok(reply)
}

async fn save(
    app: tauri::AppHandle,
    context: &ConnectionContext,
    query: OperationQuery,
    action: impl FnOnce(String) -> Action,
) -> Result<Value, String> {
    match operation(context, &query).await {
        Ok(reply) => {
            validate_reply(&reply, &query)?;
            return Ok(json!(reply));
        }
        Err(error)
            if serde_json::from_str::<Value>(&error).is_ok_and(|v| v["code"] == "not_found") => {}
        Err(error) => return Err(error),
    }
    let Some(path) = choose_file(app, false).await? else {
        return Ok(json!({"cancelled":true}));
    };
    files::checked_path(&path).map_err(failure)?;
    if std::fs::symlink_metadata(&path).is_ok() {
        return Err("请选择新的文件位置，已有连接文件不会被覆盖".into());
    }
    let key = format!(
        "la-chat-{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    let digest = files::hash(key.as_bytes());
    let reply = prepare(context, &query, action(digest.clone())).await?;
    if reply.receipt.state != OperationState::Pending {
        return Ok(json!(reply));
    }
    let contents = ConnectionFile {
        base_url: reply.base_url.clone(),
        binding_id: reply.receipt.binding_id.clone(),
        key_version: reply.receipt.key_version,
        api_key: key,
    };
    let expected = digest.clone();
    let saved = tokio::task::spawn_blocking(move || {
        let bytes = serde_json::to_vec_pretty(&contents).map_err(|_| "连接文件无法生成")?;
        files::atomic_write_new_private(&path, &bytes).map_err(failure)?;
        if verify_file(&path, &reply)? != expected {
            return Err("连接文件保存后内容已变化".into());
        }
        Ok::<_, String>(())
    })
    .await
    .map_err(|_| "连接文件保存状态待核查".to_owned());
    if let Err(error) = saved.and_then(|saved| saved) {
        let aborted = finish(context, &query, None).await?;
        return Ok(json!({"receipt":aborted.receipt,"baseUrl":aborted.base_url,"saveError":error}));
    }
    let result = finish(context, &query, Some(&digest)).await?;
    validate_reply(&result, &query)?;
    Ok(json!(result))
}

pub(crate) async fn run(
    app: tauri::AppHandle,
    backend: tauri::State<'_, Backend>,
    input: Value,
) -> Result<Value, String> {
    let input: Input = serde_json::from_value(input).map_err(|_| "本机连接操作参数无效")?;
    let context = backend.context()?;
    match input {
        Input::List => context.request(reqwest::Method::GET, ROOT, None).await,
        Input::BudgetBindings => {
            context
                .request(reqwest::Method::GET, "/api/agents/budgets", None)
                .await
        }
        Input::BudgetWarnings => {
            context
                .request(reqwest::Method::GET, "/api/agents/budget-warnings", None)
                .await
        }
        Input::ReadBudget { binding_id } => {
            uuid::Uuid::parse_str(&binding_id).map_err(|_| "本机绑定编号无效")?;
            context
                .request(
                    reqwest::Method::GET,
                    &format!("/api/agents/budgets/{binding_id}"),
                    None,
                )
                .await
        }
        Input::SaveBudget {
            binding_id,
            store_id,
            policy,
        } => {
            uuid::Uuid::parse_str(&binding_id).map_err(|_| "本机绑定编号无效")?;
            context
                .request(
                    reqwest::Method::PUT,
                    &format!("/api/agents/budgets/{binding_id}"),
                    Some(json!({"storeId":store_id,"policy":policy})),
                )
                .await
        }
        Input::ReviewBudget { review } => {
            context
                .request(
                    reqwest::Method::POST,
                    "/api/agents/budget-review",
                    Some(json!(review)),
                )
                .await
        }
        Input::RequestBudget { request_id } => {
            uuid::Uuid::parse_str(&request_id).map_err(|_| "网关请求编号无效")?;
            context
                .request(
                    reqwest::Method::GET,
                    &format!("/api/agents/budget-attempt/{request_id}"),
                    None,
                )
                .await
        }
        Input::Create {
            query,
            name,
            service_id,
            service_version,
        } => {
            save(app, &context, query, |key_digest| Action::Create {
                name,
                service_id,
                service_version,
                key_digest,
            })
            .await
        }
        Input::Rotate {
            query,
            binding_id,
            expected_key_version,
        } => {
            save(app, &context, query, |key_digest| Action::Rotate {
                binding_id,
                expected_key_version,
                key_digest,
            })
            .await
        }
        Input::Revoke {
            query,
            binding_id,
            expected_key_version,
        } => prepare(
            &context,
            &query,
            Action::Revoke {
                binding_id,
                expected_key_version,
            },
        )
        .await
        .map(|reply| json!(reply)),
        Input::Cancel { query } => finish(&context, &query, None)
            .await
            .map(|reply| json!(reply)),
        Input::Resume { query } => {
            let reply = operation(&context, &query).await?;
            validate_reply(&reply, &query)?;
            if reply.receipt.state != OperationState::Pending {
                return Ok(json!(reply));
            }
            let Some(path) = choose_file(app, true).await? else {
                return Ok(json!({"cancelled":true}));
            };
            let digest = tokio::task::spawn_blocking(move || verify_file(&path, &reply))
                .await
                .map_err(|_| "原连接文件核对任务未完成")??;
            let result = finish(&context, &query, Some(&digest)).await?;
            validate_reply(&result, &query)?;
            if result.receipt.state == OperationState::Pending {
                return Err("操作仍待激活。请核对原文件和 Key 版本，或取消此操作后重新签发".into());
            }
            Ok(json!(result))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn connection_file_preserves_previous_file_and_requires_exact_private_readback() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().canonicalize().unwrap().join("connection.json");
        let id = uuid::Uuid::new_v4().to_string();
        let reply = Reply {
            receipt: Receipt {
                operation_id: uuid::Uuid::new_v4().to_string(),
                binding_id: id.clone(),
                key_version: 2,
                state: OperationState::Pending,
            },
            base_url: format!("http://127.0.0.1:43123/gateway/{id}/v1"),
        };
        let file = ConnectionFile {
            base_url: reply.base_url.clone(),
            binding_id: id,
            key_version: 2,
            api_key: "owned-fixture".into(),
        };
        let bytes = serde_json::to_vec(&file).unwrap();
        files::atomic_write_new_private(&path, &bytes).unwrap();
        assert_eq!(
            verify_file(&path, &reply).unwrap(),
            files::hash(b"owned-fixture")
        );
        assert!(files::atomic_write_new_private(&path, b"replacement").is_err());
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(verify_file(&path, &reply).is_err());
    }
}
