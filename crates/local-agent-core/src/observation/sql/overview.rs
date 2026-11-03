use super::aggregates::quote;
use super::*;
use chrono::{DateTime, Duration, Utc};
impl History {
    pub(super) fn update_analysis(
        &self,
        key: &str,
        rows: &[ModelObservation],
    ) -> Result<(), String> {
        execute(
            &self.db,
            "DELETE FROM analyses WHERE kind='gateway' AND id=?1",
            [key],
        )
        .map_err(fail)?;
        if rows.is_empty() {
            return Ok(());
        }
        let mut cost = super::super::AnalysisCost::default();
        let mut tokens: Option<u64> = None;
        let mut first = &rows[0];
        let mut partial = false;
        let mut provisional = false;
        for row in rows {
            if row.started_at < first.started_at
                || (row.started_at == first.started_at && row.id > first.id)
            {
                first = row;
            }
            if row.usage.total_tokens.is_some() {
                tokens = Some(
                    tokens
                        .unwrap_or(0)
                        .saturating_add(row.usage.total_tokens.unwrap()),
                );
            }
            partial |= row.usage.total_tokens.is_none() || row.status != "success";
            provisional |= row.status == "in_progress";
            cost.add(&crate::provider_cost::calculate(
                Some(&row.endpoint),
                row.model.as_deref(),
                Some(&row.started_at),
                &row.status,
                &row.usage,
                row.reported_cost.as_ref(),
                row.ended_at.as_deref(),
            ));
        }
        execute(
            &self.db,
            "INSERT INTO analyses VALUES(?1,'gateway',?2,?3,?4,?5,?6,?7,?8,?9)",
            params![
                key,
                crate::agents::group(&first.agent_id, first.agent_product_id.as_deref()),
                first.model,
                timestamp(&first.started_at),
                tokens.map(|n| n.to_string()),
                partial,
                provisional,
                encode(&cost)?,
                rows.len() as i64
            ],
        )
        .map_err(fail)?;
        Ok(())
    }
    pub fn native_analysis_matches(&self, revision: u64) -> bool {
        self.native_analysis_revision == Some(revision)
    }
    pub fn sync_native_analysis(
        &mut self,
        catalog: &crate::projects::ProjectCatalog,
    ) -> Result<usize, String> {
        self.use_directory(catalog.calls.directory())?;
        if self.native_analysis_matches(catalog.revision) {
            return Ok(0);
        }
        let (removed, added) = catalog.calls.delta(&self.native_analysis_calls);
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
        let partial: HashMap<_, _> = catalog
            .sessions
            .iter()
            .map(|s| (s.id.as_str(), s.partial))
            .collect();
        self.db
            .execute_batch("SAVEPOINT native_analysis_sync")
            .map_err(fail)?;
        let result = (|| {
            if self.native_analysis_calls.is_empty() {
                execute(&self.db, "DELETE FROM analyses WHERE kind='native'", []).map_err(fail)?;
            }
            for call in removed
                .iter()
                .flat_map(|c| c.iter())
                .filter(|c| c.kind == "model" && !incoming.contains(c.id.as_str()))
            {
                execute(
                    &self.db,
                    "DELETE FROM analyses WHERE id=?1 AND kind='native'",
                    [format!("native-{}", call.id)],
                )
                .map_err(fail)?;
            }
            let mut loaded = 0;
            for call in added
                .iter()
                .flat_map(|c| c.iter())
                .filter(|c| c.kind == "model")
            {
                loaded += execute(
                    &self.db,
                    "INSERT INTO analyses VALUES(?1,'native',?2,?3,?4,?5,?6,0,?7,0) ON CONFLICT(id) DO UPDATE SET kind=excluded.kind,agent=excluded.agent,model=excluded.model,time=excluded.time,tokens=excluded.tokens,partial=excluded.partial,provisional=excluded.provisional,cost=excluded.cost,attempts=excluded.attempts WHERE analyses.kind IS NOT excluded.kind OR analyses.agent IS NOT excluded.agent OR analyses.model IS NOT excluded.model OR analyses.time IS NOT excluded.time OR analyses.tokens IS NOT excluded.tokens OR analyses.partial IS NOT excluded.partial OR analyses.provisional IS NOT excluded.provisional OR analyses.cost IS NOT excluded.cost OR analyses.attempts IS NOT excluded.attempts",
                    params![
                        format!("native-{}", call.id),
                        crate::agents::group(&call.agent_id, call.agent_product_id.as_deref()),
                        call.model,
                        call.timestamp.as_deref().and_then(timestamp),
                        call.usage
                            .as_ref()
                            .and_then(|u| u.total_tokens)
                            .map(|n| n.to_string()),
                        partial
                            .get(call.session_id.as_str())
                            .copied()
                            .unwrap_or(false),
                        encode(&super::super::AnalysisCost {
                            unknown_attempts: 1,
                            ..Default::default()
                        })?
                    ],
                )
                .map_err(fail)?;
            }
            self.db
                .execute_batch("RELEASE native_analysis_sync")
                .map_err(fail)?;
            Ok(loaded)
        })();
        if result.is_err() {
            let _ = self
                .db
                .execute_batch("ROLLBACK TO native_analysis_sync;RELEASE native_analysis_sync");
        } else {
            self.native_analysis_calls = catalog.calls.clone();
            self.native_analysis_revision = Some(catalog.revision);
        }
        result
    }
    pub fn set_native_analysis(
        &mut self,
        revision: u64,
        rows: &[super::super::AnalysisRow],
    ) -> Result<(), String> {
        self.analysis_cache.borrow_mut().clear();
        execute(&self.db, "DELETE FROM analyses WHERE kind='native'", []).map_err(fail)?;
        for (i, r) in rows.iter().enumerate() {
            execute(
                &self.db,
                "INSERT INTO analyses VALUES(?1,'native',?2,?3,?4,?5,?6,?7,?8,0)",
                params![
                    format!("native-{i}"),
                    r.agent,
                    r.model,
                    r.time.as_deref().and_then(timestamp),
                    r.tokens.map(|n| n.to_string()),
                    r.partial,
                    r.provisional,
                    encode(&r.cost)?
                ],
            )
            .map_err(fail)?;
        }
        self.native_analysis_revision = Some(revision);
        self.native_analysis_calls.clear();
        Ok(())
    }
    pub fn analysis(
        &self,
        q: &super::super::OverviewQuery,
        now: DateTime<Utc>,
    ) -> Result<Value, String> {
        q.validate_with_directory(&self.directory)?;
        if let Some(result) = self.cached_analysis(q, now)? {
            return Ok(result);
        }
        let seconds = match q.range {
            6 | 12 => 1800,
            24 => 3600,
            _ => 21600,
        };
        let n = i64::from(q.range) * 3600 / seconds;
        let end = now.timestamp_millis();
        let start = end - i64::from(q.range) * 3600000;
        let agent = q
            .agent
            .as_ref()
            .map(|a| format!("agent_group_matches({},agent)", quote(a)))
            .unwrap_or("1".into());
        let full = format!("analyses WHERE kind={} AND {agent}", quote(&q.source));
        let from = format!("(SELECT * FROM {full} AND time>={start} AND time<{end})");
        let expr = match q.metric.as_str() {
            "calls" => "1",
            "tokens" => "CAST(tokens AS REAL)",
            _ => {
                "CASE WHEN json_extract(cost,'$.actualUsd') IS NOT NULL OR json_extract(cost,'$.estimatedUsd') IS NOT NULL THEN (coalesce(json_extract(cost,'$.actualUsd'),0)+coalesce(json_extract(cost,'$.estimatedUsd'),0))*1000000 END"
            }
        };
        let partial = match q.metric.as_str() {
            "tokens" => "partial",
            "cost" => "json_extract(cost,'$.unknownAttempts')>0",
            _ => "0",
        };
        let columns = format!(
            "count(*),count({expr}),sum({expr}),coalesce(sum(CASE WHEN {expr} IS NULL OR ({partial}) THEN 1 ELSE 0 END),0)"
        );
        let totals = |r: &rusqlite::Row<'_>, offset: usize| -> rusqlite::Result<Value> {
            let count = sql_count(r, offset)?;
            let known = sql_count(r, offset + 1)?;
            let missing = sql_count(r, offset + 3)?;
            Ok(
                json!({"value":if count==0 {Some(0.0)} else if known>0 {r.get::<_,Option<f64>>(offset+2)?} else {None},"samples":count,"missing":missing,"reported":count-missing}),
            )
        };
        let total = self
            .db
            .query_row(&format!("SELECT {columns} FROM {from}"), [], |r| {
                totals(r, 0)
            })
            .map_err(fail)?;
        // Small nonnegative integer sums are exact in f64, so their dimension
        // and chart summaries can share one compact pass. Money and large token
        // totals retain the original row order to preserve floating additions.
        let raw_from = from.clone();
        let compact = q.source == "native"
            && matches!(q.metric.as_str(), "calls" | "tokens")
            && total["value"]
                .as_f64()
                .is_none_or(|n| n <= 9_007_199_254_740_991.0);
        let compact = if compact {
            self.db.execute_batch("CREATE TEMP TABLE IF NOT EXISTS analysis_cells(agent TEXT,model TEXT,time INTEGER,samples INTEGER,known INTEGER,total REAL,missing INTEGER);DELETE FROM analysis_cells;").map_err(fail)?;
            execute(&self.db,&format!("INSERT INTO analysis_cells SELECT agent,model,min(time),{columns} FROM {from} GROUP BY agent,model,(time-{start})/{} LIMIT 65537",seconds*1000),[]).map_err(fail)?;
            self.db
                .query_row("SELECT count(*)<=65536 FROM analysis_cells", [], |r| {
                    r.get::<_, bool>(0)
                })
                .map_err(fail)?
        } else {
            false
        };
        let from = if compact {
            "analysis_cells".into()
        } else {
            from
        };
        let columns = if compact {
            "coalesce(sum(samples),0),coalesce(sum(known),0),sum(total),coalesce(sum(missing),0)"
                .into()
        } else {
            columns
        };
        let dimension_values = |column: &str| -> Result<Vec<Value>, String> {
            let mut statement = self
                .db
                .prepare(&format!(
                    "SELECT {column},{columns} FROM {from} GROUP BY {column} ORDER BY {column}"
                ))
                .map_err(fail)?;
            statement
                .query_map([], |r| {
                    let mut v = totals(r, 1)?;
                    v["id"] = json!(r.get::<_, String>(0)?);
                    v.as_object_mut().unwrap().remove("reported");
                    Ok(v)
                })
                .map_err(fail)?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(fail)
        };
        let agents = dimension_values("agent")?;
        let models = dimension_values("coalesce(model,'模型未知')")?;
        let group = if q.dimension == "agent" {
            "agent"
        } else {
            "coalesce(model,'模型未知')"
        };
        let keys = if q.dimension == "agent" {
            &agents
        } else {
            &models
        };
        // Aggregate all populated buckets/groups in two passes. Empty cells
        // are filled from the complete dimension list, as in the canonical view.
        let bucket = format!("(time-{start})/{}", seconds * 1000);
        let mut bucket_totals = HashMap::new();
        let mut grouped = HashMap::new();
        {
            let mut statement = self
                .db
                .prepare(&format!("SELECT {bucket},{columns} FROM {from} GROUP BY 1"))
                .map_err(fail)?;
            let rows = statement
                .query_map([], |r| Ok((r.get::<_, i64>(0)?, totals(r, 1)?)))
                .map_err(fail)?;
            for row in rows {
                let (id, v) = row.map_err(fail)?;
                bucket_totals.insert(id, v);
            }
            let mut statement = self
                .db
                .prepare(&format!(
                    "SELECT {bucket},{group},{columns} FROM {from} GROUP BY 1,2"
                ))
                .map_err(fail)?;
            let rows = statement
                .query_map([], |r| {
                    Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, totals(r, 2)?))
                })
                .map_err(fail)?;
            for row in rows {
                let (id, key, v) = row.map_err(fail)?;
                grouped.insert((id, key), v);
            }
        }
        let empty = json!({"value":0.0,"samples":0,"missing":0,"reported":0});
        let mut buckets = vec![];
        for i in 0..n {
            let at = start + i * seconds * 1000;
            let next = at + seconds * 1000;
            let total = bucket_totals.get(&i).unwrap_or(&empty);
            let groups: Vec<_> = keys
                .iter()
                .map(|key| {
                    let id = key["id"].as_str().unwrap();
                    let mut v = grouped
                        .remove(&(i, id.to_owned()))
                        .unwrap_or_else(|| empty.clone());
                    v["id"] = json!(id);
                    v.as_object_mut().unwrap().remove("reported");
                    v
                })
                .collect();
            buckets.push(json!({"start":DateTime::from_timestamp_millis(at).unwrap().to_rfc3339(),"end":DateTime::from_timestamp_millis(next).unwrap().to_rfc3339(),"groups":groups,"value":total["value"],"missing":total["missing"]}));
        }
        let cost=self.db.query_row(&format!("SELECT sum(json_extract(cost,'$.actualUsd')),sum(json_extract(cost,'$.estimatedUsd')),coalesce(sum(json_extract(cost,'$.unknownAttempts')),0),coalesce(sum(provisional),0) FROM {raw_from}"),[],|r|Ok((r.get::<_,Option<f64>>(0)?,r.get::<_,Option<f64>>(1)?,sql_count(r,2)?,sql_count(r,3)?))).map_err(fail)?;
        let missing: usize = self
            .db
            .query_row(
                &format!("SELECT count(*) FROM {full} AND time IS NULL"),
                [],
                |r| sql_count(r, 0),
            )
            .map_err(fail)?;
        let result = json!({"cost":{"actualUsd":cost.0,"estimatedUsd":cost.1,"unknownAttempts":cost.2},"asOf":now.to_rfc3339(),"windowStart":DateTime::from_timestamp_millis(start).unwrap().to_rfc3339(),"query":q,"total":total["value"],"samples":total["samples"],"reported":total["reported"],"missing":total["missing"],"missingTime":missing,"provisional":cost.3,"agents":agents,"models":models,"buckets":buckets,"coverage":"partial","unit":match q.metric.as_str(){"tokens"=>"Token","cost"=>"USD",_=>"次"}});
        self.remember_analysis(q, now, &full, seconds * 1000, &result)?;
        Ok(result)
    }
    pub fn timing_stats(
        &self,
        q: &super::super::OverviewQuery,
        now: DateTime<Utc>,
    ) -> Result<Value, String> {
        if let Some(result) = self.cached_timing(q, now)? {
            return Ok(result);
        }
        let start = (now - Duration::hours(q.range.into())).timestamp_millis();
        let end = now.timestamp_millis();
        let a = q
            .agent
            .as_ref()
            .map(|s| format!("agent_matches({},a.family,a.product)", quote(s)))
            .unwrap_or("1".into());
        let from = format!(
            "(SELECT a.raw value FROM attempts a JOIN analyses g ON g.id=a.logical AND g.kind='gateway' WHERE g.attempts=1 AND {a} AND json_extract(a.raw,'$.status')='success' AND g.time>={start} AND g.time<{end})"
        );
        let success: usize = self
            .db
            .query_row(&format!("SELECT count(*) FROM {from}"), [], |r| {
                sql_count(r, 0)
            })
            .map_err(fail)?;
        let (n, p50) = aggregates::percentile(
            &self.db,
            &from,
            "CASE WHEN json_extract(value,'$.ttftMs')>=0 THEN json_extract(value,'$.ttftMs') END",
            50,
        )?;
        let (_, p95) = aggregates::percentile(
            &self.db,
            &from,
            "CASE WHEN json_extract(value,'$.ttftMs')>=0 THEN json_extract(value,'$.ttftMs') END",
            95,
        )?;
        let (s, speed) = aggregates::percentile(
            &self.db,
            &from,
            "CASE WHEN json_extract(value,'$.durationMs')>0 AND json_extract(value,'$.usage.outputTokens') IS NOT NULL THEN json_extract(value,'$.usage.outputTokens')*1000.0/json_extract(value,'$.durationMs') END",
            50,
        )?;
        let group_filter = q
            .agent
            .as_ref()
            .map(|s| format!("agent_group_matches({},agent)", quote(s)))
            .unwrap_or("1".into());
        let retries:usize=self.db.query_row(&format!("SELECT count(*) FROM analyses WHERE kind='gateway' AND attempts>1 AND {group_filter}"),[],|r|sql_count(r,0)).map_err(fail)?;
        let result = json!({"success":success,"ttftSamples":n,"ttftP50Ms":p50,"ttftP95Ms":p95,"throughputSamples":s,"throughputP50":speed,"retryGroupsUnmeasurable":retries});
        self.remember_timing(q, now, &result)?;
        Ok(result)
    }
}
