//! Normalize OpenCode 2.x rows into the existing read-only history model.
use super::{
    Format, History, MAX_RECORD, MAX_ROWS, MAX_SESSIONS, NativeResult, Record, has_table,
    unavailable, valid_id,
};
use rusqlite::{Connection, params};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::HashMap;

pub(super) const REVISION: [&str; 2] = [
    "SELECT id||':'||seq||':'||type,time_created,time_updated,length(CAST(data AS BLOB))
     FROM session_message WHERE session_id=?1 ORDER BY seq LIMIT 100001",
    "SELECT message_id||':'||ordinal||':'||type,time_created,time_updated,length(CAST(data AS BLOB))
     FROM session_part WHERE session_id=?1 ORDER BY message_id,ordinal LIMIT 100001",
];

pub(super) fn format(db: &Connection) -> NativeResult<Option<Format>> {
    if !has_table(db, "session_v2")? || !has_table(db, "session_message")? {
        return Ok(None);
    }
    Ok(Some(Format::V2 {
        part_rows: has_table(db, "session_part")?,
    }))
}

pub(super) fn contains(db: &Connection, id: &str) -> NativeResult<bool> {
    db.query_row(
        "SELECT EXISTS(SELECT 1 FROM session_v2 WHERE id=?1)",
        [id],
        |row| row.get(0),
    )
    .map_err(|_| unavailable())
}

pub(super) fn infos(db: &Connection, id: Option<&str>) -> NativeResult<Vec<Value>> {
    let select = "SELECT json_object('id',CASE WHEN length(CAST(id AS BLOB))<=256 THEN id END,
        'title',substr(title,1,2000),'directory',CASE WHEN length(CAST(directory AS BLOB))<=2048 THEN directory END,
        'version',substr(version,1,100),'created',time_created,'updated',time_updated,
        'fork',fork_session_id IS NOT NULL) FROM session_v2";
    let suffix = if id.is_some() {
        "WHERE id=?1 LIMIT ?2"
    } else {
        "WHERE ?1 IS NULL ORDER BY time_updated DESC,id LIMIT ?2"
    };
    db.prepare(&format!("{select} {suffix}"))
        .map_err(|_| unavailable())?
        .query_map(params![id, (MAX_SESSIONS + 1) as i64], |row| {
            row.get::<_, String>(0)
        })
        .map_err(|_| unavailable())?
        .map(|row| {
            serde_json::from_str(&row.map_err(|_| unavailable())?).map_err(|_| unavailable())
        })
        .collect()
}

pub(in crate::sessions) fn first_user_title(
    db: &Connection,
    id: &str,
) -> NativeResult<Option<String>> {
    let mut query = db.prepare_cached(
        "SELECT CASE WHEN length(CAST(data AS BLOB))<=?2 AND json_valid(data) THEN
            CASE WHEN json_type(data,'$.text')='text' THEN substr(json_extract(data,'$.text'),1,16384) END
         END FROM session_message WHERE session_id=?1 AND type='user' ORDER BY seq LIMIT 64",
    ).map_err(|_| unavailable())?;
    let mut rows = query
        .query(params![id, MAX_RECORD as i64])
        .map_err(|_| unavailable())?;
    while let Some(row) = rows.next().map_err(|_| unavailable())? {
        if let Some(text) = row.get::<_, Option<String>>(0).map_err(|_| unavailable())?
            && let Some(title) = crate::sessions::activity_input::preview(&Value::String(text))
        {
            return Ok(Some(title));
        }
    }
    Ok(None)
}

const MESSAGES: &str = "
    SELECT id,type,time_created,CASE WHEN length(CAST(data AS BLOB))<=?2 AND json_valid(data)
      AND json_type(data)='object' THEN
        json_object('modelID',substr(json_extract(data,'$.model.id'),1,300),
            'providerID',substr(json_extract(data,'$.model.providerID'),1,300),
            'time',json_object('completed',json_extract(data,'$.time.completed')),
            'finish',json_extract(data,'$.finish'),
            'status',json_extract(data,'$.status'),
            'error',CASE WHEN json_type(data,'$.error')='object' THEN
                json_object('name',substr(json_extract(data,'$.error.type'),1,160),
                    'status',json_extract(data,'$.error.status'),
                    'message',CASE WHEN ?3 THEN substr(json_extract(data,'$.error.message'),1,4097) END)
                END,
            'tokens',CASE WHEN json_type(data,'$.tokens')='object' THEN json_object(
                'input',json_extract(data,'$.tokens.input'),'output',json_extract(data,'$.tokens.output'),
                'reasoning',json_extract(data,'$.tokens.reasoning'),
                'cache',json_object('read',json_extract(data,'$.tokens.cache.read'),
                    'write',json_extract(data,'$.tokens.cache.write'))) END,
            'files',json_array_length(data,'$.files'),
            'text',CASE WHEN ?3 AND type IN ('user','synthetic','skill')
                AND json_type(data,'$.text')='text' THEN substr(json_extract(data,'$.text'),1,65537)
                WHEN ?3 AND type='compaction' AND json_type(data,'$.summary')='text'
                THEN substr(json_extract(data,'$.summary'),1,65537) END,
            'contentValid',json_type(data,'$.content')='array')
      END
    FROM session_message WHERE session_id=?1 ORDER BY seq LIMIT 100001";

