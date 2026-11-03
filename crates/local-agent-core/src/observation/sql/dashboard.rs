use super::*;
use aggregates::{quote, summary};
use chrono::{DateTime, Duration, Utc};

#[derive(Clone)]
pub(super) struct Frozen {
    pub query: requests::Query,
    pub created: DateTime<Utc>,
    pub dashboard: Value,
    pub positions: String,
    pub key: String,
    pub epoch: String,
    pub expires: DateTime<Utc>,
}

pub(super) fn insert_fragments(
    db: &Connection,
    v: i64,
    row: &requests::Request,
) -> Result<(), String> {
    // A native response has no gateway attempt parts. Derive its dimensions
    // from the already indexed record instead of storing five duplicate rows.
    if row.source == "native" {
        return Ok(());
    }
    for dimension in ["agent", "model", "provider"] {
        let key = match dimension {
            "model" => row.model.clone().unwrap_or("模型未知".into()),
            "provider" => row.provider_key.clone(),
            _ => crate::agents::group(&row.agent_id, row.agent_product_id.as_deref()),
        };
        let label = if dimension == "provider" {
            row.provider.clone()
        } else {
            key.clone()
        };
        execute(
            db,
            "INSERT INTO fragments VALUES(?1,?2,?3,?4,NULL)",
            params![v, dimension, key, label],
        )
        .map_err(fail)?;
        if dimension == "agent" {
            continue;
        }
        if row.attempts.len() <= 1 {
            execute(
                db,
                "INSERT INTO fragments VALUES(?1,?2,?3,?4,NULL)",
                params![v, format!("{dimension}Parts"), key, label],
            )
            .map_err(fail)?;
            continue;
        }
        let mut groups = std::collections::BTreeMap::<String, Vec<&requests::Attempt>>::new();
        for a in &row.attempts {
            let key = if dimension == "provider" {
                a.provider_key.clone()
            } else {
                a.observation
                    .routed_model
                    .clone()
                    .or(a.observation.model.clone())
                    .unwrap_or("模型未知".into())
            };
            groups.entry(key).or_default().push(a);
        }
        let single = groups.len() == 1;
        for (key, attempts) in groups {
            let label = if dimension == "provider" {
                attempts[0].observation.provider.clone()
            } else {
                key.clone()
            };
            if single {
                execute(
                    db,
                    "INSERT INTO fragments VALUES(?1,?2,?3,?4,NULL)",
                    params![v, format!("{dimension}Parts"), key, label],
                )
                .map_err(fail)?;
                continue;
            }
            let fragment = requests::with_attempts(row, attempts.into_iter().cloned().collect());
            execute(
                db,
                "INSERT INTO fragments VALUES(?1,?2,?3,?4,?5)",
                params![
                    v,
                    format!("{dimension}Parts"),
                    key,
                    label,
                    encode(&requests::Summary::of([&fragment]))?
                ],
            )
            .map_err(fail)?;
        }
    }
    Ok(())
}

pub(super) fn filter(q: &requests::Query, now: DateTime<Utc>, dates: bool) -> String {
    let mut parts = vec![format!(
        "(r.time IS NULL OR (r.time<={0} AND (r.time<>{0} OR instant_cmp(r.started,{1})<0)))",
        now.timestamp_millis(),
        quote(&now.to_rfc3339())
    )];
    if let Some(agent) = &q.agent {
        parts.push(format!(
            "agent_matches({},r.family,r.product)",
            quote(agent)
        ));
    }
    if dates {
        if let Some(t) = q.start.as_deref().and_then(timestamp) {
            parts.push(format!(
                "r.time>={t} AND (r.time<>{t} OR instant_cmp(r.started,{})>=0)",
                quote(q.start.as_deref().unwrap())
            ));
        }
        if let Some(t) = q.end.as_deref().and_then(timestamp) {
            parts.push(format!(
                "r.time<={t} AND (r.time<>{t} OR instant_cmp(r.started,{})<0)",
                quote(q.end.as_deref().unwrap())
            ));
        }
    }
    for (name, field) in [(&q.result, "r.status"), (&q.project, "r.project")] {
        if let Some(v) = name {
            parts.push(format!("{field}={}", quote(v)));
        }
    }
    for (input, column, path) in [
        (&q.provider, "provider", "$.providerKey"),
        (&q.model, "model", "$.routedModel"),
        (&q.session, "session", "$.sessionId"),
    ] {
        if let Some(v) = input {
            let leaf = if column == "model" {
                "coalesce(json_extract(a.value,'$.routedModel'),json_extract(a.value,'$.model'))"
                    .into()
            } else {
                format!("json_extract(a.value,'{path}')")
            };
            parts.push(format!("(r.{column}={} OR (json_extract(r.stats,'$.attempts')>0 AND EXISTS(SELECT 1 FROM json_each(r.value,'$.attempts') a WHERE {leaf}={})))",quote(v),quote(v)));
        }
    }
    if let Some(v) = &q.search {
        parts.push(format!("instr(r.search,{})>0", quote(&v.to_lowercase())));
    }
    if let Some(v) = &q.issue {
        if v == "unmeasurable" {
            parts.push("json_extract(r.value,'$.ttftMs') IS NULL AND r.status NOT IN ('waiting','streaming')".into());
        } else {
            parts.push(format!("(EXISTS(SELECT 1 FROM json_each(r.value,'$.attempts') a WHERE json_extract(a.value,'$.issue')={}) OR (r.kind='native' AND CASE json_extract(r.value,'$.native.failure.category') WHEN 'upstream' THEN 'upstream_error' ELSE json_extract(r.value,'$.native.failure.category') END={}))",quote(v),quote(v)));
        }
    }
    parts.join(" AND ")
}
fn base(kind: &str, predicate: &str) -> String {
    format!(
        "(SELECT r.* FROM records r JOIN current c ON c.v=r.v WHERE r.kind={} AND ({predicate}))",
        quote(kind)
    )
}

pub(super) fn native_provider_options(db: &Connection, from: &str) -> Result<Vec<Value>, String> {
    // With a single MAX and unique native IDs, SQLite carries v from the winning
    // row. Join by this primary key so provider count cannot multiply a full scan.
    aggregates::list(
        db,
        &format!(
            "SELECT json_object('id',r.provider,'label',json_extract(r.value,'$.provider')) FROM {from} r JOIN (SELECT v,max(id) FROM {from} GROUP BY provider) latest ON latest.v=r.v ORDER BY r.provider"
        ),
    )
}

