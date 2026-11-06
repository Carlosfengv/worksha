use super::{Approval, Error, conflict, failure, opencode, workbench};
use crate::{AppState, api};
use axum::{Json, extract::State, http::StatusCode};
use local_agent_core::sessions::sanitize_label;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    sync::{Mutex as AsyncMutex, watch},
    time,
};
use zeroize::Zeroizing;

mod gateway;
mod shared;
mod v2;

pub(super) use shared::{create as create_shared, routes as shared_routes};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(4);
const MAX_TEXT: usize = 16 * 1024;
const BOARD_HISTORY: usize = 32;

#[derive(Default)]
pub(super) struct Live(AsyncMutex<Option<Connection>>);

struct Connection {
    id: String,
    version: String,
    server: Arc<Server>,
    offered: BTreeMap<String, Session>,
    selected: Option<Attachment>,
}

impl Connection {
    fn remember(&mut self, sessions: &[Session]) {
        self.offered
            .extend(sessions.iter().map(|s| (s.id.clone(), s.clone())));
        if self.offered.len() > BOARD_HISTORY {
            let mut recent: Vec<_> = self.offered.values().cloned().collect();
            recent.sort_by_key(|session| std::cmp::Reverse(session.updated));
            self.offered = recent
                .into_iter()
                .take(BOARD_HISTORY)
                .map(|session| (session.id.clone(), session))
                .collect();
        }
    }
}

struct Server {
    client: reqwest::Client,
    base: String,
    directory: Option<String>,
    user: String,
    password: Zeroizing<String>,
    v2: Option<v2::Identity>,
}

fn unavailable() -> Error {
    failure(
        StatusCode::SERVICE_UNAVAILABLE,
        "desktop_opencode_unavailable",
        "无法读取已连接的 OpenCode 服务，请检查本机端口、凭据和项目目录",
    )
}

fn display_version(version: Option<&str>) -> String {
    version
        .map(|value| sanitize_label(value, 40))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "版本未知".into())
}

async fn read_json(mut response: reqwest::Response, limit: usize) -> Result<Value, Error> {
    let mut data = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| unavailable())? {
        if chunk.len() > limit.saturating_sub(data.len()) {
            return Err(conflict("OpenCode 会话数据超出读取上限"));
        }
        data.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&data).map_err(|_| unavailable())
}

