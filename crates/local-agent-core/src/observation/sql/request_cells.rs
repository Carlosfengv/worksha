//! Disposable native request statistics, bounded by query scopes and row blocks.
//! Public pagination continues to own immutable record positions separately.
use super::*;
use aggregates::quote;
use chrono::{DateTime, Utc};

const BLOCK: i64 = 4096;
const MAX_CELLS: usize = 65_536;

pub(super) fn init(db: &Connection) -> Result<(), String> {
    db.execute_batch("CREATE TEMP TABLE native_request_clock(scope TEXT PRIMARY KEY,created TEXT,expires INTEGER,fallback INTEGER);
      CREATE TEMP TABLE native_request_dirty(scope TEXT,bucket INTEGER,PRIMARY KEY(scope,bucket));
      CREATE TEMP TABLE native_request_cells(scope TEXT,bucket INTEGER,agent TEXT,model TEXT,provider TEXT,id TEXT,v INTEGER,label TEXT,instant TEXT,stats TEXT,matched INTEGER,ambiguous INTEGER,conflicts INTEGER,unsafe INTEGER);
      CREATE INDEX temp.native_request_cell_scope ON native_request_cells(scope,bucket);
      CREATE TEMP TRIGGER native_request_insert AFTER INSERT ON current WHEN new.kind='native' BEGIN INSERT INTO native_request_dirty SELECT c.scope,new.v/4096 FROM native_request_clock c WHERE NOT EXISTS(SELECT 1 FROM native_request_dirty d WHERE d.scope=c.scope AND d.bucket=new.v/4096);END;
      CREATE TEMP TRIGGER native_request_delete AFTER DELETE ON current WHEN old.kind='native' BEGIN INSERT INTO native_request_dirty SELECT c.scope,old.v/4096 FROM native_request_clock c WHERE NOT EXISTS(SELECT 1 FROM native_request_dirty d WHERE d.scope=c.scope AND d.bucket=old.v/4096);END;
      CREATE TEMP TRIGGER native_request_update AFTER UPDATE OF v ON current WHEN old.kind='native' AND old.v<>new.v BEGIN INSERT INTO native_request_dirty SELECT c.scope,old.v/4096 FROM native_request_clock c WHERE NOT EXISTS(SELECT 1 FROM native_request_dirty d WHERE d.scope=c.scope AND d.bucket=old.v/4096);INSERT INTO native_request_dirty SELECT c.scope,new.v/4096 FROM native_request_clock c WHERE NOT EXISTS(SELECT 1 FROM native_request_dirty d WHERE d.scope=c.scope AND d.bucket=new.v/4096);END;").map_err(fail)
}

impl History {
    pub(super) fn native_request_cells(
        &self,
        q: &requests::Query,
        now: DateTime<Utc>,
    ) -> Result<Option<String>, String> {
        let predicate = dashboard::filter(q, now, true);
        let source = format!(
            "(SELECT r.* FROM records r JOIN current c ON c.v=r.v WHERE r.kind='native' AND {predicate})"
        );
        let block_source = source.replace(
            "records r JOIN current c",
            "records r NOT INDEXED CROSS JOIN current c",
        );
        let (start, _, step) = self.request_layout(&source, q, now)?;
        let mut key_query = q.clone();
        key_query.source = Some("native".into());
        key_query.metric = None;
        key_query.dimension = None;
        key_query.sort = None;
        key_query.descending = None;
        key_query.currency = None;
        let key = quote(&encode(&(
            request_cache::query_key(&key_query)?,
            start.to_rfc3339(),
            step.num_seconds(),
        ))?);
        let previous: Option<(String, i64, bool)> = self
            .db
            .query_row(
                &format!(
                    "SELECT created,expires,fallback FROM native_request_clock WHERE scope={key}"
                ),
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()
            .map_err(fail)?;
        let clean: bool = self
            .db
            .query_row(
                &format!("SELECT NOT EXISTS(SELECT 1 FROM native_request_dirty WHERE scope={key})"),
                [],
                |r| r.get(0),
            )
            .map_err(fail)?;
        let reuse = previous.as_ref().is_some_and(|(created, expires, _)| {
            DateTime::parse_from_rfc3339(created).is_ok_and(|at| now >= at)
                && now.timestamp_millis() < *expires
        });
        if reuse && clean {
            return Ok((!previous.unwrap().2).then(|| cells(&key)));
        }
        self.db
            .execute_batch("SAVEPOINT native_request_blocks")
            .map_err(fail)?;
        let result = (|| {
            if !reuse || previous.as_ref().is_some_and(|p| p.2) {
                self.db.execute_batch(&format!("DELETE FROM native_request_cells WHERE scope={key};DELETE FROM native_request_dirty WHERE scope={key};
                  INSERT INTO native_request_dirty SELECT DISTINCT {key},r.v/{BLOCK} FROM records r JOIN current c ON c.v=r.v WHERE r.kind='native';")).map_err(fail)?;
            }
            execute(&self.db,&format!("DELETE FROM native_request_cells WHERE scope={key} AND bucket IN (SELECT bucket FROM native_request_dirty WHERE scope={key})"),[]).map_err(fail)?;
            let dirty: Vec<i64> = {
                let mut statement = self
                    .db
                    .prepare(&format!(
                        "SELECT bucket FROM native_request_dirty WHERE scope={key} ORDER BY bucket"
                    ))
                    .map_err(fail)?;
                statement
                    .query_map([], |r| r.get(0))
                    .map_err(fail)?
                    .collect::<Result<_, _>>()
                    .map_err(fail)?
            };
            let mut count: usize = self
                .db
                .query_row(
                    &format!("SELECT count(*) FROM native_request_cells WHERE scope={key}"),
                    [],
                    |r| sql_count(r, 0),
                )
                .map_err(fail)?;
            let origin = start.timestamp_millis();
            let residual = format!("{:06}", start.timestamp_subsec_nanos() % 1_000_000);
            let width = step.num_milliseconds();
            for bucket in dirty {
                let lo = bucket * BLOCK;
                let hi = lo.saturating_add(BLOCK);
                // Millisecond boundaries retain the original nanosecond offset.
                // A cell never crosses either a chart bin or a UTC+8 calendar day.
                let bin = format!(
                    "CASE WHEN time IS NOT NULL THEN (time-({origin})-CASE WHEN substr(instant,-7,6)<{} THEN 1 ELSE 0 END)/{width} END",
                    quote(&residual)
                );
                count+=execute(&self.db,&format!("WITH totals AS (
                  SELECT agent,model,provider,min(id) id,min(instant) instant,
                    summary_block_sum(stats,cache_in,cache_read) stats,
                    count(*) FILTER(WHERE association='response_id') matched,
                    count(*) FILTER(WHERE association IN ('ambiguous','identity_conflict')) ambiguous,
                    count(*) FILTER(WHERE usage_conflict=1) conflicts,
                    max(ttft_sample IS NOT NULL OR duration_sample IS NOT NULL OR throughput_sample IS NOT NULL OR (time IS NOT NULL AND strftime('%Y-%m-%d',time/1000.0,'unixepoch','+8 hours') IS NULL)) unsafe
                  FROM {block_source} WHERE v>={lo} AND v<{hi}
                  GROUP BY {bin},strftime('%Y-%m-%d',time/1000.0,'unixepoch','+8 hours'),agent,model,provider)
                  INSERT INTO native_request_cells SELECT {key},{bucket},g.agent,g.model,g.provider,g.id,c.v,b.label,g.instant,g.stats,g.matched,g.ambiguous,g.conflicts,g.unsafe
                  FROM totals g CROSS JOIN current c ON c.kind='native' AND c.id=g.id CROSS JOIN native_base b ON b.id=g.id"),[]).map_err(fail)?;
                if count > MAX_CELLS {
                    break;
                }
            }
            let unsafe_rows:bool=self.db.query_row(&format!("SELECT EXISTS(SELECT 1 FROM native_request_cells WHERE scope={key} AND (unsafe OR json_extract(stats,'$._money')))"),[],|r|r.get(0)).map_err(fail)?;
            let fallback = count > MAX_CELLS || unsafe_rows;
            if fallback {
                execute(
                    &self.db,
                    &format!("DELETE FROM native_request_cells WHERE scope={key}"),
                    [],
                )
                .map_err(fail)?;
            }
            let expires = self.next_request_time(now)?.timestamp_millis();
            execute(&self.db,&format!("INSERT INTO native_request_clock VALUES({key},?1,?2,?3) ON CONFLICT(scope) DO UPDATE SET created=excluded.created,expires=excluded.expires,fallback=excluded.fallback"),params![now.to_rfc3339(),expires,fallback]).map_err(fail)?;
            self.db.execute_batch(&format!("DELETE FROM native_request_dirty WHERE scope={key};
              CREATE TEMP TABLE IF NOT EXISTS request_evicted(scope TEXT PRIMARY KEY);DELETE FROM request_evicted;
              INSERT INTO request_evicted SELECT scope FROM native_request_clock WHERE scope<>{key} ORDER BY created DESC,scope LIMIT -1 OFFSET 7;
              DELETE FROM native_request_cells WHERE scope IN request_evicted;DELETE FROM native_request_dirty WHERE scope IN request_evicted;DELETE FROM native_request_clock WHERE scope IN request_evicted;
              RELEASE native_request_blocks;")).map_err(fail)?;
            Ok((!fallback).then(|| cells(&key)))
        })();
        if result.is_err() {
            let _ = self
                .db
                .execute_batch("ROLLBACK TO native_request_blocks;RELEASE native_request_blocks");
        }
        result
    }
}
fn cells(key: &str) -> String {
    format!(
        "(SELECT id,v,instant_ms(instant) time,instant started,instant,agent,model,provider,json_object('provider',label) value,stats,json_extract(stats,'$._cacheInput') cache_in,json_extract(stats,'$._cacheRead') cache_read,NULL ttft_sample,NULL duration_sample,NULL throughput_sample,matched,ambiguous,conflicts FROM native_request_cells WHERE scope={key})"
    )
}
