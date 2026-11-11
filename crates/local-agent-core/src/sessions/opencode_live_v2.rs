//! Recent OpenCode 2.x user messages and persisted execution milestones.
use super::{
    DateTime, NativeActivity, NativeResult, Utc, Value, opencode, params, safe_text, unavailable,
};
use crate::sessions::activity_input::{self, UserInput};
use rusqlite::Connection;
use serde_json::json;
use std::path::Path;

const DAY_MS: i64 = 86_400_000;
const ACTIVE_MS: i64 = 30_000;

struct Message {
    id: String,
    kind: String,
    at: DateTime<Utc>,
    meta: Value,
}

struct PartSignal {
    phase: &'static str,
    at: DateTime<Utc>,
    ordinal: usize,
    name: String,
    status: String,
}

const MESSAGES: &str = "
    SELECT id,type,time_created,CASE WHEN length(CAST(data AS BLOB))<=8388608 AND json_valid(data) THEN
        json_object('text',CASE WHEN type='user' AND json_type(data,'$.text')='text'
             THEN substr(json_extract(data,'$.text'),1,16384) END,
            'model',CASE WHEN type='assistant' THEN substr(json_extract(data,'$.model.id'),1,300) END,
            'completed',json_extract(data,'$.time.completed'),
            'error',json_type(data,'$.error') IS NOT NULL AND json_type(data,'$.error')!='null',
            'outcome',CASE WHEN type='idle' THEN json_extract(data,'$.outcome') END)
        END
    FROM session_message WHERE session_id=?1 AND time_created>=?2 AND type IN ('user','assistant','idle')
    ORDER BY seq DESC LIMIT 65";