impl Server {
    fn client() -> Result<reqwest::Client, Error> {
        reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| unavailable())
    }

    fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        let request = self
            .client
            .request(method, format!("{}{path}", self.base))
            .basic_auth(&self.user, Some(self.password.as_str()));
        if let Some(directory) = &self.directory {
            request.query(&[("directory", directory)])
        } else {
            request
        }
    }

    async fn get(&self, path: &str) -> Result<Value, Error> {
        self.confirm_record()?;
        let response = self
            .request(reqwest::Method::GET, path)
            .timeout(REQUEST_TIMEOUT)
            .send()
            .await
            .map_err(|_| unavailable())?;
        if !response.status().is_success() {
            return Err(unavailable());
        }
        read_json(response, 8 * 1024 * 1024).await
    }

    async fn post_response(&self, path: &str, body: Value) -> Result<reqwest::Response, Error> {
        self.confirm_record()?;
        self.request(reqwest::Method::POST, path)
            .json(&body)
            .timeout(REQUEST_TIMEOUT)
            .send()
            .await
            .map_err(|_| {
                failure(
                    StatusCode::GATEWAY_TIMEOUT,
                    "desktop_opencode_outcome_unknown",
                    "操作结果待核对，请查看原会话状态。工作台不会自动重发。",
                )
            })
    }

    fn accepted(response: reqwest::Response) -> Result<reqwest::Response, Error> {
        if !response.status().is_success() {
            return Err(conflict("OpenCode 拒绝了此次操作，请刷新原会话状态"));
        }
        Ok(response)
    }

    async fn post(&self, path: &str, body: Value) -> Result<reqwest::Response, Error> {
        Self::accepted(self.post_response(path, body).await?)
    }

    fn confirm_record(&self) -> Result<(), Error> {
        if let Some(identity) = &self.v2 {
            identity.confirm_running()?;
        }
        Ok(())
    }

    fn path(&self, path: &str) -> String {
        if self.v2.is_some() {
            format!("/api{path}")
        } else {
            path.into()
        }
    }

    async fn session_data(&self, path: &str) -> Result<Value, Error> {
        let mut value = self.get(&self.path(path)).await?;
        if self.v2.is_some() {
            value
                .get_mut("data")
                .map(Value::take)
                .ok_or_else(unavailable)
        } else {
            Ok(value)
        }
    }

    async fn statuses(&self) -> Result<Value, Error> {
        self.session_data(if self.v2.is_some() {
            "/session/active"
        } else {
            "/session/status"
        })
        .await
    }

    async fn pending(&self, session: &str) -> Result<Value, Error> {
        if self.v2.is_some() {
            self.session_data(&format!("/session/{session}/permission"))
                .await
        } else {
            self.get("/permission").await
        }
    }

    fn phase(&self, status: Option<&Value>) -> &'static str {
        match status.map(|value| value["type"].as_str()) {
            Some(Some("running")) if self.v2.is_some() => "busy",
            Some(Some("busy")) => "busy",
            Some(Some("retry")) => "retry",
            Some(Some("idle")) | None => "idle",
            _ => "unknown",
        }
    }

    async fn active(&self) -> Result<Vec<Session>, Error> {
        let statuses = self.statuses().await?;
        self.active_from(&statuses).await
    }

    async fn active_from(&self, statuses: &Value) -> Result<Vec<Session>, Error> {
        let rows = statuses.as_object().ok_or_else(unavailable)?;
        let reads = rows
            .iter()
            .filter(|(id, status)| {
                opencode::native_id(id, "ses_")
                    && matches!(self.phase(Some(status)), "busy" | "retry")
            })
            .map(|(id, _)| id)
            .take(12)
            .map(|id| async move {
                let data = self.session_data(&format!("/session/{id}")).await?;
                Session::read(&data)
                    .filter(|session| &session.id == id)
                    .ok_or_else(unavailable)
            });
        futures_util::future::join_all(reads)
            .await
            .into_iter()
            .collect()
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Session {
    id: String,
    title: String,
    directory: String,
    created: i64,
    updated: i64,
}
impl Session {
    fn read(data: &Value) -> Option<Self> {
        let id = data["id"]
            .as_str()
            .filter(|id| opencode::native_id(id, "ses_"))?;
        let directory = data["directory"]
            .as_str()
            .or_else(|| data["location"]["directory"].as_str())
            .filter(|path| path.len() <= 4096 && Path::new(path).is_absolute())?;
        let created = data["time"]["created"].as_i64()?;
        Some(Self {
            id: id.into(),
            title: sanitize_label(data["title"].as_str().unwrap_or(id), 160),
            directory: directory.into(),
            created,
            updated: data["time"]["updated"].as_i64().unwrap_or(created),
        })
    }

    async fn confirm(&self, server: &Server) -> Result<Value, Error> {
        let data = server
            .session_data(&format!("/session/{}", self.id))
            .await?;
        if Self::read(&data).is_some_and(|value| {
            value.id == self.id
                && value.created == self.created
                && value.directory == self.directory
        }) {
            Ok(data)
        } else {
            Err(conflict("原 OpenCode 会话已变化，请重新连接"))
        }
    }
}

struct Attachment {
    session: Session,
    overlay: Arc<Mutex<Overlay>>,
    _stop: watch::Sender<()>,
}
#[derive(Default)]
struct Overlay {
    live: bool,
    problem: Option<&'static str>,
    order: Vec<String>,
    text: BTreeMap<String, String>,
}
impl Overlay {
    fn append(&mut self, id: &str, delta: &str) {
        if !self.text.contains_key(id) {
            if self.order.len() >= 24 {
                self.text.remove(&self.order.remove(0));
            }
            self.order.push(id.into());
        }
        let value = self.text.entry(id.into()).or_default();
        let cap = MAX_TEXT.saturating_sub(value.len());
        value.push_str(&delta[..delta.floor_char_boundary(delta.len().min(cap))]);
    }

    fn apply(&mut self, event: &Value, session_id: &str) {
        let props = &event["properties"];
        if event["type"] == "session.error" && props["sessionID"] == session_id {
            self.problem = problem(&props["error"]);
        } else if event["type"] == "message.updated"
            && props["info"]["sessionID"] == session_id
            && props["info"]["role"] == "user"
        {
            self.problem = None;
        }
        if event["type"] == "message.part.delta"
            && props["sessionID"] == session_id
            && props["field"] == "text"
            && let Some(id) = props["partID"]
                .as_str()
                .filter(|id| opencode::native_id(id, "prt_"))
            && let Some(delta) = props["delta"].as_str()
        {
            self.append(id, delta);
        } else if event["type"] == "message.part.updated"
            && props["part"]["sessionID"] == session_id
            && let Some(id) = props["part"]["id"].as_str()
            && let Some(value) = self.text.get_mut(id)
            && let Some(text) = props["part"]["text"].as_str()
        {
            *value = bounded_text(text);
        }
    }
}
fn bounded_text(text: &str) -> String {
    text[..text.floor_char_boundary(text.len().min(MAX_TEXT))].into()
}

async fn follow(
    server: Arc<Server>,
    id: String,
    overlay: Arc<Mutex<Overlay>>,
    mut stop: watch::Receiver<()>,
) {
    loop {
        let read = async {
            if server.confirm_record().is_err() {
                return;
            }
            let path = if server.v2.is_some() {
                "/api/event?types=session.text.delta&types=session.text.ended&types=session.execution.started&types=session.execution.succeeded&types=session.execution.failed&types=session.retry.scheduled"
            } else {
                "/event"
            };
            let Ok(Ok(response)) = time::timeout(
                REQUEST_TIMEOUT,
                server.request(reqwest::Method::GET, path).send(),
            )
            .await
            else {
                return;
            };
            if !response.status().is_success() {
                return;
            }
            let mut events = opencode::Events::from_response(response);
            while let Ok(Ok(event)) = time::timeout(Duration::from_secs(45), events.next()).await {
                if let Ok(mut cache) = overlay.lock() {
                    if event["type"] == "server.connected" {
                        cache.live = true;
                    }
                    if server.v2.is_some() {
                        v2::apply(&mut cache, &event, &id);
                    } else {
                        cache.apply(&event, &id);
                    }
                }
            }
        };
        tokio::select! {
            _ = stop.changed() => return,
            _ = read => {}
        }
        if let Ok(mut cache) = overlay.lock() {
            *cache = Overlay::default();
        }
        tokio::select! {
            _ = stop.changed() => return,
            _ = time::sleep(Duration::from_secs(1)) => {}
        }
    }
}

#[derive(Serialize)]
struct Part {
    id: String,
    role: &'static str,
    text: String,
}

fn messages(data: &Value, overlay: &Overlay) -> Vec<Part> {
    let mut parts = Vec::new();
    for message in data.as_array().into_iter().flatten() {
        let role = match message["info"]["role"].as_str() {
            Some("assistant") => "assistant",
            Some("user") => "user",
            _ => continue,
        };
        for part in message["parts"].as_array().into_iter().flatten() {
            let Some(id) = part["id"]
                .as_str()
                .filter(|id| opencode::native_id(id, "prt_"))
            else {
                continue;
            };
            if part["type"] == "text"
                && let Some(text) = part["text"].as_str()
            {
                let text = overlay
                    .text
                    .get(id)
                    .filter(|v| v.len() > text.len())
                    .map_or(text, String::as_str);
                parts.push(Part {
                    id: id.into(),
                    role,
                    text: bounded_text(text),
                });
            } else if part["type"] == "tool" {
                let status = match part["state"]["status"].as_str() {
                    Some("completed") => "已结束",
                    Some("error") => "未完成",
                    _ => "执行中",
                };
                let name = sanitize_label(part["tool"].as_str().unwrap_or("工具"), 80);
                parts.push(Part {
                    id: id.into(),
                    role: "tool",
                    text: format!("{name} · {status}"),
                });
            }
        }
    }
    parts
}

fn merge_stream(mut parts: Vec<Part>, overlay: &Overlay) -> Vec<Part> {
    for id in &overlay.order {
        if !parts.iter().any(|part| &part.id == id)
            && let Some(text) = overlay.text.get(id)
        {
            parts.push(Part {
                id: id.clone(),
                role: "assistant",
                text: text.clone(),
            });
        }
    }
    parts
        .into_iter()
        .rev()
        .take(48)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect()
}

fn approvals(data: &Value, session_id: &str, v2: bool) -> (Vec<Approval>, bool) {
    let mut supported = Vec::new();
    let mut other = false;
    for request in data
        .as_array()
        .into_iter()
        .flatten()
        .filter(|item| item["sessionID"] == session_id)
    {
        let approval = if v2 {
            v2::approval(request)
        } else {
            opencode::permission(request)
        };
        if let Some(approval) = approval
            && supported.len() < 12
        {
            supported.push(approval);
        } else {
            other = true;
        }
    }
    (supported, other)
}

fn problem(error: &Value) -> Option<&'static str> {
    match error["name"].as_str()? {
        "MessageAbortedError" => None,
        "ProviderAuthError" | "APIError" | "ModelNotFoundError" | "ProviderModelNotFoundError" => {
            Some("route")
        }
        _ => Some("execution"),
    }
}

