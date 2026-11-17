use super::{db_error, decode, encode};
use crate::providers::{ConfigError, fail};
use rusqlite::{Connection, OptionalExtension, params};

pub(crate) fn initialize(db: &Connection) -> Result<(), ConfigError> {
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS gateway_route_sequences(
             kind TEXT PRIMARY KEY, sequence INTEGER NOT NULL CHECK(sequence>=0));
         INSERT OR IGNORE INTO gateway_route_sequences
             SELECT 'lease', coalesce(max(seq),0) FROM gateway_route_leases;
         INSERT OR IGNORE INTO gateway_route_sequences
             SELECT 'attempt', coalesce(max(id),0) FROM service_route_attempts;
         CREATE TABLE IF NOT EXISTS gateway_session_watermarks(
             binding TEXT NOT NULL, session TEXT NOT NULL, head_seq INTEGER NOT NULL DEFAULT 0,
             updated_at_ms INTEGER NOT NULL, max_evicted_seq INTEGER NOT NULL DEFAULT 0,
             archived INTEGER NOT NULL DEFAULT 0, anchor_chain TEXT,
             PRIMARY KEY(binding,session));
         CREATE TABLE IF NOT EXISTS gateway_dispatch_sequences(
             binding TEXT NOT NULL, session TEXT NOT NULL, turn TEXT NOT NULL, seq INTEGER NOT NULL,
             superseded_by INTEGER, PRIMARY KEY(binding,session,turn));
         CREATE TABLE IF NOT EXISTS gateway_route_change_archive(
             id TEXT PRIMARY KEY, value TEXT NOT NULL CHECK(json_valid(value)));
         CREATE INDEX IF NOT EXISTS gateway_change_archive_created
             ON gateway_route_change_archive(json_extract(value,'$.createdAt') DESC, id DESC);
         CREATE INDEX IF NOT EXISTS service_attempt_turn
             ON service_route_attempts(binding,session,turn,id);",
    )
    .map_err(db_error)
}

pub(crate) fn backfill(db: &Connection) -> Result<(), ConfigError> {
    db.execute_batch(
        "INSERT OR IGNORE INTO gateway_dispatch_sequences(binding,session,turn,seq)
           SELECT d.binding,d.session,d.turn,l.seq FROM service_route_dispatch d
           JOIN gateway_route_leases l ON l.binding=d.binding AND l.session=d.session AND l.turn=d.turn;
         INSERT OR IGNORE INTO gateway_session_watermarks(binding,session,head_seq,updated_at_ms)
           SELECT binding,session,max(CASE WHEN trusted=1 AND output_validated=1
               AND reason IN ('new_session','new_user_turn','new_session_requires_dispatch','new_user_turn_requires_dispatch')
               THEN seq ELSE 0 END), unixepoch()*1000
           FROM gateway_route_leases WHERE session<>'unknown' GROUP BY binding,session;",
    )
    .map_err(db_error)
}

pub(crate) fn sequence(db: &Connection, kind: &str) -> Result<i64, ConfigError> {
    db.query_row(
        "UPDATE gateway_route_sequences SET sequence=sequence+1 WHERE kind=?1 RETURNING sequence",
        [kind],
        |row| row.get(0),
    )
    .map_err(db_error)
}

pub(super) fn note_lease(
    db: &Connection,
    binding: &str,
    session: &str,
    seq: i64,
    advances_head: bool,
) -> Result<(), ConfigError> {
    let now = chrono::Utc::now().timestamp_millis();
    db.execute(
        "INSERT INTO gateway_session_watermarks(binding,session,head_seq,updated_at_ms)
           VALUES (?1,?2,?3,?4)
         ON CONFLICT(binding,session) DO UPDATE SET
           head_seq=max(head_seq,excluded.head_seq),updated_at_ms=excluded.updated_at_ms,
           archived=CASE WHEN excluded.head_seq>head_seq THEN 0 ELSE archived END",
        params![binding, session, if advances_head { seq } else { 0 }, now],
    )
    .map_err(db_error)?;
    Ok(())
}

