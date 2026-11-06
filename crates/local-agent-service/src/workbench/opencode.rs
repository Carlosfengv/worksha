use super::{Approval, Outcome, Phase, Task, Workbench, cancelled};
use local_agent_core::providers::files;
use serde_json::{Value, json};
use std::{
    collections::HashSet,
    io::Read,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, BufReader},
    process::{Child, Command},
    sync::{mpsc, oneshot, watch},
    time,
};

const START_TIMEOUT: Duration = Duration::from_secs(40);
const CONTROL_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_FRAME: usize = 512 * 1024;
const MAX_TURN: Duration = Duration::from_secs(20 * 60);
const ROUTE_FAILED: &str =
    "所选模型路由不可用。请检查 OpenCode 模型连接和凭据后重试，当前路由已保留。";
const EXECUTION_UNKNOWN: &str = "OpenCode 执行连接已中断，结果待确认。请先核对项目中的改动。";
const SESSION_MISSING: &str = "原 OpenCode 会话不可用。该任务的历史仍可查看，请新建任务继续。";
const STORAGE_FAILED: &str = "任务记录写入中断，已停止本轮，请核对项目中的改动。";

/// The launcher owns a pipe to this supervisor. Even an ungraceful launcher
/// exit closes the pipe, which stops this specific OpenCode process group.
pub(crate) fn supervise() -> Result<(), Box<dyn std::error::Error>> {
    let binary = std::env::args_os()
        .nth(2)
        .ok_or("missing OpenCode executable")?;
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(async {
            let mut child = Command::new(binary)
                .args(["serve", "--hostname", "127.0.0.1", "--port", "0", "--pure"])
                .stdin(Stdio::null())
                .stderr(Stdio::null())
                .process_group(0)
                .kill_on_drop(true)
                .spawn()?;
            let group = child.id().ok_or("missing OpenCode process ID")? as i32;
            let (gone, parent) = oneshot::channel();
            std::thread::spawn(move || {
                let _ = std::io::stdin().read(&mut [0; 1]);
                let _ = gone.send(());
            });
            let mut termination =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
            tokio::select! {
                _ = child.wait() => {},
                _ = parent => {},
                _ = termination.recv() => {},
            }
            // The group is created by this process and contains only this runtime.
            unsafe { libc::kill(-group, libc::SIGKILL) };
            let _ = child.wait().await;
            Ok(())
        })
}