pub(super) fn native_value(v: &Value) -> Result<requests::Request, String> {
    let text = |k: &str| v[k].as_str().unwrap_or("").to_owned();
    let optional = |k: &str| v[k].as_str().map(str::to_owned);
    Ok(requests::Request {
        client: None,
        source: "native".into(),
        native: Some(serde_json::from_value(v["native"].clone()).map_err(fail)?),
        native_links: vec![],
        id: text("id"),
        agent_id: text("agentId"),
        agent_product_id: optional("agentProductId"),
        session_id: optional("sessionId"),
        session_title: optional("sessionTitle"),
        project_id: optional("projectId"),
        project_name: optional("projectName"),
        started_at: text("startedAt"),
        ended_at: None,
        status: text("status"),
        model: optional("model"),
        provider: text("provider"),
        provider_key: text("providerKey"),
        usage: serde_json::from_value(v["usage"].clone()).map_err(fail)?,
        usage_missing: v["usageMissing"].as_u64().unwrap_or(0) as usize,
        cost: serde_json::from_value(v["cost"].clone()).map_err(fail)?,
        duration_ms: None,
        ttft_ms: None,
        throughput: None,
        timing_note: optional("timingNote"),
        attempts: vec![],
    })
}

impl History {
    pub fn native_matches(&self, key: &str) -> bool {
        self.native_revision.as_deref() == Some(key)
    }
    #[cfg(test)]
    pub fn frozen_csv(&self, id: &str, now: DateTime<Utc>) -> Result<Value, String> {
        let frozen = self
            .snapshots
            .get(id)
            .filter(|s| (now - s.created).num_minutes() < 15)
            .ok_or("请求快照已过期，请刷新后重试")?;
        let mut text = requests::csv::header();
        let mut after = -1i64;
        loop {
            let rows: Vec<(i64, String)> = {
                let mut s=self.db.prepare("SELECT p.pos,r.value FROM positions p JOIN record_payloads r ON r.v=p.v WHERE p.snapshot=?1 AND p.pos>?2 ORDER BY p.pos LIMIT 256").map_err(fail)?;
                s.query_map(params![&frozen.positions, after], |r| {
                    Ok((r.get(0)?, r.get(1)?))
                })
                .map_err(fail)?
                .collect::<Result<_, _>>()
                .map_err(fail)?
            };
            if rows.is_empty() {
                break;
            }
            after = rows.last().unwrap().0;
            for (_, raw) in rows {
                let row = serde_json::from_str(&raw).map_err(fail)?;
                requests::csv::write(&mut text, &row);
            }
        }
        Ok(
            json!({"csv":text,"count":frozen.dashboard["total"],"asOf":frozen.dashboard["asOf"],"snapshot":id}),
        )
    }
    /// Catalog metadata is retained only in this private query connection.
    pub fn sync_native(
        &mut self,
        revision: String,
        contexts: &HashMap<String, requests::Context>,
        native: impl IntoIterator<Item = requests::Request>,
    ) -> Result<(), String> {
        self.sync_native_rows(revision, contexts, native, None)?;
        self.native_request_calls.clear();
        Ok(())
    }
    pub fn sync_native_catalog(
        &mut self,
        revision: String,
        contexts: &HashMap<String, requests::Context>,
        calls: &crate::sessions::CallCatalog,
    ) -> Result<(), String> {
        self.use_directory(calls.directory())?;
        if self.native_matches(&revision) {
            return self.refresh_native();
        }
        let (removed, added) = calls.delta(&self.native_request_calls);
        let removed = crate::sessions::CallCatalog::shared(removed.into_iter().cloned());
        let added = crate::sessions::CallCatalog::shared(added.into_iter().cloned());
        let full = self.native_request_calls.is_empty();
        self.sync_native_rows(
            revision,
            contexts,
            requests::sources::native_records_iter(added.iter(), contexts),
            if full { None } else { Some((&removed, &added)) },
        )?;
        self.native_request_calls = calls.clone();
        Ok(())
    }
    fn sync_native_rows(
        &mut self,
        revision: String,
        contexts: &HashMap<String, requests::Context>,
        native: impl IntoIterator<Item = requests::Request>,
        delta: Option<(&crate::sessions::CallCatalog, &crate::sessions::CallCatalog)>,
    ) -> Result<(), String> {
        let removed = delta.map(|(old, _)| old);
        let retired: HashSet<_> = removed
            .into_iter()
            .flat_map(|c| c.iter())
            .filter(|c| c.kind == "model")
            .map(|c| c.id.as_str())
            .collect();
        if self.native_matches(&revision) {
            return self.refresh_native();
        }
        let mut native = native.into_iter().peekable();
        self.db
            .execute_batch("SAVEPOINT native_catalog;DELETE FROM contexts;")
            .map_err(fail)?;
        let result = (|| {
            // Keep primary keys and current/version ownership active. Secondary
            // indexes can be built once after the first native bulk load; all
            // DDL is inside the catalog savepoint, including failure rollback.
            // An empty or tool-only catalog can precede the first model rows;
            // its revision does not mean the initial projection was populated.
            let deferred_indexes: Vec<(String, String)> = if native.peek().is_some()
                && ((self.native_revision.is_none() && removed.is_none())
                    || self
                        .db
                        .query_row("SELECT NOT EXISTS(SELECT 1 FROM record_meta)", [], |r| {
                            r.get::<_, bool>(0)
                        })
                        .map_err(fail)?)
            {
                let mut s = self.db.prepare("SELECT name,replace(sql,'CREATE INDEX ','CREATE INDEX temp.') FROM sqlite_temp_master WHERE type='index' AND tbl_name IN ('record_meta','native_base') AND sql IS NOT NULL").map_err(fail)?;
                s.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
                    .map_err(fail)?
                    .collect::<Result<_, _>>()
                    .map_err(fail)?
            } else {
                vec![]
            };
            for (name, _) in &deferred_indexes {
                self.db
                    .execute_batch(&format!(
                        "DROP INDEX temp.\"{}\"",
                        name.replace('"', "\"\"")
                    ))
                    .map_err(fail)?;
            }
            if let Some((removed, added)) = delta {
                let incoming: HashSet<_> = added
                    .iter()
                    .filter(|c| c.kind == "model")
                    .map(|c| c.id.as_str())
                    .collect();
                for call in removed
                    .iter()
                    .filter(|c| c.kind == "model" && !incoming.contains(c.id.as_str()))
                {
                    self.retire_native(&call.id)?;
                }
            } else {
                self.db
                    .execute_batch(
                        "DELETE FROM native_base;DELETE FROM current WHERE kind='native';",
                    )
                    .map_err(fail)?;
            }
            for (id, c) in contexts {
                execute(
                    &self.db,
                    "INSERT INTO contexts VALUES(?1,?2)",
                    params![id, encode(c)?],
                )
                .map_err(fail)?;
            }
            // Full replacement retires all links. A file delta only changes its
            // old/new response keys; retain unrelated immutable linked rows.
            let mut after = String::new();
            loop {
                let rows: Vec<(String, String)> = {
                    let mut s=self.db.prepare("SELECT r.id,r.value FROM current c JOIN records r ON r.v=c.v WHERE ?2 AND c.kind='gateway' AND r.id>?1 AND r.linked=1 ORDER BY r.id LIMIT 256").map_err(fail)?;
                    s.query_map(params![&after, removed.is_none()], |r| {
                        Ok((r.get(0)?, r.get(1)?))
                    })
                    .map_err(fail)?
                    .collect::<Result<_, _>>()
                    .map_err(fail)?
                };
                if rows.is_empty() {
                    break;
                }
                after = rows.last().unwrap().0.clone();
                for (id, raw) in rows {
                    let mut v: Value = serde_json::from_str(&raw).map_err(fail)?;
                    v["nativeLinks"] = json!([]);
                    self.replace_value("gateway", &id, &v)?;
                }
            }
            execute(&self.db,"INSERT OR IGNORE INTO dirty_context SELECT c.id FROM current c JOIN records r ON r.v=c.v WHERE c.kind='gateway' AND (r.session IS NOT NULL OR r.project IS NOT NULL)",[]).map_err(fail)?;
            for row in native {
                let row = self.request_projection.project(row)?;
                if delta.is_some() {
                    let original: Option<String> = self.db.query_row("SELECT p.value FROM native_base b JOIN record_payloads p ON p.v=b.v WHERE b.id=?1",[&row.id],|r|r.get(0)).optional().map_err(fail)?;
                    if original.is_some() && !retired.contains(row.id.as_str()) {
                        return Err(fail("duplicate native request ID"));
                    }
                    if original.as_deref() == Some(encode(&row)?.as_str()) {
                        continue;
                    }
                    if original.is_some() {
                        self.retire_native(&row.id)?;
                    }
                }
                let version = self.insert_request(&row)?;
                execute(
                    &self.db,
                    "INSERT INTO native_base VALUES(?1,?2,?3,?4,?5,?6,?7)",
                    params![
                        row.id,
                        row.agent_id,
                        row.native.as_ref().and_then(|n| n.response_id.as_ref()),
                        version,
                        row.model,
                        row.provider_key,
                        row.provider
                    ],
                )
                .map_err(fail)?;
                if removed.is_some() {
                    execute(&self.db,"INSERT OR IGNORE INTO dirty_links SELECT n.family,n.response FROM native_base n WHERE n.id=?1 AND n.response IS NOT NULL AND EXISTS(SELECT 1 FROM attempts a WHERE a.family=n.family AND a.response=n.response)",[&row.id]).map_err(fail)?;
                }
            }
            for (_, sql) in deferred_indexes {
                self.db.execute_batch(&sql).map_err(fail)?;
            }
            if removed.is_none()
                && self
                    .db
                    .query_row("SELECT EXISTS(SELECT 1 FROM attempts)", [], |r| {
                        r.get::<_, bool>(0)
                    })
                    .map_err(fail)?
            {
                execute(&self.db,"INSERT OR IGNORE INTO dirty_links SELECT a.family,a.response FROM attempts a WHERE a.response IS NOT NULL AND EXISTS(SELECT 1 FROM native_base n WHERE n.family=a.family AND n.response=a.response)",[]).map_err(fail)?;
            }
            self.refresh_native()?;
            self.db
                .execute_batch("RELEASE native_catalog")
                .map_err(fail)?;
            Ok(())
        })();
        if result.is_err() {
            let _ = self
                .db
                .execute_batch("ROLLBACK TO native_catalog;RELEASE native_catalog;");
        } else {
            self.native_revision = Some(revision);
        }
        result
    }
    fn retire_native(&self, id: &str) -> Result<(), String> {
        let gateway: Option<String> = self.db.query_row("SELECT json_extract(r.value,'$.native.gatewayRequestId') FROM current c JOIN records r ON r.v=c.v WHERE c.kind='native' AND c.id=?1",[id],|r|r.get(0)).optional().map_err(fail)?.flatten();
        if let Some(gateway) = gateway {
            self.edit_link(&gateway, id, None)?;
        }
        execute(&self.db,"INSERT OR IGNORE INTO dirty_links SELECT family,response FROM native_base WHERE id=?1 AND response IS NOT NULL",[id]).map_err(fail)?;
        execute(&self.db, "DELETE FROM native_base WHERE id=?1", [id]).map_err(fail)?;
        execute(
            &self.db,
            "DELETE FROM current WHERE kind='native' AND id=?1",
            [id],
        )
        .map_err(fail)?;
        Ok(())
    }
    pub fn refresh_native(&self) -> Result<(), String> {
        self.db
            .execute_batch("SAVEPOINT native_delta")
            .map_err(fail)?;
        let result = (|| {
            loop {
                let ids: Vec<String> = {
                    let mut s = self
                        .db
                        .prepare("SELECT id FROM dirty_context LIMIT 128")
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
                    let raw:Option<String>=self.db.query_row("SELECT r.value FROM current c JOIN records r ON r.v=c.v WHERE c.kind='gateway' AND c.id=?1",[&id],|r|r.get(0)).optional().map_err(fail)?;
                    if let Some(raw) = raw {
                        let mut v: Value = serde_json::from_str(&raw).map_err(fail)?;
                        let original = v.clone();
                        // The raw attempt retains the native Session ID even after context resolution.
                        let session:Option<String>=self.db.query_row("SELECT session FROM attempts WHERE logical=?1 AND session IS NOT NULL ORDER BY id LIMIT 1",[&id],|r|r.get(0)).optional().map_err(fail)?;
                        let session = session.as_deref().unwrap_or("");
                        let family = v["agentId"].as_str().unwrap_or("");
                        let c:Option<String>=self.db.query_row("SELECT value FROM contexts WHERE id=?1 OR id=?2 ORDER BY id=?1 DESC LIMIT 1",params![format!("{family}:{session}"),session],|r|r.get(0)).optional().map_err(fail)?;
                        let c: Option<requests::Context> = c
                            .map(|v| serde_json::from_str(&v).map_err(fail))
                            .transpose()?;
                        v["sessionId"] = json!(
                            c.as_ref()
                                .map(|c| c.session_id.as_str())
                                .or(Some(session).filter(|s| !s.is_empty()))
                        );
                        v["sessionTitle"] = json!(c.as_ref().map(|c| &c.title));
                        v["projectId"] = json!(c.as_ref().and_then(|c| c.project_id.as_ref()));
                        v["projectName"] = json!(c.as_ref().and_then(|c| c.project_name.as_ref()));
                        if v != original {
                            self.replace_value("gateway", &id, &v)?;
                        }
                    }
                    execute(&self.db, "DELETE FROM dirty_context WHERE id=?1", [id])
                        .map_err(fail)?;
                }
            }
            loop {
                let keys: Vec<(String, String)> = {
                    let mut s = self
                        .db
                        .prepare("SELECT family,response FROM dirty_links LIMIT 128")
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
                            "SELECT count(*) FROM native_base WHERE family=?1 AND response=?2",
                            params![family, response],
                            |r| sql_count(r, 0),
                        )
                        .map_err(fail)?;
                    let mut after = String::new();
                    loop {
                        let rows: Vec<(String, String)> = {
                            let mut s=self.db.prepare("SELECT b.id,r.value FROM native_base b JOIN records r ON r.v=b.v WHERE b.family=?1 AND b.response=?2 AND b.id>?3 ORDER BY b.id LIMIT 256").map_err(fail)?;
                            s.query_map(params![family, response, after], |r| {
                                Ok((r.get(0)?, r.get(1)?))
                            })
                            .map_err(fail)?
                            .collect::<Result<_, _>>()
                            .map_err(fail)?
                        };
                        if rows.is_empty() {
                            break;
                        }
                        after = rows.last().unwrap().0.clone();
                        for (id, raw) in rows {
                            // Remove this event's previous link, including transitions to ambiguity/deletion.
                            let old:Option<String>=self.db.query_row("SELECT json_extract(r.value,'$.native.gatewayRequestId') FROM current c JOIN records r ON r.v=c.v WHERE c.kind='native' AND c.id=?1",[&id],|r|r.get(0)).optional().map_err(fail)?.flatten();
                            if let Some(old) = old {
                                self.edit_link(&old, &id, None)?;
                            }
                            let mut n = native_value(&serde_json::from_str(&raw).map_err(fail)?)?;
                            if net > 0 && (net != 1 || local != 1) {
                                n.native.as_mut().unwrap().association = "ambiguous".into();
                            } else if net == 1 {
                                let records: Vec<ModelObservation> = {
                                    let mut s=self.db.prepare("SELECT raw FROM attempts WHERE logical=(SELECT logical FROM attempts WHERE family=?1 AND response=?2 LIMIT 1) ORDER BY id").map_err(fail)?;
                                    s.query_map(params![family, response], |r| {
                                        r.get::<_, String>(0)
                                    })
                                    .map_err(fail)?
                                    .map(|v| serde_json::from_str(&v.map_err(fail)?).map_err(fail))
                                    .collect::<Result<_, _>>()?
                                };
                                let mut gateway =
                                    requests::logical_requests(records, &HashMap::new());
                                // Preserve the same context identity used by the request dashboard.
                                for g in &mut gateway {
                                    if let Some(v) = self.request_detail(&g.id)? {
                                        g.session_id = v["sessionId"].as_str().map(str::to_owned);
                                    }
                                }
                                requests::sources::correlate(
                                    &mut gateway,
                                    std::slice::from_mut(&mut n),
                                );
                                for g in gateway {
                                    for link in g.native_links {
                                        self.edit_link(&g.id, &id, Some(value(&link)?))?;
                                    }
                                }
                            }
                            self.insert_request(&n)?;
                        }
                    }
                    execute(
                        &self.db,
                        "DELETE FROM dirty_links WHERE family=?1 AND response=?2",
                        params![family, response],
                    )
                    .map_err(fail)?;
                }
            }
            self.db.execute_batch("RELEASE native_delta").map_err(fail)
        })();
        if result.is_err() {
            let _ = self
                .db
                .execute_batch("ROLLBACK TO native_delta;RELEASE native_delta;");
        }
        result
    }
    fn edit_link(&self, id: &str, event: &str, link: Option<Value>) -> Result<(), String> {
        let raw:Option<String>=self.db.query_row("SELECT r.value FROM current c JOIN records r ON r.v=c.v WHERE c.kind='gateway' AND c.id=?1",[id],|r|r.get(0)).optional().map_err(fail)?;
        if let Some(raw) = raw {
            let mut v: Value = serde_json::from_str(&raw).map_err(fail)?;
            let original = v.clone();
            let links = v["nativeLinks"].as_array_mut().ok_or("关联索引无效")?;
            links.retain(|v| v["eventId"] != event);
            if let Some(link) = link {
                links.push(link);
            }
            if v != original {
                self.replace_value("gateway", id, &v)?;
            }
        }
        Ok(())
    }
    fn replace_value(&self, kind: &str, id: &str, row: &Value) -> Result<(), String> {
        let old: i64 = self
            .db
            .query_row(
                "SELECT v FROM current WHERE kind=?1 AND id=?2",
                params![kind, id],
                |r| r.get(0),
            )
            .map_err(fail)?;
        let search = format!(
            "{} {} {} {} {}",
            row["id"].as_str().unwrap_or(""),
            row["model"].as_str().unwrap_or(""),
            row["provider"].as_str().unwrap_or(""),
            row["sessionTitle"].as_str().unwrap_or(""),
            row["attempts"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|a| a["id"].as_str())
                .collect::<Vec<_>>()
                .join(" ")
        )
        .to_lowercase();
        let raw = encode(row)?;
        execute(&self.db,"INSERT INTO record_meta(kind,id,stats,time,agent,family,product,status,model,provider,session,project,search,linked,cache_in,cache_read,cost_value,ttft_sample,duration_sample,throughput_sample,started,instant,association,usage_conflict,domain_usage,domain_link_count,domain_link_conflicts,domain_duration,domain_ttft) SELECT kind,id,stats,time,agent,family,product,status,model,provider,?3,?4,?5,?6,cache_in,cache_read,cost_value,ttft_sample,duration_sample,throughput_sample,started,instant,json_extract(?2,'$.native.association'),json_extract(?2,'$.native.usageConflict'),domain_usage,json_array_length(?2,'$.nativeLinks'),(SELECT count(*) FROM json_each(?2,'$.nativeLinks') WHERE json_extract(value,'$.usageConflict')=1),domain_duration,domain_ttft FROM record_meta WHERE v=?1",params![old,raw,row["sessionId"].as_str(),row["projectId"].as_str(),search,row["nativeLinks"].as_array().is_some_and(|a|!a.is_empty())]).map_err(fail)?;
        let new = self.db.last_insert_rowid();
        execute(
            &self.db,
            "INSERT INTO record_payloads VALUES(?1,?2)",
            params![new, raw],
        )
        .map_err(fail)?;
        execute(
            &self.db,
            "INSERT INTO fragments SELECT ?2,dimension,key,label,stats FROM fragments WHERE v=?1",
            params![old, new],
        )
        .map_err(fail)?;
        execute(
            &self.db,
            "UPDATE current SET v=?3 WHERE kind=?1 AND id=?2",
            params![kind, id, new],
        )
        .map_err(fail)?;
        Ok(())
    }
    pub fn request_detail(&self, id: &str) -> Result<Option<Value>, String> {
        let raw:Option<String>=self.db.query_row("SELECT r.value FROM current c JOIN records r ON r.v=c.v WHERE c.id=?1 OR (c.kind='gateway' AND c.id=(SELECT logical FROM attempts WHERE id=?1)) ORDER BY c.kind LIMIT 1",[id],|r|r.get(0)).optional().map_err(fail)?;
        raw.map(|v| serde_json::from_str(&v).map_err(fail))
            .transpose()
    }
    pub fn request_page(
        &mut self,
        q: &requests::Query,
        now: DateTime<Utc>,
        report: Value,
        error: Option<String>,
    ) -> Result<Value, String> {
        q.validate_with_directory(&self.directory)?;
        if q.snapshot.is_none() && q.cursor.is_some() {
            return Err("分页需要请求快照".into());
        }
        self.expire(now, q.snapshot.is_none())?;
        let frozen = if let Some(id) = &q.snapshot {
            self.snapshots
                .get(id)
                .cloned()
                .ok_or("请求快照已过期，请刷新后重试")?
        } else if let Some(frozen) = self.cached_dashboard(q, now, &report, &error)? {
            frozen
        } else {
            if q.cursor.is_some() {
                return Err("分页需要请求快照".into());
            }
            let id = uuid::Uuid::new_v4().to_string();
            let kind = if q.source.as_deref() == Some("native") {
                "native"
            } else {
                "gateway"
            };
            let predicate = filter(q, now, true);
            let linked = if q.source.as_deref() == Some("linked") {
                " AND r.linked=1"
            } else {
                ""
            };
            let order = match q.sort.as_deref().unwrap_or("time") {
                "ttft" => "json_extract(r.value,'$.ttftMs')",
                "duration" => "json_extract(r.value,'$.durationMs')",
                "tokens" => "CAST(json_extract(r.stats,'$.usage.totalTokens') AS REAL)",
                "cost" if q.currency.as_deref().unwrap_or("USD") == "USD" => "r.cost_value",
                "cost" => "NULL",
                _ => "r.time",
            };
            let descending = if q.descending.unwrap_or(true) {
                "DESC"
            } else {
                "ASC"
            };
            let source = format!(
                "records r JOIN current c ON c.v=r.v WHERE r.kind={} AND {predicate}{linked}",
                quote(kind)
            );
            let position_id: i64 = self
                .db
                .query_row(
                    "UPDATE position_clock SET seq=seq+1 RETURNING seq",
                    [],
                    |r| r.get(0),
                )
                .map_err(fail)?;
            // Public tokens remain UUIDs; private integer owners keep the
            // million-row membership index compact and never reuse empty owners.
            // Snapshot positions reference immutable record versions, not copies of
            // whole JSON histories. Cursor seeks use the compound primary key.
            execute(&self.db,&format!("INSERT INTO positions SELECT ?1,row_number() OVER(ORDER BY ({order}) IS NULL,({order}) {descending},r.id)-1,r.v FROM {source}"),[position_id]).map_err(fail)?;
            // All aggregates are computed inside the same serialized query as
            // position allocation. Reuse its current scope directly; probing
            // million-row positions again for every chart adds no isolation.
            // Public pagination still references the immutable versions above.
            let selected = if kind == "native" {
                self.native_request_cells(q, now)?
                    .unwrap_or_else(|| format!("(SELECT r.* FROM {source})"))
            } else {
                format!("(SELECT r.* FROM {source})")
            };
            let report_key = match q.source.as_deref() {
                Some("native") => "native",
                Some("linked") => "linked",
                _ => "gateway",
            };
            let s = if report[report_key].is_object() {
                serde_json::from_value::<requests::Summary>(report[report_key].clone())
                    .map_err(fail)?
            } else {
                summary(&self.db, &selected)?
            };
            let rawscope = base(kind, if linked.is_empty() { "1" } else { "r.linked=1" });
            let (start, end): (Option<i64>, Option<i64>) = self
                .db
                .query_row(
                    &format!("SELECT (SELECT min(time) FROM {rawscope}),(SELECT max(time) FROM {rawscope})"),
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .map_err(fail)?;
            let missing: usize = self
                .db
                .query_row(
                    &format!(
                        "SELECT count(*) FROM {rawscope} r WHERE r.time IS NULL AND {}",
                        filter(q, now, false)
                    ),
                    [],
                    |r| sql_count(r, 0),
                )
                .map_err(fail)?;
            let options = format!(
                "(SELECT r.id,0 pos,r.model,r.provider,json_extract(r.value,'$.provider') label FROM {rawscope} r WHERE r.kind='native' UNION ALL SELECT r.id,CAST(a.key AS INTEGER),coalesce(json_extract(a.value,'$.routedModel'),json_extract(a.value,'$.model')),json_extract(a.value,'$.providerKey'),json_extract(a.value,'$.provider') FROM {rawscope} r,json_each(r.value,'$.attempts') a)"
            );
            let models = aggregates::list(
                &self.db,
                &if kind == "native" {
                    "SELECT json_quote(model) FROM native_base WHERE model IS NOT NULL GROUP BY model ORDER BY model".into()
                } else {
                    format!(
                        "SELECT json_quote(model) FROM {options} WHERE model IS NOT NULL GROUP BY model ORDER BY model"
                    )
                },
            )?;
            let providers = if kind == "native" {
                native_provider_options(&self.db, "native_options")?
            } else {
                aggregates::list(
                    &self.db,
                    &format!(
                        "SELECT json_object('id',provider,'label',label) FROM (SELECT provider,label,row_number() OVER(PARTITION BY provider ORDER BY id DESC,pos DESC) rank FROM {options}) WHERE rank=1 ORDER BY provider"
                    ),
                )?
            };
            let coverage = match q.source.as_deref().unwrap_or("gateway") {
                "native" => {
                    "仅已授权来源的逐条模型响应记录；按记录时间筛选，非 HTTP 请求数；缺少时间的记录仅在全部范围纳入"
                }
                "linked" => {
                    "仅明确响应 ID 关联的网关逻辑请求；每次消费计一份，采用网关用量与计时，未关联原生不纳入"
                }
                _ => "已记录网关逻辑请求；原生记录和本地拒绝未相加，跨来源关联见下方",
            };
            let iso = |v: Option<i64>, descending: bool| -> Result<Option<String>, String> {
                if let Some(v) = v {
                    let direction = if descending { "DESC" } else { "ASC" };
                    let raw:String=self.db.query_row(&format!("SELECT started FROM {rawscope} WHERE time={v} ORDER BY instant {direction} LIMIT 1"),[],|r|r.get(0)).map_err(fail)?;
                    Ok(DateTime::parse_from_rfc3339(&raw)
                        .ok()
                        .map(|t| t.with_timezone(&Utc).to_rfc3339()))
                } else {
                    Ok(None)
                }
            };
            let mut dashboard = json!({"query":q,"asOf":now.to_rfc3339(),"snapshot":id,"summary":s,"buckets":self.buckets(&selected,q,now)?,"groups":self.groups(&selected,q)?,"items":[],"total":s.requests,"nextCursor":null,"models":models,"providers":providers,"sourceError":error,"coverage":coverage,"recordedStart":iso(start,false)?,"recordedEnd":iso(end,true)?,"missingTime":missing,"sourceReport":report,"heatmap":self.heatmap(&selected,q,now)?});
            dashboard["query"]["snapshot"] = Value::Null;
            dashboard["query"]
                .as_object_mut()
                .unwrap()
                .remove("snapshot");
            let frozen = Frozen {
                query: q.clone(),
                created: now,
                positions: position_id.to_string(),
                key: request_cache::query_key(q)?,
                epoch: self.request_epoch()?,
                expires: self.dashboard_expiry(q, &dashboard, now)?,
                dashboard,
            };
            self.remember_dashboard(&frozen)?;
            self.snapshots.insert(id, frozen.clone());
            frozen
        };
        if !frozen.query.same_filter(q) {
            return Err("快照筛选已变化，请刷新请求".into());
        }
        let id = frozen.dashboard["snapshot"]
            .as_str()
            .ok_or("快照无效")?
            .to_owned();
        let offset = if let Some(c) = &q.cursor {
            c.strip_prefix(&format!("{id}:"))
                .ok_or("分页游标不属于当前快照")?
                .parse::<usize>()
                .map_err(|_| "分页游标无效")?
        } else {
            0
        };
        let total = frozen.dashboard["total"].as_u64().unwrap_or(0) as usize;
        if offset > total {
            return Err("分页游标超出范围".into());
        }
        let mut s=self.db.prepare("SELECT r.value FROM positions p JOIN record_payloads r ON r.v=p.v WHERE p.snapshot=?1 AND p.pos>=?2 ORDER BY p.pos LIMIT 40").map_err(fail)?;
        let items = s
            .query_map(
                params![&frozen.positions, i64::try_from(offset).map_err(fail)?],
                |r| r.get::<_, String>(0),
            )
            .map_err(fail)?
            .map(|r| serde_json::from_str::<Value>(&r.map_err(fail)?).map_err(fail))
            .collect::<Result<Vec<_>, _>>()?;
        let mut page = frozen.dashboard;
        page["items"] = json!(items);
        page["nextCursor"] = json!(
            offset
                .checked_add(40)
                .filter(|next| *next < total)
                .map(|next| format!("{id}:{next}"))
        );
        Ok(page)
    }
    pub fn frozen_report(&self, id: &str) -> Result<Value, String> {
        self.snapshots
            .get(id)
            .map(|s| s.dashboard["sourceReport"].clone())
            .ok_or("请求快照已过期，请刷新后重试".into())
    }
    pub(super) fn expire(&mut self, now: DateTime<Utc>, allocate: bool) -> Result<(), String> {
        self.expire_csv_exports(now).map_err(|e| e.to_string())?;
        let mut remove: Vec<_> = self
            .snapshots
            .iter()
            .filter(|(_, s)| (now - s.created).num_minutes() >= 15)
            .map(|(id, _)| id.clone())
            .collect();
        let mut unified_remove: Vec<_> = self
            .unified_requests
            .iter()
            .filter(|(_, s)| (now - s.created).num_seconds() >= 300 || now < s.created)
            .map(|(id, _)| id.clone())
            .collect();
        while self.snapshots.len().saturating_sub(remove.len())
            + self
                .unified_requests
                .len()
                .saturating_sub(unified_remove.len())
            >= if allocate { 8 } else { 9 }
        {
            let legacy = self
                .snapshots
                .iter()
                .filter(|(id, _)| !remove.contains(id))
                .min_by_key(|(_, s)| s.created)
                .map(|(id, s)| (id.clone(), s.created));
            let unified = self
                .unified_requests
                .iter()
                .filter(|(id, _)| !unified_remove.contains(id))
                .min_by_key(|(_, s)| s.created)
                .map(|(id, s)| (id.clone(), s.created));
            match (legacy, unified) {
                (Some((id, at)), Some((_, ut))) if at <= ut => {
                    remove.push(id);
                }
                (Some(_), Some((id, _))) | (None, Some((id, _))) => unified_remove.push(id),
                (Some((id, _)), None) => remove.push(id),
                (None, None) => break,
            }
        }
        let mut removed = HashSet::new();
        for id in remove {
            if let Some(frozen) = self.snapshots.remove(&id) {
                removed.insert(frozen.positions);
            }
        }
        for id in unified_remove {
            if let Some(frozen) = self.unified_requests.remove(&id) {
                removed.insert(frozen.positions);
            }
        }
        self.request_templates.retain(|t| {
            if now >= t.created && now < t.expires {
                true
            } else {
                removed.insert(t.positions.clone());
                false
            }
        });
        self.release_request_positions(removed)?;
        self.prune()
    }
    pub fn source_report(
        &self,
        q: &requests::Query,
        now: DateTime<Utc>,
        sources: Vec<crate::sessions::SourceStatus>,
        as_of: Option<String>,
    ) -> Result<Value, String> {
        let mut report = value(&requests::SourceReport::new_with_directory(
            &[],
            &[],
            q,
            now,
            sources,
            as_of,
            &self.directory,
        ))?;
        if self.cached_request_report(q, now, &mut report)? {
            return Ok(report);
        }
        let scope = filter(q, now, true);
        for (kind, predicate) in [
            ("gateway", format!("r.kind='gateway' AND {scope}")),
            ("native", format!("r.kind='native' AND {scope}")),
            (
                "linked",
                format!("r.kind='gateway' AND r.linked=1 AND {scope}"),
            ),
        ] {
            let from =
                format!("(SELECT r.* FROM records r JOIN current c ON c.v=r.v WHERE {predicate})");
            let compact = if kind == "native" {
                self.native_request_cells(q, now)?
            } else {
                None
            };
            report[kind] = value(&if let Some(cells) = compact {
                aggregates::summary_base(&self.db, &cells)?
            } else {
                summary(&self.db, &from)?
            })?;
        }
        let cells = self.native_request_cells(q, now)?;
        let from = cells.clone().unwrap_or_else(|| base("native", &scope));
        let count_expressions = if cells.is_some() {
            "coalesce(sum(matched),0),coalesce(sum(ambiguous),0),coalesce(sum(conflicts),0)"
        } else {
            "count(*) FILTER(WHERE association='response_id'),count(*) FILTER(WHERE association IN ('ambiguous','identity_conflict')),count(*) FILTER(WHERE usage_conflict=1)"
        };
        let counts: (usize, usize, usize) = self
            .db
            .query_row(
                &format!("SELECT {count_expressions} FROM {from}"),
                [],
                |r| Ok((sql_count(r, 0)?, sql_count(r, 1)?, sql_count(r, 2)?)),
            )
            .map_err(fail)?;
        report["matchedNative"] = json!(counts.0);
        report["ambiguousNative"] = json!(counts.1);
        report["usageConflicts"] = json!(counts.2);
        report["unlinkedNative"] = json!(
            report["native"]["requests"]
                .as_u64()
                .unwrap_or(0)
                .saturating_sub(report["matchedNative"].as_u64().unwrap_or(0))
        );
        report["nativeMissingTime"] = json!(
            self.db
                .query_row(
                    &format!(
                        "SELECT count(*) FROM {} WHERE time IS NULL",
                        base("native", &filter(q, now, false))
                    ),
                    [],
                    |r| sql_count(r, 0)
                )
                .map_err(fail)?
        );
        self.remember_request_report(q, now, &report)?;
        Ok(report)
    }

    /// External declarative reports reuse the gateway summary rules, with a
    /// bounded rolling window and a SQLite instruction deadline. No native scan.
    pub fn external_gateway_summary(&self, now: DateTime<Utc>) -> Result<Value, String> {
        let start = now - chrono::Duration::hours(24);
        let query = requests::Query {
            source: Some("gateway".into()),
            start: Some(start.to_rfc3339()),
            end: Some(now.to_rfc3339()),
            ..Default::default()
        };
        let started = std::time::Instant::now();
        self.db
            .progress_handler(
                1000,
                Some(move || started.elapsed() > std::time::Duration::from_millis(200)),
            )
            .map_err(fail)?;
        let result =
            summary(&self.db, &base("gateway", &filter(&query, now, true))).and_then(|s| value(&s));
        self.db
            .progress_handler(0, None::<fn() -> bool>)
            .map_err(fail)?;
        result
    }
    fn groups(&self, from: &str, q: &requests::Query) -> Result<Value, String> {
        let metric = q.metric.as_deref().unwrap_or("calls");
        let dimension = q.dimension.as_deref().unwrap_or("agent");
        if q.source.as_deref() == Some("native") && ["calls", "tokens", "cost"].contains(&metric) {
            // Native records contain one response, without gateway attempt
            // fragments. Group their own narrow fields once instead of joining
            // records to itself and rescanning the selected history per model.
            let (key, label) = match dimension {
                "provider" => ("provider", "json_extract(value,'$.provider')"),
                "model" => ("coalesce(model,'模型未知')", "coalesce(model,'模型未知')"),
                _ => ("agent", "agent"),
            };
            let aggregate = "summary_sum(stats,'0','0')";
            let mut statement = self
                .db
                .prepare(&format!(
                    "SELECT {key} k,{label},min(id),{aggregate} FROM {from} GROUP BY k ORDER BY k"
                ))
                .map_err(fail)?;
            let rows = statement
                .query_map([], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(3)?,
                    ))
                })
                .map_err(fail)?;
            let mut groups = Vec::new();
            for row in rows {
                let (key, label, raw) = row.map_err(fail)?;
                let summary: requests::Summary = serde_json::from_str(&raw).map_err(fail)?;
                let missing = match metric {
                    "tokens" => summary.usage_missing,
                    "cost" => summary.cost.unknown_calls,
                    _ => 0,
                };
                groups.push(json!({"id":key,"label":label,"value":summary.metric(metric,q.currency.as_deref().unwrap_or("USD")),"samples":summary.requests,"missing":missing}));
            }
            groups.sort_by(|a, b| {
                b["value"]
                    .as_f64()
                    .unwrap_or(-1.0)
                    .total_cmp(&a["value"].as_f64().unwrap_or(-1.0))
                    .then(a["id"].as_str().cmp(&b["id"].as_str()))
            });
            return Ok(json!(groups));
        }
        let fragments = |dim: &str| {
            if q.source.as_deref() != Some("native") {
                return "fragments".into();
            }
            let (key, label) = match dimension {
                "provider" => ("provider", "json_extract(value,'$.provider')"),
                "model" => ("coalesce(model,'模型未知')", "coalesce(model,'模型未知')"),
                _ => ("agent", "agent"),
            };
            format!(
                "(SELECT v,{} dimension,{key} key,{label} label,NULL stats FROM records WHERE kind='native')",
                quote(dim)
            )
        };
        if metric == "calls" {
            let fragments = fragments(dimension);
            let mut groups = aggregates::list(
                &self.db,
                &format!(
                    "SELECT value FROM (SELECT json_object('id',f.key,'label',f.label,'value',count(*),'samples',count(*),'missing',0) value,min(r.id) first FROM {from} r JOIN {fragments} f ON f.v=r.v WHERE f.dimension={} GROUP BY f.key ORDER BY count(*) DESC,f.key)",
                    quote(dimension)
                ),
            )?;
            // Keep serialization identical to the canonical floating point metric.
            for g in &mut groups {
                g["value"] = json!(g["value"].as_f64());
            }
            return Ok(json!(groups));
        }
        let dim = if ["tokens", "cost"].contains(&metric) && dimension != "agent" {
            format!("{dimension}Parts")
        } else {
            dimension.into()
        };
        let fragments = fragments(&dim);
        let mut st=self.db.prepare(&format!("SELECT key,label FROM (SELECT f.key,f.label,row_number() OVER(PARTITION BY f.key ORDER BY r.id) rank FROM {fragments} f JOIN {from} r ON r.v=f.v WHERE f.dimension=?1) WHERE rank=1 ORDER BY key")).map_err(fail)?;
        let keys = st
            .query_map([&dim], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })
            .map_err(fail)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(fail)?;
        let mut groups = vec![];
        for (key, label) in keys {
            let fragment = format!(
                "(SELECT coalesce(f.stats,r.stats) stats,r.ttft_sample,r.duration_sample,r.throughput_sample,'0' AS cache_in,'0' AS cache_read FROM {fragments} f JOIN {from} r ON r.v=f.v WHERE f.dimension={} AND f.key={})",
                quote(&dim),
                quote(&key)
            );
            let s = if ["ttft", "duration", "throughput"].contains(&metric) {
                summary(&self.db, &fragment)?
            } else {
                aggregates::summary_base(&self.db, &fragment)?
            };
            let samples = match metric {
                "ttft" => s.ttft.samples,
                "duration" => s.duration.samples,
                "throughput" => s.throughput.samples,
                _ => s.requests,
            };
            let missing = if metric == "tokens" {
                s.usage_missing
            } else if metric == "cost" {
                s.cost.unknown_calls
            } else {
                s.requests.saturating_sub(samples)
            };
            groups.push(json!({"id":key,"label":label,"value":s.metric(metric,q.currency.as_deref().unwrap_or("USD")),"samples":samples,"missing":missing}));
        }
        groups.sort_by(|a, b| {
            b["value"]
                .as_f64()
                .unwrap_or(-1.0)
                .total_cmp(&a["value"].as_f64().unwrap_or(-1.0))
                .then(a["id"].as_str().cmp(&b["id"].as_str()))
        });
        Ok(json!(groups))
    }
    pub(super) fn request_layout(
        &self,
        from: &str,
        q: &requests::Query,
        now: DateTime<Utc>,
    ) -> Result<(DateTime<Utc>, DateTime<Utc>, Duration), String> {
        let first_ms: Option<i64> = if q
            .start
            .as_deref()
            .is_some_and(|t| DateTime::parse_from_rfc3339(t).is_ok())
        {
            None
        } else {
            self.db
                .query_row(&format!("SELECT min(time) FROM {from}"), [], |r| r.get(0))
                .map_err(fail)?
        };
        let first = if let Some(t) = first_ms {
            let raw: String = self
                .db
                .query_row(
                    &format!("SELECT started FROM {from} WHERE time={t} ORDER BY instant LIMIT 1"),
                    [],
                    |r| r.get(0),
                )
                .map_err(fail)?;
            DateTime::parse_from_rfc3339(&raw)
                .ok()
                .map(|t| t.with_timezone(&Utc))
        } else {
            None
        };
        let parse = |s: &str| {
            DateTime::parse_from_rfc3339(s)
                .ok()
                .map(|t| t.with_timezone(&Utc))
        };
        let start = q
            .start
            .as_deref()
            .and_then(parse)
            .or(first)
            .unwrap_or(now - Duration::hours(24));
        let end = q.end.as_deref().and_then(parse).unwrap_or(now).min(now);
        let span = (end - start).num_seconds().max(1);
        let step = Duration::seconds(if span <= 86400 {
            3600
        } else if span <= 86400 * 7 {
            21600
        } else if span <= 86400 * 60 {
            86400
        } else {
            ((span / 86400 / 52) + 1) * 86400
        });
        Ok((start, end, step))
    }
    fn buckets(
        &self,
        from: &str,
        q: &requests::Query,
        now: DateTime<Utc>,
    ) -> Result<Value, String> {
        let (start, end, step) = self.request_layout(from, q, now)?;
        let metric = q.metric.as_deref().unwrap_or("calls");
        let mut buckets = vec![];
        let mut at = start;
        while at < end && buckets.len() < 100 {
            let next = (at + step).min(end);
            let (a, b) = (at.timestamp_millis(), next.timestamp_millis());
            let scope = format!(
                "(SELECT * FROM {from} WHERE time>={a} AND time<={b} AND (time<>{a} OR instant_cmp(started,{})>=0) AND (time<>{b} OR instant_cmp(started,{})<0))",
                quote(&at.to_rfc3339()),
                quote(&next.to_rfc3339())
            );
            let s = if metric == "calls" && q.source.as_deref() != Some("native") {
                let requests = self
                    .db
                    .query_row(&format!("SELECT count(*) FROM {scope}"), [], |r| {
                        sql_count(r, 0)
                    })
                    .map_err(fail)?;
                requests::Summary {
                    requests,
                    ..Default::default()
                }
            } else if ["ttft", "duration", "throughput"].contains(&metric) {
                summary(&self.db, &scope)?
            } else {
                aggregates::summary_base(&self.db, &scope)?
            };
            let d = match metric {
                "ttft" => &s.ttft,
                "duration" => &s.duration,
                _ => &s.throughput,
            };
            let samples = if ["ttft", "duration", "throughput"].contains(&metric) {
                d.samples
            } else {
                s.requests
            };
            let missing = if metric == "tokens" {
                s.usage_missing
            } else if metric == "cost" {
                s.cost.unknown_calls
            } else {
                s.requests.saturating_sub(samples)
            };
            buckets.push(json!({"groups":self.groups(&scope,q)?,"start":at.to_rfc3339(),"end":next.to_rfc3339(),"value":s.metric(metric,q.currency.as_deref().unwrap_or("USD")),"p95":if ["ttft","duration"].contains(&metric){d.p95}else{None},"samples":samples,"missing":missing}));
            at = next;
        }
        Ok(json!(buckets))
    }
    fn heatmap(
        &self,
        from: &str,
        q: &requests::Query,
        now: DateTime<Utc>,
    ) -> Result<Value, String> {
        let mut heat = value(&requests::Heatmap::new(&[], now, q))?;
        let mut s=self.db.prepare(&format!("SELECT strftime('%Y-%m-%d',time/1000.0,'unixepoch','+8 hours'),summary_sum(stats,cache_in,cache_read) FROM {from} WHERE time IS NOT NULL GROUP BY 1")).map_err(fail)?;
        let totals = s
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
            .map_err(fail)?
            .collect::<Result<HashMap<_, _>, _>>()
            .map_err(fail)?;
        let mut outside = None;
        for day in heat["days"].as_array_mut().unwrap() {
            let start = timestamp(day["start"].as_str().unwrap()).unwrap();
            let s: requests::Summary = totals
                .get(day["date"].as_str().unwrap())
                .map(|v| serde_json::from_str(v).map_err(fail))
                .transpose()?
                .unwrap_or_default();
            if !["future", "out_of_range"].contains(&day["state"].as_str().unwrap())
                && s.requests > 0
            {
                day["state"] = json!("recorded");
            }
            day["records"] = json!(s.requests);
            day["tokens"] = json!(s.usage.total_tokens);
            day["cost"] = value(&s.cost)?;
            day["usageMissing"] = json!(s.usage_missing);
            outside.get_or_insert(start);
        }
        let outside = outside.unwrap_or(now.timestamp_millis());
        heat["outsideWindow"] = json!(
            self.db
                .query_row(
                    &format!(
                        "SELECT {} FROM {from} WHERE time<{outside}",
                        if q.source.as_deref() == Some("native") {
                            "coalesce(sum(json_extract(stats,'$.requests')),0)"
                        } else {
                            "count(*)"
                        }
                    ),
                    [],
                    |r| sql_count(r, 0)
                )
                .map_err(fail)?
        );
        heat["missingTime"] = json!(
            self.db
                .query_row(
                    &format!(
                        "SELECT {} FROM {from} WHERE time IS NULL",
                        if q.source.as_deref() == Some("native") {
                            "coalesce(sum(json_extract(stats,'$.requests')),0)"
                        } else {
                            "count(*)"
                        }
                    ),
                    [],
                    |r| sql_count(r, 0)
                )
                .map_err(fail)?
        );
        Ok(heat)
    }
}
