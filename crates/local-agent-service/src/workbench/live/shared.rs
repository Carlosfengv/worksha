use super::{
    Connection, Server, Session, conflict, current, read_json, unavailable, v2, workbench,
};
use crate::{AppState, api};
use axum::{Json, extract::State, http::StatusCode};
use local_agent_core::sessions::sanitize_label;
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::BTreeMap, os::unix::fs::MetadataExt, path::PathBuf, sync::Arc};

use super::super::{MAX_TASKS, Workbench, identifier, store_error};
use super::super::{
    RuntimeBinding,
    operations::{Kind, Operation, Scope},
};

#[derive(Serialize, Deserialize)]
struct Binding {
    task_id: String,
    session: Session,
    route: String,
    text: String,
    dev: u64,
    ino: u64,
    #[serde(default)]
    prompt_attempted: bool,
    #[serde(default)]
    runtime_binding: Option<RuntimeBinding>,
}

async fn server(bench: &Workbench, host: &str) -> Result<(String, Arc<Server>), super::Error> {
    let mut guard = bench.live.0.lock().await;
    if guard
        .as_ref()
        .is_some_and(|connection| connection.server.confirm_record().is_err())
    {
        *guard = None;
    }
    if guard.is_none() {
        let (server, version) = v2::discover(&bench.source_home, host).await?;
        *guard = Some(Connection {
            id: uuid::Uuid::new_v4().to_string(),
            version,
            server: Arc::new(server),
            offered: BTreeMap::new(),
            selected: None,
        });
    }
    let connection = guard.as_ref().ok_or_else(unavailable)?;
    if connection.server.v2.is_none() {
        return Err(conflict("请先连接 OpenCode 默认本机服务，再创建共享任务"));
    }
    Ok((connection.id.clone(), connection.server.clone()))
}

fn model_id(model: &Value) -> Option<String> {
    let part = |key| {
        model[key].as_str().filter(|text| {
            !text.is_empty() && text.len() <= 200 && !text.chars().any(char::is_control)
        })
    };
    Some(format!("{}/{}", part("providerID")?, part("id")?))
}