fn model(data: &Value) -> Option<(&str, &str, &str, Option<&str>)> {
    let info = &data
        .as_array()?
        .iter()
        .rev()
        .find(|message| message["info"]["role"] == "user")?["info"];
    let valid =
        |text: &str| !text.is_empty() && text.len() <= 200 && !text.chars().any(char::is_control);
    Some((
        info["model"]["providerID"].as_str().filter(|s| valid(s))?,
        info["model"]["modelID"].as_str().filter(|s| valid(s))?,
        info["agent"].as_str().filter(|s| valid(s))?,
        info["model"]["variant"].as_str().filter(|s| valid(s)),
    ))
}

async fn selected(
    state: &AppState,
    server: &Server,
    session: &Session,
    overlay: &Mutex<Overlay>,
) -> Result<Value, Error> {
    let native = session.confirm(server).await?;
    let history_path = format!("/session/{}/message?limit=24", session.id);
    let (statuses, history, pending) = tokio::try_join!(
        server.statuses(),
        server.session_data(&history_path),
        server.pending(&session.id),
    )?;
    let statuses = statuses.as_object().ok_or_else(unavailable)?;
    if !history.is_array() || !pending.is_array() {
        return Err(unavailable());
    }
    let phase = server.phase(statuses.get(&session.id));
    let is_v2 = server.v2.is_some();
    let (approvals, other) = approvals(&pending, &session.id, is_v2);
    let request_scope = if is_v2 {
        if let Some(route) = v2::route(&native) {
            gateway::inspect(state, server, &route).await
        } else {
            gateway::Scope::Unknown
        }
    } else {
        gateway::Scope::Unknown
    };
    let cache = overlay.lock().map_err(|_| unavailable())?;
    let last_error = if is_v2 {
        v2::history_problem(&history, phase == "busy")
    } else {
        history
            .as_array()
            .into_iter()
            .flatten()
            .rev()
            .take_while(|message| message["info"]["role"] != "user")
            .find_map(|message| problem(&message["info"]["error"]))
    };
    let (parts, route) = if is_v2 {
        (v2::messages(&history, &cache), v2::route(&native))
    } else {
        (
            messages(&history, &cache),
            model(&history).map(|(provider, model, _, _)| format!("{provider}/{model}")),
        )
    };
    Ok(json!({
        "session":session, "phase":phase, "live":cache.live, "parts":merge_stream(parts,&cache),
        "approvals":approvals,"otherApproval":other,
        "model":route,
        "requestScope":request_scope,
        "problem":last_error.or(cache.problem).or((phase == "retry").then_some("route")),
    }))
}