pub(super) fn check_archived<'a>(
    db: &Connection,
    binding: &str,
    session: &str,
    turn: &super::Turn,
    compatible: impl Iterator<Item = &'a super::Turn>,
) -> Result<(), ConfigError> {
    let saved: Option<Option<String>> = db.query_row(
        "SELECT anchor_chain FROM gateway_session_watermarks WHERE binding=?1 AND session=?2 AND archived=1",
        params![binding, session],
        |row| row.get(0),
    ).optional().map_err(db_error)?;
    let Some(anchor) = saved else {
        return Ok(());
    };
    if let Some(anchor) = anchor {
        let chain: Vec<String> = decode(&anchor)?;
        if !chain.is_empty()
            && !turn.continuation
            && turn.portable
            && compatible.into_iter().any(|candidate| {
                candidate.chain.len() > chain.len() && candidate.chain.starts_with(&chain)
            })
        {
            return Ok(());
        }
    }
    fail(
        "session_lease_archived",
        "无法核对已归档会话的新增用户轮次，请开启新会话",
    )
}

pub(crate) fn check_closed(
    db: &Connection,
    binding: &str,
    session: &str,
    digests: &[&str],
) -> Result<(), ConfigError> {
    let closed: bool = db
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM gateway_dispatch_sequences
         WHERE binding=?1 AND session=?2 AND superseded_by IS NOT NULL
         AND turn IN (SELECT value FROM json_each(?3)))",
            params![binding, session, encode(&digests)?],
            |row| row.get(0),
        )
        .map_err(db_error)?;
    if closed {
        return fail("route_turn_superseded", "原轮次已失败并被后续轮次取代");
    }
    Ok(())
}

pub(crate) fn reclaim_attempts(db: &Connection, limit: i64) -> Result<(), ConfigError> {
    // Unknown legacy outcome labels are evidence, not candidates for deletion.
    let mut rows = db
        .prepare(
            "SELECT d.binding,d.session,d.turn,w.head_seq,count(a.id),d.state
         FROM service_route_attempts a
         JOIN service_route_dispatch d USING(binding,session,turn)
         JOIN gateway_dispatch_sequences s USING(binding,session,turn)
         JOIN gateway_session_watermarks w USING(binding,session)
         WHERE d.state IN ('failed','committed') AND s.superseded_by IS NULL AND w.head_seq>s.seq
           AND json_type(d.value,'$.fallback') IS NULL
           AND NOT EXISTS(SELECT 1 FROM agent_budget_reservations r WHERE r.binding=d.binding
               AND json_extract(r.value,'$.session')=d.session AND json_extract(r.value,'$.turn')=d.turn
               AND r.state IN ('preparing','sending','uncertain'))
         GROUP BY d.binding,d.session,d.turn
         HAVING sum(a.outcome NOT IN ('not_sent','deadline_before_send','rate_limited',
           'upstream_credential_rejected','upstream_model_access_denied',
           'upstream_quota_exhausted','rejected','completed'))=0
           AND ((d.state='committed') = (sum(a.outcome='completed')>0))
         ORDER BY min(a.id) LIMIT ?1",
        )
        .map_err(db_error)?;
    let eligible = rows
        .query_map([limit], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get::<_, String>(5)?,
            ))
        })
        .map_err(db_error)?
        .collect::<rusqlite::Result<Vec<(String, String, String, i64, i64, String)>>>()
        .map_err(db_error)?;
    let mut removed = 0;
    for (binding, session, turn, head, count, state) in eligible {
        if state == "failed" {
            db.execute(
                "UPDATE gateway_dispatch_sequences SET superseded_by=?4
                 WHERE binding=?1 AND session=?2 AND turn=?3",
                params![binding, session, turn, head],
            )
            .map_err(db_error)?;
        }
        db.execute(
            "DELETE FROM service_route_attempts WHERE binding=?1 AND session=?2 AND turn=?3",
            params![binding, session, turn],
        )
        .map_err(db_error)?;
        removed += count;
        if removed >= limit {
            break;
        }
    }
    Ok(())
}

