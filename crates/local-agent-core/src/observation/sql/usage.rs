use super::aggregates::{percentile, quote};
use super::*;
use chrono::{DateTime, Duration, Utc};
use sha2::{Digest, Sha256};

pub(super) struct UsageCache {
    key: String,
    fingerprint: String,
    version: String,
    expires: i64,
    created: DateTime<Utc>,
    pub(super) value: Value,
    selection: Option<String>,
}

pub(super) fn provider_record(record: &provider_usage::Record) -> Result<String, String> {
    let mut stats = provider_usage::Totals::default();
    stats.add(record);
    stats.finish();
    let mut v = value(record)?;
    v["_stats"] = value(&stats)?;
    Ok(v.to_string())
}
pub(super) fn model_record(
    row: &ModelObservation,
    cost: &crate::provider_cost::Cost,
) -> Result<String, String> {
    model_value(&crate::model_usage::UsageRecord {
        cost: cost.clone(),
        id: row.id.clone(),
        model: row.model.clone(),
        provider: Some(row.provider.clone()),
        endpoint: Some(row.endpoint.clone()),
        route_revision: Some(row.binding_revision),
        time: Some(row.started_at.clone()),
        usage: row.usage.clone(),
        session_id: row.session_id.clone(),
        request_id: Some(row.id.clone()),
    })
    .map(|v| v.to_string())
}
pub(super) fn model_value(row: &crate::model_usage::UsageRecord) -> Result<Value, String> {
    let id = crate::model_usage::provider_identity(
        row.provider.as_deref(),
        row.endpoint.as_deref(),
        row.route_revision,
    );
    let mut stats = crate::model_usage::UsageTotals::default();
    stats.add(&row.usage, &row.cost);
    Ok(
        json!({"id":row.id,"model":row.model,"provider":row.provider,"endpoint":row.endpoint,"routeRevision":row.route_revision,"time":row.time,"usage":row.usage,"sessionId":row.session_id,"requestId":row.request_id,"cost":row.cost,"_providerId":id,"_stats":stats}),
    )
}
fn stats_column(
    db: &Connection,
    from: &str,
    column: &str,
    prototype: Value,
) -> Result<Value, String> {
    let text: String = db
        .query_row(
            &format!("SELECT stats_sum({column}) FROM {from}"),
            [],
            |r| r.get(0),
        )
        .map_err(fail)?;
    let v: Value = serde_json::from_str(&text).map_err(fail)?;
    let mut result = prototype;
    for (k, v) in v.as_object().ok_or("统计快照无效")? {
        result[k] = v.clone();
    }
    Ok(result)
}
fn time_range(time: &str, days: Option<u16>, now: DateTime<Utc>, inclusive: bool) -> String {
    let end = now.timestamp_millis();
    let cmp = if inclusive { "<=" } else { "<" };
    let mut sql = format!(
        "({time} IS NULL OR ({time}<={end} AND ({time}<>{end} OR instant_cmp(time_text,{}){cmp}0)))",
        quote(&now.to_rfc3339())
    );
    if let Some(days) = days {
        let start = now - Duration::days(days.into());
        let at = start.timestamp_millis();
        sql.push_str(&format!(
            " AND {time}>={at} AND ({time}<>{at} OR instant_cmp(time_text,{})>=0)",
            quote(&start.to_rfc3339())
        ));
    }
    sql
}
pub(super) fn range_expiry(
    db: &Connection,
    from: &str,
    days: Option<u16>,
    now: DateTime<Utc>,
) -> Result<Option<i64>, String> {
    let at = now.timestamp_millis();
    // Keep MIN directly on the indexed column: a CASE expression forces a
    // full scan and evaluates agent_matches for every authorized Agent row.
    let future: Option<i64> = db
        .query_row(
            &format!("SELECT min(time) FROM {from} WHERE time>=?1"),
            [at],
            |r| r.get(0),
        )
        .map_err(fail)?;
    let expiry = if let Some(days) = days {
        let window = i64::from(days) * 86_400_000;
        let oldest: Option<i64> = db
            .query_row(
                &format!("SELECT min(time) FROM {from} WHERE time>=?1 AND time<=?2"),
                params![at.saturating_sub(window), at],
                |r| r.get(0),
            )
            .map_err(fail)?;
        oldest.and_then(|t| t.checked_add(window))
    } else {
        None
    };
    Ok(future.into_iter().chain(expiry).min())
}
fn version(db: &Connection, from: &str, key: &str) -> Result<String, String> {
    let digest: String = db
        .query_row(
            &format!("SELECT hash_rows(value) FROM (SELECT value FROM {from} ORDER BY id)"),
            [],
            |r| r.get(0),
        )
        .map_err(fail)?;
    Ok(format!("{:x}", Sha256::digest(format!("{key}\0{digest}"))))
}
pub(super) fn cursor(
    cursor: Option<&str>,
    version: &str,
    total: usize,
    size: usize,
) -> Result<usize, String> {
    let offset = if let Some(c) = cursor {
        let (v, n) = c.split_once(':').ok_or("历史分页无效")?;
        if v != version {
            return Err("历史调用已更新，请从第一页重新读取".into());
        }
        n.parse::<usize>().map_err(|_| "历史分页无效")?
    } else {
        0
    };
    if offset > total || !offset.is_multiple_of(size) {
        return Err("历史分页无效".into());
    }
    Ok(offset)
}
impl History {
    fn prune_usage_positions(&self, kind: &str, current: Option<&str>) -> Result<(), String> {
        let cache = if kind == "provider" {
            &self.provider_cache
        } else {
            &self.model_cache
        };
        let versions = cache
            .iter()
            .map(|c| quote(&c.version))
            .chain(current.map(quote))
            .collect::<Vec<_>>()
            .join(",");
        execute(
            &self.db,
            &format!("DELETE FROM {kind}_positions WHERE version NOT IN ({versions})"),
            [],
        )
        .map_err(fail)?;
        Ok(())
    }
    fn cache_usage(&mut self, kind: &str, entry: UsageCache) -> Result<(), String> {
        let cache = if kind == "provider" {
            &mut self.provider_cache
        } else {
            &mut self.model_cache
        };
        cache.retain(|old| old.key != entry.key);
        if cache.len() == 8 {
            cache.remove(0);
        }
        cache.push(entry);
        self.prune_usage_positions(kind, None)?;
        Ok(())
    }
    fn provider_calls(
        &self,
        table: &str,
        version: &str,
        offset: usize,
        selection: Option<&str>,
    ) -> Result<Vec<Value>, String> {
        if let Some(selection) = selection {
            let exists: bool = self
                .db
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM provider_positions WHERE version=?1 AND pos=?2)",
                    params![version, i64::try_from(offset).map_err(fail)?],
                    |r| r.get(0),
                )
                .map_err(fail)?;
            if !exists {
                // Cache only this requested page. Cursor validation above still
                // covers all visible rows; untouched pages need no position rows.
                execute(&self.db, &format!("INSERT OR IGNORE INTO provider_positions SELECT ?1,{offset}+row_number() OVER(ORDER BY time_text DESC,id)-1,id FROM (SELECT id,time_text FROM {selection} ORDER BY time_text DESC,id LIMIT 40 OFFSET {offset})"),[version]).map_err(fail)?;
            }
        }
        aggregates::list(
            &self.db,
            &format!(
                "SELECT json_remove(CASE WHEN json_extract(a.provider_record,'$.providerId') IS NOT NULL AND NOT EXISTS(SELECT 1 FROM configured_providers c WHERE c.id=json_extract(a.provider_record,'$.providerId')) THEN json_set(a.provider_record,'$.providerId',NULL,'$.attribution','removed_provider') ELSE a.provider_record END,'$._stats') FROM provider_positions p JOIN {table} a ON a.id=p.id WHERE p.version={} AND p.pos>={offset} AND p.pos<{offset}+40 ORDER BY p.pos",
                quote(version)
            ),
        )
    }
    fn provider_groups(
        &self,
        from: &str,
        column: &str,
        samples: &str,
    ) -> Result<Vec<Value>, String> {
        let keys = aggregates::list(
            &self.db,
            &format!("SELECT json_quote({column}) FROM {from} LIMIT 1"),
        )?;
        let multiple = if let Some(key) = keys.first() {
            self.db.query_row(&format!("SELECT EXISTS(SELECT 1 FROM {from} WHERE {column} IS NOT json_extract(?1,'$'))"),[key.to_string()],|r|r.get::<_,bool>(0)).map_err(fail)?
        } else {
            false
        };
        if !multiple {
            return keys
                .into_iter()
                .map(|key| {
                    let mut total = stats_column(
                        &self.db,
                        from,
                        "stats",
                        value(&provider_usage::Totals::default())?,
                    )?;
                    self.provider_percentiles(samples, &mut total)?;
                    Ok(json!({"key":key,"totals":total}))
                })
                .collect();
        }
        aggregates::list(&self.db,&format!("WITH samples AS MATERIALIZED(SELECT {column} key,latency,ttft FROM {samples} WHERE latency IS NOT NULL OR ttft IS NOT NULL),
          totals AS (SELECT {column} key,stats_sum(stats) stats FROM {from} GROUP BY {column}),
          latency_rank AS (SELECT key,latency,row_number() OVER(PARTITION BY key ORDER BY latency) rank,count(*) OVER(PARTITION BY key) n FROM samples WHERE latency IS NOT NULL),
          ttft_rank AS (SELECT key,ttft,row_number() OVER(PARTITION BY key ORDER BY ttft) rank,count(*) OVER(PARTITION BY key) n FROM samples WHERE ttft IS NOT NULL),
          latency AS MATERIALIZED(SELECT key,latency,n FROM latency_rank WHERE rank=(n*95+99)/100),
          ttft AS MATERIALIZED(SELECT key,ttft,n FROM ttft_rank WHERE rank=(n*95+99)/100)
          SELECT json_object('key',g.key,'totals',json_set(g.stats,'$.latencyP95Ms',l.latency,'$.latencySamples',coalesce(l.n,0),'$.ttftP95Ms',t.ttft,'$.ttftSamples',coalesce(t.n,0))) FROM totals g LEFT JOIN latency l ON l.key IS g.key LEFT JOIN ttft t ON t.key IS g.key ORDER BY g.key"))
    }
    pub fn set_evidence(&mut self, evidence: &[provider_usage::Evidence]) -> Result<(), String> {
        let key = encode(
            &evidence
                .iter()
                .map(|e| {
                    (
                        &e.agent,
                        &e.provider_id,
                        &e.provider_name,
                        &e.endpoint,
                        &e.protocol,
                        e.revision,
                        &e.native_provider,
                        &e.model_aliases,
                    )
                })
                .collect::<Vec<_>>(),
        )?;
        let same = key == self.evidence;
        let pending: bool = self
            .db
            .query_row("SELECT EXISTS(SELECT 1 FROM legacy_pending)", [], |r| {
                r.get(0)
            })
            .map_err(fail)?;
        if same && !pending {
            return Ok(());
        }
        let mut after = String::new();
        loop {
            let rows: Vec<(String, String)> = {
                let predicate = if same {
                    "id IN legacy_pending"
                } else {
                    "json_extract(raw,'$.providerId') IS NULL"
                };
                let mut s=self.db.prepare(&format!("SELECT id,raw FROM attempts WHERE id>?1 AND {predicate} ORDER BY id LIMIT 256")).map_err(fail)?;
                s.query_map([&after], |r| Ok((r.get(0)?, r.get(1)?)))
                    .map_err(fail)?
                    .collect::<Result<_, _>>()
                    .map_err(fail)?
            };
            if rows.is_empty() {
                break;
            }
            after = rows.last().unwrap().0.clone();
            let decoded = rows
                .into_iter()
                .map(|(_, raw)| serde_json::from_str::<ModelObservation>(&raw).map_err(fail))
                .collect::<Result<_, _>>()?;
            for record in provider_usage::gateway_records(decoded, evidence) {
                execute(
                    &self.db,
                    "UPDATE attempts SET provider_record=?2 WHERE id=?1",
                    params![record.id, provider_record(&record)?],
                )
                .map_err(fail)?;
            }
        }
        self.evidence = key;
        execute(&self.db, "DELETE FROM legacy_pending", []).map_err(fail)?;
        Ok(())
    }
    pub fn set_native_usage(
        &mut self,
        provider: Vec<provider_usage::Record>,
        models: Vec<crate::model_usage::UsageRecord>,
    ) -> Result<(), String> {
        self.native_usage_tables()?;
        self.db
            .execute_batch("SAVEPOINT native_usage_sync;")
            .map_err(fail)?;
        let result = (|| {
            execute(&self.db, "DELETE FROM native_usage", []).map_err(fail)?;
            for record in provider {
                execute(
                    &self.db,
                    "INSERT OR IGNORE INTO native_usage(id,provider_record) VALUES(?1,?2)",
                    params![record.id, provider_record(&record)?],
                )
                .map_err(fail)?;
            }
            for (order, row) in models.into_iter().enumerate() {
                let mut v = model_value(&row)?;
                v["_order"] = json!(order);
                execute(&self.db,"INSERT INTO native_usage(id,model_record) VALUES(?1,?2) ON CONFLICT(id) DO UPDATE SET model_record=excluded.model_record WHERE native_usage.model_record IS NULL",params![row.id,v.to_string()]).map_err(fail)?;
            }
            self.db.execute_batch("UPDATE native_usage_clock SET seq=seq+1,provider=seq+1,model=seq+1;DELETE FROM native_usage_changes;DELETE FROM usage_projection_clock WHERE kind IN ('native_provider_facts','native_model_facts');").map_err(fail)?;
            Ok(())
        })();
        if result.is_ok() {
            self.db
                .execute_batch("RELEASE native_usage_sync")
                .map_err(fail)?;
            self.native_provider = None;
            self.native_model = None;
        } else {
            self.db
                .execute_batch("ROLLBACK TO native_usage_sync;RELEASE native_usage_sync")
                .map_err(fail)?;
        }
        result
    }
    pub fn provider_usage(
        &mut self,
        q: provider_usage::Query,
        providers: &[crate::Provider],
        source_as_of: String,
        source_state: String,
        now: DateTime<Utc>,
    ) -> Result<Value, String> {
        q.validate_with_directory(&self.directory)
            .map_err(str::to_owned)?;
        let configured: Vec<_> = providers.iter().filter(|p| p.configured).collect();
        if q.provider_id
            .as_deref()
            .is_some_and(|id| id != "_unknown" && !configured.iter().any(|p| p.id == id))
        {
            return Err("未找到供应商连接".into());
        }
        self.db.execute_batch("CREATE TEMP TABLE IF NOT EXISTS configured_providers(id TEXT PRIMARY KEY);DELETE FROM configured_providers;").map_err(fail)?;
        for p in &configured {
            execute(
                &self.db,
                "INSERT INTO configured_providers VALUES(?1)",
                [&p.id],
            )
            .map_err(fail)?;
        }
        let table = self.usage_table("provider", q.source == "native");
        let clock = self.usage_kind_clock("provider", q.source == "native")?;
        let cache_key = encode(&(&q.source, &q.agent, q.days, &q.provider_id, &q.model))?;
        let fingerprint = format!(
            "{:x}",
            Sha256::digest(encode(&(
                &cache_key,
                clock,
                &self.evidence,
                configured.iter().map(|p| &p.id).collect::<Vec<_>>()
            ))?)
        );
        if let Some(cache) = self
            .provider_cache
            .iter()
            .find(|c| c.fingerprint == fingerprint)
            && now >= cache.created
            && now.timestamp_millis() < cache.expires
        {
            let exists: bool = self
                .db
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM provider_positions WHERE version=?1)",
                    [&cache.version],
                    |r| r.get(0),
                )
                .map_err(fail)?;
            let total = cache.value["totalCalls"]
                .as_u64()
                .and_then(|n| usize::try_from(n).ok())
                .ok_or("供应商统计快照无效")?;
            if cache.selection.is_some() || exists || total == 0 {
                let offset = cursor(q.cursor.as_deref(), &cache.version, total, 40)?;
                let mut result = cache.value.clone();
                result["calls"] = value(&self.provider_calls(
                    table,
                    &cache.version,
                    offset,
                    cache.selection.as_deref(),
                )?)?;
                result["nextCursor"] = json!(
                    offset
                        .checked_add(40)
                        .filter(|n| *n < total)
                        .map(|n| format!("{}:{n}", cache.version))
                );
                result["query"] = value(&q)?;
                result["asOf"] = json!(now.to_rfc3339());
                result["sourceAsOf"] = json!(source_as_of);
                result["sourceState"] = json!(source_state);
                return Ok(result);
            }
        }
        let agent = q
            .agent
            .as_ref()
            .map(|s| format!("agent_matches({},agent,product)", quote(s)))
            .unwrap_or("1".into());
        let facts = self.usage_facts("provider", q.source == "native")?;
        // A flattened view scopes narrow facts; it never writes full payloads
        // or copies every statistic again when the user changes a filter.
        let work = format!(
            "(SELECT source_order,id,time,time_text,f.provider original_provider,CASE WHEN EXISTS(SELECT 1 FROM configured_providers p WHERE p.id=f.provider) THEN provider END provider,model,stats,latency,ttft,digest,removed_digest FROM {facts} f WHERE {agent})"
        );
        self.db.execute_batch(&format!("DROP VIEW IF EXISTS provider_work;CREATE TEMP VIEW provider_work AS SELECT * FROM {work};")).map_err(fail)?;
        let time = "time";
        let in_time = time_range(time, q.days, now, true);
        let scoped = format!("(SELECT * FROM {work} WHERE {in_time})");
        // Partial sample indexes prove a native history has no timing evidence
        // without scanning every authorized row for every provider/model group.
        // Any real sample keeps the original scoped percentile calculation.
        let no_samples = q.source == "native"
            && !self.db.query_row(
                &format!("SELECT EXISTS(SELECT 1 FROM {facts} WHERE latency IS NOT NULL) OR EXISTS(SELECT 1 FROM {facts} WHERE ttft IS NOT NULL)"),
                [], |r| r.get::<_, bool>(0),
            ).map_err(fail)?;
        let sample_scope = |scope: &str| {
            if no_samples {
                "(SELECT NULL provider,NULL model,NULL latency,NULL ttft WHERE 0)".to_owned()
            } else {
                scope.to_owned()
            }
        };
        let blocks = if q.source == "native" {
            Some(self.usage_blocks(
                "provider",
                "provider_work",
                &in_time,
                &encode(&(
                    &q.agent,
                    q.days,
                    configured.iter().map(|p| &p.id).collect::<Vec<_>>(),
                ))?,
                q.days,
                now,
            )?)
        } else {
            None
        };
        let compact = if let Some(blocks) = &blocks {
            let has_money = self
                .db
                .query_row(
                    &format!("SELECT EXISTS(SELECT 1 FROM {blocks} WHERE unsafe)"),
                    [],
                    |r| r.get::<_, bool>(0),
                )
                .map_err(fail)?;
            if has_money {
                scoped.clone()
            } else {
                blocks.clone()
            }
        } else {
            scoped.clone()
        };
        let empty = value(&provider_usage::Totals::default())?;
        let grouped = self.provider_groups(&compact, "provider", &sample_scope(&scoped))?;
        let provider_total = |id: Option<&str>| {
            grouped
                .iter()
                .find(|g| g["key"].as_str() == id)
                .map(|g| g["totals"].clone())
                .unwrap_or(empty.clone())
        };
        // These are the exact same scopes, including their percentile samples.
        // Reuse them rather than scanning/sorting the whole provider again.
        let unattributed = provider_total(None);
        let summary = if grouped.len() <= 1 {
            grouped
                .first()
                .map(|g| g["totals"].clone())
                .unwrap_or(empty.clone())
        } else {
            let mut total = stats_column(&self.db, &compact, "stats", empty.clone())?;
            self.provider_percentiles(&sample_scope(&scoped), &mut total)?;
            total
        };
        let mut provider_totals = vec![];
        for p in configured {
            let totals = provider_total(Some(&p.id));
            provider_totals.push(json!({"providerId":p.id,"totals":totals}));
        }
        provider_totals.sort_by(|a, b| a["providerId"].as_str().cmp(&b["providerId"].as_str()));
        // Compare the same model across connections using full narrow facts / native blocks,
        // never the selected connection's forty visible payloads.
        let model_scope = format!("(SELECT * FROM {compact} WHERE provider IS NOT NULL)");
        let model_samples = sample_scope(&format!(
            "(SELECT * FROM {scoped} WHERE provider IS NOT NULL)"
        ));
        let mut provider_models = self
            .provider_groups(
                &model_scope,
                "json_array(provider,model) || ''",
                &model_samples,
            )?
            .into_iter()
            .map(|group| {
                let (provider_id, model): (String, Option<String>) =
                    serde_json::from_str(group["key"].as_str().ok_or("模型连接分组无效")?)
                        .map_err(fail)?;
                Ok(json!({"providerId":provider_id,"model":model,"totals":group["totals"]}))
            })
            .collect::<Result<Vec<Value>, String>>()?;
        provider_models.sort_by(|a, b| {
            a["providerId"]
                .as_str()
                .cmp(&b["providerId"].as_str())
                .then(a["model"].as_str().cmp(&b["model"].as_str()))
        });
        let select = q
            .provider_id
            .as_deref()
            .map(|id| {
                if id == "_unknown" {
                    "provider IS NULL".into()
                } else {
                    // A validated configured ID has identical normalized/raw
                    // attribution. Seek its index before aggregating details.
                    format!("original_provider={}", quote(id))
                }
            })
            .unwrap_or("0".into());
        let selected_scope = format!("(SELECT * FROM {scoped} WHERE {select})");
        let model = q
            .model
            .as_ref()
            .map(|m| format!("coalesce(model,'')={}", quote(m)))
            .unwrap_or("1".into());
        let selected = format!("(SELECT * FROM {selected_scope} WHERE {model})");
        let compact_select = q
            .provider_id
            .as_deref()
            .map(|id| {
                if id == "_unknown" {
                    "provider IS NULL".into()
                } else {
                    format!("provider={}", quote(id))
                }
            })
            .unwrap_or("0".into());
        let compact_selected = if q.source == "native" {
            format!("(SELECT * FROM {compact} WHERE {compact_select} AND {model})")
        } else {
            selected.clone()
        };
        let option_scope = if q.source == "native" {
            format!("(SELECT * FROM {compact} WHERE {compact_select})")
        } else {
            selected_scope.clone()
        };
        let options = aggregates::list(
            &self.db,
            &format!("SELECT json_quote(model) FROM {option_scope} GROUP BY model ORDER BY model"),
        )?;
        let selected_total = if let Some(id) = &q.provider_id {
            if q.model.is_none() {
                provider_total((id != "_unknown").then_some(id.as_str()))
            } else {
                let mut total = stats_column(&self.db, &compact_selected, "stats", empty.clone())?;
                self.provider_percentiles(&sample_scope(&selected), &mut total)?;
                total
            }
        } else {
            Value::Null
        };
        let models: Vec<_> = if options.len() == 1 && q.model.is_none() {
            vec![json!({"model":options[0],"totals":selected_total})]
        } else {
            self.provider_groups(&compact_selected, "model", &sample_scope(&selected))?
                .into_iter()
                .map(|v| json!({"model":v["key"],"totals":v["totals"]}))
                .collect()
        };
        let total: usize = if blocks.is_some() {
            if q.provider_id.is_some() {
                selected_total["calls"]
                    .as_u64()
                    .and_then(|n| usize::try_from(n).ok())
                    .ok_or("供应商统计快照无效")?
            } else {
                0
            }
        } else {
            self.db
                .query_row(&format!("SELECT count(*) FROM {selected}"), [], |r| {
                    sql_count(r, 0)
                })
                .map_err(fail)?
        };
        let key = encode(&(
            &q.source,
            &q.agent,
            q.days,
            &q.provider_id,
            &q.model,
            &source_state,
        ))?;
        // Record digests are updated with the narrow projection. Membership,
        // attribution normalization and every visible field still affect the
        // cursor; unrelated mutations outside this scope do not invalidate it.
        let version_scope = if let Some(blocks) = &blocks {
            format!(
                "(SELECT json_array(bucket,provider,model) id,digest value FROM {blocks} WHERE {compact_select} AND {model})"
            )
        } else {
            // Gateway cursors retain their established byte-level contract.
            format!(
                "(SELECT s.id,CASE WHEN s.provider IS NULL AND json_extract(a.provider_record,'$.providerId') IS NOT NULL THEN json_set(a.provider_record,'$.providerId',NULL,'$.attribution','removed_provider') ELSE a.provider_record END value FROM {selected} s JOIN {table} a ON a.id=s.id)"
            )
        };
        let version = version(&self.db, &version_scope, &key)?;
        let offset = cursor(q.cursor.as_deref(), &version, total, 40)?;
        self.db.execute_batch("CREATE TEMP TABLE IF NOT EXISTS provider_positions(version TEXT,pos INTEGER,id TEXT,PRIMARY KEY(version,pos)) WITHOUT ROWID;").map_err(fail)?;
        let exists: bool = self
            .db
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM provider_positions WHERE version=?1)",
                [&version],
                |r| r.get(0),
            )
            .map_err(fail)?;
        if !exists && q.source != "native" {
            execute(&self.db,&format!("INSERT INTO provider_positions SELECT ?1,row_number() OVER(ORDER BY time_text DESC,id)-1,id FROM {selected}"),[&version]).map_err(fail)?;
        }
        // Seek the ordered IDs before decoding the forty visible records.
        self.prune_usage_positions("provider", Some(&version))?;
        // Native cursors are validated against the complete scoped digest. Seek
        // only the visible page instead of sorting/writing every historical ID.
        let selection = (q.source == "native").then_some(selected.clone());
        let calls = self.provider_calls(table, &version, offset, selection.as_deref())?;
        let missing_from = format!(
            "(SELECT * FROM provider_work WHERE {} AND {model} AND {time} IS NULL)",
            if q.provider_id.is_some() {
                select.as_str()
            } else {
                "1"
            }
        );
        let missing: usize = self
            .db
            .query_row(&format!("SELECT count(*) FROM {missing_from}"), [], |r| {
                sql_count(r, 0)
            })
            .map_err(fail)?;
        let result = json!({"query":q,"asOf":now.to_rfc3339(),"sourceAsOf":source_as_of,"sourceState":source_state,"summary":summary,"providers":provider_totals,"providerModels":provider_models,"unattributed":unattributed,"selected":selected_total,"modelOptions":options,"models":models,"calls":calls,"totalCalls":total,"nextCursor":offset.checked_add(40).filter(|n|*n<total).map(|n|format!("{version}:{n}")),"missingTime":missing});
        // Bounded response scopes. Scope/date boundaries and every
        // attempt mutation invalidate it; position rows remain SQLite-owned.
        if result["models"].as_array().is_some_and(|a| a.len() <= 1000)
            && result.to_string().len() <= 1024 * 1024
            && result["modelOptions"]
                .as_array()
                .is_some_and(|a| a.len() <= 1000)
            && result["providers"]
                .as_array()
                .is_some_and(|a| a.len() <= 1000)
        {
            let future: Option<i64> = self
                .db
                .query_row(
                    "SELECT min(time) FROM provider_work WHERE time>=?1",
                    [now.timestamp_millis()],
                    |r| r.get(0),
                )
                .map_err(fail)?;
            let expiry: Option<i64> = if let Some(days) = q.days {
                let window = i64::from(days) * 86_400_000;
                self.db
                    .query_row(
                        "SELECT min(time)+?1 FROM provider_work WHERE time>=?2 AND time<=?3",
                        params![
                            window,
                            now.timestamp_millis() - window,
                            now.timestamp_millis()
                        ],
                        |r| r.get(0),
                    )
                    .map_err(fail)?
            } else {
                None
            };
            self.cache_usage(
                "provider",
                UsageCache {
                    key: cache_key,
                    fingerprint,
                    version,
                    expires: future.into_iter().chain(expiry).min().unwrap_or(i64::MAX),
                    created: now,
                    value: result.clone(),
                    selection,
                },
            )?;
        }
        Ok(result)
    }
    fn provider_percentiles(&self, from: &str, total: &mut Value) -> Result<(), String> {
        for (path, key, samples) in [
            ("latency", "latencyP95Ms", "latencySamples"),
            ("ttft", "ttftP95Ms", "ttftSamples"),
        ] {
            let (n, p95) = percentile(&self.db, from, path, 95)?;
            total[key] = json!(p95);
            total[samples] = json!(n);
        }
        Ok(())
    }
    pub fn model_usage(
        &mut self,
        q: crate::model_usage::UsageQuery,
        source_as_of: String,
        source_state: String,
        now: DateTime<Utc>,
    ) -> Result<Value, String> {
        q.validate_with_directory(&self.directory)
            .map_err(str::to_owned)?;
        let table = self.usage_table("model", q.source == "native");
        let family = if q.source == "gateway" || self.native_model.is_some() {
            format!("agent_matches({},agent,product)", quote(&q.agent))
        } else {
            "1".into()
        };
        let time = "time";
        let key = encode(&(
            &q.agent,
            &q.source,
            q.days,
            &q.provider,
            &q.model,
            &source_state,
        ))?;
        let clock = self.usage_kind_clock("model", q.source == "native")?;
        let cache_key = encode(&(&q.agent, &q.source, q.days, &q.provider, &q.model))?;
        let fingerprint = format!("{:x}", Sha256::digest(format!("{cache_key}\0{clock}")));
        if let Some(cache) = self
            .model_cache
            .iter()
            .find(|c| c.fingerprint == fingerprint)
            && now >= cache.created
            && now.timestamp_millis() < cache.expires
        {
            let total = cache.value["total"]
                .as_u64()
                .and_then(|n| usize::try_from(n).ok())
                .ok_or("模型统计快照无效")?;
            let exists: bool = self
                .db
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM model_positions WHERE version=?1)",
                    [&cache.version],
                    |r| r.get(0),
                )
                .map_err(fail)?;
            if exists || total == 0 {
                let offset = cursor(q.cursor.as_deref(), &cache.version, total, 20)?;
                let mut result = cache.value.clone();
                result["items"] = value(&aggregates::list(
                    &self.db,
                    &format!(
                        "SELECT value FROM model_positions WHERE version={} AND pos>={offset} ORDER BY pos LIMIT 20",
                        quote(&cache.version)
                    ),
                )?)?;
                result["query"] = value(&q)?;
                result["asOf"] = json!(now.to_rfc3339());
                result["sourceAsOf"] = json!(source_as_of);
                result["sourceState"] = json!(source_state);
                result["nextCursor"] = json!(
                    offset
                        .checked_add(20)
                        .filter(|n| *n < total)
                        .map(|n| format!("{}:{n}", cache.version))
                );
                return Ok(result);
            }
        }
        let facts = self.usage_facts("model", q.source == "native")?;
        let full = format!("(SELECT * FROM {facts} WHERE {family})");
        let in_time = time_range(time, q.days, now, false);
        let provider = q
            .provider
            .as_ref()
            .map(|p| format!("pid={}", quote(p)))
            .unwrap_or("1".into());
        let model = q
            .model
            .as_ref()
            .map(|m| {
                format!(
                    "instr(unicode_lower(coalesce(model,'')),{})>0",
                    quote(&m.to_lowercase())
                )
            })
            .unwrap_or("1".into());
        let range = format!("(SELECT * FROM {full} WHERE {in_time})");
        let selected = format!("(SELECT * FROM {range} WHERE {provider} AND {model})");
        let compact = if q.source == "native" {
            let blocks = self.usage_blocks(
                "model",
                &full,
                &in_time,
                &encode(&(&q.agent, q.days))?,
                q.days,
                now,
            )?;
            format!("(SELECT *,provider pid FROM {blocks})")
        } else {
            range.clone()
        };
        let compact_selected = format!("(SELECT * FROM {compact} WHERE {provider} AND {model})");
        // Regrouping floating-point charges can change their addition order.
        // Preserve the existing exact order whenever a selected charge exists.
        let has_money = q.source == "native"
            && self
                .db
                .query_row(
                    &format!("SELECT EXISTS(SELECT 1 FROM {compact_selected} WHERE unsafe)"),
                    [],
                    |r| r.get::<_, bool>(0),
                )
                .map_err(fail)?;
        let totals_from = if q.source == "native" && !has_money {
            compact_selected.clone()
        } else {
            selected.clone()
        };
        let empty = value(&crate::model_usage::UsageTotals::default())?;
        let summary = stats_column(
            &self.db,
            &if q.source == "native" && !has_money {
                totals_from.clone()
            } else {
                format!("(SELECT * FROM {selected} ORDER BY source_order)")
            },
            "stats",
            empty.clone(),
        )?;
        let providers = aggregates::list(
            &self.db,
            &format!(
                "SELECT json_object('id',s.pid,'name',json_extract(a.model_record,'$.provider'),'endpoint',json_extract(a.model_record,'$.endpoint'),'routeRevision',json(a.model_record -> '$.routeRevision')) FROM (SELECT pid,id FROM {compact} GROUP BY pid) s JOIN {table} a ON a.id=s.id ORDER BY s.pid"
            ),
        )?;
        // Aggregate each provider/model pair once; select latest references
        // with SQL ranks instead of scanning all attempts once per model.
        self.db.execute_batch("CREATE TEMP TABLE IF NOT EXISTS model_groups(id TEXT,model TEXT,value TEXT,tokens TEXT);DELETE FROM model_groups;").map_err(fail)?;
        let tie = if q.source == "native" {
            "CASE WHEN t IS NULL THEN ord END DESC,CASE WHEN t IS NOT NULL THEN ord END ASC,id"
        } else {
            "CASE WHEN t IS NULL THEN id END ASC,id DESC"
        };
        execute(&self.db,&format!("WITH ranked AS (SELECT id,pid,model,t,row_number() OVER(PARTITION BY pid,model ORDER BY t IS NULL,t DESC,{tie}) rank FROM {compact_selected}),
          totals AS (SELECT pid,model,stats_sum(stats) stats FROM {totals_from} GROUP BY pid,model)
          INSERT INTO model_groups SELECT g.pid,g.model,json_object('providerId',g.pid,'provider',json_extract(a.model_record,'$.provider'),'endpoint',json_extract(a.model_record,'$.endpoint'),'routeRevision',json(a.model_record -> '$.routeRevision'),'model',g.model,'totals',json(g.stats),'lastSeen',CASE WHEN l.t IS NOT NULL THEN json_extract(a.model_record,'$.time') END,'sessionId',json_extract(a.model_record,'$.sessionId'),'requestId',json_extract(a.model_record,'$.requestId')),json_extract(g.stats,'$.usage.totalTokens') FROM totals g JOIN ranked l ON l.pid=g.pid AND l.model IS g.model AND l.rank=1 JOIN {table} a ON a.id=l.id"),[]).map_err(fail)?;
        let mut after = 0i64;
        loop {
            let rows: Vec<(i64, String)> = {
                let mut s=self.db.prepare("SELECT rowid,value FROM model_groups WHERE rowid>?1 ORDER BY rowid LIMIT 256").map_err(fail)?;
                s.query_map([after], |r| Ok((r.get(0)?, r.get(1)?)))
                    .map_err(fail)?
                    .collect::<Result<_, _>>()
                    .map_err(fail)?
            };
            if rows.is_empty() {
                break;
            }
            after = rows.last().unwrap().0;
            for (id, raw) in rows {
                let mut v: Value = serde_json::from_str(&raw).map_err(fail)?;
                v["lastSeen"] = json!(
                    v["lastSeen"]
                        .as_str()
                        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                        .map(|t| t.with_timezone(&Utc).to_rfc3339())
                );
                // Extract u64 in Rust: SQLite's JSON number conversion uses a
                // double above i64::MAX, which would lose exact group ordering.
                let tokens = v["totals"]["usage"]["totalTokens"]
                    .as_u64()
                    .map(|n| n.to_string());
                execute(
                    &self.db,
                    "UPDATE model_groups SET value=?2,tokens=?3 WHERE rowid=?1",
                    params![id, v.to_string(), tokens],
                )
                .map_err(fail)?;
            }
        }
        let total: usize = self
            .db
            .query_row("SELECT count(*) FROM model_groups", [], |r| sql_count(r, 0))
            .map_err(fail)?;
        let version = version(&self.db, "model_groups", &key)?;
        let offset = cursor(q.cursor.as_deref(), &version, total, 20)?;
        self.db.execute_batch("CREATE TEMP TABLE IF NOT EXISTS model_positions(version TEXT,pos INTEGER,value TEXT,PRIMARY KEY(version,pos)) WITHOUT ROWID;").map_err(fail)?;
        let exists: bool = self
            .db
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM model_positions WHERE version=?1)",
                [&version],
                |r| r.get(0),
            )
            .map_err(fail)?;
        if !exists {
            execute(&self.db,"INSERT INTO model_positions SELECT ?1,row_number() OVER(ORDER BY tokens IS NULL,length(tokens) DESC,tokens DESC,id,model)-1,value FROM model_groups",[&version]).map_err(fail)?;
        }
        let items = aggregates::list(
            &self.db,
            &format!(
                "SELECT value FROM model_positions WHERE version={} AND pos>={offset} ORDER BY pos LIMIT 20",
                quote(&version)
            ),
        )?;
        self.prune_usage_positions("model", Some(&version))?;
        let missing: usize = self
            .db
            .query_row(
                &format!(
                    "SELECT count(*) FROM {full} WHERE {provider} AND {model} AND {time} IS NULL"
                ),
                [],
                |r| sql_count(r, 0),
            )
            .map_err(fail)?;
        let result = json!({"query":q,"asOf":now.to_rfc3339(),"sourceAsOf":source_as_of,"sourceState":source_state,"summary":summary,"providers":providers,"items":items,"total":total,"nextCursor":offset.checked_add(20).filter(|n|*n<total).map(|n|format!("{version}:{n}")),"missingTime":missing});
        if result.to_string().len() <= 1024 * 1024
            && result["providers"]
                .as_array()
                .is_some_and(|a| a.len() <= 1000)
        {
            let expires = range_expiry(&self.db, &full, q.days, now)?;
            self.cache_usage(
                "model",
                UsageCache {
                    key: cache_key,
                    fingerprint,
                    version,
                    expires: expires.unwrap_or(i64::MAX),
                    created: now,
                    value: result.clone(),
                    selection: None,
                },
            )?;
        }
        Ok(result)
    }
}