struct Server {
    child: Child,
    client: reqwest::Client,
    base: String,
    password: String,
    policy: Value,
}
impl Server {
    fn start(root: &Path, binary: &Path, task: &Task, config: Value) -> Result<Self, &'static str> {
        let password = format!("{}{}", uuid::Uuid::new_v4(), uuid::Uuid::new_v4());
        let launcher = std::env::current_exe().map_err(|_| "无法启动 OpenCode 运行进程")?;
        let mut command = Command::new(launcher);
        command
            .arg("--workbench-opencode")
            .arg(binary)
            .current_dir(&task.summary.directory)
            .env_clear()
            .env(
                "PATH",
                "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin",
            )
            .env("OPENCODE_CONFIG_CONTENT", config.to_string())
            .env("OPENCODE_SERVER_PASSWORD", &password)
            .env("OPENCODE_DISABLE_PROJECT_CONFIG", "1")
            .env("OPENCODE_DISABLE_EXTERNAL_SKILLS", "1")
            .env("OPENCODE_DISABLE_CLAUDE_CODE", "1")
            .env("OPENCODE_DISABLE_EMBEDDED_WEB_UI", "1")
            .env(
                "OPENCODE_DB",
                root.join(format!("{}.opencode.db", task.summary.id)),
            )
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        if let Some(home) = dirs::home_dir() {
            command.env("HOME", home);
        }
        for (name, dir) in [
            ("XDG_CONFIG_HOME", "config"),
            ("XDG_DATA_HOME", "data"),
            ("XDG_STATE_HOME", "state"),
            ("XDG_CACHE_HOME", "cache"),
        ] {
            let path = root.join(dir);
            files::private_dir(&path).map_err(|_| "无法创建 OpenCode 私有运行目录")?;
            command.env(name, path);
        }
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(3))
            .build()
            .map_err(|_| "无法连接 OpenCode 本机服务")?;
        let child = command.spawn().map_err(|_| "无法启动 OpenCode 运行进程")?;
        Ok(Self {
            policy: json!({"model":config["model"], "permission":config["permission"]}),
            child,
            client,
            base: String::new(),
            password,
        })
    }

    async fn ready(&mut self) -> Result<(), &'static str> {
        let mut output = BufReader::new(
            self.child
                .stdout
                .take()
                .ok_or("无法读取 OpenCode 启动状态")?,
        );
        let mut line = Vec::new();
        let port = loop {
            line.clear();
            let bytes = (&mut output)
                .take(4096)
                .read_until(b'\n', &mut line)
                .await
                .map_err(|_| "无法读取 OpenCode 启动状态")?;
            if bytes == 0 || bytes >= 4096 {
                return Err("OpenCode 未能启动本机服务，请核对 CLI 版本");
            }
            if let Ok(line) = std::str::from_utf8(&line)
                && let Some((_, port)) = line.trim().rsplit_once("http://127.0.0.1:")
                && let Ok(port) = port.parse::<u16>()
                && port > 0
            {
                break port;
            }
        };
        self.base = format!("http://127.0.0.1:{port}");
        tokio::spawn(async move {
            let _ = tokio::io::copy(&mut output, &mut tokio::io::sink()).await;
        });
        Ok(())
    }

    async fn verify_policy(&self) -> Result<(), &'static str> {
        const INVALID: &str = "无法确认 OpenCode 的本机鉴权或工具审批规则，本轮未发送";
        let without_auth = self
            .client
            .get(format!("{}/config", self.base))
            .timeout(CONTROL_TIMEOUT)
            .send()
            .await
            .map_err(|_| INVALID)?;
        if without_auth.status() != reqwest::StatusCode::UNAUTHORIZED {
            return Err(INVALID);
        }
        let mut response = self
            .send(reqwest::Method::GET, "/config", None)
            .await
            .map_err(|_| INVALID)?;
        if !response.status().is_success() {
            return Err(INVALID);
        }
        let mut body = Vec::new();
        while let Some(bytes) = response.chunk().await.map_err(|_| INVALID)? {
            if bytes.len() > MAX_FRAME.saturating_sub(body.len()) {
                return Err(INVALID);
            }
            body.extend_from_slice(&bytes);
        }
        let actual: Value = serde_json::from_slice(&body).map_err(|_| INVALID)?;
        if actual["model"] != self.policy["model"]
            || actual["permission"] != self.policy["permission"]
            || actual["agent"]["build"]["permission"] != self.policy["permission"]
        {
            return Err(INVALID);
        }
        Ok(())
    }

    fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        self.client
            .request(method, format!("{}{path}", self.base))
            .basic_auth("opencode", Some(&self.password))
    }

    async fn send(
        &self,
        method: reqwest::Method,
        path: &str,
        data: Option<Value>,
    ) -> Result<reqwest::Response, &'static str> {
        let request = self.request(method, path).timeout(CONTROL_TIMEOUT);
        let response = if let Some(data) = data {
            request.json(&data)
        } else {
            request
        }
        .send()
        .await
        .map_err(|_| EXECUTION_UNKNOWN)?;
        Ok(response)
    }

    async fn post(&self, path: &str, data: Value) -> Result<reqwest::Response, &'static str> {
        self.send(reqwest::Method::POST, path, Some(data)).await
    }

    async fn stop(&mut self) -> bool {
        self.child.stdin.take();
        time::timeout(CONTROL_TIMEOUT, self.child.wait())
            .await
            .is_ok_and(|result| result.is_ok())
    }
}