pub(super) fn session_was_evicted(
    db: &Connection,
    binding: &str,
    session: &str,
) -> Result<bool, ConfigError> {
    db.query_row(
        "SELECT EXISTS(SELECT 1 FROM gateway_session_watermarks
          WHERE binding=?1 AND session=?2 AND max_evicted_seq>0)",
        params![binding, session],
        |row| row.get(0),
    )
    .map_err(db_error)
}

pub(super) fn reclaim_lease(
    db: &Connection,
    excluded_binding: &str,
    excluded_session: &str,
    retained_seq: Option<i64>,
) -> Result<(), ConfigError> {
    let candidate: Option<(i64,String,String)> = db.query_row(
        "SELECT l.seq,l.binding,l.session FROM gateway_route_leases l
         WHERE l.session<>'unknown' AND l.seq<>coalesce(?1,0) AND l.seq<(SELECT max(x.seq) FROM gateway_route_leases x
             WHERE x.binding=l.binding AND x.session=l.session)
           AND l.seq<(SELECT max(x.seq) FROM gateway_route_leases x
             WHERE x.binding=l.binding AND x.session=l.session AND x.output_validated=1)
           AND l.seq<(SELECT max(x.seq) FROM gateway_route_leases x
             WHERE x.binding=l.binding AND x.session=l.session AND x.output_validated=1 AND x.trusted=1)
           AND NOT EXISTS(SELECT 1 FROM gateway_route_leases inherited
             WHERE inherited.binding=l.binding AND inherited.session=l.session
               AND inherited.seq>l.seq AND inherited.service_selection IS NOT NULL
               AND inherited.reason IN ('history_requires_original_route','session_boundary_unconfirmed')
               AND l.seq=(SELECT max(original.seq) FROM gateway_route_leases original
                 WHERE original.binding=l.binding AND original.session=l.session
                   AND original.seq<=inherited.seq AND original.output_validated=1
                   AND original.reason NOT IN ('history_requires_original_route','session_boundary_unconfirmed')))
           AND NOT EXISTS(SELECT 1 FROM service_route_dispatch d
             LEFT JOIN gateway_dispatch_sequences s USING(binding,session,turn)
             WHERE d.binding=l.binding AND d.session=l.session AND d.turn=l.turn
               AND (d.state='pending' OR json_type(d.value,'$.fallback') IS NOT NULL
                 OR (d.state='failed' AND s.superseded_by IS NULL)))
           AND NOT EXISTS(SELECT 1 FROM service_route_attempts a WHERE a.binding=l.binding
             AND a.session=l.session AND a.turn=l.turn AND a.outcome IN ('pending','preparing','outcome_unknown','committed'))
         ORDER BY l.seq LIMIT 1", [retained_seq],
         |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?)),
    ).optional().map_err(db_error)?;
    if let Some((seq, binding, session)) = candidate {
        db.execute(
            "UPDATE gateway_session_watermarks SET max_evicted_seq=max(max_evicted_seq,?3)
          WHERE binding=?1 AND session=?2",
            params![binding, session, seq],
        )
        .map_err(db_error)?;
        db.execute("DELETE FROM gateway_route_leases WHERE seq=?1", [seq])
            .map_err(db_error)?;
        return Ok(());
    }
    let cutoff = chrono::Utc::now().timestamp_millis() - 7 * 24 * 60 * 60 * 1000;
    let Some(candidate) =
        archive_candidates(db, Some((excluded_binding, excluded_session, cutoff)), 1)?.pop()
    else {
        return fail(
            "capacity",
            "轮次路由记录已达上限；可在设置中核查可手动归档的会话",
        );
    };
    archive_session(db, &candidate)
}

const MANUAL_ARCHIVE_BATCH: i64 = 128;
const CHANGE_ARCHIVE_BYTES: i64 = 64 * 1024 * 1024;

#[derive(serde::Serialize)]
struct ArchiveCandidate {
    binding: String,
    session: String,
    max_seq: i64,
    lease_count: u32,
    anchor: Option<String>,
    updated_at_ms: i64,
}

