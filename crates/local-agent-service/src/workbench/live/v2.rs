use super::{
    Approval, Error, Overlay, Part, REQUEST_TIMEOUT, Server, bounded_text, conflict,
    display_version, opencode, read_json,
};
use local_agent_core::sessions::sanitize_label;
use serde::Deserialize;
use serde_json::Value;
use std::{
    fs::{self, Metadata, OpenOptions},
    io::Read,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    process::Stdio,
};
use tokio::{process::Command, time};
use zeroize::Zeroizing;

const MAX_RECORD_SIZE: u64 = 8192;

#[derive(Deserialize)]
struct Record {
    url: String,
    pid: u32,
    version: Option<String>,
    password: String,
}

pub(super) struct Identity {
    path: PathBuf,
    metadata: Metadata,
    pid: u32,
}

fn missing() -> Error {
    conflict("未找到 OpenCode 本机连接记录。请打开使用共享本机服务的 OpenCode Desktop。")
}

fn stale() -> Error {
    conflict("OpenCode 默认本机服务已退出、重启或更换凭据。请重新连接。")
}

fn private_file(metadata: &Metadata) -> bool {
    // SAFETY: geteuid reads the process's effective user ID and takes no pointers.
    metadata.uid() == unsafe { libc::geteuid() }
        && metadata.is_file()
        && metadata.mode() & 0o077 == 0
        && metadata.len() <= MAX_RECORD_SIZE
}

impl Identity {
    pub(super) fn scope(&self) -> String {
        self.path.to_string_lossy().into_owned()
    }

    pub(super) fn confirm_record(&self) -> Result<(), Error> {
        let current = fs::symlink_metadata(&self.path).map_err(|_| stale())?;
        let original = &self.metadata;
        if !private_file(&current)
            || current.dev() != original.dev()
            || current.ino() != original.ino()
            || current.len() != original.len()
            || (current.mtime(), current.mtime_nsec()) != (original.mtime(), original.mtime_nsec())
        {
            return Err(stale());
        }
        Ok(())
    }

    pub(super) fn confirm_running(&self) -> Result<(), Error> {
        self.confirm_record()?;
        // SAFETY: Discovery only accepts positive PIDs representable by pid_t.
        // Signal zero checks process existence and permission without sending a signal.
        if unsafe { libc::kill(self.pid as libc::pid_t, 0) } != 0 {
            return Err(stale());
        }
        Ok(())
    }
}

fn read_record(home: &Path) -> Result<(Record, Identity), Error> {
    let state = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .unwrap_or_else(|| home.join(".local/state"));
    let path = state.join("opencode/service.json");
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(&path)
        .map_err(|err| match err.kind() {
            std::io::ErrorKind::NotFound => missing(),
            _ => conflict("无法安全读取 OpenCode 本机连接记录"),
        })?;
    let metadata = file.metadata().map_err(|_| stale())?;
    if !private_file(&metadata) {
        return Err(conflict("OpenCode 本机连接记录需要仅当前用户可访问"));
    }
    let mut bytes = Zeroizing::new(Vec::new());
    file.take(MAX_RECORD_SIZE + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| stale())?;
    if bytes.len() as u64 > MAX_RECORD_SIZE {
        return Err(stale());
    }
    let record: Record = serde_json::from_slice(&bytes).map_err(|_| stale())?;
    let identity = Identity {
        path,
        metadata,
        pid: record.pid,
    };
    identity.confirm_record()?;
    Ok((record, identity))
}

pub(super) async fn discover(
    home: &Path,
    management_host: &str,
) -> Result<(Server, String), Error> {
    let (record, identity) = read_record(home)?;
    let password = Zeroizing::new(record.password);
    let url = reqwest::Url::parse(&record.url).map_err(|_| stale())?;
    let port = url.port_or_known_default().ok_or_else(stale)?;
    if url.scheme() != "http"
        || url.host_str() != Some("127.0.0.1")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
        || port == 0
        || management_host == format!("127.0.0.1:{port}")
        || record.pid <= 1
        || record.pid > i32::MAX as u32
        || password.is_empty()
        || password.len() > 512
        || password.chars().any(char::is_control)
    {
        return Err(conflict("连接记录必须指向开启认证的 OpenCode 本机服务"));
    }

    // Check the listener belongs to the recorded process before sending the password.
    let pid = record.pid.to_string();
    let output = time::timeout(
        REQUEST_TIMEOUT,
        Command::new("/usr/sbin/lsof")
            .args([
                "-a",
                "-p",
                &pid,
                &format!("-i4TCP@127.0.0.1:{port}"),
                "-sTCP:LISTEN",
                "-Fp",
            ])
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .output(),
    )
    .await
    .map_err(|_| stale())?
    .map_err(|_| stale())?;
    if !output.status.success()
        || !String::from_utf8_lossy(&output.stdout)
            .lines()
            .any(|line| line.strip_prefix('p') == Some(&pid))
    {
        return Err(stale());
    }

    let client = Server::client()?;
    let base = format!("http://127.0.0.1:{port}");
    let status = client
        .get(format!("{base}/api/session/active"))
        .timeout(REQUEST_TIMEOUT)
        .send()
        .await
        .map_err(|_| stale())?
        .status();
    if status != reqwest::StatusCode::UNAUTHORIZED {
        return Err(conflict("OpenCode 默认本机服务需要启用认证"));
    }
    let server = Server {
        client,
        base,
        directory: None,
        user: "opencode".into(),
        password,
        v2: Some(identity),
    };
    let version = display_version(record.version.as_deref());
    Ok((server, version))
}