const SPLIT_PARTS: &str = "
    SELECT part.message_id,part.ordinal,part.time_created,part.type,
        CASE WHEN length(CAST(part.data AS BLOB))<=?2 AND json_valid(part.data) THEN part.data END
    FROM session_part part JOIN session_message message
      ON message.session_id=part.session_id AND message.id=part.message_id AND message.type='assistant'
    WHERE part.session_id=?1 ORDER BY part.message_id,part.ordinal LIMIT 100001";

const INLINE_PARTS: &str = "
    SELECT message.id,part.key,message.time_created,
      CASE WHEN part.type='object' THEN json_extract(part.value,'$.type') END,
      CASE WHEN part.type='object' THEN part.value END
    FROM session_message message JOIN json_each(
        CASE WHEN length(CAST(message.data AS BLOB))<=?2 AND json_valid(message.data)
        THEN CASE WHEN json_type(message.data,'$.content')='array' THEN json_extract(message.data,'$.content') END END
    ) part
    WHERE message.session_id=?1 AND message.type='assistant'
    ORDER BY message.seq,part.key LIMIT 100001";

pub(super) fn populate(
    db: &Connection,
    history: &mut History,
    bodies: bool,
    part_rows: bool,
    hash: &mut Sha256,
) -> NativeResult<()> {
    let session = history.info["id"].as_str().ok_or_else(unavailable)?;
    let mut query = db.prepare_cached(MESSAGES).map_err(|_| unavailable())?;
    let mut rows = query
        .query(params![session, MAX_RECORD as i64, bodies])
        .map_err(|_| unavailable())?;
    let mut last_user = String::from("unknown");
    let mut assistant_parts = HashMap::<String, i64>::new();
    let mut invalid = false;
    while let Some(row) = rows.next().map_err(|_| unavailable())? {
        let id: String = row.get(0).map_err(|_| unavailable())?;
        let kind: String = row.get(1).map_err(|_| unavailable())?;
        let created: i64 = row.get(2).map_err(|_| unavailable())?;
        let raw: Option<String> = row.get(3).map_err(|_| unavailable())?;
        hash.update(serde_json::to_vec(&(&id, &kind, created, &raw)).map_err(|_| unavailable())?);
        let Some(mut data) = decode(raw).filter(|_| valid_id(&id)) else {
            invalid = true;
            if kind == "user" {
                last_user = "unknown".into();
            }
            continue;
        };
        if kind == "idle" {
            last_user = "unknown".into();
        }
        let role = match kind.as_str() {
            "user" => {
                last_user.clone_from(&id);
                "user"
            }
            "assistant" => {
                if data["contentValid"] != 1 {
                    invalid = true;
                }
                assistant_parts.insert(id.clone(), 0);
                "assistant"
            }
            "compaction" => {
                data["summary"] = true.into();
                match data["status"].as_str() {
                    Some("completed")
                        if data["tokens"].is_object() || data["modelID"].is_string() =>
                    {
                        data["finish"] = "compaction".into();
                        "assistant"
                    }
                    Some("failed") if data["error"].is_object() => "assistant",
                    _ => "event",
                }
            }
            "synthetic" | "skill" => "event",
            "idle" | "agent-switched" | "model-switched" | "location-switched" | "system"
            | "shell" => continue,
            _ => {
                invalid = true;
                continue;
            }
        };
        data["role"] = role.into();
        data["parentID"] = last_user.clone().into();
        if history.info["fork"] == 1
            && history.info["created"]
                .as_i64()
                .is_some_and(|at| created < at)
        {
            data["inherited"] = true.into();
        }
        if kind != "assistant" {
            let text = data.get_mut("text").map(Value::take);
            if kind == "user" || kind == "compaction" || text.as_ref().is_some_and(Value::is_string)
            {
                let part = if kind == "compaction" {
                    json!({"type":"compaction","summary":text})
                } else {
                    json!({"type":"text","text":text,"synthetic":role=="event"})
                };
                history.parts.push(Record {
                    id: format!("{id}_part_0"),
                    parent: id.clone(),
                    created,
                    data: part,
                });
            }
            if let Some(count) = data["files"].as_u64().filter(|&count| count > 0) {
                history.parts.push(Record {
                    id: format!("{id}_part_1"),
                    parent: id.clone(),
                    created,
                    data: json!({"type":"file","count":count}),
                });
            }
        }
        history.messages.push(Record {
            id,
            parent: session.into(),
            created,
            data,
        });
    }

    let mut parts_seen = 0;
    for source in [SPLIT_PARTS, INLINE_PARTS] {
        if source == SPLIT_PARTS && !part_rows {
            continue;
        }
        let sql = format!(
            "WITH parts(message_id,ordinal,created,kind,payload) AS ({source})
             SELECT message_id,ordinal,created,kind,CASE WHEN json_type(payload)='object' THEN
                 CASE kind WHEN 'tool' THEN json_object('type','tool',
                     'tool',substr(json_extract(payload,'$.name'),1,300),
                     'callID',substr(json_extract(payload,'$.id'),1,256),
                     'state',json_object(
                        'status',CASE json_extract(payload,'$.state.status') WHEN 'streaming' THEN 'pending'
                            ELSE json_extract(payload,'$.state.status') END,
                        'input',CASE WHEN json_type(payload,'$.state.input')='object'
                            THEN json_extract(payload,'$.state.input') END,
                        'output',CASE WHEN ?3 THEN (
                            SELECT substr(group_concat(json_extract(CASE WHEN c.type='object' THEN c.value END,'$.text'),char(10)),1,65537)
                            FROM json_each(payload,'$.state.content') c
                            WHERE json_extract(CASE WHEN c.type='object' THEN c.value END,'$.type')='text'
                        ) END,
                        'error',CASE WHEN ?3 THEN substr(json_extract(payload,'$.state.error.message'),1,65537) END,
                        'time',json_object('start',coalesce(json_extract(payload,'$.time.ran'),json_extract(payload,'$.time.created')),
                            'end',json_extract(payload,'$.time.completed'))))
                 WHEN 'text' THEN json_object('type','text',
                     'text',CASE WHEN ?3 THEN substr(json_extract(payload,'$.text'),1,65537) END)
                 WHEN 'reasoning' THEN json_object('type','reasoning',
                     'text',CASE WHEN ?3 THEN substr(json_extract(payload,'$.text'),1,65537) END,
                     'time',json_object('start',json_extract(payload,'$.time.created')))
                 END
             END FROM parts"
        );
        let mut query = db.prepare_cached(&sql).map_err(|_| unavailable())?;
        let mut rows = query
            .query(params![session, MAX_RECORD as i64, bodies])
            .map_err(|_| unavailable())?;
        while let Some(row) = rows.next().map_err(|_| unavailable())? {
            parts_seen += 1;
            if parts_seen > MAX_ROWS {
                invalid = true;
                break;
            }
            let parent: String = row.get(0).map_err(|_| unavailable())?;
            let ordinal: i64 = row.get(1).map_err(|_| unavailable())?;
            let created: i64 = row.get(2).map_err(|_| unavailable())?;
            let kind: Option<String> = row.get(3).map_err(|_| unavailable())?;
            let raw: Option<String> = row.get(4).map_err(|_| unavailable())?;
            hash.update(
                serde_json::to_vec(&(&parent, ordinal, created, &kind, &raw))
                    .map_err(|_| unavailable())?,
            );
            let Some(next) = assistant_parts.get_mut(&parent) else {
                invalid = true;
                continue;
            };
            let index = *next;
            *next += 1;
            if let Some(data) = decode(raw) {
                history.parts.push(Record {
                    id: format!("{parent}_part_{index}"),
                    parent,
                    created,
                    data,
                });
            } else {
                invalid = true;
            }
        }
    }
    if invalid {
        history
            .warnings
            .push("部分 OpenCode 2.x 消息或片段损坏、超限或格式不受支持，已保留可读内容。".into());
    }
    Ok(())
}

fn decode(raw: Option<String>) -> Option<Value> {
    serde_json::from_str::<Value>(&raw?)
        .ok()
        .filter(Value::is_object)
}