fn archive_candidates(
    db: &Connection,
    automatic: Option<(&str, &str, i64)>,
    limit: i64,
) -> Result<Vec<ArchiveCandidate>, ConfigError> {
    let (binding, session, cutoff) = automatic.map_or(("", "", None), |(b, s, c)| (b, s, Some(c)));
    let mut query = db.prepare(
        "SELECT w.binding,w.session,max(l.seq),count(l.seq),
           (SELECT a.chain FROM gateway_route_leases a WHERE a.binding=w.binding AND a.session=w.session
             AND a.seq=w.head_seq AND a.output_validated=1 AND a.trusted=1
             AND NOT EXISTS(SELECT 1 FROM gateway_route_leases b WHERE b.binding=w.binding
               AND b.session=w.session AND b.output_validated=1 AND b.seq>a.seq)),
           w.updated_at_ms
         FROM gateway_session_watermarks w JOIN gateway_route_leases l USING(binding,session)
         WHERE w.archived=0 AND w.session<>'unknown'
           AND (?3 IS NULL OR ((w.binding<>?1 OR w.session<>?2) AND w.updated_at_ms<?3
             AND (SELECT count(*) FROM gateway_session_watermarks x WHERE x.archived=0
                  AND x.session<>'unknown'
                  AND (x.updated_at_ms,x.binding,x.session)>(w.updated_at_ms,w.binding,w.session))>=512))
           AND NOT EXISTS(SELECT 1 FROM service_route_dispatch d LEFT JOIN gateway_dispatch_sequences s
                 USING(binding,session,turn)
                WHERE d.binding=w.binding AND d.session=w.session
                AND (d.state='pending' OR json_type(d.value,'$.fallback') IS NOT NULL
                  OR (d.state='failed' AND s.superseded_by IS NULL)))
           AND NOT EXISTS(SELECT 1 FROM service_route_attempts a WHERE a.binding=w.binding
             AND a.session=w.session AND a.outcome IN ('pending','preparing','outcome_unknown','committed'))
         GROUP BY w.binding,w.session ORDER BY w.updated_at_ms,w.binding,w.session LIMIT ?4",
    ).map_err(db_error)?;
    query
        .query_map(params![binding, session, cutoff, limit], |row| {
            Ok(ArchiveCandidate {
                binding: row.get(0)?,
                session: row.get(1)?,
                max_seq: row.get(2)?,
                lease_count: row.get(3)?,
                anchor: row.get(4)?,
                updated_at_ms: row.get(5)?,
            })
        })
        .map_err(db_error)?
        .collect::<rusqlite::Result<_>>()
        .map_err(db_error)
}

fn archive_session(db: &Connection, candidate: &ArchiveCandidate) -> Result<(), ConfigError> {
    let values = params![
        candidate.binding,
        candidate.session,
        candidate.max_seq,
        candidate.anchor
    ];
    db.execute(
        "UPDATE gateway_session_watermarks SET archived=1,anchor_chain=?4,
          max_evicted_seq=max(max_evicted_seq,?3) WHERE binding=?1 AND session=?2",
        values,
    )
    .map_err(db_error)?;
    let deleted = db
        .execute(
            "DELETE FROM gateway_route_leases WHERE binding=?1 AND session=?2",
            params![candidate.binding, candidate.session],
        )
        .map_err(db_error)?;
    if deleted != candidate.lease_count as usize {
        return fail("routing_unavailable", "归档的租约已变化，请重新预览");
    }
    Ok(())
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArchivePreview {
    pub store_id: String,
    pub confirmation: String,
    pub sessions: usize,
    pub leases: u64,
}

#[derive(serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArchiveRequest {
    pub store_id: String,
    pub confirmation: Option<String>,
}