pub(super) fn messages(
    db: &Connection,
    now: DateTime<Utc>,
) -> NativeResult<(Vec<NativeActivity>, bool)> {
    let from = now.timestamp_millis() - DAY_MS;
    let split = opencode::has_table(db, "session_part")?;
    let mut sessions = db
        .prepare(
            "SELECT id,substr(title,1,200),substr(directory,1,2048),
             CASE WHEN fork_session_id IS NOT NULL THEN time_created END
         FROM session_v2 WHERE time_updated>=?1 ORDER BY time_updated DESC,id LIMIT 129",
        )
        .map_err(|_| unavailable())?;
    let sessions = sessions
        .query_map([from], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<i64>>(3)?,
            ))
        })
        .map_err(|_| unavailable())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| unavailable())?;
    let mut limited = sessions.len() > 128;
    let mut result = Vec::new();
    let mut query = db.prepare_cached(MESSAGES).map_err(|_| unavailable())?;
    for (session, title, directory, fork_at) in sessions.into_iter().take(128) {
        if !opencode::valid_id(&session) {
            limited = true;
            continue;
        }
        let mut rows = query
            .query(params![session, fork_at.map_or(from, |at| at.max(from))])
            .map_err(|_| unavailable())?;
        let mut messages = Vec::new();
        for _ in 0..64 {
            let Some(row) = rows.next().map_err(|_| unavailable())? else {
                break;
            };
            let id: String = row.get(0).map_err(|_| unavailable())?;
            let kind: String = row.get(1).map_err(|_| unavailable())?;
            let at: i64 = row.get(2).map_err(|_| unavailable())?;
            let raw: Option<String> = row.get(3).map_err(|_| unavailable())?;
            let Some(at) = DateTime::<Utc>::from_timestamp_millis(at).filter(|at| *at <= now)
            else {
                limited = true;
                break;
            };
            let Some(meta) = raw
                .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
                .filter(|_| opencode::valid_id(&id))
            else {
                // A damaged boundary cannot authorize showing a prior tool as current.
                limited = true;
                break;
            };
            messages.push(Message { id, kind, at, meta });
        }
        limited |= rows.next().map_err(|_| unavailable())?.is_some();
        if !messages.iter().any(|row| row.kind == "user") {
            continue;
        }
        let info = json!({"title":title});
        let title = if let Some(title) = opencode::native_title(&info) {
            safe_text(title, 200).0
        } else {
            opencode::v2::first_user_title(db, &session)?.unwrap_or_else(|| "未命名会话".into())
        };
        let project = Path::new(&directory)
            .file_name()
            .and_then(|value| value.to_str())
            .map(|name| safe_text(name, 100).0);
        let session_id = format!("opencode:{session}");
        let mut turn: Option<(NativeActivity, Option<&Message>)> = None;
        for message in messages.iter().rev() {
            if message.kind == "user" {
                if let Some((mut prior, _)) = turn.take() {
                    prior.stale = prior.phase == "processing";
                    result.push(prior);
                }
                let at = message.at.to_rfc3339();
                turn = Some((
                    NativeActivity {
                        message_usage: None,
                        turn_id: Some(message.id.clone()),
                        #[cfg(any(target_os = "macos", test))]
                        call_id: None,
                        agent_product_id: None,
                        agent_attribution: None,
                        id: session_id.clone(),
                        agent_id: "opencode".into(),
                        session_id: session_id.clone(),
                        title: title.clone(),
                        project: project.clone(),
                        model: None,
                        phase: "processing".into(),
                        step: "收到新的用户消息".into(),
                        user_input: activity_input::preview(&message.meta["text"]).map(|text| {
                            UserInput {
                                text,
                                at: at.clone(),
                            }
                        }),
                        tool_names: vec![],
                        message_started_at: Some(at.clone()),
                        event_at: at,
                        turn_ttft_ms: None,
                        realtime: None,
                        stale: !fresh(message.at, now),
                    },
                    None,
                ));
            } else if message.kind == "idle" {
                if let Some((mut activity, _)) = turn.take() {
                    let (phase, step) = match message.meta["outcome"].as_str() {
                        Some("succeeded") => ("waiting_input", "本条消息已完成，等待输入"),
                        Some("failed") => ("error", "原生调用报告异常；详情见会话"),
                        Some("interrupted") => ("cancelled", "原生会话已中断"),
                        _ => {
                            limited = true;
                            ("processing", "原生会话结束状态未知")
                        }
                    };
                    activity.phase = phase.into();
                    activity.step = step.into();
                    activity.event_at = message.at.to_rfc3339();
                    activity.stale = phase == "processing";
                    result.push(activity);
                }
            } else if let Some((activity, assistant)) = &mut turn {
                *assistant = Some(message);
                activity.model = message.meta["model"]
                    .as_str()
                    .map(|model| safe_text(model, 300).0);
                let completed = timestamp(&message.meta["completed"])
                    .filter(|at| *at >= message.at && *at <= now);
                let failed = message.meta["error"] == 1;
                let at = completed.unwrap_or(message.at);
                activity.event_at = at.to_rfc3339();
                activity.phase = if failed { "error" } else { "processing" }.into();
                activity.step = if failed {
                    "原生调用报告异常；详情见会话"
                } else {
                    "消息已记录，后续状态待更新"
                }
                .into();
                activity.stale = !failed && !fresh(at, now);
            }
        }
        if let Some((mut activity, assistant)) = turn {
            if activity.phase == "processing"
                && let Some(assistant) = assistant
            {
                limited |= apply_parts(db, &session, assistant, split, now, &mut activity)?;
            }
            result.push(activity);
        }
    }
    result.sort_by(|a, b| {
        b.event_at
            .cmp(&a.event_at)
            .then_with(|| a.turn_id.cmp(&b.turn_id))
    });
    limited |= result.len() > 128;
    result.truncate(128);
    Ok((result, limited))
}

const SPLIT_PARTS: &str = "
    SELECT type AS kind,
        CASE WHEN length(CAST(data AS BLOB))<=8388608 AND json_valid(data) THEN data END AS payload
    FROM session_part WHERE session_id=?1 AND message_id=?2 ORDER BY ordinal LIMIT 129";

const INLINE_PARTS: &str = "
    SELECT CASE WHEN part.type='object' THEN json_extract(part.value,'$.type') END AS kind,
        CASE WHEN part.type='object' THEN part.value END AS payload
    FROM session_message message JOIN json_each(
        CASE WHEN length(CAST(message.data AS BLOB))<=8388608 AND json_valid(message.data) THEN
            CASE WHEN json_type(message.data,'$.content')='array' THEN json_extract(message.data,'$.content') END END
    ) part WHERE message.session_id=?1 AND message.id=?2 ORDER BY part.key LIMIT 129";

