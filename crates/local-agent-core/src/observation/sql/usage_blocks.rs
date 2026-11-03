//! Bounded disposable ranges per native consumer. Integer totals are associative,
//! so unchanged blocks can be reused without subtracting saturated counters.
//! Money-bearing model queries retain their original row-by-row summation.
use super::*;
use aggregates::quote;
use chrono::{DateTime, Utc};

const BLOCK: i64 = 4096;

impl History {
    pub(super) fn usage_block_tables(&self, kind: &str, facts: &str) -> Result<(), String> {
        let base = format!("native_{kind}_blocks");
        self.db.execute_batch(&format!(
            "CREATE TEMP TABLE IF NOT EXISTS {base}_dirty(scope TEXT,bucket INTEGER,PRIMARY KEY(scope,bucket));
             CREATE TEMP TABLE IF NOT EXISTS {base}_clock(scope TEXT PRIMARY KEY,created TEXT,expires INTEGER);
             CREATE TEMP TABLE IF NOT EXISTS {base}(scope TEXT,bucket INTEGER,provider TEXT,model TEXT,stats TEXT,id TEXT,t TEXT,ord,unsafe INTEGER,digest TEXT);
             CREATE INDEX IF NOT EXISTS temp.{base}_bucket ON {base}(scope,bucket);
             CREATE INDEX IF NOT EXISTS temp.{base}_source_bucket ON {facts}(source_order/{BLOCK});
             CREATE TEMP TRIGGER IF NOT EXISTS {base}_insert AFTER INSERT ON {facts} BEGIN INSERT OR IGNORE INTO {base}_dirty SELECT scope,new.source_order/{BLOCK} FROM {base}_clock;END;
             CREATE TEMP TRIGGER IF NOT EXISTS {base}_delete AFTER DELETE ON {facts} BEGIN INSERT OR IGNORE INTO {base}_dirty SELECT scope,old.source_order/{BLOCK} FROM {base}_clock;END;"
        )).map_err(fail)
    }

    pub(super) fn usage_blocks(
        &self,
        kind: &str,
        full: &str,
        in_time: &str,
        scope: &str,
        days: Option<u16>,
        now: DateTime<Utc>,
    ) -> Result<String, String> {
        let base = format!("native_{kind}_blocks");
        let key = quote(scope);
        let previous: Option<(String, i64)> = self
            .db
            .query_row(
                &format!("SELECT created,expires FROM {base}_clock WHERE scope=?1"),
                [scope],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .map_err(fail)?;
        let reuse = previous.is_some_and(|(created, expires)| {
            DateTime::parse_from_rfc3339(&created).is_ok_and(|t| now >= t)
                && now.timestamp_millis() < expires
        });
        self.db
            .execute_batch("SAVEPOINT usage_blocks")
            .map_err(fail)?;
        let result = (|| {
            if !reuse {
                self.db
                    .execute_batch(&format!("DELETE FROM {base} WHERE scope={key};DELETE FROM {base}_dirty WHERE scope={key};"))
                    .map_err(fail)?;
                execute(
                    &self.db,
                    &format!(
                        "INSERT INTO {base}_dirty SELECT DISTINCT {key},source_order/{BLOCK} FROM {full}"
                    ),
                    [],
                )
                .map_err(fail)?;
            }
            execute(
                &self.db,
                &format!("DELETE FROM {base} WHERE scope={key} AND bucket IN (SELECT bucket FROM {base}_dirty WHERE scope={key})"),
                [],
            )
            .map_err(fail)?;
            // Bound every sort/materialized CTE to one indexed block. A cold
            // range used to materialize and rank the entire native history.
            let dirty: Vec<i64> = {
                let mut statement = self
                    .db
                    .prepare(&format!(
                        "SELECT bucket FROM {base}_dirty WHERE scope={key} ORDER BY bucket"
                    ))
                    .map_err(fail)?;
                statement
                    .query_map([], |row| row.get(0))
                    .map_err(fail)?
                    .collect::<Result<_, _>>()
                    .map_err(fail)?
            };
            for bucket in dirty {
                let selected = format!(
                    "(SELECT *,source_order/{BLOCK} bucket FROM {full} WHERE {in_time} AND source_order/{BLOCK}={bucket})"
                );
                if kind == "provider" {
                    execute(&self.db,&format!("INSERT INTO {base} SELECT {key},bucket,provider,model,stats_sum(stats),NULL,NULL,NULL,max(json_array_length(stats,'$.cost.actual')+json_array_length(stats,'$.cost.estimated')>0),hash_rows(CASE WHEN provider IS NULL THEN coalesce(removed_digest,digest) ELSE digest END ORDER BY id) FROM {selected} GROUP BY bucket,provider,model"),[]).map_err(fail)?;
                } else {
                    execute(&self.db,&format!("WITH selected AS MATERIALIZED(SELECT * FROM {selected}),
                  totals AS (SELECT bucket,pid,model,stats_sum(stats) stats,max(json_array_length(stats,'$.cost.actual')+json_array_length(stats,'$.cost.estimated')>0) unsafe FROM selected GROUP BY bucket,pid,model),
                  ranked AS (SELECT *,row_number() OVER(PARTITION BY bucket,pid,model ORDER BY t IS NULL,t DESC,CASE WHEN t IS NULL THEN ord END DESC,CASE WHEN t IS NOT NULL THEN ord END ASC,id) rank FROM selected)
                  INSERT INTO {base} SELECT {key},g.bucket,g.pid,g.model,g.stats,r.id,r.t,r.ord,g.unsafe,NULL FROM totals g JOIN ranked r ON r.bucket=g.bucket AND r.pid=g.pid AND r.model IS g.model AND r.rank=1"),[]).map_err(fail)?;
                }
            }
            // Millisecond boundaries expire conservatively; the raw predicate
            // still compares RFC3339 nanoseconds on either edge of the range.
            let expires = usage::range_expiry(&self.db, full, days, now)?;
            self.db
                .execute_batch(&format!("DELETE FROM {base}_dirty WHERE scope={key};"))
                .map_err(fail)?;
            execute(
                &self.db,
                &format!("INSERT INTO {base}_clock VALUES(?1,?2,?3) ON CONFLICT(scope) DO UPDATE SET created=excluded.created,expires=excluded.expires"),
                params![scope, now.to_rfc3339(), expires.unwrap_or(i64::MAX)],
            )
            .map_err(fail)?;
            // Retain at most eight ranges, including the just-published scope.
            // Evict rows and dirty markers together inside the same savepoint.
            self.db.execute_batch(&format!("CREATE TEMP TABLE IF NOT EXISTS usage_evicted(scope TEXT PRIMARY KEY);DELETE FROM usage_evicted;
              INSERT INTO usage_evicted SELECT scope FROM {base}_clock WHERE scope<>{key} ORDER BY created DESC,scope LIMIT -1 OFFSET 7;
              DELETE FROM {base} WHERE scope IN usage_evicted;
              DELETE FROM {base}_dirty WHERE scope IN usage_evicted;
              DELETE FROM {base}_clock WHERE scope IN usage_evicted;
              RELEASE usage_blocks;")).map_err(fail)
        })();
        if result.is_err() {
            let _ = self
                .db
                .execute_batch("ROLLBACK TO usage_blocks;RELEASE usage_blocks");
        }
        result.map(|()| {
            format!(
                "(SELECT bucket,provider,model,stats,id,t,ord,unsafe,digest FROM {base} WHERE scope={key})"
            )
        })
    }
}
