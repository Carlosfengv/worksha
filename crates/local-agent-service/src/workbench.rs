mod claude;
mod claude_config;
mod config;
mod desktop;
mod gateway;
mod live;
mod opencode;

use crate::{AppState, api};
use axum::{
    Json, Router,
    extract::{Query, State},
    http::StatusCode,
    routing::{get, post},
};
use local_agent_core::providers::files;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard},
    time::Duration,
};
use tokio::sync::{Notify, mpsc, watch};

pub(crate) use claude::supervise as supervise_claude;
pub(crate) use opencode::supervise;

const MAX_TASKS: i64 = 200;
const MAX_TURNS: usize = 100;
const MAX_MESSAGE: usize = 24 * 1024;
const MAX_PARTS: usize = 240;

type Error = (StatusCode, Json<serde_json::Value>);
fn failure(status: StatusCode, code: &str, message: &str) -> Error {
    (status, Json(json!({"code":code,"message":message})))
}
fn store_error(_: impl std::fmt::Display) -> Error {
    failure(
        StatusCode::SERVICE_UNAVAILABLE,
        "workbench_storage",
        "任务记录暂不可用",
    )
}
fn conflict(message: &str) -> Error {
    failure(StatusCode::CONFLICT, "workbench_conflict", message)
}
fn identifier(id: &str) -> Result<(), Error> {
    if uuid::Uuid::parse_str(id).is_ok_and(|parsed| parsed.to_string() == id) {
        Ok(())
    } else {
        Err(failure(
            StatusCode::BAD_REQUEST,
            "invalid_id",
            "任务或操作编号无效",
        ))
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Phase {
    Ready,
    Starting,
    Running,
    Approval,
    Stopping,
    Completed,
    Failed,
    Stopped,
    Unknown,
}
impl Phase {
    fn active(self) -> bool {
        matches!(
            self,
            Self::Starting | Self::Running | Self::Approval | Self::Stopping
        )
    }
}

#[derive(Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Agent {
    #[default]
    Opencode,
    Claude,
}
impl Agent {
    fn binary(self) -> &'static str {
        match self {
            Self::Opencode => "opencode",
            Self::Claude => "claude",
        }
    }
    fn routes(self, home: &Path) -> Result<Vec<config::Route>, &'static str> {
        match self {
            Self::Opencode => config::routes(home),
            Self::Claude => claude_config::routes(home),
        }
    }
    fn configuration(self, home: &Path, route: &str) -> Result<ExecutionConfig, &'static str> {
        match self {
            Self::Opencode => config::runtime(home, route).map(ExecutionConfig::Opencode),
            Self::Claude => claude_config::runtime(home, route).map(ExecutionConfig::Claude),
        }
    }
}

enum ExecutionConfig {
    Opencode(serde_json::Value),
    Claude(claude_config::Config),
}

impl ExecutionConfig {
    fn require_current(&self, host: &str) -> Result<(), Error> {
        if let Self::Claude(config) = self {
            config.require_current(host).map_err(route_error)?;
        }
        Ok(())
    }
}

