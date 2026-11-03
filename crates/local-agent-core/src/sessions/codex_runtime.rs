//! Read-only execution evidence for fresh activities, never historical usage.
//! A process merely existing, a renderer receipt or a Session creator is not evidence.
use super::{live::NativeActivity, safe_path};
use chrono::{DateTime, Utc};
use rusqlite::{Connection, OpenFlags, params};
use std::{collections::HashMap, path::Path, time::Duration};

#[derive(Default)]
pub(super) struct RuntimeEvidence {
    database: Option<(u64, u64)>,
    tui_modes: HashMap<String, (Process, Option<Process>)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Process {
    path: String,
    parent: i32,
    uid: u32,
    started: i64,
    started_micros: u64,
    tty: u32,
}

#[cfg(target_os = "macos")]
fn inspect(pid: i32) -> Option<Process> {
    use std::{ffi::CStr, mem::MaybeUninit};
    let mut info = MaybeUninit::<libc::proc_bsdinfo>::zeroed();
    let size = std::mem::size_of::<libc::proc_bsdinfo>();
    // Both calls fill bounded, caller-owned buffers. No command line or environment is read.
    if unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            0,
            info.as_mut_ptr().cast(),
            size as i32,
        )
    } != size as i32
    {
        return None;
    }
    let info = unsafe { info.assume_init() };
    let mut path = [0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    if unsafe { libc::proc_pidpath(pid, path.as_mut_ptr().cast(), path.len() as u32) } <= 0 {
        return None;
    }
    Some(Process {
        path: CStr::from_bytes_until_nul(&path)
            .ok()?
            .to_str()
            .ok()?
            .into(),
        parent: i32::try_from(info.pbi_ppid).ok()?,
        uid: info.pbi_uid,
        started: i64::try_from(info.pbi_start_tvsec).ok()?,
        started_micros: info.pbi_start_tvusec,
        tty: info.e_tdev,
    })
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 512
        && id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))
}

fn process_id(value: &str) -> Option<i32> {
    let value = value.strip_prefix("pid:")?;
    let (pid, run) = value.split_once(':')?;
    uuid::Uuid::parse_str(run).ok()?;
    let pid: i32 = pid.parse().ok()?;
    (pid > 1).then_some(pid)
}

fn atom(value: &str) -> Option<&str> {
    let value = if let Some(value) = value.strip_prefix('"') {
        value.strip_suffix('"')?
    } else {
        value
    };
    (!value.contains(['"', '\\'])).then_some(value)
}

// Parse only two fixed tracing spans, before any request, tool or response body.
fn scope(body: &str) -> Option<(&str, &str)> {
    let (session, rest) = body.strip_prefix("session_loop{")?.split_once("}:")?;
    let session = atom(session.strip_prefix("thread_id=")?)?;
    let (dispatch, _) = rest
        .strip_prefix("submission_dispatch{")?
        .split_once("}:")?;
    let mut fields = HashMap::new();
    for token in dispatch.split_whitespace() {
        let (key, value) = token.split_once('=')?;
        let value = atom(value)?;
        if !["otel.name", "submission.id", "codex.op"].contains(&key)
            || fields.insert(key, value).is_some()
        {
            return None;
        }
    }
    let turn = *fields.get("submission.id")?;
    (fields.get("otel.name") == Some(&"op.dispatch.turn_input")
        && fields.get("codex.op") == Some(&"turn_input")
        && valid_id(session)
        && valid_id(turn))
    .then_some((session, turn))
}

fn executed_tool(body: &str, session: &str, turn: &str, call: &str) -> bool {
    let Some((_, event)) = body.rsplit_once("}:") else {
        return false;
    };
    let event = event.trim_start();
    let code_mode = event.starts_with("code-mode host operation completed ");
    let Some(event) = (if code_mode {
        event.strip_prefix("code-mode host operation completed ")
    } else {
        event.strip_prefix(
            "handle_output_item_done:handle_tool_call_with_source: tool call completed ",
        )
    }) else {
        return false;
    };
    let mut fields = HashMap::new();
    for token in event.split_whitespace() {
        let Some((key, value)) = token.split_once('=') else {
            return false;
        };
        let Some(value) = atom(value) else {
            return false;
        };
        if ![
            "event.name",
            "trace_id",
            "conversation.id",
            "conversation_id",
            "turn_id",
            "tool_name",
            "call_id",
            "tool_source",
            "execution_started",
            "dispatch_duration_ms",
            "handler_duration_ms",
            "total_duration_ms",
            "cell_id",
            "code_mode_host_duration_ns",
        ]
        .contains(&key)
            || fields.insert(key, value).is_some()
        {
            return false;
        }
    }
    fields.get("event.name")
        == Some(&if code_mode {
            "codex.code_mode.host_timing"
        } else {
            "codex.tool_call"
        })
        && fields.get(if code_mode {
            "conversation_id"
        } else {
            "conversation.id"
        }) == Some(&session)
        && fields.get("turn_id") == Some(&turn)
        && fields.get("call_id") == Some(&call)
        && (if code_mode {
            fields
                .get("code_mode_host_duration_ns")
                .is_some_and(|v| v.parse::<u64>().is_ok())
        } else {
            fields.get("execution_started") == Some(&"true")
        })
}