fn preview(
    db: &Connection,
    store_id: &str,
) -> Result<(ArchivePreview, Vec<ArchiveCandidate>), ConfigError> {
    let actual: String = db
        .query_row(
            "SELECT store_id FROM model_service_operation_scope WHERE id=1",
            [],
            |row| row.get(0),
        )
        .map_err(db_error)?;
    if actual != store_id {
        return fail("conflict", "后台数据库已变化，请重新读取设置");
    }
    let candidates = archive_candidates(db, None, MANUAL_ARCHIVE_BATCH)?;
    let confirmation = crate::providers::files::hash(encode(&(store_id, &candidates))?.as_bytes());
    let summary = ArchivePreview {
        store_id: actual,
        confirmation,
        sessions: candidates.len(),
        leases: candidates
            .iter()
            .map(|candidate| u64::from(candidate.lease_count))
            .sum(),
    };
    Ok((summary, candidates))
}

impl crate::Store {
    pub fn preview_route_archive(&self, store_id: &str) -> Result<ArchivePreview, ConfigError> {
        let tx = self.connection.unchecked_transaction().map_err(db_error)?;
        let (summary, _) = preview(&tx, store_id)?;
        tx.commit().map_err(db_error)?;
        Ok(summary)
    }

    pub fn archive_route_sessions(
        &self,
        request: &ArchiveRequest,
    ) -> Result<ArchivePreview, ConfigError> {
        let tx = rusqlite::Transaction::new_unchecked(
            &self.connection,
            rusqlite::TransactionBehavior::Immediate,
        )
        .map_err(db_error)?;
        let (summary, candidates) = preview(&tx, &request.store_id)?;
        if Some(summary.confirmation.as_str()) != request.confirmation.as_deref()
            || candidates.is_empty()
        {
            return fail("conflict", "可归档的会话已变化，请重新预览");
        }
        for candidate in &candidates {
            archive_session(&tx, candidate)?;
        }
        tx.commit().map_err(db_error)?;
        Ok(summary)
    }
}

