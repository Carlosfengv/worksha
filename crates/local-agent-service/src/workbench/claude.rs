use super::{Approval, Outcome, Phase, Task, Workbench, cancelled, claude_config::Config};
use local_agent_core::providers::{files, vault::Vault};
use serde_json::{Value, json};
use std::{io::Read, path::PathBuf, process::Stdio, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::{ChildStdin, ChildStdout, Command},
    sync::{mpsc, watch},
    time,
};

const UNKNOWN: &str = "Claude Code 执行连接已中断，结果待确认。请先核对项目中的改动。";
const INCOMPATIBLE: &str = "无法确认 Claude Code 的审批通道，未开始执行。请核对 CLI 版本。";
const ROUTE_FAILED: &str = "Claude Code 所选模型路由不可用。请检查连接和凭据，当前路由已保留。";
const STORAGE: &str = "无法保存任务记录，已结束执行连接，请先核对项目中的改动。";
const MAX_FRAME: usize = 1024 * 1024;

mod history;
pub(super) use history::{checkpoint, native_busy, refresh_native};

pub(crate) fn supervise() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(2);
    let binary = args.next().ok_or("missing Claude Code executable")?;
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(async {
            let mut child = Command::new(binary)
                .args(args)
                .stdin(Stdio::piped())
                .stderr(Stdio::null())
                .process_group(0)
                .kill_on_drop(true)
                .spawn()?;
            let group = child.id().ok_or("missing Claude Code process ID")? as i32;
            let mut input = child.stdin.take().ok_or("missing Claude Code input")?;
            let (parent, mut bytes) = mpsc::channel::<Vec<u8>>(8);
            std::thread::spawn(move || {
                let mut buffer = [0; 8192];
                let mut source = std::io::stdin().lock();
                while let Ok(length) = source.read(&mut buffer) {
                    if length == 0 || parent.blocking_send(buffer[..length].to_vec()).is_err() {
                        break;
                    }
                }
            });
            let forward = async {
                while let Some(buffer) = bytes.recv().await {
                    if input.write_all(&buffer).await.is_err() {
                        break;
                    }
                }
            };
            let mut termination =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
            tokio::select! {
                _ = child.wait() => {}
                _ = forward => {}
                _ = termination.recv() => {}
            }
            // Only the group owned by this supervisor can be signalled.
            unsafe { libc::kill(-group, libc::SIGINT) };
            let _ = time::timeout(Duration::from_millis(600), child.wait()).await;
            unsafe { libc::kill(-group, libc::SIGKILL) };
            let _ = child.wait().await;
            Ok(())
        })
}

async fn write(input: &mut ChildStdin, frame: Value) -> Result<(), Outcome> {
    let mut data = serde_json::to_vec(&frame).map_err(|_| Outcome::Unknown(UNKNOWN))?;
    data.push(b'\n');
    time::timeout(Duration::from_secs(3), input.write_all(&data))
        .await
        .map_err(|_| Outcome::Unknown(UNKNOWN))?
        .map_err(|_| Outcome::Unknown(UNKNOWN))
}

struct Frames {
    reader: BufReader<ChildStdout>,
    buffer: Vec<u8>,
}
impl Frames {
    async fn next(&mut self) -> Result<Value, Outcome> {
        let remaining = MAX_FRAME.saturating_sub(self.buffer.len());
        let count = (&mut self.reader)
            .take(remaining as u64)
            .read_until(b'\n', &mut self.buffer)
            .await
            .map_err(|_| Outcome::Unknown(UNKNOWN))?;
        if count == 0 || !self.buffer.ends_with(b"\n") {
            return Err(Outcome::Unknown(UNKNOWN));
        }
        let result = serde_json::from_slice(&self.buffer).map_err(|_| Outcome::Unknown(UNKNOWN));
        self.buffer.clear();
        result
    }
}

fn setting() -> Value {
    json!({"permissions":{
        "ask":["Bash","Write","Edit"],
        "deny":["Read(//**/.env)","Read(//**/.env.*)"]
    }})
}

