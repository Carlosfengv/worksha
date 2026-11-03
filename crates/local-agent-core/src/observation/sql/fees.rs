//! Fee views share the canonical request/cost rules, without loading historical
//! attempts into project and Session responses. All temporary data is disposable.
use super::aggregates::{list, quote};
use super::*;
use crate::{associated_cost, projects::ProjectCatalog, provider_cost};

impl History {
    pub fn prepare_fees(&mut self, catalog: &ProjectCatalog) -> Result<String, String> {
        self.use_directory(catalog.calls.directory())?;
        self.db.execute_batch("CREATE TEMP TABLE IF NOT EXISTS fee_sessions(id TEXT PRIMARY KEY,family TEXT);
          CREATE TEMP TABLE IF NOT EXISTS fee_native_base(id TEXT,family TEXT,response TEXT,value TEXT);
          CREATE TEMP TABLE IF NOT EXISTS fee_native(id TEXT,session TEXT,family TEXT,product TEXT,response TEXT,association TEXT,attempt TEXT,conflict INTEGER,stats TEXT);
          CREATE TEMP TABLE IF NOT EXISTS fees(id TEXT PRIMARY KEY,session TEXT,family TEXT,product TEXT,response TEXT,value TEXT,stats TEXT,time TEXT,provider TEXT,model TEXT);
          CREATE INDEX IF NOT EXISTS temp.fees_scope ON fees(session,family,product);
          CREATE INDEX IF NOT EXISTS temp.fees_response ON fees(family,response);
          CREATE INDEX IF NOT EXISTS temp.fees_time ON fees(time DESC,id);
          CREATE TEMP TABLE IF NOT EXISTS fee_responses(family TEXT,response TEXT,PRIMARY KEY(family,response));
          CREATE TEMP TABLE IF NOT EXISTS fee_dirty(id TEXT PRIMARY KEY);
          CREATE TEMP TABLE IF NOT EXISTS fee_clock(session TEXT PRIMARY KEY,seq INTEGER);
          CREATE TEMP TRIGGER IF NOT EXISTS fee_insert AFTER INSERT ON fees BEGIN UPDATE query_clock SET seq=seq+1;INSERT INTO fee_clock VALUES(new.session,(SELECT seq FROM query_clock)) ON CONFLICT(session) DO UPDATE SET seq=excluded.seq;END;
          CREATE TEMP TRIGGER IF NOT EXISTS fee_delete AFTER DELETE ON fees BEGIN UPDATE query_clock SET seq=seq+1;INSERT INTO fee_clock VALUES(old.session,(SELECT seq FROM query_clock)) ON CONFLICT(session) DO UPDATE SET seq=excluded.seq;END;
          CREATE TEMP TRIGGER IF NOT EXISTS fee_native_insert AFTER INSERT ON fee_native BEGIN UPDATE query_clock SET seq=seq+1;INSERT INTO fee_clock VALUES(new.session,(SELECT seq FROM query_clock)) ON CONFLICT(session) DO UPDATE SET seq=excluded.seq;END;
          CREATE TEMP TRIGGER IF NOT EXISTS fee_native_delete AFTER DELETE ON fee_native BEGIN UPDATE query_clock SET seq=seq+1;INSERT INTO fee_clock VALUES(old.session,(SELECT seq FROM query_clock)) ON CONFLICT(session) DO UPDATE SET seq=excluded.seq;END;
          CREATE TEMP TRIGGER IF NOT EXISTS fee_native_update AFTER UPDATE ON fee_native BEGIN UPDATE query_clock SET seq=seq+1;INSERT INTO fee_clock VALUES(old.session,(SELECT seq FROM query_clock)) ON CONFLICT(session) DO UPDATE SET seq=excluded.seq;INSERT INTO fee_clock VALUES(new.session,(SELECT seq FROM query_clock)) ON CONFLICT(session) DO UPDATE SET seq=excluded.seq;END;
          CREATE TEMP TABLE IF NOT EXISTS fee_positions(version TEXT,pos INTEGER,id TEXT,PRIMARY KEY(version,pos)) WITHOUT ROWID;
          SAVEPOINT fee_sync;").map_err(fail)?;
        let changed = self.fee_revision != Some(catalog.revision);
        let (removed, added) = if changed {
            catalog.calls.delta(&self.fee_calls)
        } else {
            (vec![], vec![])
        };
        let incoming: HashSet<_> = if removed.is_empty() {
            HashSet::new()
        } else {
            added
                .iter()
                .flat_map(|c| c.iter())
                .filter(|c| c.kind == "model")
                .map(|c| c.id.as_str())
                .collect()
        };
        let retired: HashSet<_> = removed
            .iter()
            .flat_map(|c| c.iter())
            .map(|c| c.id.as_str())
            .collect();
        let result: Result<(i64, i64, i64), String> = (|| {
            let before: i64 = self
                .db
                .query_row("SELECT coalesce(max(seq),0) FROM fee_clock", [], |r| {
                    r.get(0)
                })
                .map_err(fail)?;
            if changed {
                if self.fee_revision.is_none() {
                    self.db.execute_batch("INSERT OR IGNORE INTO attempt_changes SELECT id FROM attempts;DELETE FROM fees;DELETE FROM fee_native;DELETE FROM fee_native_base;").map_err(fail)?;
                }
                self.db
                    .execute_batch("DELETE FROM fee_sessions;")
                    .map_err(fail)?;
                for call in removed
                    .iter()
                    .flat_map(|c| c.iter())
                    .filter(|c| !incoming.contains(c.id.as_str()))
                {
                    self.retire_fee_native(&call.id)?;
                }
                for s in &catalog.sessions {
                    execute(
                        &self.db,
                        "INSERT OR IGNORE INTO fee_sessions VALUES(?1,?2)",
                        params![s.id, s.agent_id],
                    )
                    .map_err(fail)?;
                }
                // Session authorization/identity can change without an attempt
                // mutation. Re-evaluate explicit Session IDs as well as links.
                execute(&self.db,"INSERT OR IGNORE INTO fee_dirty SELECT a.id FROM attempts a JOIN fee_sessions s ON s.id=a.session AND s.family=a.family",[]).map_err(fail)?;
                self.db.execute_batch("INSERT OR IGNORE INTO fee_dirty SELECT id FROM fees WHERE NOT EXISTS(SELECT 1 FROM fee_sessions s WHERE s.id=fees.session AND s.family=fees.family);DELETE FROM fees WHERE NOT EXISTS(SELECT 1 FROM fee_sessions s WHERE s.id=fees.session AND s.family=fees.family);").map_err(fail)?;
                let mut seen = HashSet::new();
                for call in added
                    .iter()
                    .flat_map(|c| c.iter())
                    .filter(|c| c.kind == "model" && seen.insert(c.id.as_str()))
                {
                    // Fee correlation needs identity and usage, not a complete
                    // request payload, provider hash or transcript presentation.
                    // Reconstruct the canonical native record only on a real
                    // response-ID intersection (including later gateway writes).
                    let raw = encode(&(&call.session_id, &call.agent_product_id, &call.usage))?;
                    if self.fee_revision.is_some() {
                        let old: Option<(String, Option<String>, String)> = self
                            .db
                            .query_row(
                                "SELECT family,response,value FROM fee_native_base WHERE id=?1",
                                [&call.id],
                                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                            )
                            .optional()
                            .map_err(fail)?;
                        if old.is_some() && !retired.contains(call.id.as_str()) {
                            return Err(fail("duplicate native fee ID"));
                        }
                        if old.as_ref().is_some_and(|(family, response, value)| {
                            family == &call.agent_id
                                && response == &call.response_id
                                && value == &raw
                        }) {
                            continue;
                        }
                        if old.is_some() {
                            self.retire_fee_native(&call.id)?;
                        }
                    }
                    execute(
                        &self.db,
                        "INSERT INTO fee_native_base VALUES(?1,?2,?3,?4)",
                        params![call.id, call.agent_id, call.response_id, raw],
                    )
                    .map_err(fail)?;
                    execute(&self.db,"INSERT INTO fee_native VALUES(?1,?2,?3,?4,?5,'unlinked',NULL,0,?6)",
                        params![call.id,call.session_id,call.agent_id,call.agent_product_id,call.response_id,
                            r#"{"nativeResponses":1,"matchedNativeResponses":0,"unlinkedNativeResponses":1,"conflictingNativeResponses":0}"#]).map_err(fail)?;
                    if self.fee_revision.is_some() {
                        execute(&self.db,"INSERT OR IGNORE INTO fee_responses SELECT n.family,n.response FROM fee_native_base n WHERE n.id=?1 AND n.response IS NOT NULL AND EXISTS(SELECT 1 FROM attempts a WHERE a.family=n.family AND a.response=n.response)",[&call.id]).map_err(fail)?;
                    }
                }
                // Build cold indexes in bulk instead of interleaving millions
                // of random B-tree writes. Source IDs are deduplicated above;
                // uniqueness is checked before publication in this savepoint.
                self.db.execute_batch("CREATE UNIQUE INDEX IF NOT EXISTS temp.fee_native_base_id ON fee_native_base(id);
                  CREATE UNIQUE INDEX IF NOT EXISTS temp.fee_native_id ON fee_native(id);
                  CREATE INDEX IF NOT EXISTS temp.fee_native_response ON fee_native_base(family,response);
CREATE INDEX IF NOT EXISTS temp.fee_native_attempt ON fee_native(attempt) WHERE attempt IS NOT NULL;
CREATE INDEX IF NOT EXISTS temp.fee_native_confirmed ON fee_native(attempt,id) WHERE association='response_id' AND conflict=0;
CREATE INDEX IF NOT EXISTS temp.fee_native_session ON fee_native(session,family,product,association,conflict);
CREATE INDEX IF NOT EXISTS temp.fee_native_response_current ON fee_native(family,response);").map_err(fail)?;
                if self.fee_revision.is_none()
                    && self
                        .db
                        .query_row("SELECT EXISTS(SELECT 1 FROM attempts)", [], |r| {
                            r.get::<_, bool>(0)
                        })
                        .map_err(fail)?
                {
                    execute(&self.db,"INSERT OR IGNORE INTO fee_responses SELECT n.family,n.response FROM fee_native_base n WHERE n.response IS NOT NULL AND EXISTS(SELECT 1 FROM attempts a WHERE a.family=n.family AND a.response=n.response)",[]).map_err(fail)?;
                }
            }
            self.db.execute_batch("INSERT OR IGNORE INTO fee_dirty SELECT id FROM attempt_changes;
              INSERT OR IGNORE INTO fee_responses SELECT a.family,a.response FROM attempts a JOIN attempt_changes d ON a.id=d.id WHERE a.response IS NOT NULL;
              INSERT OR IGNORE INTO fee_responses SELECT a.family,a.response FROM fees a JOIN attempt_changes d ON a.id=d.id WHERE a.response IS NOT NULL;").map_err(fail)?;
            self.db.execute_batch("DELETE FROM fee_responses WHERE NOT EXISTS(SELECT 1 FROM fee_native_base n WHERE n.family=fee_responses.family AND n.response=fee_responses.response) AND NOT EXISTS(SELECT 1 FROM fee_native n WHERE n.family=fee_responses.family AND n.response=fee_responses.response);").map_err(fail)?;
            loop {
                let keys: Vec<(String, String)> = {
                    let mut s = self
                        .db
                        .prepare("SELECT family,response FROM fee_responses LIMIT 128")
                        .map_err(fail)?;
                    s.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
                        .map_err(fail)?
                        .collect::<Result<_, _>>()
                        .map_err(fail)?
                };
                if keys.is_empty() {
                    break;
                }
                for (family, response) in keys {
                    execute(&self.db,"INSERT OR IGNORE INTO fee_dirty SELECT attempt FROM fee_native WHERE family=?1 AND response=?2 AND attempt IS NOT NULL",params![family,response]).map_err(fail)?;
                    execute(&self.db,"INSERT OR IGNORE INTO fee_dirty SELECT id FROM attempts WHERE family=?1 AND response=?2",params![family,response]).map_err(fail)?;
                    let net: usize = self
                        .db
                        .query_row(
                            "SELECT count(*) FROM attempts WHERE family=?1 AND response=?2",
                            params![family, response],
                            |r| sql_count(r, 0),
                        )
                        .map_err(fail)?;
                    let local: usize = self
                        .db
                        .query_row(
                            "SELECT count(*) FROM fee_native_base WHERE family=?1 AND response=?2",
                            params![family, response],
                            |r| sql_count(r, 0),
                        )
                        .map_err(fail)?;
                    let mut after = String::new();
                    loop {
                        let batch: Vec<(String, String)> = {
                            let mut s=self.db.prepare("SELECT id,value FROM fee_native_base WHERE family=?1 AND response=?2 AND id>?3 ORDER BY id LIMIT 256").map_err(fail)?;
                            s.query_map(params![family, response, after], |r| {
                                Ok((r.get(0)?, r.get(1)?))
                            })
                            .map_err(fail)?
                            .collect::<Result<_, _>>()
                            .map_err(fail)?
                        };
                        if batch.is_empty() {
                            break;
                        }
                        after = batch.last().unwrap().0.clone();
                        for (id, raw) in batch {
                            let (session, product, usage): (String, Option<String>, Option<Usage>) =
                                serde_json::from_str(&raw).map_err(fail)?;
                            let mut n = super::dashboard::native_value(&json!({
                                "id":id,"agentId":family,"agentProductId":product,"sessionId":session,
                                "usage":usage.unwrap_or_default(),"cost":provider_cost::Totals::default(),
                                "native":requests::sources::NativeRecord {
                                    failure:None,event_id:id,turn_id:String::new(),response_id:Some(response.clone()),
                                    reported_status:String::new(),association:"unlinked".into(),gateway_request_id:None,
                                    matched_attempt_id:None,usage_conflict:false
                                }
                            }))?;
                            if net > 0 && (net != 1 || local != 1) {
                                n.native.as_mut().unwrap().association = "ambiguous".into();
                            } else if net == 1 {
                                let mut gateway = requests::logical_requests(
                                    self.response_group(&family, &response)?,
                                    &HashMap::new(),
                                );
                                requests::sources::correlate(
                                    &mut gateway,
                                    std::slice::from_mut(&mut n),
                                );
                            }
                            execute(&self.db, "DELETE FROM fee_native WHERE id=?1", [&n.id])
                                .map_err(fail)?;
                            self.insert_fee_native(&n)?;
                        }
                    }
                    execute(
                        &self.db,
                        "DELETE FROM fee_responses WHERE family=?1 AND response=?2",
                        params![family, response],
                    )
                    .map_err(fail)?;
                }
            }
            // Unrelated history cannot contribute to an authorized Session.
            // Drop those IDs in SQL before decoding any fee attempts.
            self.db.execute_batch("DELETE FROM fee_dirty WHERE NOT EXISTS(SELECT 1 FROM fees f WHERE f.id=fee_dirty.id) AND NOT EXISTS(SELECT 1 FROM attempts a JOIN fee_sessions s ON s.id=a.session AND s.family=a.family WHERE a.id=fee_dirty.id) AND NOT EXISTS(SELECT 1 FROM fee_native n WHERE n.attempt=fee_dirty.id);").map_err(fail)?;
            loop {
                let ids: Vec<String> = {
                    let mut s = self
                        .db
                        .prepare("SELECT id FROM fee_dirty LIMIT 128")
                        .map_err(fail)?;
                    s.query_map([], |r| r.get(0))
                        .map_err(fail)?
                        .collect::<Result<_, _>>()
                        .map_err(fail)?
                };
                if ids.is_empty() {
                    break;
                }
                for id in ids {
                    let logical: Option<String> = self
                        .db
                        .query_row("SELECT logical FROM attempts WHERE id=?1", [&id], |r| {
                            r.get(0)
                        })
                        .optional()
                        .map_err(fail)?;
                    execute(&self.db, "DELETE FROM fees WHERE id=?1", [&id]).map_err(fail)?;
                    if let Some(logical) = logical {
                        let rows: Vec<ModelObservation> = {
                            let mut s = self
                                .db
                                .prepare_cached(
                                    "SELECT raw FROM attempts WHERE logical=?1 ORDER BY id",
                                )
                                .map_err(fail)?;
                            s.query_map([&logical], |r| r.get::<_, String>(0))
                                .map_err(fail)?
                                .map(|r| serde_json::from_str(&r.map_err(fail)?).map_err(fail))
                                .collect::<Result<_, _>>()?
                        };
                        for request in requests::logical_requests(rows, &HashMap::new()) {
                            for a in request.attempts {
                                let r = a.observation;
                                let paired:Option<(String,Option<String>)>=self.db.query_row("SELECT session,product FROM fee_native WHERE association='response_id' AND attempt=?1",[&r.id],|r|Ok((r.get(0)?,r.get(1)?))).optional().map_err(fail)?;
                                let conflict:bool=self.db.query_row("SELECT EXISTS(SELECT 1 FROM fee_native WHERE family=?1 AND response=?2 AND association='identity_conflict')",params![r.agent_id,r.native_response_id()],|r|r.get(0)).map_err(fail)?;
                                let session = if conflict {
                                    None
                                } else {
                                    r.session_id
                                        .clone()
                                        .or_else(|| paired.as_ref().map(|p| p.0.clone()))
                                };
                                let valid = if let Some(s) = &session {
                                    self.db.query_row("SELECT EXISTS(SELECT 1 FROM fee_sessions WHERE id=?1 AND family=?2)",params![s,r.agent_id],|r|r.get::<_,bool>(0)).map_err(fail)?
                                } else {
                                    false
                                };
                                if valid {
                                    let response = r.native_response_id().map(str::to_owned);
                                    let linked = paired.is_some() && !conflict;
                                    let call = associated_cost::detail::Call {
                                        id: r.id.clone(),
                                        request_id: request.id.clone(),
                                        session_id: session.clone(),
                                        provider_key: a.provider_key.clone(),
                                        provider: r.provider,
                                        model: r.routed_model.or(r.model),
                                        time: r.started_at.clone(),
                                        status: r.status,
                                        usage: r.usage,
                                        cost: a.cost.clone(),
                                        association: if linked {
                                            "response_id"
                                        } else {
                                            "session_id"
                                        },
                                    };
                                    let mut cost = provider_cost::Totals::default();
                                    cost.add(&a.cost);
                                    let stats = json!({"cost":cost,"gatewayAttempts":1,"responseLinkedAttempts":usize::from(linked),"sessionLinkedAttempts":usize::from(!linked)});
                                    execute(&self.db, "DELETE FROM fees WHERE id=?1", [&r.id])
                                        .map_err(fail)?;
                                    execute(
                                        &self.db,
                                        "INSERT INTO fees VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
                                        params![
                                            r.id,
                                            session,
                                            r.agent_id,
                                            r.agent_product_id.or_else(|| paired.and_then(|p| p.1)),
                                            response,
                                            encode(&call)?,
                                            encode(&stats)?,
                                            r.started_at,
                                            a.provider_key,
                                            call.model
                                        ],
                                    )
                                    .map_err(fail)?;
                                }
                                execute(&self.db, "DELETE FROM fee_dirty WHERE id=?1", [&r.id])
                                    .map_err(fail)?;
                            }
                        }
                    }
                    execute(&self.db, "DELETE FROM fee_dirty WHERE id=?1", [id]).map_err(fail)?;
                }
            }
            let after: i64 = self
                .db
                .query_row("SELECT coalesce(max(seq),0) FROM fee_clock", [], |r| {
                    r.get(0)
                })
                .map_err(fail)?;
            let gateway: i64 = self
                .db
                .query_row("SELECT seq FROM attempt_clock", [], |r| r.get(0))
                .map_err(fail)?;
            self.db
                .execute_batch("DELETE FROM attempt_changes;RELEASE fee_sync")
                .map_err(fail)?;
            Ok((before, after, gateway))
        })();
        if let Ok(&(before, after, gateway)) = result.as_ref() {
            self.fee_version = self
                .fee_versions
                .iter()
                .find(|(revision, epoch, _)| *revision == catalog.revision && *epoch == gateway)
                .map(|(_, _, version)| *version)
                .unwrap_or(if before == after {
                    self.fee_version
                } else {
                    after
                });
            self.fee_versions
                .retain(|(revision, epoch, _)| *revision != catalog.revision || *epoch != gateway);
            if self.fee_versions.len() == 8 {
                self.fee_versions.remove(0);
            }
            self.fee_versions
                .push((catalog.revision, gateway, self.fee_version));
            self.fee_revision = Some(catalog.revision);
            self.fee_calls = catalog.calls.clone();
        } else {
            let _ = self
                .db
                .execute_batch("ROLLBACK TO fee_sync;RELEASE fee_sync;");
        }
        result?;
        self.fee_epoch()
    }
    fn response_group(
        &self,
        family: &str,
        response: &str,
    ) -> Result<Vec<ModelObservation>, String> {
        let mut s=self.db.prepare("SELECT raw FROM attempts WHERE logical=(SELECT logical FROM attempts WHERE family=?1 AND response=?2 LIMIT 1) ORDER BY id").map_err(fail)?;
        s.query_map(params![family, response], |r| r.get::<_, String>(0))
            .map_err(fail)?
            .map(|v| serde_json::from_str(&v.map_err(fail)?).map_err(fail))
            .collect()
    }
    fn retire_fee_native(&self, id: &str) -> Result<(), String> {
        execute(&self.db,"INSERT OR IGNORE INTO fee_dirty SELECT attempt FROM fee_native WHERE id=?1 AND attempt IS NOT NULL",[id]).map_err(fail)?;
        execute(&self.db,"INSERT OR IGNORE INTO fee_responses SELECT family,response FROM fee_native_base WHERE id=?1 AND response IS NOT NULL",[id]).map_err(fail)?;
        execute(&self.db, "DELETE FROM fee_native WHERE id=?1", [id]).map_err(fail)?;
        execute(&self.db, "DELETE FROM fee_native_base WHERE id=?1", [id]).map_err(fail)?;
        Ok(())
    }
    fn insert_fee_native(&self, n: &requests::Request) -> Result<(), String> {
        let r = n.native.as_ref().ok_or("费用证据无效")?;
        let matched = r.association == "response_id" && !r.usage_conflict;
        let stats = json!({"nativeResponses":1,"matchedNativeResponses":usize::from(matched),"unlinkedNativeResponses":usize::from(!matched),"conflictingNativeResponses":usize::from(!matched&&(["ambiguous","identity_conflict"].contains(&r.association.as_str())||r.usage_conflict))});
        execute(
            &self.db,
            "INSERT INTO fee_native VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            params![
                n.id,
                n.session_id,
                n.agent_id,
                n.agent_product_id,
                r.response_id,
                r.association,
                r.matched_attempt_id,
                r.usage_conflict,
                encode(&stats)?
            ],
        )
        .map_err(fail)?;
        Ok(())
    }
    pub fn fee_epoch(&self) -> Result<String, String> {
        let n: i64 = self
            .db
            .query_row("SELECT seq FROM query_clock", [], |r| r.get(0))
            .map_err(fail)?;
        Ok(format!("{}:{n}", self.fee_revision.unwrap_or(0)))
    }
    fn fee_scope(
        &self,
        ids: &[String],
        agent: Option<&str>,
        table: &str,
    ) -> Result<String, String> {
        self.db.execute_batch("CREATE TEMP TABLE IF NOT EXISTS selected_sessions(id TEXT PRIMARY KEY);SAVEPOINT fee_scope;DELETE FROM selected_sessions;").map_err(fail)?;
        let result = (|| {
            for id in ids {
                execute(
                    &self.db,
                    "INSERT OR IGNORE INTO selected_sessions VALUES(?1)",
                    [id],
                )
                .map_err(fail)?;
            }
            self.db.execute_batch("RELEASE fee_scope").map_err(fail)
        })();
        if let Err(error) = result {
            let _ = self
                .db
                .execute_batch("ROLLBACK TO fee_scope;RELEASE fee_scope");
            return Err(error);
        }
        Ok(format!(
            "(SELECT a.* FROM {table} a JOIN selected_sessions s ON s.id=a.session WHERE {})",
            match agent {
                None => "1".into(),
                Some(a)
                    if a != "unknown"
                        && !a.ends_with("-unknown")
                        && self.directory.source(a).is_some() =>
                    format!("a.family={}", quote(a)),
                Some(a) => format!("agent_matches({},a.family,a.product)", quote(a)),
            }
        ))
    }
    pub fn fee_summary(
        &self,
        ids: &[String],
        agent: Option<&str>,
        partial: bool,
    ) -> Result<Value, String> {
        let mut canonical: Vec<_> = ids.iter().collect();
        canonical.sort_unstable();
        canonical.dedup();
        let logical_key =
            super::native_usage::scope_hash(&("snapshot", self.fee_version, &canonical, agent))?;
        let physical: i64 = self
            .db
            .query_row("SELECT coalesce(max(seq),0) FROM fee_clock", [], |r| {
                r.get(0)
            })
            .map_err(fail)?;
        let physical_key =
            super::native_usage::scope_hash(&("ordered", physical, &canonical, agent))?;
        if let Some((_, value)) = self
            .fee_summaries
            .borrow()
            .iter()
            .find(|(k, _)| k == &logical_key || k == &physical_key)
        {
            let mut value = value.clone();
            value["sourceState"] = json!(if partial { "partial" } else { "ready" });
            return Ok(value);
        }
        let from = self.fee_scope(ids, agent, "fees")?;
        let mut summary = value(&associated_cost::Summary {
            source_state: if partial { "partial" } else { "ready" },
            ..Default::default()
        })?;
        let text: String = self
            .db
            .query_row(&format!("SELECT stats_sum(stats) FROM {from}"), [], |r| {
                r.get(0)
            })
            .map_err(fail)?;
        for (k, v) in serde_json::from_str::<Value>(&text)
            .map_err(fail)?
            .as_object()
            .ok_or("费用汇总无效")?
        {
            summary[k] = v.clone();
        }
        self.prepare_native_fee_counts()?;
        let native = self.fee_scope(ids, agent, "fee_native_counts")?;
        let (responses,matched,conflicts):(usize,usize,usize)=self.db.query_row(&format!("SELECT coalesce(sum(n),0),coalesce(sum(n) FILTER(WHERE association='response_id' AND conflict=0),0),coalesce(sum(n) FILTER(WHERE NOT(association='response_id' AND conflict=0) AND (association IN ('ambiguous','identity_conflict') OR conflict<>0)),0) FROM {native}"),[],|r|Ok((sql_count(r,0)?,sql_count(r,1)?,sql_count(r,2)?))).map_err(fail)?;
        summary["nativeResponses"] = json!(responses);
        summary["matchedNativeResponses"] = json!(matched);
        summary["unlinkedNativeResponses"] = json!(responses - matched);
        summary["conflictingNativeResponses"] = json!(conflicts);
        let mut cache = self.fee_summaries.borrow_mut();
        if cache.len() == 8 {
            cache.remove(0);
        }
        // Money retains the existing physical row-by-row addition order. Only
        // integer coverage/unknown counters can reuse a restored catalog's sum.
        let money = ["actual", "estimated"].iter().any(|k| {
            summary["cost"][k]
                .as_array()
                .is_some_and(|rows| !rows.is_empty())
        });
        cache.push((
            if money { physical_key } else { logical_key },
            summary.clone(),
        ));
        Ok(summary)
    }
    fn prepare_native_fee_counts(&self) -> Result<(), String> {
        self.db.execute_batch("CREATE TEMP TABLE IF NOT EXISTS fee_native_counts(session TEXT,family TEXT,product TEXT,association TEXT,conflict INTEGER,n INTEGER);
          CREATE INDEX IF NOT EXISTS temp.fee_native_counts_session ON fee_native_counts(session,family,product);
          CREATE TEMP TABLE IF NOT EXISTS fee_native_counts_clock(seq INTEGER NOT NULL);
          INSERT INTO fee_native_counts_clock SELECT 0 WHERE NOT EXISTS(SELECT 1 FROM fee_native_counts_clock);").map_err(fail)?;
        let previous: i64 = self
            .db
            .query_row("SELECT seq FROM fee_native_counts_clock", [], |r| r.get(0))
            .map_err(fail)?;
        let current: i64 = self
            .db
            .query_row("SELECT coalesce(max(seq),0) FROM fee_clock", [], |r| {
                r.get(0)
            })
            .map_err(fail)?;
        if previous == current {
            return Ok(());
        }
        self.db
            .execute_batch("SAVEPOINT fee_native_counts")
            .map_err(fail)?;
        let result = (|| {
            execute(&self.db,"DELETE FROM fee_native_counts WHERE session IN(SELECT session FROM fee_clock WHERE seq>?1)",[previous]).map_err(fail)?;
            execute(&self.db,"INSERT INTO fee_native_counts SELECT session,family,product,association,conflict,count(*) FROM fee_native WHERE session IN(SELECT session FROM fee_clock WHERE seq>?1) GROUP BY session,family,product,association,conflict",[previous]).map_err(fail)?;
            execute(
                &self.db,
                "UPDATE fee_native_counts_clock SET seq=?1",
                [current],
            )
            .map_err(fail)?;
            self.db
                .execute_batch("RELEASE fee_native_counts")
                .map_err(fail)
        })();
        if result.is_err() {
            let _ = self
                .db
                .execute_batch("ROLLBACK TO fee_native_counts;RELEASE fee_native_counts");
        }
        result
    }
    pub fn response_cost(&self, id: &str) -> Result<provider_cost::Cost, String> {
        let raw:Option<String>=self.db.query_row("SELECT a.raw FROM fee_native n JOIN fees f ON f.id=n.attempt JOIN attempts a ON a.id=n.attempt WHERE n.id=?1 AND n.association='response_id' AND n.conflict=0",[id],|r|r.get(0)).optional().map_err(fail)?;
        Ok(if let Some(raw) = raw {
            let r: ModelObservation = serde_json::from_str(&raw).map_err(fail)?;
            provider_cost::calculate(
                Some(&r.endpoint),
                r.model.as_deref(),
                Some(&r.started_at),
                &r.status,
                &r.usage,
                r.reported_cost.as_ref(),
                r.ended_at.as_deref(),
            )
        } else {
            provider_cost::Cost::unknown("native_price_unconfirmed")
        })
    }
    /// Only explicit, conflict-free associations carry a confirmed cost.
    /// The caller's unlinked rows already contain the canonical unknown cost.
    pub fn response_costs(&self) -> Result<HashMap<String, provider_cost::Cost>, String> {
        let mut statement = self.db.prepare("SELECT n.id,a.raw FROM fee_native n JOIN fees f ON f.id=n.attempt JOIN attempts a ON a.id=n.attempt WHERE n.association='response_id' AND n.conflict=0").map_err(fail)?;
        let mut rows = statement.query([]).map_err(fail)?;
        let mut result = HashMap::new();
        while let Some(row) = rows.next().map_err(fail)? {
            let raw: String = row.get(1).map_err(fail)?;
            let r: ModelObservation = serde_json::from_str(&raw).map_err(fail)?;
            result.insert(
                row.get(0).map_err(fail)?,
                provider_cost::calculate(
                    Some(&r.endpoint),
                    r.model.as_deref(),
                    Some(&r.started_at),
                    &r.status,
                    &r.usage,
                    r.reported_cost.as_ref(),
                    r.ended_at.as_deref(),
                ),
            );
        }
        Ok(result)
    }
    fn fee_groups(&self, from: &str, models: bool) -> Result<Vec<Value>, String> {
        let model = if models { "model" } else { "NULL" };
        list(
            &self.db,
            &format!(
                "SELECT json_object('providerKey',provider,'provider',json_extract(value,'$.provider'),'model',{model},'attempts',count(*),'cost',json_extract(stats_sum(stats),'$.cost')) FROM (SELECT * FROM {from} ORDER BY time DESC,id) GROUP BY provider,{model} ORDER BY provider,{model}"
            ),
        )
    }
    pub fn fee_details(
        &self,
        ids: &[String],
        q: &associated_cost::detail::Query,
        partial: bool,
    ) -> Result<Value, String> {
        q.validate_with_directory(&self.directory)
            .map_err(|e| e.message)?;
        let summary = self.fee_summary(ids, q.agent.as_deref(), partial)?;
        let from = self.fee_scope(ids, q.agent.as_deref(), "fees")?;
        let mut predicates = vec!["1".into()];
        if let Some(p) = &q.provider {
            predicates.push(format!("provider={}", quote(p)));
        }
        if let Some(m) = &q.model {
            predicates.push(if m == "_unknown" {
                "model IS NULL".into()
            } else {
                format!("model={}", quote(m))
            });
        }
        let selected = format!("(SELECT * FROM {from} WHERE {})", predicates.join(" AND "));
        let total: usize = self
            .db
            .query_row(&format!("SELECT count(*) FROM {selected}"), [], |r| {
                sql_count(r, 0)
            })
            .map_err(fail)?;
        let mut canonical = q.clone();
        canonical.cursor = None;
        let stamp: Option<i64> = self
            .db
            .query_row(
                "SELECT max(c.seq) FROM fee_clock c JOIN selected_sessions s ON s.id=c.session",
                [],
                |r| r.get(0),
            )
            .map_err(fail)?;
        let version = crate::projects::hash(&encode(&(canonical, ids, stamp, partial))?);
        let offset = super::usage::cursor(q.cursor.as_deref(), &version, total, 40)?;
        if !self
            .db
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM fee_positions WHERE version=?1)",
                [&version],
                |r| r.get::<_, bool>(0),
            )
            .map_err(fail)?
        {
            execute(
                &self.db,
                "DELETE FROM fee_positions WHERE version<>?1",
                [&version],
            )
            .map_err(fail)?;
            execute(&self.db,&format!("INSERT INTO fee_positions SELECT ?1,row_number() OVER(ORDER BY time DESC,id)-1,id FROM {selected}"),[&version]).map_err(fail)?;
        }
        let calls = list(
            &self.db,
            &format!(
                "SELECT f.value FROM fee_positions p JOIN fees f ON f.id=p.id WHERE p.version={} AND p.pos>={offset} ORDER BY p.pos LIMIT 40",
                quote(&version)
            ),
        )?;
        let text: String = self
            .db
            .query_row(
                &format!("SELECT stats_sum(stats) FROM {selected}"),
                [],
                |r| r.get(0),
            )
            .map_err(fail)?;
        let stats: Value = serde_json::from_str(&text).map_err(fail)?;
        Ok(
            json!({"summary":summary,"filteredCost":stats.get("cost").cloned().unwrap_or(value(&provider_cost::Totals::default())?),"total":total,"providers":self.fee_groups(&from,false)?,"models":self.fee_groups(&from,true)?,"filteredProviders":self.fee_groups(&selected,false)?,"filteredModels":self.fee_groups(&selected,true)?,"calls":calls,"nextCursor":offset.checked_add(40).filter(|n|*n<total).map(|n|format!("{version}:{n}"))}),
        )
    }
}
