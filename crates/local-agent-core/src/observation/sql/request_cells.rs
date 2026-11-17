//! Disposable native request statistics, bounded by query scopes and row blocks.
//! Public pagination continues to own immutable record positions separately.
use super::*;
use aggregates::quote;
use chrono::{DateTime, Duration, Utc};

const BLOCK: i64 = 4096;
const MAX_CELLS: usize = 65_536;

pub(super) fn init(db: &Connection) -> Result<(), String> {
    db.execute_batch("CREATE TEMP TABLE native_overview_facts(v INTEGER PRIMARY KEY,id TEXT,agent TEXT,model TEXT,time INTEGER,instant TEXT,tokens REAL,partial INTEGER,gateway_match TEXT);
      CREATE INDEX temp.native_overview_time ON native_overview_facts(time);
      CREATE TEMP VIEW native_overview_input AS
        SELECT v,id,agent,model,time,instant,CAST(json_extract(stats,'$.usage.totalTokens') AS REAL) tokens,
          json_extract(stats,'$.usageMissing')>0 partial,CASE WHEN association='response_id' THEN gateway_match END gateway_match
        FROM record_meta;
      CREATE TEMP TRIGGER native_overview_insert AFTER INSERT ON current WHEN new.kind='native' BEGIN
        INSERT INTO native_overview_facts SELECT * FROM native_overview_input WHERE v=new.v;
      END;
      CREATE TEMP TRIGGER native_overview_delete AFTER DELETE ON current WHEN old.kind='native' BEGIN
        DELETE FROM native_overview_facts WHERE v=old.v;
      END;
      CREATE TEMP TRIGGER native_overview_update AFTER UPDATE OF v ON current WHEN new.kind='native' AND old.v<>new.v BEGIN
        DELETE FROM native_overview_facts WHERE v=old.v;
        INSERT INTO native_overview_facts SELECT * FROM native_overview_input WHERE v=new.v;
      END;
      CREATE TEMP TABLE native_request_clock(scope TEXT PRIMARY KEY,created TEXT,expires INTEGER,fallback INTEGER);
      CREATE TEMP TABLE native_request_dirty(scope TEXT,bucket INTEGER,PRIMARY KEY(scope,bucket));
      CREATE TEMP TABLE native_request_cells(scope TEXT,bucket INTEGER,agent TEXT,model TEXT,provider TEXT,id TEXT,v INTEGER,label TEXT,instant TEXT,stats TEXT,matched INTEGER,ambiguous INTEGER,conflicts INTEGER,unsafe INTEGER,missing_response INTEGER,no_gateway INTEGER,identity_conflict INTEGER,known_tokens INTEGER,missing_tokens INTEGER);
      CREATE INDEX temp.native_request_cell_scope ON native_request_cells(scope,bucket);
      CREATE TEMP TRIGGER native_request_insert AFTER INSERT ON current WHEN new.kind='native' BEGIN INSERT INTO native_request_dirty SELECT c.scope,new.v/4096 FROM native_request_clock c WHERE NOT EXISTS(SELECT 1 FROM native_request_dirty d WHERE d.scope=c.scope AND d.bucket=new.v/4096);END;
      CREATE TEMP TRIGGER native_request_delete AFTER DELETE ON current WHEN old.kind='native' BEGIN INSERT INTO native_request_dirty SELECT c.scope,old.v/4096 FROM native_request_clock c WHERE NOT EXISTS(SELECT 1 FROM native_request_dirty d WHERE d.scope=c.scope AND d.bucket=old.v/4096);END;
      CREATE TEMP TRIGGER native_request_update AFTER UPDATE OF v ON current WHEN old.kind='native' AND old.v<>new.v BEGIN INSERT INTO native_request_dirty SELECT c.scope,old.v/4096 FROM native_request_clock c WHERE NOT EXISTS(SELECT 1 FROM native_request_dirty d WHERE d.scope=c.scope AND d.bucket=old.v/4096);INSERT INTO native_request_dirty SELECT c.scope,new.v/4096 FROM native_request_clock c WHERE NOT EXISTS(SELECT 1 FROM native_request_dirty d WHERE d.scope=c.scope AND d.bucket=new.v/4096);END;
      CREATE INDEX temp.request_gateway_match ON record_meta(gateway_match) WHERE gateway_match IS NOT NULL;
      CREATE TEMP TRIGGER unified_request_gateway_update AFTER UPDATE OF v ON current
      WHEN new.kind='gateway' AND old.v<>new.v BEGIN
        INSERT INTO native_request_dirty
        SELECT DISTINCT w.scope,n.v/4096 FROM native_request_clock w,record_meta n
        JOIN current c ON c.v=n.v AND c.kind='native'
        WHERE n.gateway_match=new.id AND json_extract(json_extract(w.scope,'$[0]'),'$.source') IN ('unified','overview-unified')
          AND NOT EXISTS(SELECT 1 FROM native_request_dirty d WHERE d.scope=w.scope AND d.bucket=n.v/4096);
      END;").map_err(fail)
}

impl History {
    pub(super) fn native_request_cells(
        &self,
        q: &requests::Query,
        now: DateTime<Utc>,
    ) -> Result<Option<String>, String> {
        self.native_request_cells_for(q, now, None, false)
    }

    pub(super) fn native_request_cells_for(
        &self,
        q: &requests::Query,
        now: DateTime<Utc>,
        layout: Option<(DateTime<Utc>, DateTime<Utc>, Duration)>,
        unified: bool,
    ) -> Result<Option<String>, String> {
        self.native_cells(q, now, layout, unified, false)
    }

    pub(super) fn native_overview_cells(
        &self,
        q: &requests::Query,
        now: DateTime<Utc>,
        layout: (DateTime<Utc>, DateTime<Utc>, Duration),
    ) -> Result<Option<String>, String> {
        self.native_cells(q, now, Some(layout), true, true)
    }

    fn native_cells(
        &self,
        q: &requests::Query,
        now: DateTime<Utc>,
        layout: Option<(DateTime<Utc>, DateTime<Utc>, Duration)>,
        unified: bool,
        overview: bool,
    ) -> Result<Option<String>, String> {
        let predicate = dashboard::filter(q, now, true);
        let layout = match layout {
            Some(layout) => layout,
            None => self.request_layout(
                &dashboard::native_request_scope(&predicate, unified, false),
                q,
                now,
            )?,
        };
        let (start, end, step) = layout;
        let block_source = if overview {
            let agent = q.agent.as_deref().map_or("1".into(), |a| {
                format!("agent_group_matches({},agent)", quote(a))
            });
            let source = overview_scope(
                &agent,
                start.timestamp_millis(),
                end.timestamp_millis(),
                unified,
                true,
            );
            format!(
                "(SELECT *,NULL provider,CASE WHEN gateway_match IS NOT NULL THEN 'response_id' END association,NULL usage_conflict,NULL native_response FROM {source})"
            )
        } else {
            dashboard::native_request_scope(&predicate, unified, true)
        };
        let mut key_query = q.clone();
        key_query.metric = None;
        key_query.dimension = None;
        key_query.sort = None;
        key_query.descending = None;
        key_query.currency = None;
        let mut scope_key = |source: &str| -> Result<String, String> {
            key_query.source = Some(source.into());
            Ok(quote(&encode(&(
                request_cache::query_key(&key_query)?,
                start.to_rfc3339(),
                step.num_seconds(),
            ))?))
        };
        let (native_source, selected_source) = if overview {
            ("overview-native", "overview-unified")
        } else {
            ("native", "unified")
        };
        let key = scope_key(if unified {
            selected_source
        } else {
            native_source
        })?;
        let base = if unified {
            // Unlinked blocks have identical membership. Only rebuild the
            // blocks that contain precise matches, using the same boundaries.
            if self
                .native_cells(q, now, Some(layout), false, overview)?
                .is_none()
            {
                return Ok(None);
            }
            Some(scope_key(native_source)?)
        } else {
            None
        };
        let match_blocks = base.as_ref().map(|base| {
            format!(
                "SELECT DISTINCT bucket FROM native_request_cells WHERE scope={base} AND matched>0"
            )
        });
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
            return Ok((!previous.unwrap().2)
                .then(|| cells(&key, base.as_deref(), match_blocks.as_deref())));
        }
        self.db
            .execute_batch("SAVEPOINT native_request_blocks")
            .map_err(fail)?;
        let result = (|| {
            if !reuse || previous.as_ref().is_some_and(|p| p.2) {
                let blocks = match_blocks.clone().unwrap_or_else(|| if overview {
                    format!("SELECT DISTINCT v/{BLOCK} bucket FROM native_overview_facts")
                } else {
                    format!("SELECT DISTINCT r.v/{BLOCK} bucket FROM records r JOIN current c ON c.v=r.v WHERE r.kind='native'")
                });
                self.db.execute_batch(&format!("DELETE FROM native_request_cells WHERE scope={key};DELETE FROM native_request_dirty WHERE scope={key};
                  INSERT INTO native_request_dirty SELECT {key},bucket FROM ({blocks});")).map_err(fail)?;
            }
            execute(&self.db,&format!("DELETE FROM native_request_cells WHERE scope={key} AND bucket IN (SELECT bucket FROM native_request_dirty WHERE scope={key})"),[]).map_err(fail)?;
            let needs_rebuild = match_blocks
                .as_ref()
                .map_or_else(String::new, |blocks| format!("AND bucket IN ({blocks})"));
            let dirty: Vec<i64> = {
                let mut statement = self
                    .db
                    .prepare(&format!(
                        "SELECT bucket FROM native_request_dirty WHERE scope={key} {needs_rebuild} ORDER BY bucket"
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
            let (totals, dimensions, unsafe_rows, known, missing) = if overview {
                (
                    "json_object('requests',count(*),'usage',json_object('totalTokens',sum(tokens)))",
                    "agent,model",
                    "0",
                    "count(tokens)",
                    "count(*) FILTER(WHERE tokens IS NULL OR partial)",
                )
            } else {
                (
                    "summary_block_sum(stats,cache_in,cache_read)",
                    "strftime('%Y-%m-%d',time/1000.0,'unixepoch','+8 hours'),agent,model,provider",
                    "max(ttft_sample IS NOT NULL OR duration_sample IS NOT NULL OR throughput_sample IS NOT NULL OR (time IS NOT NULL AND strftime('%Y-%m-%d',time/1000.0,'unixepoch','+8 hours') IS NULL))",
                    "0",
                    "0",
                )
            };
            for bucket in dirty {
                let lo = bucket * BLOCK;
                let hi = lo.saturating_add(BLOCK);
                // Millisecond boundaries retain the original nanosecond offset.
                // A cell never crosses either a chart bin or a UTC+8 calendar day.
                let bin = if overview {
                    format!("(time-({origin}))/{width}")
                } else {
                    format!(
                        "CASE WHEN time IS NOT NULL THEN (time-({origin})-CASE WHEN substr(instant,-7,6)<{} THEN 1 ELSE 0 END)/{width} END",
                        quote(&residual)
                    )
                };
                count+=execute(&self.db,&format!("WITH totals AS (
                  SELECT agent,model,provider,min(id) id,min(instant) instant,
                    {totals} stats,
                    count(*) FILTER(WHERE association='response_id') matched,
                    count(*) FILTER(WHERE association IN ('ambiguous','identity_conflict')) ambiguous,
                    count(*) FILTER(WHERE usage_conflict=1) conflicts,
                    {unsafe_rows} unsafe,
                    count(*) FILTER(WHERE native_response IS NULL) missing_response,
                    count(*) FILTER(WHERE native_response IS NOT NULL AND association='unlinked') no_gateway,
                    count(*) FILTER(WHERE association='identity_conflict') identity_conflict,
                    {known} known_tokens,
                    {missing} missing_tokens
                  FROM {block_source} WHERE v>={lo} AND v<{hi}
                  GROUP BY {bin},{dimensions})
                  INSERT INTO native_request_cells SELECT {key},{bucket},g.agent,g.model,g.provider,g.id,c.v,b.label,g.instant,g.stats,g.matched,g.ambiguous,g.conflicts,g.unsafe,g.missing_response,g.no_gateway,g.identity_conflict,g.known_tokens,g.missing_tokens
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
            let protected = base
                .as_ref()
                .map_or_else(|| key.clone(), |base| format!("{key},{base}"));
            let retain = if base.is_some() { 6 } else { 7 };
            self.db.execute_batch(&format!("DELETE FROM native_request_dirty WHERE scope={key};
              CREATE TEMP TABLE IF NOT EXISTS request_evicted(scope TEXT PRIMARY KEY);DELETE FROM request_evicted;
              INSERT INTO request_evicted SELECT scope FROM native_request_clock WHERE scope NOT IN ({protected}) ORDER BY created DESC,scope LIMIT -1 OFFSET {retain};
              DELETE FROM native_request_cells WHERE scope IN request_evicted;DELETE FROM native_request_dirty WHERE scope IN request_evicted;DELETE FROM native_request_clock WHERE scope IN request_evicted;
              RELEASE native_request_blocks;")).map_err(fail)?;
            Ok((!fallback).then(|| cells(&key, base.as_deref(), match_blocks.as_deref())))
        })();
        if result.is_err() {
            let _ = self
                .db
                .execute_batch("ROLLBACK TO native_request_blocks;RELEASE native_request_blocks");
        }
        result
    }
}
pub(super) fn overview_scope(
    agent: &str,
    start: i64,
    end: i64,
    unified: bool,
    block_scan: bool,
) -> String {
    let interval = format!("time>={start} AND time<{end}");
    let fold = if unified {
        format!("AND (n.gateway_match IS NULL OR NOT EXISTS(
          SELECT 1 FROM analyses WHERE id=n.gateway_match AND kind='gateway' AND {agent} AND {interval}
        ))")
    } else {
        String::new()
    };
    let index = if block_scan { "NOT INDEXED" } else { "" };
    format!("(SELECT n.* FROM native_overview_facts n {index} WHERE {agent} AND {interval} {fold})")
}

fn cells(key: &str, base: Option<&str>, match_blocks: Option<&str>) -> String {
    let scope = match (base, match_blocks) {
        (Some(base), Some(blocks)) => {
            format!("scope={key} OR (scope={base} AND bucket NOT IN ({blocks}))")
        }
        _ => format!("scope={key}"),
    };
    format!(
        "(SELECT id,v,instant_ms(instant) time,instant started,instant,agent,model,provider,json_object('provider',label) value,stats,json_extract(stats,'$._cacheInput') cache_in,json_extract(stats,'$._cacheRead') cache_read,NULL ttft_sample,NULL duration_sample,NULL throughput_sample,matched,ambiguous,conflicts,missing_response,no_gateway,identity_conflict,known_tokens,missing_tokens FROM native_request_cells WHERE {scope})"
    )
}
