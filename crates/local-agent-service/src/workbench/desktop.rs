use super::{Agent, identifier, workbench};
use crate::{AppState, api};
use axum::{Json, extract::State};
use serde::Deserialize;
use serde_json::Value;
use std::{process::Stdio, time::Duration};
use tokio::{io::AsyncReadExt, process::Command, time};

const EXPORT_LIMIT: u64 = 4 * 1024 * 1024;

pub(super) fn connection(bench: &super::Workbench, host: &str) -> Value {
    let desktop =
        local_agent_core::providers::config::inspect(&bench.source_home, "claude-desktop");
    let scope = if desktop.error.is_some() {
        super::gateway::Scope::Unknown
    } else {
        super::gateway::classify(desktop.endpoint.as_deref(), false, host)
    };
    serde_json::json!({
        "requestScope":super::claude_config::scope(&bench.source_home, host),
        "desktopRequestScope":scope,
        "desktopError":desktop.error,
    })
}

pub(super) fn task_value(
    bench: &super::Workbench,
    task: &super::Task,
) -> Result<Value, super::Error> {
    let mut value = serde_json::to_value(task).map_err(super::store_error)?;
    if task.summary.agent == Agent::Claude
        && let Some(id) = task
            .session_id
            .as_deref()
            .filter(|id| super::identifier(id).is_ok())
    {
        let home = if task.summary.native_history {
            bench.source_home.join(".claude")
        } else {
            bench.root.join("claude")
        };
        let quote = |text: &str| format!("'{}'", text.replace('\'', "'\\''"));
        value["cliResumeCommand"] = serde_json::json!(format!(
            "cd -- {} && CLAUDE_CONFIG_DIR={} claude --resume {} --model {}",
            quote(&task.summary.directory.to_string_lossy()),
            quote(&home.to_string_lossy()),
            quote(id),
            quote(&task.summary.route)
        ));
    }
    Ok(value)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Export {
    task_id: String,
}

pub(super) async fn opencode_export(
    State(state): State<AppState>,
    Json(input): Json<Export>,
) -> api::ApiResult {
    identifier(&input.task_id)?;
    let bench = workbench(&state)?;
    let task = {
        let storage = bench.lock()?;
        let task = storage.read(&input.task_id)?;
        if task.summary.agent != Agent::Opencode
            || task.summary.phase.active()
            || storage
                .active
                .as_ref()
                .is_some_and(|run| run.task_id == input.task_id)
        {
            return Err(super::conflict("请在 OpenCode 本轮结束后导出会话"));
        }
        task
    };
    let session = task
        .session_id
        .filter(|id| {
            id.starts_with("ses_")
                && id.len() <= 100
                && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
        })
        .ok_or_else(|| super::conflict("任务还没有可导出的 OpenCode 会话"))?;
    let binary = crate::providers::binary("opencode")
        .ok_or_else(|| super::conflict("未找到 OpenCode CLI"))?;
    let mut command = Command::new(binary);
    command
        .args(["export", &session, "--sanitize", "--pure"])
        .env_clear()
        .env("PATH", "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin")
        .env("OPENCODE_DISABLE_PROJECT_CONFIG", "1")
        .env("OPENCODE_DISABLE_EXTERNAL_SKILLS", "1")
        .env("OPENCODE_DISABLE_CLAUDE_CODE", "1")
        .env(
            "OPENCODE_DB",
            bench.root.join(format!("{}.opencode.db", task.summary.id)),
        )
        .env("XDG_CONFIG_HOME", bench.root.join("config"))
        .env("XDG_DATA_HOME", bench.root.join("data"))
        .env("XDG_STATE_HOME", bench.root.join("state"))
        .env("XDG_CACHE_HOME", bench.root.join("cache"))
        .current_dir(&bench.root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    if let Some(home) = dirs::home_dir() {
        command.env("HOME", home);
    }
    let unavailable = || super::conflict("OpenCode 无法导出此会话，原会话仍保留");
    let mut child = command.spawn().map_err(|_| unavailable())?;
    let stdout = child.stdout.take().ok_or_else(unavailable)?;
    let result = time::timeout(Duration::from_secs(15), async {
        let mut bytes = Vec::new();
        stdout
            .take(EXPORT_LIMIT + 1)
            .read_to_end(&mut bytes)
            .await
            .map_err(|_| unavailable())?;
        if bytes.len() as u64 > EXPORT_LIMIT {
            return Err(super::conflict("会话导出超过 4 MB"));
        }
        if !child.wait().await.is_ok_and(|status| status.success()) {
            return Err(unavailable());
        }
        let value: Value = serde_json::from_slice(&bytes).map_err(|_| unavailable())?;
        if value["info"]["id"] != session || !value["messages"].is_array() {
            return Err(unavailable());
        }
        Ok(value)
    })
    .await
    .map_err(|_| unavailable())??;
    api::json_value(result)
}