pub(super) async fn read(State(state): State<AppState>) -> api::ApiResult {
    let bench = workbench(&state)?;
    let guard = bench.live.0.lock().await;
    let Some(connection) = guard.as_ref() else {
        return api::json_value(Value::Null);
    };
    let (id, version, server) = (
        connection.id.clone(),
        connection.version.clone(),
        connection.server.clone(),
    );
    let attachment = connection
        .selected
        .as_ref()
        .map(|a| (a.session.clone(), a.overlay.clone()));
    drop(guard);
    let (sessions, view) = if let Some((session, cache)) = attachment {
        (
            vec![],
            Some(selected(&state, &server, &session, &cache).await?),
        )
    } else {
        let sessions = server.active().await?;
        let mut guard = bench.live.0.lock().await;
        if let Some(connection) = guard.as_mut().filter(|c| c.id == id) {
            connection.remember(&sessions);
        }
        (sessions, None)
    };
    api::json_value(json!({"id":id,"version":version,"sessions":sessions,"selected":view}))
}

pub(super) async fn board(State(state): State<AppState>) -> api::ApiResult {
    let bench = workbench(&state)?;
    let mut guard = bench.live.0.lock().await;
    if guard.as_ref().is_some_and(|connection| {
        connection.server.v2.is_some() && connection.server.confirm_record().is_err()
    }) {
        *guard = None;
    }
    if guard.is_none() {
        let (server, version) = v2::discover(&bench.source_home, &state.host).await?;
        *guard = Some(Connection {
            id: uuid::Uuid::new_v4().to_string(),
            version,
            server: Arc::new(server),
            offered: BTreeMap::new(),
            selected: None,
        });
    }
    let connection = guard.as_ref().ok_or_else(unavailable)?;
    let (id, server) = (connection.id.clone(), connection.server.clone());
    drop(guard);

    let statuses = server.statuses().await?;
    let active = server.active_from(&statuses).await?;
    let saved = shared::sessions(bench, &server).await?;
    let mut waiting = futures_util::future::join_all(active.iter().map(|session| async {
        let result = server.pending(&session.id).await;
        let phase = match result {
            Ok(value)
                if value.as_array().is_some_and(|pending| {
                    pending.iter().any(|item| item["sessionID"] == session.id)
                }) =>
            {
                "approval"
            }
            Ok(value) if value.is_array() => "running",
            _ => "unknown",
        };
        (session.id.clone(), phase)
    }))
    .await
    .into_iter()
    .collect::<BTreeMap<_, _>>();
    for (session, confirmed) in &saved {
        if !confirmed {
            waiting.insert(session.id.clone(), "unknown");
        }
    }

    let mut guard = bench.live.0.lock().await;
    let connection = current(&mut guard, &id)?;
    connection.remember(&active);
    connection.remember(
        &saved
            .into_iter()
            .map(|(session, _)| session)
            .collect::<Vec<_>>(),
    );
    let mut sessions = connection.offered.values().collect::<Vec<_>>();
    sessions.sort_by_key(|session| std::cmp::Reverse(session.updated));
    let sessions: Vec<_> = sessions.into_iter().map(|session| {
        let phase = waiting.get(&session.id).copied().unwrap_or_else(|| {
            match server.phase(statuses.get(&session.id)) {
                "busy" | "retry" => "running",
                "idle" => "ended",
                _ => "unknown",
            }
        });
        json!({"id":session.id,"title":session.title,"directory":session.directory,"updated":session.updated,"phase":phase})
    }).collect();
    api::json_value(json!({"connectionId": id, "sessions":sessions}))
}