pub(super) async fn run(
    bench: &Workbench,
    task: Task,
    binary: PathBuf,
    (mut config, vault): (Config, Arc<dyn Vault>),
    text: String,
    mut stop: watch::Receiver<bool>,
    mut decisions: mpsc::Receiver<(String, bool)>,
) -> Outcome {
    if let Some(reference) = config.credential.take() {
        let material = tokio::task::spawn_blocking(move || vault.get(&reference));
        let credential = tokio::select! {
            _ = cancelled(&mut stop) => return Outcome::Stopped,
            result = time::timeout(Duration::from_secs(5),material) => result,
        };
        let Ok(Ok(Ok(secret))) = credential else {
            return Outcome::Failed(ROUTE_FAILED);
        };
        let Ok(secret) = std::str::from_utf8(&secret) else {
            return Outcome::Failed(ROUTE_FAILED);
        };
        if secret.is_empty() || secret.contains('\0') {
            return Outcome::Failed(ROUTE_FAILED);
        }
        config.env.insert("ANTHROPIC_API_KEY".into(), secret.into());
    }

    let home = if task.summary.native_history {
        bench.source_home.join(".claude")
    } else {
        bench.root.join("claude")
    };
    if files::private_dir(&home).is_err() {
        return Outcome::Failed("无法创建 Claude Code 的私有会话目录");
    }
    let Ok(supervisor) = std::env::current_exe() else {
        return Outcome::Failed("无法启动 Claude Code 执行进程");
    };
    let mut command = Command::new(supervisor);
    command
        .arg("--workbench-claude")
        .arg(binary)
        .args([
            "--restricted",
            "--settings",
            &setting().to_string(),
            "--strict-mcp-config",
            "--tools",
            "Bash,Read,Glob,Grep,Edit,Write",
            "--permission-mode",
            "default",
            "--permission-prompts",
            "host",
            "--permission-prompt-tool",
            "stdio",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--include-partial-messages",
            "--verbose",
            "--model",
            &task.summary.route,
            "-p",
        ])
        .current_dir(&task.summary.directory)
        .env_clear()
        .env(
            "PATH",
            "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin",
        )
        .env("CLAUDE_CONFIG_DIR", home)
        .env("DISABLE_AUTOUPDATER", "1")
        .env("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1")
        .env("CLAUDE_CODE_DISABLE_BACKGROUND_TASKS", "1")
        .envs(config.env)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    if let Some(home) = dirs::home_dir() {
        command.env("HOME", home);
    }
    if let Some(id) = &task.session_id {
        if uuid::Uuid::parse_str(id).is_err() {
            return Outcome::Failed("原 Claude Code 会话编号无效，请创建新任务");
        }
        command.args(["--resume", id]);
    }
    if *stop.borrow() {
        return Outcome::Stopped;
    }
    let Ok(mut child) = command.spawn() else {
        return Outcome::Failed("无法启动 Claude Code 执行进程");
    };
    let (Some(mut input), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
        return Outcome::Failed(INCOMPATIBLE);
    };
    let mut frames = Frames {
        reader: BufReader::new(stdout),
        buffer: Vec::new(),
    };
    let result = time::timeout(
        Duration::from_secs(20 * 60),
        turn(
            bench,
            &task,
            &mut input,
            &mut frames,
            &text,
            &mut stop,
            &mut decisions,
        ),
    )
    .await
    .unwrap_or(Err(Outcome::Unknown(
        "Claude Code 执行超时，已关闭连接，请核对项目中的改动",
    )));
    drop(input);
    if !time::timeout(Duration::from_secs(3), child.wait())
        .await
        .is_ok_and(|result| result.is_ok())
    {
        return Outcome::Unknown(UNKNOWN);
    }
    result.unwrap_or_else(|outcome| outcome)
}