pub(super) fn native_id(id: &str, prefix: &str) -> bool {
    id.starts_with(prefix)
        && id.len() <= 100
        && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

fn bounded_string(value: &Value) -> Option<String> {
    let text = value.as_str()?;
    (!text.is_empty() && text.len() <= 4096 && !text.contains('\0')).then(|| text.to_owned())
}

pub(super) fn permission(props: &Value) -> Option<Approval> {
    let id = props["id"].as_str().filter(|id| native_id(id, "per_"))?;
    let tool = props["permission"].as_str()?;
    let targets = match tool {
        "bash" => vec![bounded_string(&props["metadata"]["command"])?],
        "edit" => {
            let patterns = props["patterns"].as_array()?;
            if patterns.is_empty() || patterns.len() > 24 {
                return None;
            }
            patterns
                .iter()
                .map(bounded_string)
                .collect::<Option<Vec<_>>>()?
        }
        _ => return None,
    };
    Some(Approval {
        id: id.into(),
        tool: tool.into(),
        targets,
    })
}

pub(super) struct Events {
    response: reqwest::Response,
    buffer: Vec<u8>,
}
impl Events {
    async fn open(server: &Server) -> Result<Self, &'static str> {
        let response = time::timeout(
            CONTROL_TIMEOUT,
            server.request(reqwest::Method::GET, "/event").send(),
        )
        .await
        .map_err(|_| EXECUTION_UNKNOWN)?
        .map_err(|_| EXECUTION_UNKNOWN)?;
        if !response.status().is_success() {
            return Err("当前 OpenCode 不支持任务事件接口");
        }
        Ok(Self::from_response(response))
    }

    pub(super) fn from_response(response: reqwest::Response) -> Self {
        Self {
            response,
            buffer: Vec::new(),
        }
    }

    pub(super) async fn next(&mut self) -> Result<Value, &'static str> {
        loop {
            if let Some(index) = self.buffer.iter().position(|&byte| byte == b'\n') {
                let line: Vec<u8> = self.buffer.drain(..=index).collect();
                if let Some(line) = line.strip_prefix(b"data:") {
                    return serde_json::from_slice(line).map_err(|_| EXECUTION_UNKNOWN);
                }
                continue;
            }
            let bytes = self
                .response
                .chunk()
                .await
                .map_err(|_| EXECUTION_UNKNOWN)?
                .ok_or(EXECUTION_UNKNOWN)?;
            if self.buffer.len().saturating_add(bytes.len()) > MAX_FRAME {
                return Err("OpenCode 事件超出可处理大小，已停止本轮");
            }
            self.buffer.extend(bytes);
        }
    }
}

pub(super) async fn run(
    bench: &Workbench,
    task: Task,
    binary: PathBuf,
    config: Value,
    text: String,
    mut stop: watch::Receiver<bool>,
    mut decisions: mpsc::Receiver<(String, bool)>,
) -> Outcome {
    match Server::start(&bench.root, &binary, &task, config) {
        Ok(mut server) => {
            let mut native_session = task.session_id.clone();
            let turn = run_turn(
                bench,
                &task,
                &mut server,
                &mut native_session,
                &text,
                &mut stop,
                &mut decisions,
            );
            let result = match time::timeout(MAX_TURN, turn).await {
                Ok(Ok(result)) => result,
                Ok(Err(result)) => result,
                Err(_) => Outcome::Unknown("本轮执行超时，已关闭执行进程，请核对项目中的改动"),
            };
            if !matches!(result, Outcome::Completed | Outcome::Stopped)
                && !server.base.is_empty()
                && let Some(sid) = &native_session
            {
                let _ = time::timeout(
                    Duration::from_secs(2),
                    server.post(&format!("/session/{sid}/abort"), json!({})),
                )
                .await;
            }
            let stopped = server.stop().await;
            if stopped {
                result
            } else {
                Outcome::Unknown(EXECUTION_UNKNOWN)
            }
        }
        Err(error) => Outcome::Failed(error),
    }
}