#[derive(Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub(super) enum Action {
    Discover,
    Connect {
        port: u16,
        user: String,
        password: String,
        directory: Option<String>,
    },
    Select {
        connection_id: String,
        session_id: String,
    },
    Disconnect {
        connection_id: String,
    },
    Stop {
        connection_id: String,
        session_id: String,
    },
    Decide {
        connection_id: String,
        session_id: String,
        approval_id: String,
        allow: bool,
    },
    Send {
        connection_id: String,
        session_id: String,
        text: String,
        message_id: Option<String>,
    },
}

pub(super) async fn act(
    State(state): State<AppState>,
    Json(action): Json<Action>,
) -> api::ApiResult {
    let bench = workbench(&state)?;
    let mut guard = bench.live.0.lock().await;
    match action {
        Action::Discover => {
            if guard.is_some() {
                return Err(conflict("请先断开当前连接"));
            }
            let (server, version) = v2::discover(&bench.source_home, &state.host).await?;
            connect(&mut guard, server, version).await
        }
        Action::Connect {
            port,
            user,
            password,
            directory,
        } => {
            let password = Zeroizing::new(password);
            if guard.is_some() {
                return Err(conflict("请先断开当前连接"));
            }
            if port == 0
                || port.to_string() == state.host.rsplit(':').next().unwrap_or("")
                || user.is_empty()
                || user.len() > 80
                || user.contains(':')
                || user.chars().any(char::is_control)
                || password.is_empty()
                || password.len() > 512
                || password.chars().any(char::is_control)
            {
                return Err(conflict("请输入 OpenCode 的本机端口、用户名和服务密码"));
            }
            let directory = directory.filter(|value| !value.is_empty());
            if directory.as_ref().is_some_and(|value| {
                value.len() > 4096
                    || !Path::new(value).is_absolute()
                    || value.chars().any(char::is_control)
            }) {
                return Err(conflict("请输入 OpenCode Desktop 项目的绝对目录"));
            }
            let client = Server::client()?;
            let base = format!("http://127.0.0.1:{port}");
            let probe = client
                .get(format!("{base}/session/status"))
                .timeout(REQUEST_TIMEOUT)
                .send()
                .await
                .map_err(|_| unavailable())?;
            if probe.status() != reqwest::StatusCode::UNAUTHORIZED {
                return Err(conflict("OpenCode 需要启用本机服务密码，工作台才可接管"));
            }
            let server = Server {
                client,
                base,
                directory,
                user,
                password,
                v2: None,
            };
            let health = server.get("/global/health").await?;
            if health["healthy"] != true {
                return Err(conflict("共享服务未提供工作台所需的 OpenCode 接口"));
            }
            let version = display_version(health["version"].as_str());
            connect(&mut guard, server, version).await
        }
        Action::Disconnect { connection_id } => {
            let _ = current(&mut guard, &connection_id)?;
            *guard = None;
            api::json_value(Value::Null)
        }
        Action::Select {
            connection_id,
            session_id,
        } => {
            let connection = current(&mut guard, &connection_id)?;
            let session = connection
                .offered
                .get(&session_id)
                .cloned()
                .ok_or_else(|| conflict("请从当前服务的活动会话中选择"))?;
            session.confirm(&connection.server).await?;
            let overlay = Arc::new(Mutex::new(Overlay::default()));
            let (stop, receiver) = watch::channel(());
            connection.selected = Some(Attachment {
                session,
                overlay: overlay.clone(),
                _stop: stop,
            });
            tokio::spawn(follow(
                connection.server.clone(),
                session_id,
                overlay,
                receiver,
            ));
            api::json_value(json!({"accepted":true}))
        }
        Action::Stop {
            connection_id,
            session_id,
        } => {
            let (server, session) = target(&mut guard, &connection_id, &session_id)?;
            session.confirm(server).await?;
            let is_v2 = server.v2.is_some();
            let stop = if is_v2 {
                "interrupt?resume=false"
            } else {
                "abort"
            };
            let result = server
                .post(
                    &server.path(&format!("/session/{session_id}/{stop}")),
                    json!({}),
                )
                .await?;
            if !result.json::<Value>().await.is_ok_and(|value| {
                if is_v2 {
                    value["interrupted"] == true
                } else {
                    value == true
                }
            }) {
                return Err(conflict("OpenCode 未确认停止，请核对原会话"));
            }
            api::json_value(json!({"accepted":true}))
        }
        Action::Decide {
            connection_id,
            session_id,
            approval_id,
            allow,
        } => {
            let (server, session) = target(&mut guard, &connection_id, &session_id)?;
            session.confirm(server).await?;
            let is_v2 = server.v2.is_some();
            let (pending, _) = approvals(&server.pending(&session_id).await?, &session_id, is_v2);
            if !pending.iter().any(|request| request.id == approval_id) {
                return Err(conflict("审批已处理、失效或不属于当前会话"));
            }
            let decision = if allow { "once" } else { "reject" };
            if is_v2 {
                v2::reply(server, &session_id, &approval_id, decision).await?;
            } else {
                server
                    .post(
                        &format!("/permission/{approval_id}/reply"),
                        json!({"reply":decision}),
                    )
                    .await?;
            }
            api::json_value(json!({"accepted":true}))
        }
        Action::Send {
            connection_id,
            session_id,
            text,
            message_id,
        } => {
            let text = text.trim();
            if text.is_empty() || text.len() > 24 * 1024 {
                return Err(conflict("请输入消息，单条最多 24 KB"));
            }
            let (server, session) = target(&mut guard, &connection_id, &session_id)?;
            let native = session.confirm(server).await?;
            let statuses = server.statuses().await?;
            let statuses = statuses.as_object().ok_or_else(unavailable)?;
            if server.phase(statuses.get(&session_id)) != "idle" {
                return Err(conflict("原会话仍在运行，请等待本轮结束或先停止"));
            }
            if server.v2.is_some() {
                if let Some(id) = &message_id {
                    super::identifier(id)?;
                }
                let route =
                    v2::route(&native).ok_or_else(|| conflict("无法确认原会话模型，未发送消息"))?;
                gateway::require_current(&state, server, &route).await?;
                server
                    .post(
                        &format!("/api/session/{session_id}/prompt"),
                        json!({"text":text,"id":message_id.map(|id|format!("msg_{}",id.replace('-',"")))}),
                    )
                    .await?;
                return api::json_value(json!({"accepted":true}));
            }
            let history = server
                .get(&format!("/session/{session_id}/message?limit=24"))
                .await?;
            let (provider, model, agent, variant) = model(&history)
                .ok_or_else(|| conflict("无法确认原会话模型与 Agent，未发送消息"))?;
            let mut prompt = json!({
                "agent":agent,"model":{"providerID":provider,"modelID":model},
                "parts":[{"type":"text","text":text}],
            });
            if let Some(variant) = variant {
                prompt["variant"] = json!(variant);
            }
            server
                .post(&format!("/session/{session_id}/prompt_async"), prompt)
                .await?;
            api::json_value(json!({"accepted":true}))
        }
    }
}

