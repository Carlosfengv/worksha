//! Recent user-message boundaries, including completed messages. Only bounded
//! metadata and redacted user excerpts are read; tool bodies are never selected.
use super::{
    NativeError, NativeResult, NativeSessions, activity_input, live::NativeActivity, opencode,
    parser::safe_text,
};
use chrono::{DateTime, Utc};
use rusqlite::{Connection, params};
use serde_json::Value;
use std::{collections::HashSet, path::Path};

fn unavailable() -> NativeError {
    NativeError::new("unavailable", "OpenCode 消息活动暂不可读取")
}

pub struct RoutingBoundaries {
    pub ends: Vec<Vec<usize>>,
    pub v2: bool,
    pub authority: (u64, u64),
}

impl NativeSessions {
    /// Locate native user boundaries inside the Messages transport's text groups.
    /// Only partition positions and the verified source format leave the reader.
    pub fn opencode_routing_boundaries(
        &self,
        session: &str,
        groups: &[Vec<String>],
    ) -> NativeResult<Option<RoutingBoundaries>> {
        if !session.starts_with("ses_") || !opencode::valid_id(session) {
            return Ok(None);
        }
        self.with_source_context("opencode", None, true, |_, context| {
            let authority = context.authorization_identity();
            let db = opencode::open_database(context.home)?;
            if opencode::has_table(&db, "session_v2")?
                && db
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM session_v2 WHERE id=?1)",
                        [session],
                        |row| row.get::<_, bool>(0),
                    )
                    .map_err(|_| unavailable())?
            {
                if !opencode::has_table(&db, "session_message")? {
                    return Ok(None);
                }
                return v2_routing_boundaries(&db, session, groups).map(|proof| {
                    proof.map(|ends| RoutingBoundaries {
                        ends,
                        v2: true,
                        authority,
                    })
                });
            }
            if !groups.iter().any(|group| group.len() > 1) || !opencode::has_legacy_tables(&db)? {
                return Ok(None);
            }
            routing_boundaries(&db, session, groups).map(|proof| {
                proof.map(|ends| RoutingBoundaries {
                    ends,
                    v2: false,
                    authority,
                })
            })
        })
    }

    pub fn opencode_routing_authorized(&self, authority: (u64, u64)) -> bool {
        self.with_source_context("opencode", Some(authority.0), true, |_, context| {
            Ok(context.authorization_identity() == authority)
        })
        .unwrap_or(false)
    }
}

fn v2_routing_boundaries(
    db: &Connection,
    session: &str,
    groups: &[Vec<String>],
) -> NativeResult<Option<Vec<Vec<usize>>>> {
    let mut query = db.prepare(
        "SELECT CASE WHEN length(CAST(data AS BLOB))<=524288 AND json_valid(data) THEN
          CASE WHEN json_type(data,'$.text')='text'
            AND (json_type(data,'$.files') IS NULL OR (json_type(data,'$.files')='array' AND json_array_length(data,'$.files')=0))
            AND (json_type(data,'$.agents') IS NULL OR (json_type(data,'$.agents')='array' AND json_array_length(data,'$.agents')=0))
            AND (json_type(data,'$.skills') IS NULL OR (json_type(data,'$.skills')='array' AND json_array_length(data,'$.skills')=0))
          THEN json_extract(data,'$.text') END END
         FROM session_message WHERE session_id=?1 AND type='user' ORDER BY seq LIMIT 257"
    ).map_err(|_| unavailable())?;
    let mut rows = query.query([session]).map_err(|_| unavailable())?;
    for (index, text) in groups.iter().flatten().enumerate() {
        if index >= 256 || text.is_empty() {
            return Ok(None);
        }
        let Some(row) = rows.next().map_err(|_| unavailable())? else {
            return Ok(None);
        };
        let native: Option<String> = row.get(0).map_err(|_| unavailable())?;
        if native.as_deref() != Some(text.as_str()) {
            return Ok(None);
        }
    }
    if rows.next().map_err(|_| unavailable())?.is_some() {
        return Ok(None);
    }
    Ok(Some(
        groups
            .iter()
            .map(|group| (1..=group.len()).collect())
            .collect(),
    ))
}