async fn run_turn(
    bench: &Workbench,
    task: &Task,
    server: &mut Server,
    native_session: &mut Option<String>,
    text: &str,
    stop: &mut watch::Receiver<bool>,
    decisions: &mut mpsc::Receiver<(String, bool)>,
) -> Result<Outcome, Outcome> {
    let task_id = &task.summary.id;
    tokio::select! {
        _ = cancelled(stop) => return Ok(Outcome::Stopped),
        result = time::timeout(START_TIMEOUT,server.ready()) => {
            result.map_err(|_| "OpenCode 启动超时，请核对 CLI 安装与连接")??;
        }
    }
    server.verify_policy().await?;
    let mut events = Events::open(server).await?;
    let connected = time::timeout(CONTROL_TIMEOUT, events.next())
        .await
        .map_err(|_| EXECUTION_UNKNOWN)??;
    if connected["type"] != "server.connected" {
        return Err("OpenCode 事件协议不兼容".into());
    }

    let sid = if let Some(sid) = &task.session_id {
        if !native_id(sid, "ses_")
            || !server
                .send(reqwest::Method::GET, &format!("/session/{sid}"), None)
                .await?
                .status()
                .is_success()
        {
            return Err(SESSION_MISSING.into());
        }
        sid.clone()
    } else {
        let response = server
            .post("/session", json!({"title":task.summary.title}))
            .await?;
        if !response.status().is_success() {
            return Err("OpenCode 无法创建会话".into());
        }
        let payload: Value = response.json().await.map_err(|_| "OpenCode 会话响应无效")?;
        let sid = payload["id"]
            .as_str()
            .filter(|id| native_id(id, "ses_"))
            .ok_or("OpenCode 会话编号无效")?
            .to_owned();
        *native_session = Some(sid.clone());
        bench
            .change(task_id, |task| task.session_id = Some(sid.clone()))
            .map_err(|_| STORAGE_FAILED)?;
        sid
    };
    if *stop.borrow() {
        return Ok(Outcome::Stopped);
    }
    let (provider, model) = task.summary.route.split_once('/').ok_or(ROUTE_FAILED)?;
    let response = server
        .post(
            &format!("/session/{sid}/prompt_async"),
            json!({
                "agent":"build", "model":{"providerID":provider,"modelID":model},
                "parts":[{"type":"text","text":text}]
            }),
        )
        .await
        .map_err(Outcome::Unknown)?;
    if !response.status().is_success() {
        return Err(ROUTE_FAILED.into());
    }
    bench
        .change(task_id, |task| {
            if task.summary.phase == Phase::Starting {
                task.summary.phase = Phase::Running;
            }
        })
        .map_err(|_| STORAGE_FAILED)?;

    let mut assistant = HashSet::new();
    let mut parts = Vec::new();
    let mut progress = time::interval(Duration::from_millis(180));
    let mut busy = false;
    let mut pending = None;
    loop {
        tokio::select! {
            biased;
            _ = cancelled(stop) => {
                flush(bench,task_id,&mut parts)?;
                let response = server.post(&format!("/session/{sid}/abort"),json!({})).await
                    .map_err(Outcome::Unknown)?;
                if response.status().is_success() && response.json::<bool>().await.is_ok_and(|value| value) {
                    return Ok(Outcome::Stopped);
                }
                return Ok(Outcome::Unknown(EXECUTION_UNKNOWN));
            },
            reply = decisions.recv(), if pending.is_some() => {
                let (id,allow) = reply.ok_or(Outcome::Unknown(EXECUTION_UNKNOWN))?;
                if pending.as_deref() != Some(id.as_str()) {
                    return Err(Outcome::Unknown(EXECUTION_UNKNOWN));
                }
                answer(server,&id,allow).await?;
                pending = None;
            },
            _ = progress.tick(), if !parts.is_empty() => flush(bench,task_id,&mut parts)?,
            event = events.next() => {
                let event = event.map_err(Outcome::Unknown)?;
                let props = &event["properties"];
                let kind = event["type"].as_str().unwrap_or("");
                let session = match kind {
                    "message.updated" => &props["info"]["sessionID"],
                    "message.part.updated" => &props["part"]["sessionID"],
                    _ => &props["sessionID"]
                };
                if session.as_str() != Some(&sid) {
                    continue;
                }
                match kind {
                    "session.error" => {
                        flush(bench,task_id,&mut parts)?;
                        return Err(if props["error"]["name"] == "MessageAbortedError" {
                            Outcome::Stopped
                        } else {
                            Outcome::Failed(ROUTE_FAILED)
                        });
                    }
                    "session.status" if props["status"]["type"] == "retry" => {
                        flush(bench,task_id,&mut parts)?;
                        return Err(ROUTE_FAILED.into());
                    }
                    "session.status" if props["status"]["type"] == "busy" => busy = true,
                    "session.idle" | "session.status" if busy
                        && (kind == "session.idle" || props["status"]["type"] == "idle") => {
                        flush(bench,task_id,&mut parts)?;
                        return Ok(Outcome::Completed);
                    }
                    "message.updated" => {
                        let info = &props["info"];
                        if info["role"] == "assistant" && let Some(id) = info["id"].as_str() {
                            if !info["error"].is_null() {
                                flush(bench,task_id,&mut parts)?;
                                return Err(if info["error"]["name"] == "MessageAbortedError" {
                                    Outcome::Stopped
                                } else {
                                    Outcome::Failed(ROUTE_FAILED)
                                });
                            }
                            if assistant.len() < 256 {
                                assistant.insert(id.to_owned());
                            }
                        }
                    }
                    "message.part.delta" if props["field"] == "text"
                        && props["messageID"].as_str().is_some_and(|id| assistant.contains(id)) => {
                        if let (Some(id),Some(delta)) = (props["partID"].as_str(),props["delta"].as_str())
                            && native_id(id,"prt_")
                        {
                            parts.push((id.to_owned(),"assistant",delta.to_owned(),true));
                        }
                    }
                    "message.part.updated" => {
                        let part = &props["part"];
                        let Some(id) = part["id"].as_str().filter(|id| native_id(id,"prt_")) else { continue; };
                        if part["messageID"].as_str().is_none_or(|id| !assistant.contains(id)) {
                            continue;
                        }
                        if part["type"] == "text" && let Some(text) = part["text"].as_str() {
                            // Native updates are complete snapshots for this part.
                            parts.push((id.to_owned(),"assistant",text.to_owned(),false));
                        } else if part["type"] == "tool" {
                            let tool = part["tool"].as_str().unwrap_or("tool");
                            let state = match part["state"]["status"].as_str() {
                                Some("completed") => "已结束",
                                Some("error") => "未完成",
                                _ => "进行中"
                            };
                            let text = format!("{} · {state}",local_agent_core::sessions::sanitize_label(tool,80));
                            parts.push((id.to_owned(),"tool",text,false));
                        }
                    }
                    "permission.asked" => {
                        flush(bench,task_id,&mut parts)?;
                        let id = props["id"].as_str().filter(|id| native_id(id,"per_")).ok_or("OpenCode 权限请求无效")?;
                        if pending.as_deref() == Some(id) {
                            continue;
                        }
                        if pending.is_some() {
                            answer(server,id,false).await?;
                            continue;
                        }
                        if let Some(approval) = permission(props) {
                            bench.change(task_id,|task| {
                                if task.summary.phase != Phase::Stopping {
                                    task.summary.phase = Phase::Approval;
                                    task.approval = Some(approval);
                                }
                            }).map_err(|_| STORAGE_FAILED)?;
                            pending = Some(id.to_owned());
                        } else {
                            answer(server,id,false).await?;
                        }
                    }
                    _ => {}
                }
                if parts.len() >= 128 {
                    flush(bench,task_id,&mut parts)?;
                }
            }
        }
    }
}

async fn answer(server: &Server, id: &str, allow: bool) -> Result<(), Outcome> {
    let response = server
        .post(
            &format!("/permission/{id}/reply"),
            json!({
                "reply":if allow {"once"} else {"reject"}
            }),
        )
        .await
        .map_err(Outcome::Unknown)?;
    if response.status().is_success() {
        Ok(())
    } else {
        Err(Outcome::Unknown("审批结果无法确认，已关闭本轮执行进程"))
    }
}

type Update = (String, &'static str, String, bool);
fn flush(bench: &Workbench, task_id: &str, updates: &mut Vec<Update>) -> Result<(), Outcome> {
    if !updates.is_empty() {
        bench
            .change(task_id, |task| {
                for (id, kind, text, append) in updates.drain(..) {
                    task.part(&id, kind, &text, append);
                }
            })
            .map_err(|_| STORAGE_FAILED)?;
    }
    Ok(())
}
