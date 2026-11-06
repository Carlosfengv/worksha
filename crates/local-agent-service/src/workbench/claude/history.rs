use super::{Task, Workbench};
use crate::workbench::{Error, Phase, conflict};
use local_agent_core::sessions::{TranscriptWindow, read_transcript_snapshot};
use serde_json::Value;
use std::{fs, os::unix::fs::MetadataExt, path::PathBuf};

fn stamp(metadata: &fs::Metadata) -> String {
    format!(
        "{}:{}:{}:{}",
        metadata.dev(),
        metadata.ino(),
        metadata.mtime(),
        metadata.mtime_nsec()
    )
}

fn transcript(bench: &Workbench, task: &Task) -> Option<PathBuf> {
    if !task.summary.native_history {
        return None;
    }
    let id = uuid::Uuid::parse_str(task.session_id.as_ref()?)
        .ok()?
        .to_string();
    fs::read_dir(bench.source_home.join(".claude/projects"))
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path().join(format!("{id}.jsonl")))
        .find(|path| path.is_file())
}

pub(in super::super) fn native_busy(bench: &Workbench, task: &Task) -> Result<bool, Error> {
    let Some(id) = task
        .session_id
        .as_deref()
        .filter(|_| task.summary.native_history)
    else {
        return Ok(false);
    };
    let entries = match fs::read_dir(bench.source_home.join(".claude/sessions")) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(_) => return Err(conflict("无法核对原生 Claude Code 的会话占用情况")),
    };
    for entry in entries.filter_map(Result::ok) {
        let file = entry.path();
        if file.extension().and_then(|extension| extension.to_str()) != Some("json") {
            continue;
        }
        let metadata =
            fs::symlink_metadata(&file).map_err(|_| conflict("原生会话占用信息不可读取"))?;
        if !metadata.is_file() || metadata.len() > 64 * 1024 {
            continue;
        }
        let record: Value = match fs::read(&file)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        {
            Some(record) => record,
            None => continue,
        };
        if record["sessionId"] != id {
            continue;
        }
        if let Some(pid) = record["pid"]
            .as_i64()
            .filter(|pid| *pid > 1 && *pid <= i32::MAX as i64)
        {
            // SAFETY: Signal zero checks the positive process ID without sending a signal.
            if unsafe { libc::kill(pid as libc::pid_t, 0) } == 0 {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

pub(in super::super) fn checkpoint(bench: &Workbench, task: &mut Task) {
    let Some(path) = transcript(bench, task) else {
        return;
    };
    let Ok(metadata) = path.metadata() else {
        return;
    };
    if let Ok(parsed) =
        read_transcript_snapshot(&bench.source_home, &path, "claude", None, metadata.len())
    {
        task.native_cursor = metadata.len();
        task.native_events = parsed.total;
        task.native_fingerprint = parsed.fingerprint;
        task.native_stamp = stamp(&metadata);
    }
}

pub(in super::super) fn refresh_native(bench: &Workbench, task: &mut Task) -> Result<bool, Error> {
    let busy = native_busy(bench, task)?;
    let changed = busy != task.summary.native_busy;
    task.summary.native_busy = busy;
    let Some(path) = transcript(bench, task) else {
        return Ok(changed);
    };
    let metadata = path
        .metadata()
        .map_err(|_| conflict("原生 Claude 会话记录不可读取"))?;
    let bytes = metadata.len();
    let current_stamp = stamp(&metadata);
    if (bytes == task.native_cursor && current_stamp == task.native_stamp && !changed)
        || (task.summary.phase == Phase::Unknown && task.native_cursor == 0)
    {
        return Ok(changed);
    }
    if !task.native_fingerprint.is_empty() {
        let original = read_transcript_snapshot(
            &bench.source_home,
            &path,
            "claude",
            None,
            task.native_cursor,
        )
        .map_err(|_| conflict("原生 Claude 会话历史已变化，请核对原记录后重试"))?;
        if original.fingerprint != task.native_fingerprint {
            return Err(conflict("原生 Claude 会话历史已改写，保留当前工作台记录"));
        }
    }
    let parsed = read_transcript_snapshot(
        &bench.source_home,
        &path,
        "claude",
        Some(TranscriptWindow {
            offset: task.native_events,
            anchor: None,
        }),
        bytes,
    )
    .map_err(|_| conflict("原生 Claude 会话记录暂不可读取，请稍后重试"))?;
    if parsed.summary.native_id != task.session_id.as_deref().unwrap_or("")
        || parsed.summary.project_path.as_deref() != task.summary.directory.to_str()
    {
        return Err(conflict("原生 Claude 会话的项目或身份已变化"));
    }
    for event in parsed
        .events
        .into_iter()
        .filter(|event| event.source_line > 0)
    {
        let Some(role) = event
            .role
            .as_deref()
            .filter(|role| ["user", "assistant"].contains(role))
        else {
            continue;
        };
        let Some(text) = event.text.filter(|text| !text.is_empty()) else {
            continue;
        };
        if event.kind == "context" {
            continue;
        }
        let id = format!("native-{}", event.id);
        task.part(&id, role, &text, false);
    }
    if !busy || parsed.end < parsed.total {
        task.native_events = parsed.end;
    }
    if parsed.end >= parsed.total {
        task.native_cursor = bytes;
        task.native_fingerprint = parsed.fingerprint;
        task.native_stamp = current_stamp;
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::Write;

    fn fixture() -> (tempfile::TempDir, std::sync::Arc<Workbench>, Task, PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().canonicalize().unwrap();
        let project = home.join("project");
        fs::create_dir(&project).unwrap();
        let bench = Workbench::open(&home.join("data"), home.clone()).unwrap();
        let task: Task = serde_json::from_value(json!({
            "id":uuid::Uuid::new_v4().to_string(), "agent":"claude", "nativeHistory":true,
            "title":"Native history", "directory":project, "route":"fixture-pinned",
            "phase":"completed", "updated":0, "revision":1,
            "sessionId":uuid::Uuid::new_v4().to_string(), "turnId":null, "parts":[], "approval":null,"error":null
        })).unwrap();
        let directory = home.join(".claude/projects/fixture");
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join(format!("{}.jsonl", task.session_id.as_ref().unwrap()));
        (temp, bench, task, path)
    }

    fn append(path: &std::path::Path, task: &Task, suffix: &str, user: &str, assistant: &str) {
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap();
        for (role, text) in [("user", user), ("assistant", assistant)] {
            writeln!(file,"{}",json!({
                "type":role,"uuid":format!("{role}-{suffix}"),"sessionId":task.session_id,
                "cwd":task.summary.directory,"timestamp":"2026-11-05T04:00:00Z",
                "message":{"id":format!("response-{suffix}"),"role":role,"model":"fixture-pinned","content":[{"type":"text","text":text}]}
            })).unwrap();
        }
    }

    #[test]
    fn external_continuation_imports_once_and_rewritten_history_is_rejected() {
        let (_temp, bench, mut task, path) = fixture();
        task.part("owned-user", "user", "original prompt", false);
        task.part("owned-assistant", "assistant", "original reply", false);
        append(&path, &task, "owned", "original prompt", "original reply");
        checkpoint(&bench, &mut task);
        assert!(task.native_cursor > 0);
        append(&path, &task, "external", "native prompt", "native reply");
        assert!(refresh_native(&bench, &mut task).unwrap());
        assert_eq!(task.parts.len(), 4);
        assert_eq!(task.parts.last().unwrap().text, "native reply");
        assert!(!refresh_native(&bench, &mut task).unwrap());
        let text = fs::read_to_string(&path)
            .unwrap()
            .replace("original reply", "tampered reply");
        fs::write(&path, text).unwrap();
        assert!(refresh_native(&bench, &mut task).is_err());
        assert_eq!(task.parts.len(), 4);
        assert_eq!(task.summary.route, "fixture-pinned");
    }

    #[test]
    fn native_ownership_is_scoped_to_the_exact_session_and_live_process() {
        let (_temp, bench, mut task, _path) = fixture();
        let directory = bench.source_home.join(".claude/sessions");
        fs::create_dir_all(&directory).unwrap();
        fs::write(
            directory.join("owner.json"),
            json!({"sessionId":task.session_id,"pid":std::process::id()}).to_string(),
        )
        .unwrap();
        assert!(native_busy(&bench, &task).unwrap());
        task.session_id = Some(uuid::Uuid::new_v4().to_string());
        assert!(!native_busy(&bench, &task).unwrap());
    }
}
