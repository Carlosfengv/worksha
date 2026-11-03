//! Fresh, bounded SQLite metadata for tools executing between model requests.
//! Selects tool metadata and a bounded redacted excerpt of the latest user input.
use super::{NativeError, NativeResult, live::NativeActivity, opencode, parser::safe_text};
use chrono::{DateTime, Utc};
use rusqlite::params;
use serde_json::Value;
use std::path::Path;

fn unavailable() -> NativeError {
    NativeError::new("unavailable", "OpenCode 工具活动暂不可读取")
}
#[cfg(test)]
pub(super) fn activities(
    home: &Path,
    now: DateTime<Utc>,
) -> NativeResult<(Vec<NativeActivity>, bool)> {
    let db = opencode::open(home)?;
    live(&db, now)
}
pub(super) fn messages(
    home: &Path,
    now: DateTime<Utc>,
) -> NativeResult<(Vec<NativeActivity>, bool)> {
    let db = opencode::open(home)?;
    let (mut rows, limited) = super::opencode_messages::recent(&db, now)?;
    let (active, live_limited) = live(&db, now)?;
    for activity in active {
        rows.retain(|old| old.session_id != activity.session_id || old.turn_id != activity.turn_id);
        rows.insert(0, activity);
    }
    Ok((rows, limited || live_limited))
}
fn live(
    db: &rusqlite::Connection,
    now: DateTime<Utc>,
) -> NativeResult<(Vec<NativeActivity>, bool)> {
    let mut query = db.prepare("SELECT id,title,directory FROM session WHERE time_updated>=?1 ORDER BY time_updated DESC,id LIMIT 129").map_err(|_| unavailable())?;
    let sessions = query
        .query_map([now.timestamp_millis() - 86_400_000], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })
        .map_err(|_| unavailable())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| unavailable())?;
    let mut limited = sessions.len() > 128;
    let mut result = vec![];
    for (session, title, directory) in sessions.into_iter().take(128) {
        if !opencode::valid_id(&session) {
            limited = true;
            continue;
        }
        // The latest user message is required. An older pending tool cannot
        // reactivate after a new turn, completion, cancellation or an error.
        let mut q = db.prepare("SELECT id,CASE WHEN length(CAST(data AS BLOB))<=8388608 AND json_valid(data) THEN json_object('role',json_extract(data,'$.role'),'parentID',json_extract(data,'$.parentID'),'completed',json_extract(data,'$.time.completed'),'error',json_type(data,'$.error'),'model',json_extract(data,'$.modelID'),'createdAt',time_created) END FROM message WHERE session_id=?1 ORDER BY time_created DESC,id DESC LIMIT 65").map_err(|_| unavailable())?;
        let messages = q
            .query_map([&session], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?))
            })
            .map_err(|_| unavailable())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| unavailable())?;
        let parsed: Option<Vec<(String, Value)>> = messages
            .into_iter()
            .take(64)
            .map(|(id, raw)| Some((id, serde_json::from_str(&raw?).ok()?)))
            .collect();
        let Some(messages) = parsed else {
            limited = true;
            continue;
        };
        let Some(user) = messages
            .iter()
            .find(|(_, m)| m["role"] == "user")
            .map(|(id, _)| id)
        else {
            limited |= messages.len() == 64;
            continue;
        };
        let message_started_at = messages
            .iter()
            .find(|(id, _)| id == user)
            .and_then(|(_, message)| message["createdAt"].as_i64())
            .and_then(DateTime::<Utc>::from_timestamp_millis)
            .filter(|at| *at <= now)
            .map(|at| at.to_rfc3339());
        let Some((assistant_id, assistant)) = messages
            .iter()
            .take_while(|(id, _)| id != user)
            .find(|(_, m)| m["role"] == "assistant")
        else {
            continue;
        };
        if assistant["parentID"].as_str() != Some(user)
            || !assistant["completed"].is_null()
            || !(assistant["error"].is_null() || assistant["error"] == "null")
        {
            continue;
        }
        let mut q = db.prepare("SELECT id,message_id,time_created,CASE WHEN length(CAST(data AS BLOB))<=8388608 AND json_valid(data) THEN json_object('type',json_extract(data,'$.type'),'tool',json_extract(data,'$.tool'),'status',json_extract(data,'$.state.status'),'start',json_extract(data,'$.state.time.start'),'end',json_extract(data,'$.state.time.end'),'reasoningStart',json_extract(data,'$.time.start'),'reasoningEnd',json_extract(data,'$.time.end')) END FROM part WHERE session_id=?1 AND time_updated>=?2 ORDER BY time_updated DESC,id DESC LIMIT 129").map_err(|_| unavailable())?;
        let parts = q
            .query_map(params![session, now.timestamp_millis() - 30_000], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, Option<String>>(3)?,
                ))
            })
            .map_err(|_| unavailable())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| unavailable())?;
        limited |= parts.len() > 128;
        let mut latest: Option<NativeActivity> = None;
        let mut active_names = Vec::new();
        for (id, message, created, raw) in parts.into_iter().take(128) {
            if &message != assistant_id {
                continue;
            }
            let Some(v) = raw.and_then(|s| serde_json::from_str::<Value>(&s).ok()) else {
                limited = true;
                continue;
            };
            let tool = v["type"] == "tool";
            if !opencode::valid_id(&id)
                || !matches!(v["type"].as_str(), Some("tool" | "reasoning"))
                || (tool
                    && (!v["end"].is_null()
                        || !matches!(v["status"].as_str(), Some("pending" | "running"))))
                || (!tool && !v["reasoningEnd"].is_null())
            {
                continue;
            }
            let start = if !tool {
                v["reasoningStart"].as_i64()
            } else {
                match v["status"].as_str() {
                    Some("running") => v["start"].as_i64(),
                    _ => Some(created),
                }
            };
            let Some(at) = start
                .and_then(DateTime::<Utc>::from_timestamp_millis)
                .filter(|at| {
                    (0..=30_000).contains(&now.signed_duration_since(*at).num_milliseconds())
                })
            else {
                continue;
            };
            let name = v["tool"]
                .as_str()
                .map(|n| safe_text(n, 100).0)
                .unwrap_or_else(|| "未知工具".into());
            if tool && !active_names.contains(&name) && active_names.len() < 8 {
                active_names.push(name.clone());
            }
            let a = NativeActivity {
                message_usage: None,
                turn_id: Some(user.clone()),
                #[cfg(any(target_os = "macos", test))]
                call_id: Some(id),
                agent_product_id: Some("opencode".into()),
                agent_attribution: Some("native_record".into()),
                id: format!("opencode:{session}"),
                agent_id: "opencode".into(),
                session_id: format!("opencode:{session}"),
                title: safe_text(&title, 200).0,
                project: Path::new(&directory)
                    .file_name()
                    .and_then(|p| p.to_str())
                    .map(|p| safe_text(p, 100).0),
                model: assistant["model"].as_str().map(|m| safe_text(m, 300).0),
                phase: if tool { "tool_requested" } else { "thinking" }.into(),
                step: if tool {
                    format!(
                        "{}：{name}",
                        if v["status"] == "running" {
                            "正在执行工具"
                        } else {
                            "已提出工具调用"
                        }
                    )
                } else {
                    "收到原生思考进度".into()
                },
                user_input: None,
                tool_names: if tool { vec![name] } else { vec![] },
                message_started_at: message_started_at.clone(),
                event_at: at.to_rfc3339(),
                turn_ttft_ms: None,
                realtime: None,
                stale: false,
            };
            if latest.as_ref().is_none_or(|old| old.event_at < a.event_at) {
                latest = Some(a);
            }
        }
        if let Some(activity) = latest.as_mut() {
            if activity.phase == "tool_requested" {
                activity.tool_names = active_names;
                if activity.tool_names.len() > 1 {
                    activity
                        .step
                        .push_str(&format!(" · 并行工具：{}", activity.tool_names.join("、")));
                }
            }
            let mut q = db.prepare("SELECT time_created,CASE WHEN length(CAST(data AS BLOB))<=8388608 AND json_valid(data) THEN json_object('type',json_extract(data,'$.type'),'text',substr(json_extract(data,'$.text'),1,16384),'synthetic',json_extract(data,'$.synthetic'),'ignored',json_extract(data,'$.ignored')) END FROM part WHERE session_id=?1 AND message_id=?2 ORDER BY time_created,id LIMIT 64").map_err(|_|unavailable())?;
            let parts = q
                .query_map(params![session, user], |r| {
                    Ok((r.get::<_, i64>(0)?, r.get::<_, Option<String>>(1)?))
                })
                .map_err(|_| unavailable())?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|_| unavailable())?;
            let at = parts
                .first()
                .and_then(|(at, _)| DateTime::<Utc>::from_timestamp_millis(*at))
                .filter(|at| *at <= now)
                .map(|at| at.to_rfc3339());
            let parts = parts
                .into_iter()
                .filter_map(|(_, raw)| serde_json::from_str::<Value>(&raw?).ok())
                // SQLite projects JSON booleans as 0/1 in json_object.
                .filter(|p| {
                    p["synthetic"] != true
                        && p["synthetic"] != 1
                        && p["ignored"] != true
                        && p["ignored"] != 1
                })
                .collect();
            activity.user_input = super::activity_input::preview(&Value::Array(parts))
                .zip(at)
                .map(|(text, at)| super::activity_input::UserInput { text, at });
        }
        result.extend(latest);
    }
    Ok((result, limited))
}