pub(super) fn reclaim_versions(
    db: &Connection,
    binding: &str,
    session: &str,
) -> Result<(), ConfigError> {
    reclaim_attempts(db, 4096)?;
    remove_unreferenced_versions(db)?;
    if db.changes() != 0 {
        return Ok(());
    }
    let leases: i64 = db
        .query_row("SELECT count(*) FROM gateway_route_leases", [], |row| {
            row.get(0)
        })
        .map_err(db_error)?;
    if leases >= super::MAX_LEASES {
        match reclaim_lease(db, binding, session, None) {
            Ok(()) => remove_unreferenced_versions(db)?,
            Err(error) if error.code == "capacity" => (),
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn remove_unreferenced_versions(db: &Connection) -> Result<(), ConfigError> {
    db.execute_batch(
        "WITH RECURSIVE reachable(binding,version) AS (
           SELECT binding,version FROM model_service_bindings
           UNION SELECT b.binding,m.current FROM model_service_bindings b JOIN model_services m
                 ON b.service=m.id WHERE m.deleted=0
           UNION SELECT r.binding,json_extract(x.value,'$.child.versionId')
                FROM reachable r JOIN model_service_versions v ON r.version=v.id,
                json_each(v.value,'$.models') x WHERE json_type(x.value,'$.child')='object'
         )
         DELETE FROM model_service_targets AS t
           WHERE NOT EXISTS(SELECT 1 FROM reachable r
             WHERE r.binding=t.binding AND r.version=t.version)
             AND NOT EXISTS(SELECT 1 FROM gateway_route_leases l WHERE l.version=t.route)
             AND NOT EXISTS(SELECT 1 FROM service_route_attempts a WHERE a.route=t.route)
             AND NOT EXISTS(SELECT 1 FROM service_route_dispatch d
               LEFT JOIN gateway_dispatch_sequences s USING(binding,session,turn), json_tree(d.value) j
               WHERE d.binding=t.binding AND (d.state='pending' OR
                 (d.state='failed' AND s.superseded_by IS NULL) OR json_type(d.value,'$.fallback') IS NOT NULL)
                 AND j.type='text' AND j.atom=t.route);
         DELETE FROM gateway_route_versions AS v WHERE v.id IN (
           SELECT r.id FROM gateway_route_versions r
           WHERE NOT EXISTS(SELECT 1 FROM gateway_bindings b WHERE b.target=r.id)
             AND NOT EXISTS(SELECT 1 FROM model_service_targets t WHERE t.route=r.id)
             AND NOT EXISTS(SELECT 1 FROM gateway_route_leases l WHERE l.version=r.id)
             AND NOT EXISTS(SELECT 1 FROM service_route_attempts a WHERE a.route=r.id)
             AND NOT EXISTS(SELECT 1 FROM gateway_route_changes c WHERE c.state='pending'
               AND json_extract(c.value,'$.before.id')=r.id)
             AND NOT EXISTS(SELECT 1 FROM service_route_dispatch d
               LEFT JOIN gateway_dispatch_sequences s USING(binding,session,turn), json_tree(d.value) j
               WHERE (d.state='pending' OR (d.state='failed' AND s.superseded_by IS NULL)
                   OR json_type(d.value,'$.fallback') IS NOT NULL)
                 AND j.type='text' AND j.atom=r.id)
           ORDER BY json_extract(r.value,'$.createdAt'),r.id LIMIT 512);",
    )
    .map_err(db_error)
}

pub(super) fn reclaim_changes(db: &Connection) -> Result<(), ConfigError> {
    reclaim_changes_with_limit(db, CHANGE_ARCHIVE_BYTES)
}

pub(super) fn reclaim_changes_with_limit(
    db: &Connection,
    byte_limit: i64,
) -> Result<(), ConfigError> {
    for batch in [512, 1] {
        db.execute_batch("SAVEPOINT route_archive_batch")
            .map_err(db_error)?;
        let archived = (|| {
            let added = db.execute(
                "INSERT INTO gateway_route_change_archive(id,value)
                 SELECT c.id,c.value FROM gateway_route_changes c
                 WHERE c.state IN ('applied','cancelled')
                   AND NOT EXISTS(SELECT 1 FROM gateway_route_changes p
                     WHERE p.binding=c.binding AND p.state='pending')
                   AND (c.state='cancelled' OR c.id<>(
                     SELECT a.id FROM gateway_route_changes a WHERE a.binding=c.binding AND a.state='applied'
                     ORDER BY julianday(json_extract(a.value,'$.createdAt')) DESC,a.rowid DESC LIMIT 1))
                 ORDER BY julianday(json_extract(c.value,'$.createdAt')),c.id LIMIT ?1",
                [batch],
            ).map_err(db_error)?;
            if added == 0 {
                return fail(
                    "capacity",
                    "动态操作记录已达上限；请先处理待确认或待核查操作",
                );
            }
            // Account for actual archive pages, including the history index
            // and the table's primary-key index, in the same write snapshot.
            let bytes: i64 = db
                .query_row(
                    "SELECT coalesce(sum(pgsize),0) FROM dbstat
                 WHERE name IN ('gateway_route_change_archive','gateway_change_archive_created',
                   'sqlite_autoindex_gateway_route_change_archive_1')",
                    [],
                    |row| row.get(0),
                )
                .map_err(db_error)?;
            if bytes > byte_limit {
                return Ok(false);
            }
            db.execute(
                "DELETE FROM gateway_route_changes AS active WHERE EXISTS(
                  SELECT 1 FROM gateway_route_change_archive archive WHERE archive.id=active.id)",
                [],
            )
            .map_err(db_error)?;
            Ok(true)
        })();
        match archived {
            Ok(true) => {
                db.execute_batch("RELEASE route_archive_batch")
                    .map_err(db_error)?;
                return Ok(());
            }
            Ok(false) => db
                .execute_batch("ROLLBACK TO route_archive_batch; RELEASE route_archive_batch")
                .map_err(db_error)?,
            Err(error) => {
                let _ = db
                    .execute_batch("ROLLBACK TO route_archive_batch; RELEASE route_archive_batch");
                return Err(error);
            }
        }
    }
    fail(
        "route_archive_capacity",
        "动态切换历史归档已达磁盘容量上限，原回执仍可查询",
    )
}