fn safe_database(home: &Path, path: &Path) -> bool {
    if !safe_path(home, path) {
        return false;
    }
    for suffix in ["", "-wal", "-shm"] {
        let candidate = path.with_file_name(format!("logs_2.sqlite{suffix}"));
        let Ok(meta) = candidate.symlink_metadata() else {
            if suffix.is_empty() {
                return false;
            }
            continue;
        };
        if !meta.is_file() || !safe_path(home, &candidate) || meta.len() > 512 * 1024 * 1024 {
            return false;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if meta.nlink() != 1 {
                return false;
            }
        }
    }
    true
}

fn open(home: &Path) -> Option<Connection> {
    let path = home.join(".codex/logs_2.sqlite");
    if !safe_database(home, &path) {
        return None;
    }
    let canonical = path.canonicalize().ok()?;
    if !canonical.starts_with(home.canonicalize().ok()?) {
        return None;
    }
    let connection = Connection::open_with_flags(
        canonical,
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .ok()?;
    connection.busy_timeout(Duration::from_millis(100)).ok()?;
    connection
        .execute_batch("PRAGMA query_only=ON; PRAGMA trusted_schema=OFF; BEGIN DEFERRED;")
        .ok()?;
    let kind: String = connection
        .query_row(
            "SELECT type FROM sqlite_master WHERE name='logs'",
            [],
            |r| r.get(0),
        )
        .ok()?;
    let columns: String = connection
        .query_row(
            "SELECT group_concat(name) FROM pragma_index_info('idx_logs_thread_id_ts')",
            [],
            |r| r.get(0),
        )
        .ok()?;
    (kind == "table" && columns == "thread_id,ts,ts_nanos,id").then_some(connection)
}

fn desktop(process: &Process, parent: &Process, ts: i64, nanos: u32, uid: u32) -> bool {
    // Only this installed desktop distribution has been verified locally.
    // Other paths / launchers stay unknown until separately supported.
    process.path
        == "/Applications/ChatGPT.app/Contents/Resources/codex-cli/CodexCLI.app/Contents/MacOS/codex"
        && parent.path == "/Applications/ChatGPT.app/Contents/MacOS/ChatGPT"
        && process.uid == uid
        && parent.uid == uid
        && process.tty == u32::MAX
        && parent.started <= process.started
        && (process.started < ts
            || process.started == ts && process.started_micros <= u64::from(nanos / 1000))
}

fn tui_start(body: &str) -> bool {
    let Some(fields) = body.strip_prefix("tui startup initial frame scheduled ") else {
        return false;
    };
    let mut seen = std::collections::HashSet::new();
    fields.split_whitespace().all(|field| {
        let Some((key, value)) = field.split_once('=') else {
            return false;
        };
        [
            "duration_ms",
            "bootstrap_ms",
            "thread_and_widget_ms",
            "initial_session_ms",
            "event_stream_ms",
        ]
        .contains(&key)
            && seen.insert(key)
            && value.parse::<u64>().is_ok()
    }) && seen.len() == 5
}

fn world_state(body: &str) -> bool {
    let Some((_, event)) = body.rsplit_once("}:") else {
        return false;
    };
    [
        "session_task.run:run_turn:record_context_updates_and_set_reference_context_item:world_state.build: building step world state selected_capability_root_count=",
        "session_task.run:run_turn:world_state.build: building step world state selected_capability_root_count=",
    ]
    .iter()
    .any(|prefix| event.strip_prefix(prefix).is_some_and(|v| v.parse::<u64>().is_ok()))
}

fn tui(
    connection: &Connection,
    runtime: &str,
    process: &Process,
    ts: i64,
    nanos: u32,
    uid: u32,
) -> bool {
    if process.tty == u32::MAX
        || process.uid != uid
        || process.path
            != "/Applications/ChatGPT.app/Contents/Resources/codex-cli/CodexCLI.app/Contents/MacOS/codex"
        || process.started > ts
        || process.started == ts && process.started_micros > u64::from(nanos / 1000)
    {
        return false;
    }
    let index: rusqlite::Result<(String,String)> = connection.query_row(
        "SELECT sql,(SELECT group_concat(name) FROM pragma_index_info('idx_logs_process_uuid_threadless_ts')) FROM sqlite_master WHERE type='index' AND name='idx_logs_process_uuid_threadless_ts'",
        [], |r| Ok((r.get(0)?,r.get(1)?)));
    if !index.is_ok_and(|(sql, columns)| {
        columns == "process_uuid,ts,ts_nanos,id"
            && sql.trim_end().ends_with("WHERE thread_id IS NULL")
    }) {
        return false;
    }
    // This proves the runtime mode, not a Session. Its execution was separately
    // matched by Session / turn / time and (for tools) exact completed call.
    let Ok(mut statement) = connection.prepare("SELECT ts,ts_nanos,target,substr(feedback_log_body,1,512) FROM logs INDEXED BY idx_logs_process_uuid_threadless_ts WHERE process_uuid=?1 AND thread_id IS NULL AND ts BETWEEN ?2 AND ?3 ORDER BY ts ASC,ts_nanos ASC,id ASC LIMIT 257") else { return false; };
    let Ok(rows) = statement.query_map(
        params![runtime, process.started, process.started.saturating_add(30)],
        |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, u32>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
            ))
        },
    ) else {
        return false;
    };
    for (index, row) in rows.enumerate() {
        if index >= 256 {
            return false;
        }
        let Ok((start, subsecond, target, body)) = row else {
            return false;
        };
        if subsecond >= 1_000_000_000 {
            return false;
        }
        if target == "codex_tui::app::startup"
            && tui_start(&body)
            && (start > process.started || u64::from(subsecond / 1000) >= process.started_micros)
            && (start < ts || start == ts && subsecond <= nanos)
        {
            // A later rendering burst cannot undo an already verified startup.
            // This proves mode; unique execution ownership is checked separately.
            return true;
        }
    }
    false
}

