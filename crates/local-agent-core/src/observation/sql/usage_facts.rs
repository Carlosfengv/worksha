//! Narrow, lazy usage projections. Payloads remain in attempts/native_usage;
//! aggregate/rank work never copies the original records into sort runs.
use super::*;

impl History {
    pub(super) fn usage_facts(&self, kind: &str, native: bool) -> Result<String, String> {
        let table = format!("{}{kind}_facts", if native { "native_" } else { "" });
        let source = self.usage_table(kind, native);
        let clock = self.usage_kind_clock(kind, native)?;
        let previous: Option<i64> = self
            .db
            .query_row(
                "SELECT seq FROM usage_projection_clock WHERE kind=?1",
                [&table],
                |r| r.get(0),
            )
            .optional()
            .map_err(fail)?;
        if previous == Some(clock) {
            return Ok(table);
        }
        self.db
            .execute_batch("SAVEPOINT usage_projection")
            .map_err(fail)?;
        let result = (|| {
            let (columns, projection, indexes) = if kind == "provider" {
                let digest = if native {
                    "row_digest(value),CASE WHEN json_extract(value,'$.providerId') IS NOT NULL THEN row_digest(json_set(value,'$.providerId',NULL,'$.attribution','removed_provider')) END"
                } else {
                    "NULL,NULL"
                };
                (
                    "source_order INTEGER PRIMARY KEY,id TEXT,time INTEGER,time_text TEXT,agent TEXT,product TEXT,provider TEXT,model TEXT,stats TEXT,latency REAL,ttft REAL,digest TEXT,removed_digest TEXT",
                    &*format!(
                        "id,instant_ms(json_extract(value,'$.time')),json_extract(value,'$.time'),json_extract(value,'$.agentId'),json_extract(value,'$.agentProductId'),json_extract(value,'$.providerId'),json_extract(value,'$.model'),json_extract(value,'$._stats'),CASE WHEN json_extract(value,'$.status') IN ('success','returned','error','failed') AND json_extract(value,'$.durationMs')>=0 THEN json_extract(value,'$.durationMs') END,CASE WHEN json_extract(value,'$.status') IN ('success','returned','error','failed') AND json_extract(value,'$.ttftMs')>=0 THEN json_extract(value,'$.ttftMs') END,{digest}"
                    ),
                    format!(
                        "CREATE INDEX IF NOT EXISTS temp.{table}_scope ON {table}(provider,model,time);
                         CREATE INDEX IF NOT EXISTS temp.{table}_page ON {table}(time_text DESC,id);
                         CREATE INDEX IF NOT EXISTS temp.{table}_latency ON {table}(latency) WHERE latency IS NOT NULL;
                         CREATE INDEX IF NOT EXISTS temp.{table}_ttft ON {table}(ttft) WHERE ttft IS NOT NULL;"
                    ),
                )
            } else {
                let family = if native {
                    "json_extract(value,'$._agentId'),json_extract(value,'$._agentProductId')"
                } else {
                    "family,product"
                };
                (
                    "source_order INTEGER PRIMARY KEY,id TEXT,time INTEGER,time_text TEXT,agent TEXT,product TEXT,pid TEXT,model TEXT,stats TEXT,t TEXT,ord INTEGER",
                    // family/product are authoritative gateway agent identities.
                    // Native rows have already been scoped by the authorized catalog.
                    &*format!(
                        "id,instant_ms(json_extract(value,'$.time')),json_extract(value,'$.time'),{family},json_extract(value,'$._providerId'),json_extract(value,'$.model'),json_extract(value,'$._stats'),instant_key(json_extract(value,'$.time')),json_extract(value,'$._order')"
                    ),
                    format!(
                        "CREATE INDEX IF NOT EXISTS temp.{table}_groups ON {table}(pid,model);"
                    ),
                )
            };
            self.db
                .execute_batch(&format!(
                    "CREATE TEMP TABLE IF NOT EXISTS {table}({columns});"
                ))
                .map_err(fail)?;
            let changed = if let Some(seq) = previous {
                if native {
                    format!(
                        "id IN (SELECT id FROM native_usage_changes WHERE kind='{kind}' AND seq>{seq})"
                    )
                } else {
                    format!("id IN (SELECT id FROM usage_changes WHERE seq>{seq})")
                }
            } else {
                "1".into()
            };
            execute(
                &self.db,
                &format!("DELETE FROM {table} WHERE {changed}"),
                [],
            )
            .map_err(fail)?;
            let fields = if kind == "model" && !native {
                ",family,product"
            } else {
                ""
            };
            // Keep source row order through delta upserts too. Float money
            // additions must not change order merely because dirty IDs sort
            // differently from their original insert/update sequence.
            execute(&self.db, &format!("INSERT INTO {table} SELECT source_order,{projection} FROM (SELECT rowid source_order,id,{kind}_record value{fields} FROM {source} WHERE {kind}_record IS NOT NULL AND {changed})"), []).map_err(fail)?;
            if native {
                // Existing delta triggers remain active. On a cold projection,
                // build the block index once after the bulk insert instead.
                self.usage_block_tables(kind, &table)?;
            }
            self.db
                .execute_batch(&format!(
                    "CREATE UNIQUE INDEX IF NOT EXISTS temp.{table}_id ON {table}(id);{indexes} CREATE INDEX IF NOT EXISTS temp.{table}_time ON {table}(time);"
                ))
                .map_err(fail)?;
            execute(&self.db,"INSERT INTO usage_projection_clock VALUES(?1,?2) ON CONFLICT(kind) DO UPDATE SET seq=excluded.seq", params![table,clock]).map_err(fail)?;
            if native {
                execute(
                    &self.db,
                    "DELETE FROM native_usage_changes WHERE kind=?1 AND seq<=?2",
                    params![kind, clock],
                )
                .map_err(fail)?;
            }
            // An uninitialized projection always rebuilds from attempts. Once
            // initialized, retain only changes not yet seen by both consumers.
            execute(&self.db,"DELETE FROM usage_changes WHERE seq<=(SELECT min(seq) FROM usage_projection_clock WHERE kind IN ('provider_facts','model_facts'))", []).map_err(fail)?;
            self.db
                .execute_batch("RELEASE usage_projection")
                .map_err(fail)
        })();
        if result.is_err() {
            let _ = self
                .db
                .execute_batch("ROLLBACK TO usage_projection; RELEASE usage_projection");
        }
        result.map(|()| table)
    }
}
