//! Rebuildable, connection-private SQLite read models. Source JSON is authoritative.
//! TEMP tables spill to SQLite's private temporary files; no native body is copied.
pub(super) mod aggregates;
#[cfg(test)]
mod cny_tests;
mod dashboard;
mod export;
mod fees;
mod native_usage;
mod overview;
mod overview_cache;
pub(super) mod raw_tracking;
mod request_cache;
mod request_cells;
#[cfg(test)]
mod retention_tests;
#[cfg(test)]
mod tests;
pub mod unified_requests;
mod usage;
mod usage_blocks;
mod usage_facts;
use super::{ModelObservation, ObservationStore};
use crate::{provider_usage, requests, sessions::Usage};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    path::Path,
};

pub struct History {
    request_projection: requests::domain_read::RequestProjection,
    pub(super) directory: std::sync::Arc<crate::agents::AgentDirectory>,
    identity: std::fs::Metadata,
    content_epoch: super::content_epoch::ContentEpoch,
    pub(super) db: Connection,
    watermark: Option<i64>,
    pub invalid: usize,
    pub(super) native_revision: Option<String>,
    pub(super) native_analysis_revision: Option<u64>,
    pub(super) evidence: String,
    fee_revision: Option<u64>,
    fee_calls: crate::sessions::CallCatalog,
    fee_version: i64,
    fee_versions: Vec<(u64, i64, i64)>,
    native_provider: Option<native_usage::Snapshot>,
    native_model: Option<native_usage::Snapshot>,
    native_model_fee_epoch: Option<String>,
    native_analysis_calls: crate::sessions::CallCatalog,
    native_request_calls: crate::sessions::CallCatalog,
    provider_cache: Vec<usage::UsageCache>,
    model_cache: Vec<usage::UsageCache>,
    snapshots: HashMap<String, dashboard::Frozen>,
    unified_requests: HashMap<String, unified_requests::Frozen>,
    exports: HashMap<String, export::ExportLease>,
    unified_exports: HashMap<String, unified_requests::export::ExportLease>,
    request_templates: Vec<dashboard::Frozen>,
    request_reports: RefCell<Vec<request_cache::Report>>,
    fee_summaries: RefCell<Vec<(String, Value)>>,
    analysis_cache: RefCell<Vec<overview_cache::Cached>>,
    timing_cache: RefCell<Vec<overview_cache::Cached>>,
    #[cfg(test)]
    pub decoded: usize,
}
pub(super) fn fail(error: impl std::fmt::Display) -> String {
    #[cfg(test)]
    if std::env::var("LOCAL_AGENT_SQL_PROFILE").as_deref() == Ok("1") {
        eprintln!("SQL_FAILURE {error}");
    }
    let _ = error;
    "历史查询索引暂不可用；原始记录保持不变".into()
}
pub(super) fn encode<T: serde::Serialize + ?Sized>(v: &T) -> Result<String, String> {
    serde_json::to_string(v).map_err(fail)
}
pub(super) fn value<T: serde::Serialize>(v: &T) -> Result<Value, String> {
    serde_json::to_value(v).map_err(fail)
}
pub(super) fn timestamp(v: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(v)
        .ok()
        .map(|t| t.timestamp_millis())
}
pub(super) fn sql_count(r: &rusqlite::Row<'_>, idx: usize) -> rusqlite::Result<usize> {
    let n = r.get::<_, i64>(idx)?;
    usize::try_from(n).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(idx, rusqlite::types::Type::Integer, Box::new(e))
    })
}
fn execute(db: &Connection, sql: &str, params: impl rusqlite::Params) -> rusqlite::Result<usize> {
    db.prepare_cached(sql)?.execute(params)
}