enum Outcome {
    Completed,
    Stopped,
    Failed(&'static str),
    Unknown(&'static str),
}
impl From<&'static str> for Outcome {
    fn from(reason: &'static str) -> Self {
        Self::Failed(reason)
    }
}

async fn cancelled(stop: &mut watch::Receiver<bool>) {
    let _ = stop.wait_for(|stop| *stop).await;
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Summary {
    id: String,
    #[serde(default)]
    agent: Agent,
    #[serde(default)]
    native_history: bool,
    #[serde(default)]
    native_busy: bool,
    #[serde(default)]
    parent_task_id: Option<String>,
    title: String,
    directory: PathBuf,
    route: String,
    phase: Phase,
    updated: i64,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Part {
    id: String,
    kind: String,
    text: String,
    #[serde(default)]
    truncated: bool,
}

#[derive(Clone, Serialize, Deserialize)]
struct Approval {
    id: String,
    tool: String,
    targets: Vec<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Task {
    #[serde(flatten)]
    summary: Summary,
    revision: u64,
    session_id: Option<String>,
    turn_id: Option<String>,
    parts: Vec<Part>,
    approval: Option<Approval>,
    error: Option<String>,
    #[serde(default)]
    native_cursor: u64,
    #[serde(default)]
    native_events: usize,
    #[serde(default)]
    native_fingerprint: String,
    #[serde(default)]
    native_stamp: String,
    #[serde(skip_serializing, default)]
    directory_dev: i64,
    #[serde(skip_serializing, default)]
    directory_ino: i64,
}
impl Task {
    fn part(&mut self, id: &str, kind: &str, text: &str, append: bool) {
        let index = self.parts.iter().position(|part| part.id == id);
        if index.is_none() && self.parts.len() >= MAX_PARTS {
            return;
        }
        let part = if let Some(index) = index {
            &mut self.parts[index]
        } else {
            self.parts.push(Part {
                id: id.into(),
                kind: kind.into(),
                text: String::new(),
                truncated: false,
            });
            self.parts.last_mut().unwrap()
        };
        if !append {
            part.text.clear();
            part.truncated = false;
        }
        let remaining = MAX_MESSAGE.saturating_sub(part.text.len());
        let end = text.floor_char_boundary(text.len().min(remaining));
        part.text.push_str(&text[..end]);
        part.truncated |= end < text.len();
    }
}

struct Active {
    task_id: String,
    turn_id: String,
    stop: watch::Sender<bool>,
    decisions: mpsc::Sender<(String, bool)>,
}
struct Storage {
    db: Connection,
    active: Option<Active>,
    closing: bool,
}
impl Storage {
    fn session_task(&self, session: &str) -> Result<Option<String>, Error> {
        let mut statement = self.db.prepare("SELECT id FROM workbench_tasks WHERE json_extract(value,'$.agent')='claude' AND json_extract(value,'$.sessionId')=?1 LIMIT 2").map_err(store_error)?;
        let ids = statement
            .query_map([session], |row| row.get::<_, String>(0))
            .map_err(store_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(store_error)?;
        Ok(if ids.len() == 1 {
            ids.into_iter().next()
        } else {
            None
        })
    }

    fn read(&self, id: &str) -> Result<Task, Error> {
        let (value, dev, ino): (String, i64, i64) = self
            .db
            .query_row(
                "SELECT value,dev,ino FROM workbench_tasks WHERE id=?1",
                [id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()
            .map_err(store_error)?
            .ok_or_else(|| failure(StatusCode::NOT_FOUND, "task_missing", "找不到该任务"))?;
        let mut task: Task = serde_json::from_str(&value).map_err(store_error)?;
        task.directory_dev = dev;
        task.directory_ino = ino;
        Ok(task)
    }
    fn save(&self, task: &mut Task) -> Result<(), Error> {
        task.revision += 1;
        task.summary.updated = chrono::Utc::now().timestamp_millis();
        self.db.execute(
            "INSERT INTO workbench_tasks(id,updated,phase,value,dev,ino) VALUES(?1,?2,?3,?4,?5,?6)
             ON CONFLICT(id) DO UPDATE SET updated=excluded.updated,phase=excluded.phase,value=excluded.value",
            params![
                task.summary.id, task.summary.updated, i64::from(task.summary.phase.active()),
                serde_json::to_string(task).map_err(store_error)?, task.directory_dev, task.directory_ino
            ],
        ).map_err(store_error)?;
        Ok(())
    }
    fn list(&self) -> Result<Vec<Summary>, Error> {
        let mut stmt = self
            .db
            .prepare(
                "SELECT value -> 'id',value -> 'title',value -> 'directory',value -> 'route',
                    value -> 'phase',updated,value -> 'agent',value -> 'parentTaskId',
                    coalesce(value -> 'nativeHistory','false'),coalesce(value -> 'nativeBusy','false') FROM workbench_tasks ORDER BY updated DESC,id",
            )
            .map_err(store_error)?;
        let mut rows = stmt.query([]).map_err(store_error)?;
        let mut list = Vec::new();
        while let Some(row) = rows.next().map_err(store_error)? {
            let decode = |index| -> Result<serde_json::Value, Error> {
                serde_json::from_str(&row.get::<_, String>(index).map_err(store_error)?)
                    .map_err(store_error)
            };
            let agent = row.get::<_, Option<String>>(6).map_err(store_error)?;
            let parent = row.get::<_, Option<String>>(7).map_err(store_error)?;
            list.push(serde_json::from_value(json!({
                "id":decode(0)?, "title":decode(1)?, "directory":decode(2)?,
                "route":decode(3)?, "phase":decode(4)?, "updated":row.get::<_,i64>(5).map_err(store_error)?,
                "agent":agent.as_deref().map(serde_json::from_str::<Agent>).transpose().map_err(store_error)?.unwrap_or_default(),
                "nativeHistory": decode(8)?, "nativeBusy": decode(9)?,
                "parentTaskId":parent.as_deref().map(serde_json::from_str::<Option<String>>).transpose().map_err(store_error)?.flatten()
            })).map_err(store_error)?);
        }
        Ok(list)
    }
}

pub(crate) struct Workbench {
    storage: Mutex<Storage>,
    root: PathBuf,
    source_home: PathBuf,
    drained: Notify,
    live: live::Live,
}

impl Workbench {
    pub(crate) fn has_active_task(&self) -> bool {
        self.lock().map_or(true, |storage| storage.active.is_some())
    }
    pub(crate) fn open(data: &Path, source_home: PathBuf) -> Result<Arc<Self>, String> {
        let root = data.join("workbench");
        files::private_dir(&root).map_err(|error| error.message)?;
        let db = Connection::open(root.join("tasks.sqlite3")).map_err(|error| error.to_string())?;
        db.execute_batch(
            "PRAGMA journal_mode=WAL;
             CREATE TABLE IF NOT EXISTS workbench_tasks (
                 id TEXT PRIMARY KEY,updated INTEGER NOT NULL,phase INTEGER NOT NULL,
                 value TEXT NOT NULL,dev INTEGER NOT NULL,ino INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS workbench_shared_sessions (
                 id TEXT PRIMARY KEY, scope TEXT NOT NULL, updated INTEGER NOT NULL,
                 value TEXT NOT NULL
             );",
        )
        .map_err(|error| error.to_string())?;
        let storage = Storage {
            db,
            active: None,
            closing: false,
        };
        let restore = || -> Result<(), Error> {
            let mut query = storage
                .db
                .prepare("SELECT id FROM workbench_tasks WHERE phase=1")
                .map_err(store_error)?;
            let interrupted = query
                .query_map([], |row| row.get::<_, String>(0))
                .map_err(store_error)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(store_error)?;
            drop(query);
            for id in interrupted {
                let mut task = storage.read(&id)?;
                task.summary.phase = Phase::Unknown;
                task.approval = None;
                task.error =
                    Some("上次执行中断，结果待确认。请先核对项目中的改动，再继续发送。".into());
                storage.save(&mut task)?;
            }
            Ok(())
        };
        restore().map_err(|_| "任务恢复失败".to_string())?;
        Ok(Arc::new(Self {
            storage: Mutex::new(storage),
            root,
            source_home,
            drained: Notify::new(),
            live: live::Live::default(),
        }))
    }

    fn lock(&self) -> Result<MutexGuard<'_, Storage>, Error> {
        self.storage.lock().map_err(store_error)
    }

    fn change(&self, id: &str, update: impl FnOnce(&mut Task)) -> Result<(), Error> {
        let storage = self.lock()?;
        let mut task = storage.read(id)?;
        update(&mut task);
        storage.save(&mut task)
    }

    fn finish(&self, task_id: &str, outcome: Outcome) {
        let (phase, error) = match outcome {
            Outcome::Completed => (Phase::Completed, None),
            Outcome::Stopped => (Phase::Stopped, None),
            Outcome::Failed(message) => (Phase::Failed, Some(message.to_owned())),
            Outcome::Unknown(message) => (Phase::Unknown, Some(message.to_owned())),
        };
        if let Ok(mut storage) = self.lock() {
            if let Ok(mut task) = storage.read(task_id) {
                claude::checkpoint(self, &mut task);
                task.summary.phase = phase;
                task.error = error;
                task.approval = None;
                let _ = storage.save(&mut task);
            }
            storage.active = None;
        }
        self.drained.notify_one();
    }

    pub(crate) fn stop(&self) {
        if let Ok(mut storage) = self.lock() {
            storage.closing = true;
            if let Some(active) = &storage.active {
                active.stop.send_replace(true);
            }
        }
    }

    pub(crate) async fn drain(&self, budget: Duration) {
        let _ = tokio::time::timeout(budget, async {
            loop {
                let notified = self.drained.notified();
                if self.lock().map_or(true, |storage| storage.active.is_none()) {
                    return;
                }
                notified.await;
            }
        })
        .await;
    }
}

pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/workbench/routes", get(read_routes))
        .route("/api/workbench/tasks", get(read).post(create))
        .route("/api/workbench/session", get(find_session))
        .route("/api/workbench/send", post(send))
        .route("/api/workbench/control", post(control))
        .route("/api/workbench/live", get(live::read).post(live::act))
        .route("/api/workbench/live-board", get(live::board))
        .route("/api/workbench/shared-routes", get(live::shared_routes))
        .route("/api/workbench/shared", post(live::create_shared))
        .route(
            "/api/workbench/export-opencode",
            post(desktop::opencode_export),
        )
}

fn workbench(state: &AppState) -> Result<&Arc<Workbench>, Error> {
    state.workbench.as_ref().ok_or_else(|| {
        failure(
            StatusCode::SERVICE_UNAVAILABLE,
            "workbench_unavailable",
            "当前本地服务无法运行任务工作台",
        )
    })
}
#[derive(Deserialize)]
struct RoutesQuery {
    #[serde(default)]
    agent: Agent,
}
async fn read_routes(
    State(state): State<AppState>,
    Query(query): Query<RoutesQuery>,
) -> api::ApiResult {
    let bench = workbench(&state)?;
    let routes = query.agent.routes(&bench.source_home);
    let mut items = serde_json::to_value(routes.as_ref().ok()).map_err(store_error)?;
    if query.agent == Agent::Claude
        && let Some(items) = items.as_array_mut()
    {
        let scope = claude_config::scope(&bench.source_home, &state.gateway.host);
        for item in items {
            item["requestScope"] = json!(scope);
        }
    }
    api::json_value(json!({
        "installed":crate::providers::binary(query.agent.binary()).is_some(),
        "routes":items,
        "error":routes.err()
    }))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Read {
    task_id: Option<String>,
    revision: Option<u64>,
}
async fn read(State(state): State<AppState>, Query(query): Query<Read>) -> api::ApiResult {
    let bench = workbench(&state)?;
    let storage = bench.lock()?;
    for summary in storage
        .list()?
        .into_iter()
        .filter(|task| task.native_history && !task.phase.active())
        .filter(|task| query.task_id.as_ref().is_none_or(|id| id == &task.id))
    {
        let mut task = storage.read(&summary.id)?;
        if claude::refresh_native(bench, &mut task)? {
            storage.save(&mut task)?;
        }
    }
    let task = query
        .task_id
        .map(|id| storage.read(&id))
        .transpose()?
        .filter(|task| Some(task.revision) != query.revision);
    api::json_value(json!({
        "tasks":storage.list()?, "task":task.as_ref().map(|task| desktop::task_value(bench, task)).transpose()?, "activeTaskId":storage.active.as_ref().map(|run| &run.task_id),
        "claudeConnection":desktop::connection(bench, &state.gateway.host),
    }))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SessionQuery {
    session_id: String,
}

async fn find_session(
    State(state): State<AppState>,
    Query(query): Query<SessionQuery>,
) -> api::ApiResult {
    let session = query
        .session_id
        .strip_prefix("claude:")
        .ok_or_else(|| conflict("会话编号无效"))?;
    identifier(session)?;
    let storage = workbench(&state)?.lock()?;
    api::json_value(json!({"taskId":storage.session_task(session)?}))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Create {
    task_id: String,
    #[serde(default)]
    agent: Agent,
    parent_task_id: Option<String>,
    directory: PathBuf,
    route: String,
}
async fn create(State(state): State<AppState>, Json(input): Json<Create>) -> api::ApiResult {
    identifier(&input.task_id)?;
    let bench = workbench(&state)?;
    let directory = input
        .directory
        .canonicalize()
        .map_err(|_| conflict("项目目录已不可访问"))?;
    if directory == Path::new("/")
        || dirs::home_dir().as_ref() == Some(&directory)
        || !directory.is_dir()
    {
        return Err(conflict("请选择一个具体的项目目录"));
    }
    let metadata = directory.metadata().map_err(store_error)?;
    input
        .agent
        .configuration(&bench.source_home, &input.route)
        .map_err(route_error)?
        .require_current(&state.gateway.host)?;
    let storage = bench.lock()?;
    if storage.closing {
        return Err(conflict("应用正在退出"));
    }
    match storage.read(&input.task_id) {
        Ok(task)
            if task.summary.directory == directory
                && task.summary.route == input.route
                && task.summary.agent == input.agent
                && task.summary.parent_task_id == input.parent_task_id =>
        {
            return api::json_value(task);
        }
        Ok(_) => return Err(conflict("任务编号已经用于另一项任务")),
        Err(error) if error.0 == StatusCode::NOT_FOUND => {}
        Err(error) => return Err(error),
    }
    if let Some(parent_id) = &input.parent_task_id {
        identifier(parent_id)?;
        let parent = storage.read(parent_id)?;
        if parent.summary.directory != directory
            || parent.summary.agent == input.agent
            || parent.directory_dev != metadata.dev() as i64
            || parent.directory_ino != metadata.ino() as i64
            || parent.summary.phase.active()
            || parent.summary.phase == Phase::Unknown
        {
            return Err(conflict("请先确认原任务已结束，并在同一个项目目录中交接"));
        }
    }
    let count: i64 = storage
        .db
        .query_row("SELECT count(*) FROM workbench_tasks", [], |row| row.get(0))
        .map_err(store_error)?;
    if count >= MAX_TASKS {
        return Err(conflict("工作台已达到任务保存上限"));
    }
    let mut task = Task {
        summary: Summary {
            id: input.task_id,
            agent: input.agent,
            native_history: input.agent == Agent::Claude,
            native_busy: false,
            parent_task_id: input.parent_task_id,
            title: "新任务".into(),
            directory,
            route: input.route,
            phase: Phase::Ready,
            updated: 0,
        },
        revision: 0,
        session_id: None,
        turn_id: None,
        parts: vec![],
        approval: None,
        error: None,
        native_cursor: 0,
        native_events: 0,
        native_fingerprint: String::new(),
        native_stamp: String::new(),
        directory_dev: metadata.dev() as i64,
        directory_ino: metadata.ino() as i64,
    };
    storage.save(&mut task)?;
    api::json_value(task)
}

fn route_error(message: &str) -> Error {
    failure(
        StatusCode::SERVICE_UNAVAILABLE,
        "model_route_unavailable",
        message,
    )
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Send {
    task_id: String,
    turn_id: String,
    text: String,
}
async fn send(State(state): State<AppState>, Json(input): Json<Send>) -> api::ApiResult {
    identifier(&input.task_id)?;
    identifier(&input.turn_id)?;
    let text = input.text.trim();
    if text.is_empty() || text.len() > MAX_MESSAGE {
        return Err(conflict("请输入任务内容，单条消息最多 24 KB"));
    }
    let bench = workbench(&state)?.clone();
    let mut storage = bench.lock()?;
    let mut task = storage.read(&input.task_id)?;
    if let Some(prior) = task.parts.iter().find(|part| part.id == input.turn_id) {
        if prior.text != text {
            return Err(conflict("该发送编号已用于另一条消息"));
        }
        return api::json_value(task);
    }
    if task.summary.native_history && claude::native_busy(&bench, &task)? {
        return Err(conflict(
            "原生 Claude Code 正在使用这条会话，请先在原生客户端退出这条会话",
        ));
    }
    if storage.closing || storage.active.is_some() {
        return Err(conflict("请等待正在执行的任务结束"));
    }
    if task.parts.iter().filter(|part| part.kind == "user").count() >= MAX_TURNS
        || task.parts.len() >= MAX_PARTS.saturating_sub(4)
    {
        return Err(conflict("该任务记录已达上限，请创建新任务"));
    }
    let directory = &task.summary.directory;
    if !directory
        .canonicalize()
        .is_ok_and(|path| path == *directory)
        || !directory.metadata().is_ok_and(|value| {
            value.dev() as i64 == task.directory_dev && value.ino() as i64 == task.directory_ino
        })
    {
        return Err(conflict("项目目录已移动或被替换，请重新选择目录并创建任务"));
    }
    let agent = task.summary.agent;
    let binary = crate::providers::binary(agent.binary())
        .ok_or_else(|| conflict("未找到任务使用的 Agent CLI，请先安装"))?;
    let config = agent
        .configuration(&bench.source_home, &task.summary.route)
        .map_err(route_error)?;
    config.require_current(&state.gateway.host)?;
    if task.parts.is_empty() {
        task.summary.title =
            local_agent_core::sessions::sanitize_label(text.lines().next().unwrap_or(text), 72);
    }
    task.part(&input.turn_id, "user", text, false);
    task.turn_id = Some(input.turn_id.clone());
    task.summary.phase = Phase::Starting;
    task.error = None;
    task.approval = None;
    storage.save(&mut task)?;
    let (stop, stopping) = watch::channel(false);
    let (decisions, replies) = mpsc::channel(1);
    storage.active = Some(Active {
        task_id: input.task_id,
        turn_id: input.turn_id,
        stop,
        decisions,
    });
    drop(storage);
    let response = api::json_value(&task);
    let vault = state.credentials.clone();
    let text = text.to_owned();
    tokio::spawn(async move {
        let id = task.summary.id.clone();
        let outcome = match config {
            ExecutionConfig::Opencode(config) => {
                opencode::run(&bench, task, binary, config, text, stopping, replies).await
            }
            ExecutionConfig::Claude(config) => {
                claude::run(
                    &bench,
                    task,
                    binary,
                    (config, vault),
                    text,
                    stopping,
                    replies,
                )
                .await
            }
        };
        bench.finish(&id, outcome);
    });
    response
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Control {
    task_id: String,
    turn_id: String,
    #[serde(flatten)]
    action: Action,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Action {
    Stop,
    Decide {
        #[serde(rename = "approvalId")]
        approval_id: String,
        allow: bool,
    },
}

async fn control(State(state): State<AppState>, Json(input): Json<Control>) -> api::ApiResult {
    let storage = workbench(&state)?.lock()?;
    let active = storage
        .active
        .as_ref()
        .filter(|active| active.task_id == input.task_id && active.turn_id == input.turn_id)
        .ok_or_else(|| conflict("该轮任务已经结束或发生变化"))?;
    let mut task = storage.read(&input.task_id)?;
    match input.action {
        Action::Stop => {
            task.approval = None;
            task.summary.phase = Phase::Stopping;
            storage.save(&mut task)?;
            active.stop.send_replace(true);
        }
        Action::Decide { approval_id, allow } => {
            if task
                .approval
                .as_ref()
                .is_none_or(|request| request.id != approval_id)
            {
                return Err(conflict("该审批已处理或失效"));
            }
            task.approval = None;
            task.summary.phase = Phase::Running;
            task.part(
                &format!("decision-{approval_id}"),
                "tool",
                if allow {
                    "已允许本次操作"
                } else {
                    "已拒绝本次操作"
                },
                false,
            );
            storage.save(&mut task)?;
            active
                .decisions
                .try_send((approval_id, allow))
                .map_err(|_| conflict("审批连接已结束，请核对任务状态"))?;
        }
    }
    api::json_value(task)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_session_resolution_rejects_unknown_wrong_agent_and_ambiguous_tasks() {
        let fixture = tempfile::tempdir().unwrap();
        let directory = fixture.path().canonicalize().unwrap();
        let bench = Workbench::open(&directory, directory.clone()).unwrap();
        let storage = bench.lock().unwrap();
        let session = uuid::Uuid::new_v4().to_string();
        assert!(storage.session_task(&session).unwrap().is_none());
        for (id, agent) in [
            ("wrong-agent", "opencode"),
            ("one", "claude"),
            ("two", "claude"),
        ] {
            storage
                .db
                .execute(
                    "INSERT INTO workbench_tasks VALUES(?1,0,0,?2,1,1)",
                    params![id, json!({"agent":agent,"sessionId":session}).to_string()],
                )
                .unwrap();
            assert_eq!(
                storage.session_task(&session).unwrap().as_deref(),
                if id == "one" { Some("one") } else { None }
            );
        }
    }

    #[test]
    fn reopen_preserves_route_and_session_without_replaying_or_restoring_approval() {
        let fixture = tempfile::tempdir().unwrap();
        let directory = fixture.path().canonicalize().unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        {
            let bench = Workbench::open(&directory, directory.clone()).unwrap();
            let mut task = Task {
                summary: Summary {
                    id: id.clone(),
                    agent: Agent::Opencode,
                    native_history: false,
                    native_busy: false,
                    parent_task_id: None,
                    title: "测试任务".into(),
                    directory: directory.clone(),
                    route: "fixture/selected".into(),
                    phase: Phase::Approval,
                    updated: 0,
                },
                revision: 0,
                session_id: Some("ses_fixture".into()),
                turn_id: Some("turn_fixture".into()),
                parts: vec![],
                approval: Some(Approval {
                    id: "per_fixture".into(),
                    tool: "bash".into(),
                    targets: vec!["./fixture".into()],
                }),
                error: None,
                native_cursor: 0,
                native_events: 0,
                native_fingerprint: String::new(),
                native_stamp: String::new(),
                directory_dev: 1,
                directory_ino: 2,
            };
            task.part("turn_fixture", "user", "原消息", false);
            bench.lock().unwrap().save(&mut task).unwrap();
        }
        let bench = Workbench::open(&directory, directory.clone()).unwrap();
        let storage = bench.lock().unwrap();
        let task = storage.read(&id).unwrap();
        assert!(task.summary.phase == Phase::Unknown);
        assert_eq!(task.summary.route, "fixture/selected");
        assert_eq!(task.session_id.as_deref(), Some("ses_fixture"));
        assert_eq!(task.parts[0].text, "原消息");
        assert!(task.approval.is_none() && storage.active.is_none());
    }
}