fn apply_parts(
    db: &Connection,
    session: &str,
    assistant: &Message,
    split: bool,
    now: DateTime<Utc>,
    activity: &mut NativeActivity,
) -> NativeResult<bool> {
    let mut active: Option<PartSignal> = None;
    let mut returned = None;
    let mut names = Vec::new();
    let mut limited = false;
    let mut index = 0;
    for source in [SPLIT_PARTS, INLINE_PARTS] {
        if source == SPLIT_PARTS && !split {
            continue;
        }
        let sql = format!("SELECT kind,CASE WHEN json_type(payload)='object' THEN
             json_object('name',substr(json_extract(payload,'$.name'),1,100),'status',json_extract(payload,'$.state.status'),
             'created',json_extract(payload,'$.time.created'),'ran',json_extract(payload,'$.time.ran'),
             'completed',json_extract(payload,'$.time.completed')) END FROM ({source})");
        let mut query = db.prepare_cached(&sql).map_err(|_| unavailable())?;
        let mut parts = query
            .query(params![session, assistant.id])
            .map_err(|_| unavailable())?;
        while let Some(row) = parts.next().map_err(|_| unavailable())? {
            let ordinal = index;
            index += 1;
            if index > 128 {
                limited = true;
                break;
            }
            let kind: Option<String> = row.get(0).map_err(|_| unavailable())?;
            let raw: Option<String> = row.get(1).map_err(|_| unavailable())?;
            let Some(kind) = kind else {
                limited = true;
                continue;
            };
            if !matches!(kind.as_str(), "tool" | "reasoning") {
                continue;
            }
            let Some(meta) = raw.and_then(|raw| serde_json::from_str::<Value>(&raw).ok()) else {
                limited = true;
                continue;
            };
            let tool = kind == "tool";
            let status = meta["status"].as_str().unwrap_or("");
            let ended = !meta["completed"].is_null();
            let assistant_open = assistant.meta["completed"].is_null();
            let (phase, at) = match (tool, status) {
                (true, "running") if assistant_open && !ended => {
                    ("tool_requested", timestamp(&meta["ran"]))
                }
                (true, "streaming") if assistant_open && !ended => {
                    ("tool_requested", timestamp(&meta["created"]))
                }
                (true, "completed" | "error") => ("tool_returned", timestamp(&meta["completed"])),
                (false, _) if assistant_open && !ended => ("thinking", timestamp(&meta["created"])),
                _ => continue,
            };
            let Some(at) = at.filter(|at| *at >= assistant.at && fresh(*at, now)) else {
                continue;
            };
            let name = meta["name"]
                .as_str()
                .filter(|name| !name.is_empty())
                .map(|name| safe_text(name, 100).0)
                .unwrap_or_else(|| "未知工具".into());
            if phase == "tool_requested" && names.len() < 8 && !names.contains(&name) {
                names.push(name.clone());
            }
            let slot = if phase == "tool_returned" {
                &mut returned
            } else {
                &mut active
            };
            if slot.as_ref().is_none_or(|latest| latest.at < at) {
                *slot = Some(PartSignal {
                    phase,
                    at,
                    ordinal,
                    name,
                    status: status.into(),
                });
            }
        }
    }
    if let Some(PartSignal {
        phase,
        at,
        ordinal,
        name,
        status,
    }) = active.or(returned)
    {
        activity.phase = phase.into();
        activity.event_at = at.to_rfc3339();
        activity.stale = false;
        #[cfg(any(target_os = "macos", test))]
        {
            activity.call_id = Some(format!("{}_part_{ordinal}", assistant.id));
        }
        let _ = ordinal;
        activity.step = match phase {
            "tool_requested" if status == "running" => format!("正在执行工具：{name}"),
            "tool_requested" => format!("已提出工具调用：{name}"),
            "tool_returned" if status == "error" => format!("工具报告异常：{name}"),
            "tool_returned" => format!("工具已返回：{name}"),
            _ => "收到原生思考进度".into(),
        };
        activity.tool_names = match phase {
            "tool_requested" => names,
            "tool_returned" => vec![name],
            _ => Vec::new(),
        };
    }
    Ok(limited)
}

fn timestamp(value: &Value) -> Option<DateTime<Utc>> {
    value
        .as_i64()
        .and_then(DateTime::<Utc>::from_timestamp_millis)
}

fn fresh(at: DateTime<Utc>, now: DateTime<Utc>) -> bool {
    (0..=ACTIVE_MS).contains(&now.signed_duration_since(at).num_milliseconds())
}