fn approval(frame: &Value) -> Option<Approval> {
    let id = frame["request_id"].as_str()?;
    if id.is_empty()
        || id.len() > 120
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_".contains(&byte))
    {
        return None;
    }
    let request = &frame["request"];
    let (tool, target) = match request["tool_name"].as_str()? {
        "Bash" => ("bash", request["input"]["command"].as_str()?),
        "Write" | "Edit" => ("edit", request["input"]["file_path"].as_str()?),
        _ => return None,
    };
    if target.is_empty() || target.len() > 4096 || target.contains('\0') {
        return None;
    }
    Some(Approval {
        id: id.into(),
        tool: tool.into(),
        targets: vec![target.into()],
    })
}

async fn respond(
    input: &mut ChildStdin,
    id: &str,
    allow: bool,
    original: Value,
) -> Result<(), Outcome> {
    let response = if allow {
        json!({"behavior":"allow","updatedInput":original})
    } else {
        json!({"behavior":"deny","message":"用户或任务工作台拒绝此操作。"})
    };
    write(
        input,
        json!({"type":"control_response","response":{
            "subtype":"success","request_id":id,"response":response
        }}),
    )
    .await
}

async fn interrupt(input: &mut ChildStdin, frames: &mut Frames) -> Outcome {
    let id = uuid::Uuid::new_v4().to_string();
    let interrupted = async {
        write(
            input,
            json!({"type":"control_request","request_id":id,"request":{"subtype":"interrupt"}}),
        )
        .await?;
        loop {
            let response = frames.next().await?;
            if response["type"] == "control_response" && response["response"]["request_id"] == id {
                return Ok::<_, Outcome>(response["response"]["subtype"] == "success");
            }
        }
    };
    if matches!(
        time::timeout(Duration::from_secs(2), interrupted).await,
        Ok(Ok(true))
    ) {
        Outcome::Stopped
    } else {
        Outcome::Unknown(UNKNOWN)
    }
}