async fn connect(
    guard: &mut Option<Connection>,
    server: Server,
    version: String,
) -> api::ApiResult {
    let active = server.active().await?;
    let id = uuid::Uuid::new_v4().to_string();
    let offered = active.iter().map(|s| (s.id.clone(), s.clone())).collect();
    let response = json!({"id":id,"version":version,"sessions":active,"selected":null});
    *guard = Some(Connection {
        id,
        version,
        server: Arc::new(server),
        offered,
        selected: None,
    });
    api::json_value(response)
}

fn current<'a>(guard: &'a mut Option<Connection>, id: &str) -> Result<&'a mut Connection, Error> {
    guard
        .as_mut()
        .filter(|connection| connection.id == id)
        .ok_or_else(|| conflict("原连接已变化，请重新读取"))
}

fn target<'a>(
    guard: &'a mut Option<Connection>,
    connection_id: &str,
    session_id: &str,
) -> Result<(&'a Server, &'a Session), Error> {
    let connection = current(guard, connection_id)?;
    let attachment = connection
        .selected
        .as_ref()
        .filter(|a| a.session.id == session_id)
        .ok_or_else(|| conflict("原会话已经切换，请重新选择"))?;
    if !attachment.overlay.lock().map_err(|_| unavailable())?.live {
        return Err(conflict("实时连接未确认，请等待重连后再操作"));
    }
    Ok((&connection.server, &attachment.session))
}