impl History {
    /// Use the host's validated composition for both gateway and native filters.
    pub fn use_directory(
        &mut self,
        directory: &crate::agents::AgentDirectory,
    ) -> Result<(), String> {
        if self.directory.as_ref() != directory {
            let directory = std::sync::Arc::new(directory.clone());
            aggregates::register_directory(&self.db, directory.clone())?;
            self.directory = directory;
            self.request_templates.clear();
            self.request_reports.borrow_mut().clear();
            self.fee_summaries.borrow_mut().clear();
            self.fee_versions.clear();
            self.analysis_cache.borrow_mut().clear();
            self.timing_cache.borrow_mut().clear();
            self.provider_cache.clear();
            self.model_cache.clear();
            self.db.execute_batch("DELETE FROM native_request_cells;DELETE FROM native_request_dirty;DELETE FROM native_request_clock;").map_err(fail)?;
            for kind in ["provider", "model"] {
                let table = format!("native_{kind}_blocks");
                let exists: bool = self
                    .db
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM sqlite_temp_master WHERE name=?1)",
                        [&table],
                        |r| r.get(0),
                    )
                    .map_err(fail)?;
                if exists {
                    self.db.execute_batch(&format!("DELETE FROM {table};DELETE FROM {table}_dirty;DELETE FROM {table}_clock;")).map_err(fail)?;
                }
            }
        }
        Ok(())
    }
    pub fn open(directory: &Path) -> Result<Self, String> {
        Self::open_with_request_projection(
            directory,
            requests::domain_read::RequestProjection::default(),
        )
    }
    pub fn open_with_request_projection(
        directory: &Path,
        request_projection: requests::domain_read::RequestProjection,
    ) -> Result<Self, String> {
        let store = ObservationStore::open(directory)?;
        let db = store.0;
        db.set_prepared_statement_cache_capacity(96);
        aggregates::register(&db)?;
        #[cfg(test)]
        if std::env::var("LOCAL_AGENT_SQL_PROFILE").as_deref() == Ok("1") {
            db.trace_v2(rusqlite::trace::TraceEventCodes::SQLITE_TRACE_PROFILE, Some(|event| {
                if let rusqlite::trace::TraceEvent::Profile(statement, elapsed) = event
                    && elapsed.as_millis() >= 50 {
                    eprintln!("SQL_PROFILE {}", json!({"ms":elapsed.as_millis(),"sorts":statement.get_status(rusqlite::StatementStatus::Sort),"steps":statement.get_status(rusqlite::StatementStatus::VmStep),"sql":statement.sql()}));
                }
            }));
        }
        // Coalesced changes include deletion and same-revision external edits.
        // Installing these tables never rewrites the authoritative JSON values.
        db.execute_batch("CREATE TABLE IF NOT EXISTS observation_query_clock(id INTEGER PRIMARY KEY CHECK(id=1),seq INTEGER NOT NULL);
          INSERT OR IGNORE INTO observation_query_clock VALUES(1,0);
          CREATE TABLE IF NOT EXISTS observation_query_changes(id TEXT PRIMARY KEY,seq INTEGER NOT NULL);
          CREATE INDEX IF NOT EXISTS observation_query_changes_seq ON observation_query_changes(seq);
          ").map_err(fail)?;
        for (_, sql) in raw_tracking::TRIGGERS {
            db.execute_batch(&sql.replacen("CREATE TRIGGER ", "CREATE TRIGGER IF NOT EXISTS ", 1))
                .map_err(fail)?;
        }
        db.execute_batch("PRAGMA temp_store=FILE;
          PRAGMA temp.cache_size=-32768;
          CREATE TEMP TABLE attempts(id TEXT PRIMARY KEY,logical TEXT NOT NULL,revision TEXT NOT NULL,raw TEXT NOT NULL,family TEXT,product TEXT,session TEXT,response TEXT,provider_record TEXT,model_record TEXT);
          CREATE TEMP TABLE attempt_clock(seq INTEGER NOT NULL);INSERT INTO attempt_clock VALUES(0);
          CREATE TEMP TRIGGER attempt_data_insert AFTER INSERT ON attempts BEGIN UPDATE attempt_clock SET seq=seq+1;END;
          CREATE TEMP TRIGGER attempt_data_delete AFTER DELETE ON attempts BEGIN UPDATE attempt_clock SET seq=seq+1;END;
          CREATE TEMP TRIGGER attempt_data_update AFTER UPDATE ON attempts BEGIN UPDATE attempt_clock SET seq=seq+1;END;
          CREATE INDEX temp.attempts_logical ON attempts(logical);
          CREATE TEMP TABLE attempt_changes(id TEXT PRIMARY KEY);
          CREATE TEMP TABLE legacy_pending(id TEXT PRIMARY KEY);
          CREATE TEMP TABLE query_clock(seq INTEGER NOT NULL);
          INSERT INTO query_clock VALUES(0);
          CREATE TEMP TABLE usage_changes(id TEXT PRIMARY KEY,seq INTEGER NOT NULL);
          CREATE INDEX temp.usage_changes_seq ON usage_changes(seq);
          CREATE TEMP TABLE usage_projection_clock(kind TEXT PRIMARY KEY,seq INTEGER NOT NULL);
          CREATE TEMP TRIGGER attempts_insert AFTER INSERT ON attempts BEGIN INSERT OR IGNORE INTO attempt_changes VALUES(new.id);UPDATE query_clock SET seq=seq+1;INSERT INTO usage_changes SELECT new.id,(SELECT seq FROM query_clock) WHERE EXISTS(SELECT 1 FROM usage_projection_clock WHERE kind IN ('provider_facts','model_facts')) ON CONFLICT(id) DO UPDATE SET seq=excluded.seq;END;
          CREATE TEMP TRIGGER attempts_delete AFTER DELETE ON attempts BEGIN INSERT OR IGNORE INTO attempt_changes VALUES(old.id);UPDATE query_clock SET seq=seq+1;INSERT INTO usage_changes SELECT old.id,(SELECT seq FROM query_clock) WHERE EXISTS(SELECT 1 FROM usage_projection_clock WHERE kind IN ('provider_facts','model_facts')) ON CONFLICT(id) DO UPDATE SET seq=excluded.seq;END;
          CREATE TEMP TRIGGER attempts_usage_update AFTER UPDATE OF provider_record,model_record ON attempts BEGIN UPDATE query_clock SET seq=seq+1;INSERT INTO usage_changes SELECT new.id,(SELECT seq FROM query_clock) WHERE EXISTS(SELECT 1 FROM usage_projection_clock WHERE kind IN ('provider_facts','model_facts')) ON CONFLICT(id) DO UPDATE SET seq=excluded.seq;END;
          CREATE TEMP TABLE analyses(id TEXT PRIMARY KEY,kind TEXT,agent TEXT,model TEXT,time INTEGER,tokens TEXT,partial INTEGER,provisional INTEGER,cost TEXT,attempts INTEGER);
          CREATE TEMP TABLE native_analysis_clock(seq INTEGER NOT NULL);INSERT INTO native_analysis_clock VALUES(0);
          CREATE TEMP TRIGGER native_analysis_insert AFTER INSERT ON analyses WHEN new.kind='native' BEGIN UPDATE native_analysis_clock SET seq=seq+1;END;
          CREATE TEMP TRIGGER native_analysis_delete AFTER DELETE ON analyses WHEN old.kind='native' BEGIN UPDATE native_analysis_clock SET seq=seq+1;END;
          CREATE TEMP TRIGGER native_analysis_update AFTER UPDATE ON analyses WHEN old.kind='native' OR new.kind='native' BEGIN UPDATE native_analysis_clock SET seq=seq+1;END;
          CREATE TEMP TABLE invalid_rows(id TEXT PRIMARY KEY);
          CREATE TEMP TABLE dirty_groups(id TEXT PRIMARY KEY);
          CREATE TEMP TABLE dirty_links(family TEXT,response TEXT,PRIMARY KEY(family,response));
          CREATE TEMP TABLE fee_responses(family TEXT,response TEXT,PRIMARY KEY(family,response));
          CREATE TEMP TABLE dirty_context(id TEXT PRIMARY KEY);
          CREATE TEMP TABLE native_base(id TEXT PRIMARY KEY,family TEXT,response TEXT,v INTEGER,model TEXT,provider TEXT,label TEXT);
          CREATE INDEX temp.native_base_version ON native_base(v);
          CREATE INDEX temp.native_base_model ON native_base(model);
          CREATE INDEX temp.native_base_provider ON native_base(provider,id,v);
          CREATE TEMP VIEW native_options AS SELECT v,id,model,provider,json_object('provider',label) value FROM native_base;
          CREATE INDEX temp.native_base_response ON native_base(family,response);
          CREATE TEMP TABLE contexts(id TEXT PRIMARY KEY,value TEXT);
          CREATE TEMP TABLE record_meta(v INTEGER PRIMARY KEY,kind TEXT NOT NULL,id TEXT NOT NULL,stats TEXT NOT NULL,time INTEGER,agent TEXT,family TEXT,product TEXT,status TEXT,model TEXT,provider TEXT,session TEXT,project TEXT,search TEXT,linked INTEGER NOT NULL DEFAULT 0,cache_in TEXT,cache_read TEXT,cost_value REAL,ttft_sample REAL,duration_sample REAL,throughput_sample REAL,started TEXT,instant TEXT,association TEXT,usage_conflict INTEGER,domain_usage TEXT NOT NULL,domain_link_count INTEGER NOT NULL,domain_link_conflicts INTEGER NOT NULL,domain_duration REAL,domain_ttft REAL);
          CREATE TEMP TABLE record_payloads(v INTEGER PRIMARY KEY,value TEXT NOT NULL);
          CREATE TEMP VIEW records AS SELECT m.*,(SELECT p.value FROM record_payloads p WHERE p.v=m.v) value FROM record_meta m;
          CREATE TEMP TABLE current(kind TEXT,id TEXT,v INTEGER UNIQUE,PRIMARY KEY(kind,id));
          CREATE INDEX temp.current_id ON current(id,kind);
          CREATE TEMP TABLE request_clock(seq INTEGER NOT NULL);INSERT INTO request_clock VALUES(0);
          CREATE TEMP TRIGGER request_insert AFTER INSERT ON current BEGIN UPDATE request_clock SET seq=seq+1;END;
          CREATE TEMP TRIGGER request_delete AFTER DELETE ON current BEGIN UPDATE request_clock SET seq=seq+1;END;
          CREATE TEMP TRIGGER request_update AFTER UPDATE ON current WHEN old.v<>new.v OR old.kind<>new.kind OR old.id<>new.id BEGIN UPDATE request_clock SET seq=seq+1;END;
          CREATE TEMP TABLE garbage(v INTEGER PRIMARY KEY);
          CREATE TEMP TRIGGER retired_delete AFTER DELETE ON current BEGIN INSERT OR IGNORE INTO garbage VALUES(old.v); END;
          CREATE TEMP TRIGGER retired_update AFTER UPDATE OF v ON current WHEN old.v<>new.v BEGIN INSERT OR IGNORE INTO garbage VALUES(old.v); END;
          CREATE TEMP TABLE fragments(v INTEGER,dimension TEXT,key TEXT,label TEXT,stats TEXT,PRIMARY KEY(v,dimension,key));
          CREATE TEMP TABLE position_clock(seq INTEGER NOT NULL);INSERT INTO position_clock VALUES(0);
          CREATE TEMP TABLE positions(snapshot INTEGER,pos INTEGER,v INTEGER,PRIMARY KEY(snapshot,pos)) WITHOUT ROWID;
          CREATE INDEX temp.positions_version ON positions(v);") .map_err(fail)?;
        unified_requests::init(&db)?;
        request_cells::init(&db)?;
        let identity = std::fs::metadata(directory.join("observations.sqlite3")).map_err(fail)?;
        let content_epoch = super::content_epoch::read(&db).map_err(|_| "历史内容版本不可读取")?;
        Ok(Self {
            request_projection,
            directory: std::sync::Arc::new(crate::agents::builtin_directory().clone()),
            identity,
            content_epoch,
            db,
            watermark: None,
            invalid: 0,
            native_revision: None,
            native_analysis_revision: None,
            evidence: String::new(),
            fee_revision: None,
            fee_calls: Default::default(),
            fee_version: 0,
            fee_versions: vec![],
            native_provider: None,
            native_model: None,
            native_model_fee_epoch: None,
            native_analysis_calls: Default::default(),
            native_request_calls: Default::default(),
            provider_cache: vec![],
            model_cache: vec![],
            snapshots: HashMap::new(),
            unified_requests: HashMap::new(),
            exports: HashMap::new(),
            unified_exports: HashMap::new(),
            request_templates: vec![],
            request_reports: RefCell::new(vec![]),
            fee_summaries: RefCell::new(vec![]),
            analysis_cache: RefCell::new(vec![]),
            timing_cache: RefCell::new(vec![]),
            #[cfg(test)]
            decoded: 0,
        })
    }
    /// Release shared native payloads after consent changes. Disposable native
    /// views rebuild on their next read; gateway history/projections stay intact.
    pub fn release_native_snapshots(&mut self) {
        self.request_templates.clear();
        self.request_reports.borrow_mut().clear();
        self.fee_summaries.borrow_mut().clear();
        self.fee_versions.clear();
        self.analysis_cache.borrow_mut().clear();
        self.timing_cache.borrow_mut().clear();
        self.fee_calls.clear();
        self.fee_revision = None;
        self.native_provider = None;
        self.native_model = None;
        self.native_model_fee_epoch = None;
        self.native_analysis_calls.clear();
        self.native_analysis_revision = None;
        self.native_request_calls.clear();
        self.native_revision = None;
        self.provider_cache
            .retain(|c| c.value["query"]["source"] != "native");
        self.model_cache
            .retain(|c| c.value["query"]["source"] != "native");
    }
    /// Bounded decoding during cold start; subsequent work touches changed IDs/groups only.
    pub fn refresh(&mut self, live: &[ModelObservation]) -> Result<(), String> {
        self.refresh_pending(live, &HashSet::new())
    }
    pub fn same_file(&self, directory: &Path) -> bool {
        std::fs::metadata(directory.join("observations.sqlite3")).is_ok_and(|meta| {
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                meta.dev() == self.identity.dev() && meta.ino() == self.identity.ino()
            }
            #[cfg(not(unix))]
            {
                meta.created().ok() == self.identity.created().ok()
            }
        })
    }
    /// A committed content token, never a value pinned by a leftover read
    /// transaction. The service drops this entire projection on false/error.
    pub fn content_is_current(&self) -> Result<bool, String> {
        if !self.db.is_autocommit() {
            return Err("历史内容版本读取仍在旧事务中".into());
        }
        super::content_epoch::read(&self.db)
            .map(|current| current == self.content_epoch)
            .map_err(|_| "历史内容版本不可确认，旧快照已停止提供".into())
    }
    pub fn content_epoch(&self) -> &super::content_epoch::ContentEpoch {
        &self.content_epoch
    }
    pub fn refresh_pending(
        &mut self,
        live: &[ModelObservation],
        pending: &HashSet<String>,
    ) -> Result<(), String> {
        self.db.execute_batch("BEGIN").map_err(fail)?;
        let result: Result<(), String> = (|| {
            let clock: i64 = self
                .db
                .query_row(
                    "SELECT seq FROM observation_query_clock WHERE id=1",
                    [],
                    |r| r.get(0),
                )
                .map_err(fail)?;
            if let Some(watermark) = self.watermark {
                let mut after = 0i64;
                loop {
                    let batch: Vec<(i64, String, Option<String>)> = {
                        let mut s=self.db.prepare("SELECT c.seq,c.id,m.value FROM observation_query_changes c LEFT JOIN model_observations m ON m.id=c.id WHERE c.seq>?1 AND c.seq<=?2 ORDER BY c.seq,c.id LIMIT 256").map_err(fail)?;
                        s.query_map(params![watermark.max(after), clock], |r| {
                            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
                        })
                        .map_err(fail)?
                        .collect::<Result<_, _>>()
                        .map_err(fail)?
                    };
                    if batch.is_empty() {
                        break;
                    }
                    after = batch.last().unwrap().0;
                    for (_, id, raw) in batch {
                        self.ingest(&id, raw.as_deref())?;
                    }
                }
            } else {
                let mut after = None::<String>;
                loop {
                    let batch: Vec<(String, String)> = {
                        let sql = if after.is_some() {
                            "SELECT id,value FROM model_observations WHERE id>?1 ORDER BY id LIMIT 256"
                        } else {
                            "SELECT id,value FROM model_observations WHERE ?1 IS NULL ORDER BY id LIMIT 256"
                        };
                        let mut s = self.db.prepare(sql).map_err(fail)?;
                        s.query_map([after.as_deref()], |r| Ok((r.get(0)?, r.get(1)?)))
                            .map_err(fail)?
                            .collect::<Result<_, _>>()
                            .map_err(fail)?
                    };
                    if batch.is_empty() {
                        break;
                    }
                    after = Some(batch.last().unwrap().0.clone());
                    for (id, raw) in batch {
                        self.ingest_cold(&id, &raw)?;
                    }
                }
            }
            for row in live {
                if row.status == "in_progress" || pending.contains(&row.id) {
                    self.ingest_live(row)?;
                }
            }
            self.rebuild_groups()?;
            if self.watermark.is_none() {
                // Bulk-build read indexes after loading. The group/current
                // primary keys remain available throughout the cold transaction.
                self.db.execute_batch("CREATE INDEX temp.attempts_response ON attempts(family,response);
                  CREATE INDEX temp.attempts_session ON attempts(session);
                  CREATE INDEX temp.attempts_time ON attempts(json_extract(raw,'$.startedAt'),id);
                  CREATE INDEX temp.analyses_time ON analyses(kind,time,agent);
                  CREATE INDEX temp.records_time ON record_meta(kind,time,id);
                  CREATE INDEX temp.records_agent ON record_meta(kind,agent,time);
                  CREATE INDEX temp.records_linked ON record_meta(kind,linked,time);
                  CREATE INDEX temp.records_samples ON record_meta(v,ttft_sample,duration_sample,throughput_sample);
                  CREATE INDEX temp.records_session ON record_meta(kind,session);
                  CREATE INDEX temp.records_range ON record_meta(time,instant,v);
                  CREATE INDEX temp.records_exact_time ON record_meta(kind,time,instant,id);
                  CREATE INDEX temp.records_order ON record_meta(kind,(time IS NULL),time DESC,id);
                  CREATE INDEX temp.records_ttft ON record_meta(ttft_sample,v) WHERE ttft_sample IS NOT NULL;
                  CREATE INDEX temp.records_duration ON record_meta(duration_sample,v) WHERE duration_sample IS NOT NULL;
                  CREATE INDEX temp.records_throughput ON record_meta(throughput_sample,v) WHERE throughput_sample IS NOT NULL;
                  CREATE INDEX temp.fragments_key ON fragments(dimension,key,v);").map_err(fail)?;
            }
            let invalid = self
                .db
                .query_row("SELECT count(*) FROM invalid_rows", [], |r| sql_count(r, 0))
                .map_err(fail)?;
            self.db.execute_batch("COMMIT").map_err(fail)?;
            self.watermark = Some(clock);
            self.invalid = invalid;
            Ok(())
        })();
        if result.is_err() {
            let _ = self.db.execute_batch("ROLLBACK");
        }
        result?;
        self.prune()
    }
    fn mark_group(&self, id: &str) -> Result<(), String> {
        execute(
            &self.db,
            "INSERT OR IGNORE INTO dirty_groups VALUES(?1)",
            [id],
        )
        .map_err(fail)?;
        Ok(())
    }
    fn ingest_cold(&mut self, id: &str, raw: &str) -> Result<(), String> {
        // Unrelated requests have unique IDs. Explicit retry keys always use
        // the reserved logical- prefix; leave that whole namespace on the
        // normal group rebuild path, including deliberately colliding IDs.
        let Ok(mut row) = serde_json::from_str::<ModelObservation>(raw) else {
            return self.ingest(id, Some(raw));
        };
        if row.id != id || row.logical_id.is_some() || id.starts_with("logical-") {
            return self.ingest(id, Some(raw));
        }
        #[cfg(test)]
        {
            self.decoded += 1;
        }
        if row.status == "in_progress" {
            row.status = "interrupted".into();
            row.timing_issue = Some("当前进程没有该历史请求的活跃时序，结果未确认".into());
        }
        self.insert_attempt(&row, false)?;
        self.update_analysis(id, std::slice::from_ref(&row))?;
        if row.session_id.is_some() {
            execute(
                &self.db,
                "INSERT OR IGNORE INTO dirty_context VALUES(?1)",
                [id],
            )
            .map_err(fail)?;
        }
        for request in requests::logical_requests(vec![row], &HashMap::new()) {
            let request = self.request_projection.project(request)?;
            self.insert_request(&request)?;
        }
        Ok(())
    }
    fn ingest(&mut self, id: &str, raw: Option<&str>) -> Result<(), String> {
        #[cfg(test)]
        {
            self.decoded += usize::from(raw.is_some());
        }
        let old: Option<String> = self
            .db
            .query_row("SELECT logical FROM attempts WHERE id=?1", [id], |r| {
                r.get(0)
            })
            .optional()
            .map_err(fail)?;
        if let Some(old) = old {
            self.mark_group(&old)?;
        }
        execute(&self.db,"INSERT OR IGNORE INTO dirty_links SELECT family,response FROM attempts WHERE id=?1 AND response IS NOT NULL",[id]).map_err(fail)?;
        execute(&self.db,"INSERT OR IGNORE INTO fee_responses SELECT family,response FROM attempts WHERE id=?1 AND response IS NOT NULL",[id]).map_err(fail)?;
        execute(&self.db, "DELETE FROM attempts WHERE id=?1", [id]).map_err(fail)?;
        execute(&self.db, "DELETE FROM invalid_rows WHERE id=?1", [id]).map_err(fail)?;
        if let Some(raw) = raw {
            match serde_json::from_str::<ModelObservation>(raw) {
                Ok(mut row) if row.id == id => {
                    if row.status == "in_progress" {
                        row.status = "interrupted".into();
                        row.timing_issue =
                            Some("当前进程没有该历史请求的活跃时序，结果未确认".into());
                    }
                    self.insert_attempt(&row, true)?;
                }
                _ => {
                    execute(
                        &self.db,
                        "INSERT OR IGNORE INTO invalid_rows VALUES(?1)",
                        [id],
                    )
                    .map_err(fail)?;
                }
            }
        }
        Ok(())
    }
    fn ingest_live(&self, row: &ModelObservation) -> Result<(), String> {
        let old: Option<(String, String)> = self
            .db
            .query_row(
                "SELECT revision,raw FROM attempts WHERE id=?1",
                [&row.id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .map_err(fail)?;
        if old.as_ref().is_some_and(|(rev, raw)| {
            rev.parse::<u64>().is_ok_and(|rev| {
                rev > row.revision
                    || (rev == row.revision && encode(row).ok().as_ref() == Some(raw))
            })
        }) {
            return Ok(());
        }
        if let Some(old) = self
            .db
            .query_row("SELECT logical FROM attempts WHERE id=?1", [&row.id], |r| {
                r.get::<_, String>(0)
            })
            .optional()
            .map_err(fail)?
        {
            self.mark_group(&old)?;
        }
        execute(&self.db,"INSERT OR IGNORE INTO dirty_links SELECT family,response FROM attempts WHERE id=?1 AND response IS NOT NULL",[&row.id]).map_err(fail)?;
        execute(&self.db,"INSERT OR IGNORE INTO fee_responses SELECT family,response FROM attempts WHERE id=?1 AND response IS NOT NULL",[&row.id]).map_err(fail)?;
        self.insert_attempt(row, true)
    }
    fn insert_attempt(&self, row: &ModelObservation, dirty: bool) -> Result<(), String> {
        let key = requests::logical_request_key(row);
        if dirty {
            self.mark_group(&key)?;
        }
        if row.provider_id.is_none() {
            execute(
                &self.db,
                "INSERT OR IGNORE INTO legacy_pending VALUES(?1)",
                [&row.id],
            )
            .map_err(fail)?;
        }
        if let Some(response) = row.native_response_id() {
            execute(
                &self.db,
                "INSERT OR IGNORE INTO fee_responses VALUES(?1,?2)",
                params![row.agent_id, response],
            )
            .map_err(fail)?;
            execute(
                &self.db,
                "INSERT OR IGNORE INTO dirty_links VALUES(?1,?2)",
                params![row.agent_id, response],
            )
            .map_err(fail)?;
        }
        let record = provider_usage::gateway_records(vec![row.clone()], &[]).remove(0);
        let provider = usage::provider_record(&record)?;
        let model_cost = crate::provider_cost::calculate(
            Some(&row.endpoint),
            row.model.as_deref(),
            Some(&row.started_at),
            &row.status,
            &row.usage,
            row.reported_cost.as_ref(),
            row.ended_at.as_deref(),
        );
        let model = usage::model_record(row, &model_cost)?;
        execute(
            &self.db,
            "INSERT OR REPLACE INTO attempts VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
            params![
                row.id,
                key,
                row.revision.to_string(),
                encode(row)?,
                row.agent_id,
                row.agent_product_id,
                row.session_id,
                row.native_response_id(),
                provider,
                model
            ],
        )
        .map_err(fail)?;
        Ok(())
    }
    fn rebuild_groups(&mut self) -> Result<(), String> {
        loop {
            let keys: Vec<String> = {
                let mut s = self
                    .db
                    .prepare("SELECT id FROM dirty_groups LIMIT 128")
                    .map_err(fail)?;
                s.query_map([], |r| r.get(0))
                    .map_err(fail)?
                    .collect::<Result<_, _>>()
                    .map_err(fail)?
            };
            if keys.is_empty() {
                break;
            }
            for key in keys {
                let rows: Vec<ModelObservation> = {
                    let mut s = self
                        .db
                        .prepare_cached("SELECT raw FROM attempts WHERE logical=?1 ORDER BY id")
                        .map_err(fail)?;
                    let raws = s
                        .query_map([&key], |r| r.get::<_, String>(0))
                        .map_err(fail)?;
                    raws.map(|r| serde_json::from_str(&r.map_err(fail)?).map_err(fail))
                        .collect::<Result<_, _>>()?
                };
                execute(
                    &self.db,
                    "DELETE FROM current WHERE kind='gateway' AND id=?1",
                    [&key],
                )
                .map_err(fail)?;
                self.update_analysis(&key, &rows)?;
                execute(&self.db,"INSERT OR IGNORE INTO attempt_changes SELECT id FROM attempts WHERE logical=?1",[&key]).map_err(fail)?;
                for row in requests::logical_requests(rows, &HashMap::new()) {
                    let row = self.request_projection.project(row)?;
                    self.insert_request(&row)?;
                }
                execute(&self.db, "DELETE FROM dirty_groups WHERE id=?1", [&key]).map_err(fail)?;
                execute(
                    &self.db,
                    "INSERT OR IGNORE INTO dirty_context VALUES(?1)",
                    [&key],
                )
                .map_err(fail)?;
                execute(&self.db,"INSERT OR IGNORE INTO dirty_links SELECT family,response FROM attempts WHERE logical=?1 AND response IS NOT NULL",[&key]).map_err(fail)?;
            }
        }
        Ok(())
    }
    pub(super) fn insert_request(&self, row: &requests::Request) -> Result<i64, String> {
        let stats = requests::Summary::of([row]);
        let (mut input, mut cached) = (0u128, 0u128);
        for u in row
            .native
            .as_ref()
            .map(|_| &row.usage)
            .into_iter()
            .chain(row.attempts.iter().map(|a| &a.observation.usage))
        {
            if let Some((i, c)) = u
                .input_tokens
                .zip(u.cached_input_tokens)
                .filter(|(i, c)| c <= i)
            {
                input += u128::from(i);
                cached += u128::from(c);
            }
        }
        let search = format!(
            "{} {} {} {} {} {}",
            row.id,
            row.model.as_deref().unwrap_or(""),
            row.provider,
            row.session_title.as_deref().unwrap_or(""),
            row.client
                .as_ref()
                .map(|c| format!("{} {} {}", c.id, c.name, c.peer))
                .unwrap_or_default(),
            row.attempts
                .iter()
                .map(|a| a.observation.id.as_str())
                .collect::<Vec<_>>()
                .join(" ")
        )
        .to_lowercase();
        let raw = encode(row)?;
        execute(&self.db,"INSERT INTO record_meta(kind,id,stats,time,agent,family,product,status,model,provider,session,project,search,linked,cache_in,cache_read,cost_value,ttft_sample,duration_sample,throughput_sample,started,instant,association,usage_conflict,domain_usage,domain_link_count,domain_link_conflicts,domain_duration,domain_ttft) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,instant_key(?21),?22,?23,?24,?25,?26,?27,?28)",params![row.source,row.id,encode(&stats)?,timestamp(&row.started_at),self.directory.group(&row.agent_id,row.agent_product_id.as_deref()),row.agent_id,row.agent_product_id,row.status,row.model,row.provider_key,row.session_id,row.project_id,search,!row.native_links.is_empty(),input.to_string(),cached.to_string(),aggregates::cost_metric(&row.cost,"USD"),stats.ttft.p50,stats.duration.p50,stats.throughput.p50,row.started_at,row.native.as_ref().map(|n|n.association.as_str()),row.native.as_ref().map(|n|n.usage_conflict),unified_requests::usage_metadata(row),i64::try_from(row.native_links.len()).map_err(fail)?,i64::try_from(row.native_links.iter().filter(|l|l.usage_conflict).count()).map_err(fail)?,row.duration_ms.filter(|v|v.is_finite()),row.ttft_ms.filter(|v|v.is_finite())]).map_err(fail)?;
        let version = self.db.last_insert_rowid();
        execute(
            &self.db,
            "INSERT INTO record_payloads VALUES(?1,?2)",
            params![version, raw],
        )
        .map_err(fail)?;
        execute(
            &self.db,
            "INSERT INTO current VALUES(?1,?2,?3) ON CONFLICT(kind,id) DO UPDATE SET v=excluded.v",
            params![row.source, row.id, version],
        )
        .map_err(fail)?;
        dashboard::insert_fragments(&self.db, version, row)?;
        Ok(version)
    }
    /// Only the session identity and request metadata observed by this gateway.
    /// Native response enrichment is authorized separately by the session reader.
    pub fn gateway_sessions(&self) -> Result<(Vec<crate::sessions::SessionSummary>, bool), String> {
        use crate::sessions::{SessionSummary, Usage};
        const LIMIT: usize = 10_000;
        let mut query = self
            .db
            .prepare_cached(
                "SELECT session,family,json_group_array(DISTINCT product),
                    min(json_extract(raw,'$.startedAt')),max(json_extract(raw,'$.startedAt')),
                    count(DISTINCT logical)
             FROM attempts
             WHERE session IS NOT NULL AND session LIKE family || ':%'
                   AND json_extract(raw,'$.client') IS NULL
             GROUP BY session,family
             ORDER BY 5 DESC,session LIMIT ?1",
            )
            .map_err(fail)?;
        let rows = query
            .query_map([(LIMIT + 1) as i64], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    sql_count(row, 5)?,
                ))
            })
            .map_err(fail)?;
        let mut sessions = Vec::new();
        for row in rows {
            if sessions.len() == LIMIT {
                return Ok((sessions, true));
            }
            let (id, agent_id, products, created_at, updated_at, model_calls) =
                row.map_err(fail)?;
            let Some(native_id) = id.strip_prefix(&format!("{agent_id}:")) else {
                continue;
            };
            let products: Vec<Option<String>> = serde_json::from_str(&products).map_err(fail)?;
            let has_unknown_client = products.iter().any(Option::is_none);
            let mut agent_products: Vec<_> = products.into_iter().flatten().collect();
            agent_products.sort();
            let agent_product_id = (!has_unknown_client && agent_products.len() == 1)
                .then(|| agent_products[0].clone());
            let title = format!("会话 {}", native_id.chars().take(8).collect::<String>());
            sessions.push(SessionSummary {
                project_hidden: false,
                native_id: native_id.to_owned(),
                id,
                agent_id,
                agent_product_id,
                agent_products,
                has_unknown_client,
                agent_version: None,
                title,
                title_source: "fallback".into(),
                project_path: None,
                project_name: None,
                project_id: None,
                project_root: None,
                git_remote: None,
                created_at,
                updated_at,
                status: "observed",
                message_count: 0,
                turn_count: None,
                model_calls,
                model_error_count: 0,
                latest_failure_id: None,
                tool_calls: 0,
                mcp_calls: 0,
                skill_calls: 0,
                usage: Usage::default(),
                usage_quality: "unknown",
                disk_bytes: 0,
                disk_allocated_bytes: None,
                disk_scope: "gateway_metadata",
                source_path: String::new(),
                partial: false,
            });
        }
        Ok((sessions, false))
    }
    pub fn observation_page(
        &self,
        agent: Option<&str>,
        id: Option<&str>,
        offset: usize,
    ) -> Result<Value, String> {
        let predicate = "agent_matches(?1,family,product) AND (?2 IS NULL OR id=?2)";
        let count = self
            .db
            .query_row(
                &format!("SELECT count(*) FROM attempts WHERE {predicate}"),
                params![agent, id],
                |r| sql_count(r, 0),
            )
            .map_err(fail)?;
        let mut s=self.db.prepare(&format!("SELECT raw FROM attempts WHERE {predicate} ORDER BY json_extract(raw,'$.startedAt') DESC,id DESC LIMIT 40 OFFSET ?3")).map_err(fail)?;
        let items = s
            .query_map(
                params![agent, id, i64::try_from(offset).unwrap_or(i64::MAX)],
                |r| r.get::<_, String>(0),
            )
            .map_err(fail)?
            .map(|r| serde_json::from_str::<Value>(&r.map_err(fail)?).map_err(fail))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(
            json!({"items":items,"total":count,"nextOffset":offset.checked_add(40).filter(|v|*v<count),"asOf":chrono::Utc::now().to_rfc3339()}),
        )
    }
    pub fn prune(&self) -> Result<(), String> {
        self.db.execute_batch("CREATE TEMP TABLE IF NOT EXISTS disposable(v INTEGER PRIMARY KEY);DELETE FROM disposable;INSERT INTO disposable SELECT g.v FROM garbage g WHERE NOT EXISTS(SELECT 1 FROM positions p WHERE p.v=g.v) AND NOT EXISTS(SELECT 1 FROM unified_request_positions p WHERE p.host_v=g.v) AND NOT EXISTS(SELECT 1 FROM current c WHERE c.v=g.v) AND NOT EXISTS(SELECT 1 FROM native_base b WHERE b.v=g.v);DELETE FROM fragments WHERE v IN disposable;DELETE FROM record_payloads WHERE v IN disposable;DELETE FROM record_meta WHERE v IN disposable;DELETE FROM garbage WHERE v IN disposable;").map_err(fail)
    }
    pub fn initialized(&self) -> bool {
        self.watermark.is_some()
    }
}
