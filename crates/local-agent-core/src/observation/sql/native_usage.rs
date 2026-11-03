//! Connection-private, file-granular native usage loads. Snapshots share the
//! scanner's immutable batches; only changed files are decoded into SQL rows.
use super::*;
use crate::{
    projects::ProjectCatalog,
    sessions::{CallCatalog, CallSummary},
};
use sha2::{Digest, Sha256};

pub(super) fn scope_hash(value: &impl serde::Serialize) -> Result<String, String> {
    struct HashWriter(Sha256);
    impl std::io::Write for HashWriter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.update(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut writer = std::io::BufWriter::with_capacity(64 * 1024, HashWriter(Sha256::new()));
    serde_json::to_writer(&mut writer, value).map_err(fail)?;
    Ok(format!(
        "{:x}",
        writer.into_inner().map_err(fail)?.0.finalize()
    ))
}

pub(super) struct Snapshot {
    revision: u64,
    scope: String,
    calls: CallCatalog,
}
impl Snapshot {
    fn matches(&self, revision: u64, scope: &str) -> bool {
        self.revision == revision && self.scope == scope
    }
    fn delta(
        previous: Option<&Self>,
        catalog: &ProjectCatalog,
        scope: &str,
    ) -> (bool, CallCatalog, CallCatalog) {
        if let Some(previous) = previous.filter(|p| p.scope == scope) {
            let (removed, added) = catalog.calls.delta(&previous.calls);
            (
                false,
                CallCatalog::shared(removed.into_iter().cloned()),
                CallCatalog::shared(added.into_iter().cloned()),
            )
        } else {
            (true, CallCatalog::default(), catalog.calls.clone())
        }
    }
}
impl History {
    pub(super) fn native_usage_tables(&self) -> Result<(), String> {
        self.db.execute_batch("CREATE TEMP TABLE IF NOT EXISTS native_usage(id TEXT PRIMARY KEY,provider_record TEXT,model_record TEXT);
          CREATE TEMP TABLE IF NOT EXISTS native_provider_usage(id TEXT,provider_record TEXT NOT NULL);
          CREATE TEMP TABLE IF NOT EXISTS native_model_usage(id TEXT,model_record TEXT NOT NULL);
          CREATE TEMP TABLE IF NOT EXISTS native_usage_clock(seq INTEGER NOT NULL,provider INTEGER NOT NULL,model INTEGER NOT NULL);
          INSERT INTO native_usage_clock SELECT 0,0,0 WHERE NOT EXISTS(SELECT 1 FROM native_usage_clock);
          CREATE TEMP TABLE IF NOT EXISTS native_usage_changes(kind TEXT,id TEXT,seq INTEGER,PRIMARY KEY(kind,id));
          CREATE INDEX IF NOT EXISTS temp.native_usage_changes_seq ON native_usage_changes(kind,seq);").map_err(fail)
    }
    pub(super) fn usage_table(&self, kind: &str, native: bool) -> &'static str {
        match (kind, native) {
            ("provider", true) if self.native_provider.is_some() => "native_provider_usage",
            ("model", true) if self.native_model.is_some() => "native_model_usage",
            (_, true) => "native_usage",
            _ => "attempts",
        }
    }
    pub(super) fn usage_clock(&self, native: bool) -> Result<i64, String> {
        if native {
            self.native_usage_tables()?;
        }
        self.db
            .query_row(
                if native {
                    "SELECT seq FROM native_usage_clock"
                } else {
                    "SELECT seq FROM query_clock"
                },
                [],
                |r| r.get(0),
            )
            .map_err(fail)
    }
    pub(super) fn usage_kind_clock(&self, kind: &str, native: bool) -> Result<i64, String> {
        if !native {
            return self.usage_clock(false);
        }
        self.native_usage_tables()?;
        self.db
            .query_row(&format!("SELECT {kind} FROM native_usage_clock"), [], |r| {
                r.get(0)
            })
            .map_err(fail)
    }
    fn load_native_usage(
        &self,
        kind: &str,
        full: bool,
        removed: &CallCatalog,
        added: &CallCatalog,
        mut record: impl FnMut(&CallSummary, usize) -> Result<Option<String>, String>,
    ) -> Result<usize, String> {
        self.native_usage_tables()?;
        self.db
            .execute_batch("SAVEPOINT native_usage_delta")
            .map_err(fail)?;
        let result = (|| {
            let clock = self.usage_clock(true)? + 1;
            let column = format!("{kind}_record");
            let table = format!("native_{kind}_usage");
            let delete_row = format!("DELETE FROM {table} WHERE id=?1");
            let insert_row = format!("INSERT OR IGNORE INTO {table}(id,{column}) VALUES(?1,?2)");
            let upsert_row = format!(
                "INSERT INTO {table}(id,{column}) VALUES(?1,?2) ON CONFLICT(id) DO UPDATE SET {column}=excluded.{column} WHERE {table}.{column} IS NOT excluded.{column}"
            );
            let projection = format!("native_{kind}_facts");
            let incoming: HashSet<_> = if full || removed.is_empty() {
                HashSet::new()
            } else {
                added
                    .iter()
                    .filter(|c| c.kind == "model")
                    .map(|c| c.id.as_str())
                    .collect()
            };
            let retired: HashSet<_> = removed
                .iter()
                .filter(|c| c.kind == "model")
                .map(|c| c.id.as_str())
                .collect();
            let mut changed = full;
            if full {
                // A cold/replaced projection will read the complete authoritative
                // source. Logging every ID only duplicates that future bulk read.
                execute(
                    &self.db,
                    "DELETE FROM usage_projection_clock WHERE kind=?1",
                    [&projection],
                )
                .map_err(fail)?;
                execute(
                    &self.db,
                    "DELETE FROM native_usage_changes WHERE kind=?1",
                    [kind],
                )
                .map_err(fail)?;
                execute(&self.db, &format!("DELETE FROM {table}"), []).map_err(fail)?;
            }
            let tracked: bool = self
                .db
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM usage_projection_clock WHERE kind=?1)",
                    [&projection],
                    |r| r.get(0),
                )
                .map_err(fail)?;
            if !full {
                for call in removed
                    .iter()
                    .filter(|c| c.kind == "model" && !incoming.contains(c.id.as_str()))
                {
                    let deleted = execute(&self.db, &delete_row, [&call.id]).map_err(fail)?;
                    changed |= deleted > 0;
                    if tracked && deleted > 0 {
                        execute(&self.db,"INSERT INTO native_usage_changes VALUES(?1,?2,?3) ON CONFLICT(kind,id) DO UPDATE SET seq=excluded.seq",params![kind,call.id,clock]).map_err(fail)?;
                    }
                }
            }
            let mut seen = HashSet::new();
            let mut loaded = 0;
            for batch in &added.chunks {
                for (order, call) in batch.iter().enumerate() {
                    if call.kind != "model" || !seen.insert(call.id.as_str()) {
                        continue;
                    }
                    if let Some(value) = record(call, order)? {
                        let written = execute(
                            &self.db,
                            if full || !retired.contains(call.id.as_str()) {
                                &insert_row
                            } else {
                                &upsert_row
                            },
                            params![call.id, value],
                        )
                        .map_err(fail)?;
                        loaded += written;
                        changed |= written > 0;
                        if tracked && written > 0 {
                            execute(&self.db,"INSERT INTO native_usage_changes VALUES(?1,?2,?3) ON CONFLICT(kind,id) DO UPDATE SET seq=excluded.seq",params![kind,call.id,clock]).map_err(fail)?;
                        }
                    }
                }
            }
            self.db
                .execute_batch(&format!(
                    "CREATE UNIQUE INDEX IF NOT EXISTS temp.{table}_id ON {table}(id)"
                ))
                .map_err(fail)?;
            if changed {
                execute(
                    &self.db,
                    &format!("UPDATE native_usage_clock SET seq=?1,{kind}=?1"),
                    [clock],
                )
                .map_err(fail)?;
            }
            self.db
                .execute_batch("RELEASE native_usage_delta")
                .map_err(fail)?;
            Ok(loaded)
        })();
        if result.is_err() {
            let _ = self
                .db
                .execute_batch("ROLLBACK TO native_usage_delta;RELEASE native_usage_delta");
        }
        result
    }
    pub fn sync_native_provider(
        &mut self,
        catalog: &ProjectCatalog,
        evidence: &[provider_usage::Evidence],
    ) -> Result<usize, String> {
        self.use_directory(catalog.calls.directory())?;
        let scope = encode(evidence)?;
        if self
            .native_provider
            .as_ref()
            .is_some_and(|p| p.matches(catalog.revision, &scope))
        {
            return Ok(0);
        }
        let (full, removed, added) =
            Snapshot::delta(self.native_provider.as_ref(), catalog, &scope);
        let loaded = self.load_native_usage("provider", full, &removed, &added, |call, _| {
            crate::provider_usage::native_records_iter(std::iter::once(call.clone()), evidence)
                .next()
                .map(|r| super::usage::provider_record(&r))
                .transpose()
        })?;
        self.native_provider = Some(Snapshot {
            revision: catalog.revision,
            scope,
            calls: catalog.calls.clone(),
        });
        Ok(loaded)
    }
    pub fn sync_native_models(
        &mut self,
        catalog: &ProjectCatalog,
        agent: &str,
    ) -> Result<usize, String> {
        self.use_directory(catalog.calls.directory())?;
        if !self.directory.valid_filter(agent) {
            return Err("未知 Agent".into());
        }
        let epoch = self.prepare_fees(catalog)?;
        if self
            .native_model
            .as_ref()
            .is_some_and(|s| s.revision == catalog.revision)
            && self
                .native_model_fee_epoch
                .as_ref()
                .is_some_and(|e| e == &epoch)
        {
            return Ok(0);
        }
        let costs = self.response_costs()?;
        let mut cost_key: Vec<_> = costs.iter().collect();
        cost_key.sort_by_key(|(id, _)| *id);
        let scope = scope_hash(&cost_key)?;
        if self
            .native_model
            .as_ref()
            .is_some_and(|p| p.matches(catalog.revision, &scope))
        {
            self.native_model_fee_epoch = Some(epoch);
            return Ok(0);
        }
        let (full, removed, added) = Snapshot::delta(self.native_model.as_ref(), catalog, &scope);
        let loaded = self.load_native_usage("model", full, &removed, &added, |call, order| {
            let mut row = crate::sessions::usage_record(call);
            if let Some(cost) = costs.get(&call.id) {
                row.cost = cost.clone();
            }
            let mut value = super::usage::model_value(&row)?;
            value["_agentId"] = json!(call.agent_id);
            value["_agentProductId"] = json!(call.agent_product_id);
            // Catalog batches sort by Session ID. The file-local ordinal is
            // stable when another Session is added or removed; equal-time
            // references retain canonical catalog order without rewriting N rows.
            value["_order"] = json!(format!("{}\0{order:020}", call.session_id));
            Ok(Some(value.to_string()))
        })?;
        self.native_model = Some(Snapshot {
            revision: catalog.revision,
            scope,
            calls: catalog.calls.clone(),
        });
        self.native_model_fee_epoch = Some(epoch);
        Ok(loaded)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn large_cost_scope_has_fixed_size_and_hashes_canonical_serialization() {
        let costs: Vec<_> = (0..10_000)
            .map(|i| {
                (
                    format!("response-{i}"),
                    crate::provider_cost::Cost::unknown("native_price_unconfirmed"),
                )
            })
            .collect();
        let scope = ("claude", costs);
        let digest = scope_hash(&scope).unwrap();
        assert_eq!(digest.len(), 64);
        assert_eq!(
            digest,
            format!("{:x}", Sha256::digest(serde_json::to_vec(&scope).unwrap()))
        );
    }
}