pub(super) fn route(session: &Value) -> Option<String> {
    let field = |value: &Value| {
        value
            .as_str()
            .filter(|text| {
                !text.is_empty() && text.len() <= 200 && !text.chars().any(char::is_control)
            })
            .map(str::to_owned)
    };
    let model = &session["model"];
    Some(format!(
        "{}/{}",
        field(&model["providerID"])?,
        field(&model["id"])?
    ))
}

pub(super) fn approval(request: &Value) -> Option<Approval> {
    let id = request["id"]
        .as_str()
        .filter(|id| opencode::native_id(id, "per_"))?;
    let tool = match request["action"].as_str()? {
        "shell" => "bash",
        "edit" => "edit",
        _ => return None,
    };
    let resources = request["resources"].as_array()?;
    if resources.is_empty() || resources.len() > 24 {
        return None;
    }
    let targets = resources
        .iter()
        .map(|resource| {
            resource
                .as_str()
                .filter(|text| !text.is_empty() && text.len() <= 4096 && !text.contains('\0'))
                .map(str::to_owned)
        })
        .collect::<Option<Vec<_>>>()?;
    Some(Approval {
        id: id.into(),
        tool: tool.into(),
        targets,
    })
}

pub(super) async fn reply(
    server: &Server,
    session: &str,
    approval: &str,
    decision: &str,
) -> Result<(), Error> {
    let path = format!("/api/session/{session}/permission/{approval}/reply");
    let response = server
        .post_response(&path, serde_json::json!({"decision":decision}))
        .await?;
    if response.status() != reqwest::StatusCode::BAD_REQUEST {
        return Server::accepted(response).map(|_| ());
    }
    let error = read_json(response, 8192).await?;
    // The older payload is safe to try only after the server rejected the first request before handling it.
    if error["_tag"] != "InvalidRequestError"
        || error["kind"] != "Payload"
        || error["message"] != "Missing key\n  at [\"reply\"]"
    {
        return Err(conflict("OpenCode 拒绝了审批请求，请刷新原会话状态"));
    }
    server
        .post(&path, serde_json::json!({"reply":decision}))
        .await
        .map(|_| ())
}

pub(super) fn messages(history: &Value, overlay: &Overlay) -> Vec<Part> {
    let mut parts = Vec::new();
    for message in history.as_array().into_iter().flatten().rev() {
        let Some(id) = message["id"]
            .as_str()
            .filter(|id| opencode::native_id(id, "msg_"))
        else {
            continue;
        };
        if message["type"] == "user"
            && let Some(text) = message["text"].as_str()
        {
            parts.push(Part {
                id: id.into(),
                role: "user",
                text: bounded_text(text),
            });
        }
        if message["type"] != "assistant" {
            continue;
        }
        let mut text_ordinal = 0;
        for (index, content) in message["content"]
            .as_array()
            .into_iter()
            .flatten()
            .enumerate()
        {
            if content["type"] == "text"
                && let Some(text) = content["text"].as_str()
            {
                let key = format!("{id}:text:{text_ordinal}");
                text_ordinal += 1;
                let text = overlay
                    .text
                    .get(&key)
                    .filter(|cached| cached.len() > text.len())
                    .map_or(text, String::as_str);
                parts.push(Part {
                    id: key,
                    role: "assistant",
                    text: bounded_text(text),
                });
            } else if content["type"] == "tool" {
                let state = match content["state"]["status"].as_str() {
                    Some("completed" | "success") => "已结束",
                    Some("error" | "failed") => "未完成",
                    _ => "执行中",
                };
                let name = sanitize_label(content["name"].as_str().unwrap_or("工具"), 80);
                parts.push(Part {
                    id: format!("{id}:tool:{index}"),
                    role: "tool",
                    text: format!("{name} · {state}"),
                });
            }
        }
    }
    parts
}

fn problem(error: &Value) -> Option<&'static str> {
    match error["type"].as_str()? {
        "aborted" => None,
        "api" | "auth" | "model" | "provider" | "retry" => Some("route"),
        kind if kind.starts_with("provider.") || kind.starts_with("model.") => Some("route"),
        _ => Some("execution"),
    }
}

pub(super) fn history_problem(history: &Value, active: bool) -> Option<&'static str> {
    history
        .as_array()?
        .iter()
        .take_while(|message| message["type"] != "user")
        .find_map(|message| {
            problem(&message["error"]).or_else(|| {
                if active {
                    problem(&message["retry"]["error"])
                } else {
                    None
                }
            })
        })
}

pub(super) fn apply(overlay: &mut Overlay, event: &Value, session: &str) {
    let data = &event["data"];
    if data["sessionID"] != session {
        return;
    }
    match event["type"].as_str() {
        Some(
            "session.execution.started"
            | "session.execution.succeeded"
            | "session.execution.interrupted",
        ) => overlay.problem = None,
        Some("session.execution.failed") => {
            overlay.problem = problem(&data["error"]).or(Some("execution"))
        }
        Some("session.retry.scheduled") => overlay.problem = Some("route"),
        Some("session.text.delta" | "session.text.ended") => {
            let Some(id) = data["assistantMessageID"]
                .as_str()
                .filter(|id| opencode::native_id(id, "msg_"))
            else {
                return;
            };
            let Some(ordinal) = data["ordinal"].as_u64() else {
                return;
            };
            let key = format!("{id}:text:{ordinal}");
            if event["type"] == "session.text.delta"
                && let Some(delta) = data["delta"].as_str()
            {
                overlay.append(&key, delta);
            } else if let Some(text) = data["text"].as_str() {
                if let Some(cached) = overlay.text.get_mut(&key) {
                    *cached = bounded_text(text);
                } else {
                    overlay.append(&key, text);
                }
            }
        }
        _ => {}
    }
}
