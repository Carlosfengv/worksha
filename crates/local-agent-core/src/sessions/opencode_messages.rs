//! Recent user-message boundaries, including completed messages. Only bounded
//! metadata and redacted user excerpts are read; tool bodies are never selected.
use super::{
    NativeError, NativeResult, activity_input, live::NativeActivity, opencode, parser::safe_text,
};
use chrono::{DateTime, Utc};
use rusqlite::{Connection, params};
use serde_json::Value;
use std::{collections::HashSet, path::Path};

fn unavailable() -> NativeError {
    NativeError::new("unavailable", "OpenCode 消息活动暂不可读取")
}

pub(super) fn recent(
    db: &Connection,
    now: DateTime<Utc>,
) -> NativeResult<(Vec<NativeActivity>, bool)> {
    let scan_limited: bool = db.query_row("SELECT count(*)>4096 FROM (SELECT id FROM message WHERE session_id IN (SELECT id FROM session WHERE time_updated>=?1 ORDER BY time_updated DESC,id LIMIT 128) LIMIT 4097)", [now.timestamp_millis()-86_400_000], |r|r.get(0)).map_err(|_|unavailable())?;
    let mut q = db.prepare("WITH recent AS MATERIALIZED (SELECT id,session_id,time_created FROM message WHERE session_id IN (SELECT id FROM session WHERE time_updated>=?1 ORDER BY time_updated DESC,id LIMIT 128) ORDER BY time_created DESC,id DESC LIMIT 4097) SELECT m.id,m.session_id,m.time_created,s.title,s.directory FROM recent r JOIN message m ON m.id=r.id JOIN session s ON s.id=m.session_id WHERE length(CAST(m.data AS BLOB))<=8388608 AND CASE WHEN json_valid(m.data) THEN json_extract(m.data,'$.role')='user' AND coalesce(json_extract(m.data,'$.summary'),0)!=1 ELSE 0 END ORDER BY m.time_created DESC,m.id DESC LIMIT 129").map_err(|_|unavailable())?;
    let users = q
        .query_map([now.timestamp_millis() - 86_400_000], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
            ))
        })
        .map_err(|_| unavailable())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| unavailable())?;
    let mut limited = users.len() > 128 || scan_limited;
    let mut seen = HashSet::new();
    let mut rows = Vec::new();
    for (user, session, created, title, directory) in users.into_iter().take(128) {
        if !opencode::valid_id(&user) || !opencode::valid_id(&session) {
            limited = true;
            continue;
        }
        let Some(start) = DateTime::<Utc>::from_timestamp_millis(created).filter(|at| *at <= now)
        else {
            limited = true;
            continue;
        };
        let mut q = db.prepare("SELECT CASE WHEN length(CAST(data AS BLOB))<=8388608 AND json_valid(data) THEN json_object('type',json_extract(data,'$.type'),'text',substr(json_extract(data,'$.text'),1,16384),'synthetic',json_extract(data,'$.synthetic'),'ignored',json_extract(data,'$.ignored')) END FROM part WHERE session_id=?1 AND message_id=?2 ORDER BY time_created,id LIMIT 65").map_err(|_|unavailable())?;
        let parts = q
            .query_map(params![session, user], |r| r.get::<_, Option<String>>(0))
            .map_err(|_| unavailable())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| unavailable())?;
        limited |= parts.len() > 64;
        let has_parts = !parts.is_empty();
        let mut content = Vec::new();
        for raw in parts.into_iter().take(64) {
            let Some(v) = raw.and_then(|s| serde_json::from_str::<Value>(&s).ok()) else {
                limited = true;
                continue;
            };
            if v["synthetic"] != true
                && v["synthetic"] != 1
                && v["ignored"] != true
                && v["ignored"] != 1
            {
                content.push(v);
            }
        }
        if has_parts && content.is_empty() {
            continue;
        }
        let current = seen.insert(session.clone());
        let user_input =
            activity_input::preview(&Value::Array(content)).map(|text| activity_input::UserInput {
                text,
                at: start.to_rfc3339(),
            });
        let mut q = db.prepare("SELECT CASE WHEN length(CAST(data AS BLOB))<=8388608 AND json_valid(data) THEN json_object('model',json_extract(data,'$.modelID'),'completed',json_extract(data,'$.time.completed'),'error',json_type(data,'$.error'),'created',time_created) END FROM message WHERE session_id=?1 AND CASE WHEN json_valid(data) THEN json_extract(data,'$.role')='assistant' AND json_extract(data,'$.parentID')=?2 ELSE 0 END ORDER BY time_created DESC,id DESC LIMIT 1").map_err(|_|unavailable())?;
        let assistant = q
            .query_map(params![session, user], |r| r.get::<_, Option<String>>(0))
            .map_err(|_| unavailable())?
            .next()
            .transpose()
            .map_err(|_| unavailable())?
            .flatten()
            .and_then(|s| serde_json::from_str::<Value>(&s).ok());
        let completed = assistant
            .as_ref()
            .and_then(|a| a["completed"].as_i64())
            .and_then(DateTime::<Utc>::from_timestamp_millis)
            .filter(|at| *at >= start && *at <= now);
        let failed = assistant
            .as_ref()
            .is_some_and(|a| !a["error"].is_null() && a["error"] != "null");
        let event = completed
            .or_else(|| {
                assistant
                    .as_ref()
                    .and_then(|a| a["created"].as_i64())
                    .and_then(DateTime::<Utc>::from_timestamp_millis)
                    .filter(|at| *at >= start && *at <= now)
            })
            .unwrap_or(start);
        let (phase, step) = if failed {
            ("error", "原生调用报告异常；详情见会话")
        } else if completed.is_some() {
            ("waiting_input", "本条消息已完成，等待输入")
        } else if assistant.is_none() {
            ("processing", "收到新的用户消息")
        } else {
            ("processing", "消息已记录，后续状态待更新")
        };
        rows.push(NativeActivity {
            message_usage: None,
            turn_id: Some(user),
            #[cfg(any(target_os = "macos", test))]
            call_id: None,
            agent_product_id: Some("opencode".into()),
            agent_attribution: Some("native_record".into()),
            id: format!("opencode:{session}"),
            session_id: format!("opencode:{session}"),
            agent_id: "opencode".into(),
            title: safe_text(&title, 200).0,
            project: Path::new(&directory)
                .file_name()
                .and_then(|s| s.to_str())
                .map(|s| safe_text(s, 100).0),
            model: assistant
                .as_ref()
                .and_then(|a| a["model"].as_str())
                .map(|s| safe_text(s, 300).0),
            phase: phase.into(),
            step: step.into(),
            user_input,
            tool_names: vec![],
            message_started_at: Some(start.to_rfc3339()),
            event_at: event.to_rfc3339(),
            turn_ttft_ms: None,
            realtime: None,
            stale: phase == "processing"
                && (!current
                    || assistant.is_some()
                    || now.signed_duration_since(event).num_seconds() > 30),
        });
    }
    Ok((rows, limited))
}