async fn turn(
    bench: &Workbench,
    task: &Task,
    input: &mut ChildStdin,
    frames: &mut Frames,
    text: &str,
    stop: &mut watch::Receiver<bool>,
    decisions: &mut mpsc::Receiver<(String, bool)>,
) -> Result<Outcome, Outcome> {
    let init = uuid::Uuid::new_v4().to_string();
    write(
        input,
        json!({"type":"control_request","request_id":init,"request":{"subtype":"initialize"}}),
    )
    .await?;
    let response = tokio::select! {
        _ = cancelled(stop) => return Ok(Outcome::Stopped),
        response = time::timeout(Duration::from_secs(15),frames.next()) => {
            response.map_err(|_| INCOMPATIBLE)?.map_err(|_| INCOMPATIBLE)?
        }
    };
    if response["type"] != "control_response"
        || response["response"]["request_id"] != init
        || response["response"]["subtype"] != "success"
    {
        return Err(INCOMPATIBLE.into());
    }
    write(
        input,
        json!({"type":"user","message":{"role":"user","content":[{"type":"text","text":text}]},
            "parent_tool_use_id":null,"session_id":task.session_id.as_deref().unwrap_or("")
        }),
    )
    .await?;
    let task_id = &task.summary.id;
    let mut pending: Option<(String, Value)> = None;
    let mut initialized = false;
    let mut message_id = String::new();
    let mut updates = Vec::new();
    let mut ticker = time::interval(Duration::from_millis(180));
    loop {
        tokio::select! {
            biased;
            _ = cancelled(stop) => {
                flush(bench,task_id,&mut updates)?;
                return Ok(interrupt(input,frames).await);
            }
            decision = decisions.recv(), if pending.is_some() => {
                let (id,allow) = decision.ok_or(Outcome::Unknown(UNKNOWN))?;
                let (request,original) = pending.take().ok_or(Outcome::Unknown(UNKNOWN))?;
                if id != request { return Err(Outcome::Unknown(UNKNOWN)); }
                respond(input,&id,allow,original).await?;
            }
            _ = ticker.tick(), if !updates.is_empty() => flush(bench,task_id,&mut updates)?,
            frame = frames.next() => {
                let frame = frame?;
                match frame["type"].as_str() {
                    Some("system") if frame["subtype"] == "init" => {
                        let sid = frame["session_id"].as_str()
                            .filter(|sid| uuid::Uuid::parse_str(sid).is_ok()).ok_or(INCOMPATIBLE)?;
                        if frame["permissionMode"] != "default" || task.session_id.as_deref().is_some_and(|id| id != sid) {
                            return Err(INCOMPATIBLE.into());
                        }
                        initialized = true;
                        bench.change(task_id,|task| {
                            task.session_id = Some(sid.to_owned());
                            if task.summary.phase == Phase::Starting {
                                task.summary.phase = Phase::Running;
                            }
                        }).map_err(|_| STORAGE)?;
                    }
                    Some("control_request") if frame["request"]["subtype"] == "can_use_tool" => {
                        flush(bench,task_id,&mut updates)?;
                        let id = frame["request_id"].as_str().ok_or(INCOMPATIBLE)?;
                        if initialized && pending.is_none() && let Some(approval) = approval(&frame) {
                            bench.change(task_id,|task| {
                                if task.summary.phase != Phase::Stopping {
                                    task.summary.phase = Phase::Approval;
                                    task.approval = Some(approval);
                                }
                            }).map_err(|_| STORAGE)?;
                            pending = Some((id.into(),frame["request"]["input"].clone()));
                        } else {
                            respond(input,id,false,Value::Null).await?;
                        }
                    }
                    Some("control_cancel_request") => {
                        if pending.as_ref().is_some_and(|(id,_)| frame["request_id"] == id.as_str()) {
                            pending = None;
                            bench.change(task_id,|task| {
                                task.approval = None;
                                if task.summary.phase == Phase::Approval { task.summary.phase = Phase::Running; }
                            }).map_err(|_| STORAGE)?;
                        }
                    }
                    Some("stream_event") if initialized => {
                        let event = &frame["event"];
                        if event["type"] == "message_start" {
                            message_id = event["message"]["id"].as_str().unwrap_or("").to_owned();
                        } else if event["delta"]["type"] == "text_delta"
                            && !message_id.is_empty() && let Some(text) = event["delta"]["text"].as_str()
                        {
                            let index = event["index"].as_u64().unwrap_or(0);
                            updates.push((format!("{message_id}-{index}"),"assistant",text.to_owned(),true));
                        }
                    }
                    Some("assistant") if initialized && frame["error"].is_null() => {
                        if let Some(id) = frame["message"]["id"].as_str()
                            && let Some(content) = frame["message"]["content"].as_array()
                        {
                            for (index,part) in content.iter().enumerate() {
                                match part["type"].as_str() {
                                    Some("text") if part["text"].is_string() => {
                                        let text = part["text"].as_str().unwrap_or("");
                                        updates.push((format!("{id}-{index}"),"assistant",text.to_owned(),false));
                                    }
                                    Some("tool_use") if part["name"].is_string() => {
                                        let name = local_agent_core::sessions::sanitize_label(part["name"].as_str().unwrap_or(""),80);
                                        updates.push((format!("{id}-{index}"),"tool",name,false));
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }
                    Some("result") => {
                        flush(bench,task_id,&mut updates)?;
                        if frame["is_error"] == true || !initialized {
                            return Err(ROUTE_FAILED.into());
                        }
                        return Ok(Outcome::Completed);
                    }
                    _ => {}
                }
                if updates.len() >= 128 {
                    flush(bench,task_id,&mut updates)?;
                }
            }
        }
    }
}

type Update = (String, &'static str, String, bool);
fn flush(bench: &Workbench, id: &str, updates: &mut Vec<Update>) -> Result<(), Outcome> {
    if !updates.is_empty() {
        bench
            .change(id, |task| {
                for (id, kind, text, append) in updates.drain(..) {
                    task.part(&id, kind, &text, append);
                }
            })
            .map_err(|_| STORAGE)?;
    }
    Ok(())
}