pub(in super::super) async fn routes(State(state): State<AppState>) -> api::ApiResult {
    let bench = workbench(&state)?;
    let result = async {
        let (connection_id, server) = server(bench, &state.host).await?;
        let models = server.session_data("/model").await?;
        let providers = server.session_data("/provider").await?;
        let default = server.session_data("/model/default").await?;
        let default_id = model_id(&default);
        let models = models.as_array().ok_or_else(unavailable)?;
        let routes = models
            .iter()
            .filter(|model| model["enabled"] == true)
            .filter_map(|model| {
                let id = model_id(model)?;
                let name = sanitize_label(model["name"].as_str().unwrap_or(&id), 120);
                let provider_id = model["providerID"].as_str()?;
                let provider = providers
                    .as_array()?
                    .iter()
                    .find(|provider| provider["id"] == provider_id);
                let provider_name = sanitize_label(
                    provider
                        .and_then(|provider| provider["name"].as_str())
                        .unwrap_or(provider_id),
                    120,
                );
                Some(json!({
                    "id": id,
                    "label": format!("{provider_name} · {name}"),
                    "isDefault": Some(&id) == default_id.as_ref(),
                    "requestScope": provider.map(|provider| super::gateway::classify(provider, &state.gateway.host)),
                }))
            })
            .collect::<Vec<_>>();
        Ok::<_, super::Error>((routes, connection_id))
    }
    .await;
    match result {
        Ok((routes, connection_id)) => api::json_value(json!({
            "installed":true,"routes":routes,"error":null,"connectionId":connection_id,
            "operationScope":Scope::current(bench, &state)?
        })),
        Err(error) => {
            api::json_value(json!({"installed":true,"routes":null,"error":error.1.0["message"]}))
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in super::super) struct Create {
    task_id: String,
    directory: PathBuf,
    route: String,
    text: String,
    scope: Option<Scope>,
    connection_id: Option<String>,
}

fn save(
    bench: &Workbench,
    scope: &str,
    binding: &Binding,
    op: &Operation<'_>,
    accepted: bool,
) -> Result<(), super::Error> {
    let mut storage = bench.lock()?;
    let tx = storage
        .db
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(store_error)?;
    tx.execute(
        "INSERT INTO workbench_shared_sessions(id,scope,updated,value) VALUES(?1,?2,?3,?4)
         ON CONFLICT(id) DO UPDATE SET updated=excluded.updated,value=excluded.value",
        params![
            binding.task_id,
            scope,
            binding.session.updated,
            serde_json::to_string(binding).map_err(store_error)?
        ],
    )
    .map_err(store_error)?;
    let session = (binding.session.created != 0).then_some(binding.session.id.as_str());
    op.save(&tx, accepted, None, session)?;
    tx.commit().map_err(store_error)
}

fn owned(binding: &Binding, native: &Value) -> bool {
    native["metadata"]["localAgent"]["taskId"] == binding.task_id
        && Session::read(native).is_some_and(|session| {
            session.id == binding.session.id
                && session.directory == binding.session.directory
                && (binding.session.created == 0 || session.created == binding.session.created)
        })
}

pub(in super::super) async fn create(
    State(state): State<AppState>,
    Json(input): Json<Create>,
) -> api::ApiResult {
    identifier(&input.task_id)?;
    let text = input.text.trim();
    if text.is_empty() || text.len() > 24 * 1024 {
        return Err(conflict("请输入任务内容，单条最多 24 KB"));
    }
    let bench = workbench(&state)?;
    Scope::check(bench, &state, input.scope.as_ref())?;
    let op = Operation::new(
        Kind::SharedCreate,
        &input.task_id,
        &input.task_id,
        json!([input.directory, input.route, text]),
    );
    let directory = input
        .directory
        .canonicalize()
        .map_err(|_| conflict("项目目录不可访问"))?;
    if !directory.is_dir()
        || directory == std::path::Path::new("/")
        || dirs::home_dir().as_ref() == Some(&directory)
    {
        return Err(conflict("请选择一个具体的项目目录"));
    }
    let metadata = directory.metadata().map_err(store_error)?;
    let (connection_id, server) = server(bench, &state.host).await?;
    if input
        .connection_id
        .as_ref()
        .is_some_and(|expected| expected != &connection_id)
    {
        return Err(conflict("原 OpenCode 连接已变化，请核查原会话后重新读取"));
    }
    let scope = server.v2.as_ref().ok_or_else(unavailable)?.scope();
    let mut guard = bench.live.0.lock().await;
    let _ = current(&mut guard, &connection_id)?;
    let prior = op.prior(&bench.lock()?.db)?;
    let mut binding = {
        let storage = bench.lock()?;
        if storage.closing {
            return Err(conflict("应用正在退出"));
        }
        let saved: Option<(String, String)> = storage
            .db
            .query_row(
                "SELECT scope,value FROM workbench_shared_sessions WHERE id=?1",
                [&input.task_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(store_error)?;
        if let Some((prior_scope, value)) = saved {
            let binding: Binding = serde_json::from_str(&value).map_err(store_error)?;
            if prior_scope != scope
                || binding.session.directory != directory.to_string_lossy()
                || binding.route != input.route
                || binding.text != text
                || binding.dev != metadata.dev()
                || binding.ino != metadata.ino()
            {
                return Err(conflict("该任务编号已用于另一项任务或项目目录已变化"));
            }
            binding
        } else {
            let count: i64 = storage
                .db
                .query_row(
                    "SELECT count(*) FROM workbench_shared_sessions",
                    [],
                    |row| row.get(0),
                )
                .map_err(store_error)?;
            if count >= MAX_TASKS {
                return Err(conflict("共享任务已达到保存上限"));
            }
            Binding {
                task_id: input.task_id.clone(),
                session: Session {
                    id: format!(
                        "ses_{}",
                        uuid::Uuid::parse_str(&input.task_id)
                            .map_err(store_error)?
                            .simple()
                    ),
                    title: sanitize_label(text, 80),
                    directory: directory.to_string_lossy().into_owned(),
                    created: 0,
                    updated: chrono::Utc::now().timestamp_millis(),
                },
                route: input.route.clone(),
                text: text.into(),
                dev: metadata.dev(),
                ino: metadata.ino(),
                prompt_attempted: false,
                runtime_binding: None,
            }
        }
    };
    if binding.runtime_binding.is_none() {
        binding.runtime_binding = Some(RuntimeBinding {
            product_id: "opencode".into(),
            source_id: Some("opencode".into()),
            connection_id: format!(
                "opencode-shared:{}",
                local_agent_core::providers::files::hash(scope.as_bytes())
            ),
            implementation_version: 1,
        });
    }
    if prior.as_ref().is_some_and(|receipt| receipt.accepted) {
        current(&mut guard, &connection_id)?.remember(&[binding.session.clone()]);
        return api::json_value(json!({
            "sessionId":binding.session.id,"connectionId":connection_id,"delivered":true,
            "error":null,"runtimeBinding":binding.runtime_binding
        }));
    }
    server.confirm_record()?;
    let lookup = server
        .request(
            reqwest::Method::GET,
            &format!("/api/session/{}", binding.session.id),
        )
        .timeout(super::REQUEST_TIMEOUT)
        .send()
        .await
        .map_err(|_| unavailable())?;
    let native = if lookup.status() == StatusCode::NOT_FOUND {
        if binding.session.created != 0 {
            return Err(conflict("原共享会话已不存在，请确认后创建新任务"));
        }
        let models = server.session_data("/model").await?;
        if !models
            .as_array()
            .ok_or_else(unavailable)?
            .iter()
            .any(|model| model["enabled"] == true && model_id(model).as_ref() == Some(&input.route))
        {
            return Err(super::super::route_error(
                "所选 OpenCode 模型路由不可用，请检查原服务配置",
            ));
        }
        super::gateway::require_current(&state, &server, &input.route).await?;
        let (provider, model) = input
            .route
            .split_once('/')
            .ok_or_else(|| conflict("模型路由无效"))?;
        if prior.is_some() {
            return Err(super::super::operations::unknown());
        }
        save(bench, &scope, &binding, &op, false)?;
        let response = server
            .post(
                "/api/session",
                json!({
                    "id": binding.session.id,
                    "title": binding.session.title,
                    "location": {"directory": binding.session.directory},
                    "agent": "build",
                    "model": {"providerID": provider, "id": model},
                    "metadata": {"localAgent": {"taskId": binding.task_id}},
                    "permissions": [
                        {"action": "shell", "resource": "*", "effect": "ask"},
                        {"action": "edit", "resource": "*", "effect": "ask"},
                    ],
                }),
            )
            .await
            .map_err(|_| super::super::operations::unknown())?;
        read_json(response, 256 * 1024).await?["data"].take()
    } else {
        let response = Server::accepted(lookup)?;
        read_json(response, 256 * 1024).await?["data"].take()
    };
    if !owned(&binding, &native) || v2::route(&native).as_ref() != Some(&binding.route) {
        return Err(conflict("原共享会话的身份或模型已变化，未发送任务"));
    }
    binding.session = Session::read(&native).ok_or_else(unavailable)?;
    save(bench, &scope, &binding, &op, false)?;
    current(&mut guard, &connection_id)?.remember(&[binding.session.clone()]);
    let message_id = format!(
        "msg_{}",
        uuid::Uuid::parse_str(&input.task_id)
            .map_err(store_error)?
            .simple()
    );
    let history = server
        .session_data(&format!("/session/{}/message", binding.session.id))
        .await?;
    let admitted = history
        .as_array()
        .ok_or_else(unavailable)?
        .iter()
        .any(|message| {
            message["id"] == message_id
                && message["type"] == "user"
                && message["text"] == binding.text
        });
    let result = if admitted {
        Ok(())
    } else if binding.prompt_attempted {
        Err(super::super::operations::unknown())
    } else {
        super::gateway::require_current(&state, &server, &binding.route).await?;
        binding.prompt_attempted = true;
        save(bench, &scope, &binding, &op, false)?;
        server
            .post(
                &format!("/api/session/{}/prompt", binding.session.id),
                json!({"id":message_id,"text":binding.text}),
            )
            .await
            .map(|_| ())
            .map_err(|_| super::super::operations::unknown())
    };
    if result.is_ok() {
        save(bench, &scope, &binding, &op, true)
            .map_err(|_| super::super::operations::unknown())?;
    }
    api::json_value(
        json!({"sessionId":binding.session.id,"connectionId":connection_id,"delivered":result.is_ok(),"error":result.err().map(|error|error.1.0["message"].clone()),"runtimeBinding":binding.runtime_binding}),
    )
}

pub(super) async fn sessions(
    bench: &Workbench,
    server: &Server,
) -> Result<Vec<(Session, bool)>, super::Error> {
    let Some(identity) = &server.v2 else {
        return Ok(vec![]);
    };
    let bindings = {
        let storage = bench.lock()?;
        let mut query = storage.db.prepare("SELECT value FROM workbench_shared_sessions WHERE scope=?1 ORDER BY updated DESC LIMIT 32").map_err(store_error)?;
        query
            .query_map([identity.scope()], |row| row.get::<_, String>(0))
            .map_err(store_error)?
            .map(|row| {
                serde_json::from_str::<Binding>(&row.map_err(store_error)?).map_err(store_error)
            })
            .collect::<Result<Vec<_>, _>>()?
    };
    Ok(
        futures_util::future::join_all(bindings.into_iter().map(|binding| async move {
            match server
                .session_data(&format!("/session/{}", binding.session.id))
                .await
            {
                Ok(native) if owned(&binding, &native) => {
                    (Session::read(&native).unwrap_or(binding.session), true)
                }
                _ => (binding.session, false),
            }
        }))
        .await,
    )
}