fn attribute_cached_with(
    home: &Path,
    activities: &mut [NativeActivity],
    uid: u32,
    evidence: &mut RuntimeEvidence,
    mut inspect: impl FnMut(i32) -> Option<Process>,
) {
    // Mode proof has the lifetime of one verified OS process and native database,
    // never a Session or turn. Every activity still needs fresh execution below.
    use std::os::unix::fs::MetadataExt;
    let database = std::fs::metadata(home.join(".codex/logs_2.sqlite"))
        .ok()
        .map(|m| (m.dev(), m.ino()));
    if evidence.database != database || database.is_none() {
        evidence.tui_modes.clear();
        evidence.database = database;
    }
    evidence.tui_modes.retain(|runtime, (process, parent)| {
        process.uid == uid
            && process_id(runtime).and_then(&mut inspect).as_ref() == Some(process)
            && inspect(process.parent) == *parent
    });
    for a in activities.iter_mut().filter(|a| {
        matches!(
            a.agent_attribution.as_deref(),
            Some("desktop_runtime_execution" | "cli_runtime_execution")
        )
    }) {
        a.agent_product_id = None;
        a.agent_attribution = None;
    }
    let eligible: Vec<_> = activities
        .iter()
        .enumerate()
        .filter_map(|(index, a)| {
            let age = Utc::now()
                .signed_duration_since(DateTime::parse_from_rfc3339(&a.event_at).ok()?)
                .num_milliseconds();
            (a.agent_id == "codex"
                && !a.stale
                && (0..=30_000).contains(&age)
                && ["processing", "thinking", "tool_requested", "tool_returned"]
                    .contains(&a.phase.as_str())
                && !matches!(
                    a.agent_attribution.as_deref(),
                    Some(
                        "conflicting_evidence"
                            | "ambiguous_evidence"
                            | "desktop_tool_execution"
                            | "unrecognized_client"
                    )
                ))
            .then_some(index)
        })
        .take(64)
        .collect();
    if eligible.is_empty() {
        return;
    }
    let Some(connection) = open(home) else {
        evidence.tui_modes.clear();
        return;
    };
    // Force the native index: absent / changed schema must not cause a full-table scan.
    let Ok(mut statement) = connection.prepare("SELECT ts,ts_nanos,target,process_uuid,substr(feedback_log_body,1,4096) FROM logs INDEXED BY idx_logs_thread_id_ts WHERE thread_id=?1 AND ts BETWEEN ?2 AND ?3 AND target IN ('codex_core::stream_events_utils','codex_core::tools::parallel','codex_core::session::turn','codex_core::session::world_state','codex_code_mode::timing') ORDER BY ts DESC,ts_nanos DESC,id DESC LIMIT 257") else { return; };
    for index in eligible {
        let a = &mut activities[index];
        let Some(session) = a
            .session_id
            .strip_prefix("codex:")
            .filter(|id| valid_id(id))
        else {
            continue;
        };
        let Some(turn) = a.turn_id.as_deref().filter(|id| valid_id(id)) else {
            continue;
        };
        let Ok(event) = DateTime::parse_from_rfc3339(&a.event_at) else {
            continue;
        };
        let event = event.timestamp();
        let Ok(rows) = statement.query_map(params![session, event - 5, event + 5], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, u32>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
            ))
        }) else {
            continue;
        };
        let mut candidates = HashMap::new();
        let mut count = 0;
        let mut complete = true;
        for row in rows {
            count += 1;
            let Ok((ts, nanos, target, process_uuid, body)) = row else {
                complete = false;
                break;
            };
            if count > 256 {
                complete = false;
                break;
            }
            if nanos >= 1_000_000_000 {
                complete = false;
                break;
            }
            if scope(&body) != Some((session, turn)) {
                continue;
            }
            if target == "codex_core::session::world_state" && !world_state(&body) {
                continue;
            }
            if a.phase != "processing" {
                let Some(call) = a.call_id.as_deref().filter(|id| valid_id(id)) else {
                    continue;
                };
                if !["codex_core::tools::parallel", "codex_code_mode::timing"]
                    .contains(&target.as_str())
                    || !executed_tool(&body, session, turn, call)
                {
                    continue;
                }
            }
            // Keep even an unrecognised or exited runtime as a competing candidate.
            candidates.insert(process_uuid, (ts, nanos));
        }
        if !complete || candidates.len() != 1 {
            continue;
        }
        let (runtime, (ts, nanos)) = candidates.into_iter().next().unwrap();
        let Some(pid) = process_id(&runtime) else {
            continue;
        };
        let Some(process) = inspect(pid) else {
            continue;
        };
        let parent = inspect(process.parent);
        let identity = if parent
            .as_ref()
            .is_some_and(|p| desktop(&process, p, ts, nanos, uid))
        {
            Some(("codex-desktop", "desktop_runtime_execution"))
        } else if (evidence
            .tui_modes
            .get(&runtime)
            .is_some_and(|(cached, owner)| cached == &process && owner == &parent)
            && (process.started < ts
                || process.started == ts && process.started_micros <= u64::from(nanos / 1000)))
            || tui(&connection, &runtime, &process, ts, nanos, uid)
        {
            Some(("codex-cli", "cli_runtime_execution"))
        } else {
            None
        };
        let Some((product, attribution)) = identity else {
            continue;
        };
        if inspect(pid).as_ref() != Some(&process) || inspect(process.parent) != parent {
            continue;
        }
        if product == "codex-cli" && evidence.tui_modes.len() < 64 {
            evidence.tui_modes.insert(runtime, (process, parent));
        }
        if a.agent_product_id
            .as_deref()
            .is_some_and(|id| id != product)
        {
            a.agent_product_id = None;
            a.agent_attribution = Some("conflicting_evidence".into());
        } else if a.agent_product_id.is_none() {
            a.agent_product_id = Some(product.into());
            a.agent_attribution = Some(attribution.into());
        }
    }
}

#[cfg(target_os = "macos")]
pub(super) fn attribute(
    home: &Path,
    activities: &mut [NativeActivity],
    evidence: &mut RuntimeEvidence,
) {
    // The current uid is used only to restrict read-only runtime evidence.
    attribute_cached_with(
        home,
        activities,
        unsafe { libc::getuid() },
        evidence,
        inspect,
    );
}

#[cfg(test)]
fn attribute_with(
    home: &Path,
    activities: &mut [NativeActivity],
    uid: u32,
    inspect: impl FnMut(i32) -> Option<Process>,
) {
    attribute_cached_with(
        home,
        activities,
        uid,
        &mut RuntimeEvidence::default(),
        inspect,
    );
}

#[cfg(test)]
#[path = "codex_runtime_tests.rs"]
mod tests;