fn routing_boundaries(
    db: &Connection,
    session: &str,
    groups: &[Vec<String>],
) -> NativeResult<Option<Vec<Vec<usize>>>> {
    let mut users = db
        .prepare(
            "SELECT id FROM message WHERE session_id=?1 AND
        CASE WHEN length(CAST(data AS BLOB))<=524288 AND json_valid(data)
        THEN json_extract(data,'$.role')='user' ELSE 0 END ORDER BY time_created,id LIMIT 257",
        )
        .map_err(|_| unavailable())?;
    let users = users
        .query_map([session], |r| r.get::<_, String>(0))
        .map_err(|_| unavailable())?;
    let mut parts = db
        .prepare(
            "SELECT CASE WHEN length(CAST(data AS BLOB))<=524288
        THEN data END FROM part WHERE session_id=?1 AND message_id=?2
        ORDER BY time_created,id LIMIT 65",
        )
        .map_err(|_| unavailable())?;
    let mut boundaries = vec![Vec::new(); groups.len()];
    let (mut group, mut offset) = (0, 0);
    let mut bytes = 0;
    for (user_index, id) in users.enumerate() {
        if user_index >= 256 {
            return Ok(None);
        }
        let id = id.map_err(|_| unavailable())?;
        let mut rows = parts
            .query(params![session, id])
            .map_err(|_| unavailable())?;
        let mut count = 0;
        let start = offset;
        while let Some(row) = rows.next().map_err(|_| unavailable())? {
            count += 1;
            let Some(raw): Option<String> = row.get(0).map_err(|_| unavailable())? else {
                return Ok(None);
            };
            bytes += raw.len();
            if count > 64 || bytes > 1024 * 1024 {
                return Ok(None);
            }
            let part: Value = serde_json::from_str(&raw).map_err(|_| unavailable())?;
            if part["type"] != "text" {
                return Ok(None);
            }
            let Some(text) = part["text"].as_str() else {
                return Ok(None);
            };
            if text.is_empty() || part["ignored"] == true {
                continue;
            }
            if groups
                .get(group)
                .and_then(|g| g.get(offset))
                .map(String::as_str)
                != Some(text)
            {
                return Ok(None);
            }
            offset += 1;
        }
        if offset != start {
            boundaries[group].push(offset);
            if offset == groups[group].len() {
                group += 1;
                offset = 0;
            }
        }
    }
    Ok((group == groups.len()).then_some(boundaries))
}

pub(super) fn recent(
    db: &Connection,
    now: DateTime<Utc>,
    exclude_migrated: bool,
) -> NativeResult<(Vec<NativeActivity>, bool)> {
    let legacy = super::opencode_live::legacy_session_filter(exclude_migrated);
    let scan_limited: bool = db.query_row(&format!("SELECT count(*)>4096 FROM (SELECT id FROM message WHERE session_id IN (SELECT id FROM session WHERE time_updated>=?1 {legacy} ORDER BY time_updated DESC,id LIMIT 128) LIMIT 4097)"), [now.timestamp_millis()-86_400_000], |r|r.get(0)).map_err(|_|unavailable())?;
    let mut q = db.prepare(&format!("WITH recent AS MATERIALIZED (SELECT id,session_id,time_created FROM message WHERE session_id IN (SELECT id FROM session WHERE time_updated>=?1 {legacy} ORDER BY time_updated DESC,id DESC LIMIT 128) ORDER BY time_created DESC,id DESC LIMIT 4097) SELECT m.id,m.session_id,m.time_created,s.title,s.directory FROM recent r JOIN message m ON m.id=r.id JOIN session s ON s.id=m.session_id WHERE length(CAST(m.data AS BLOB))<=8388608 AND CASE WHEN json_valid(m.data) THEN json_extract(m.data,'$.role')='user' AND coalesce(json_extract(m.data,'$.summary'),0)!=1 ELSE 0 END ORDER BY m.time_created DESC,m.id DESC LIMIT 129")).map_err(|_|unavailable())?;
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
            agent_product_id: None,
            agent_attribution: None,
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
