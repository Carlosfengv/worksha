use super::{
    Cached, Checked, Manager, Result, fail, package,
    store::{Installed, Selection},
    worker::RpcRequest,
};
use local_agent_core::{Provider, plugin::external::*};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    time::Duration,
};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWriteExt, BufReader};

struct Context {
    settings: BTreeMap<String, String>,
    id: String,
    plugin_id: String,
    digest: String,
    selection: Selection,
    preflight: bool,
    source_ref: String,
    connection_ref: String,
    source: Option<String>,
    source_revision: Option<String>,
    provider: Option<Provider>,
    network_used: bool,
    broker_sequence: u64,
}
impl Manager {
    fn context(
        &self,
        p: &Installed,
        s: &Selection,
        preflight: bool,
        provider: Option<Provider>,
    ) -> Result<Context> {
        let mut context = Context {
            settings: self.status(&p.manifest.id, &p.digest)?.settings,
            id: uuid::Uuid::new_v4().to_string(),
            plugin_id: p.manifest.id.clone(),
            digest: p.digest.clone(),
            selection: s.clone(),
            preflight,
            source_ref: uuid::Uuid::new_v4().to_string(),
            connection_ref: uuid::Uuid::new_v4().to_string(),
            source: None,
            source_revision: None,
            provider,
            network_used: false,
            broker_sequence: 0,
        };
        if !preflight {
            self.alive(&context)?;
            if p.manifest
                .capabilities
                .iter()
                .any(|c| c == "session.source@1")
            {
                let grant = s.source.as_ref().ok_or(fail("permission_denied"))?;
                let bytes =
                    package::read_granted_file(&grant.path, MAX_SOURCE, grant.device, grant.inode)?;
                context.source_revision = Some(package::hash(&bytes));
                context.source = Some(String::from_utf8(bytes).checked("invalid_input")?);
            } else if p
                .manifest
                .capabilities
                .iter()
                .any(|c| c == "account.quota-source@1")
            {
                let grant = s.connection.as_ref().ok_or(fail("permission_denied"))?;
                let provider = context.provider.as_ref().ok_or(fail("permission_denied"))?;
                if provider.id != grant.provider_id
                    || provider.endpoint != grant.endpoint
                    || provider.revision != grant.revision
                    || provider.credential_ref != grant.credential_ref
                {
                    return Err(fail("grant_revoked"));
                }
            }
        }
        Ok(context)
    }
    fn alive(&self, context: &Context) -> Result<()> {
        self.alive_selection(
            &context.plugin_id,
            &context.digest,
            &context.selection,
            context.preflight,
        )
    }
    fn alive_selection(
        &self,
        plugin_id: &str,
        digest: &str,
        expected: &Selection,
        preflight: bool,
    ) -> Result<()> {
        self.ensure_running()?;
        let s = self.selection(plugin_id)?;
        if s.digest != expected.digest
            || s.epoch != expected.epoch
            || s.revision != expected.revision
        {
            return Err(fail("grant_revoked"));
        }
        if !preflight && (!s.enabled || s.digest != digest) {
            return Err(fail("plugin_disabled"));
        }
        Ok(())
    }
    async fn alive_async(self: &std::sync::Arc<Self>, context: &Context) -> Result<()> {
        self.ensure_running()?;
        let manager = self.clone();
        let id = context.plugin_id.clone();
        let digest = context.digest.clone();
        let selected = context.selection.clone();
        let preflight = context.preflight;
        super::run_sync(move || manager.alive_selection(&id, &digest, &selected, preflight))
            .await?;
        self.ensure_running()
    }
    pub(crate) async fn prepare_worker(
        self: &std::sync::Arc<Self>,
        p: &Installed,
        s: &Selection,
    ) -> Result<()> {
        if matches!(p.manifest.runtime, Runtime::Declarative) {
            return Ok(());
        }
        let _permit = self.workers.try_acquire().checked("resource_limit")?;
        let manager = self.clone();
        let installed = p.clone();
        let selected = s.clone();
        let mut context =
            super::run_sync(move || manager.context(&installed, &selected, true, None)).await?;
        self.invoke(p, &mut context, None).await?;
        Ok(())
    }
    async fn broker(
        self: &std::sync::Arc<Self>,
        context: &mut Context,
        p: &Installed,
        method: &str,
        payload: &str,
    ) -> Result<String> {
        if context.preflight {
            return Err(fail("permission_denied"));
        }
        self.alive_async(context).await?;
        let value = match method {
            "source.read" => {
                let request: ReadChunk = serde_json::from_str(payload).checked("invalid_input")?;
                if request.context_id != context.id
                    || request.source_ref != context.source_ref
                    || request.max_bytes == 0
                    || request.max_bytes as usize > MAX_CHUNK
                {
                    return Err(fail("permission_denied"));
                }
                let offset = decimal_u64(&request.offset)? as usize;
                let text = context.source.as_ref().ok_or(fail("permission_denied"))?;
                if offset > text.len() || !text.is_char_boundary(offset) {
                    return Err(fail("invalid_input"));
                }
                let mut end = (offset + request.max_bytes as usize).min(text.len());
                while end > offset && !text.is_char_boundary(end) {
                    end -= 1;
                }
                if end == offset && offset < text.len() {
                    return Err(fail("resource_limit"));
                }
                serde_json::to_value(SourceChunk {
                    text: text[offset..end].into(),
                    next_offset: end.to_string(),
                    complete: end == text.len(),
                    source_revision: context
                        .source_revision
                        .clone()
                        .ok_or(fail("permission_denied"))?,
                })
                .checked("invalid_input")?
            }
            "connection.request" => {
                let request: ConnectionRequest =
                    serde_json::from_str(payload).checked("invalid_input")?;
                if request.context_id != context.id
                    || request.connection_ref != context.connection_ref
                    || context.network_used
                {
                    return Err(fail("permission_denied"));
                }
                context.network_used = true;
                let provider = context.provider.as_ref().ok_or(fail("permission_denied"))?;
                let path = p
                    .manifest
                    .permissions
                    .iter()
                    .find_map(|p| {
                        if let Permission::ConnectionRequest { path } = p {
                            Some(path)
                        } else {
                            None
                        }
                    })
                    .ok_or(fail("permission_denied"))?;
                let grant = context
                    .selection
                    .connection
                    .as_ref()
                    .ok_or(fail("permission_denied"))?;
                let mut url = url::Url::parse(&provider.endpoint).checked("invalid_input")?;
                let endpoint_origin = url.origin();
                let target = format!("{}{path}", url.path().trim_end_matches('/'));
                url.set_path(&target);
                if url.origin() != endpoint_origin
                    || url.query().is_some()
                    || url.fragment().is_some()
                    || !url.username().is_empty()
                    || url.password().is_some()
                {
                    return Err(fail("permission_denied"));
                }
                let host = url
                    .host_str()
                    .ok_or(fail("permission_denied"))?
                    .trim_matches(['[', ']']);
                let port = url
                    .port_or_known_default()
                    .ok_or(fail("permission_denied"))?;
                let addresses: Vec<_> = tokio::time::timeout(
                    Duration::from_secs(3),
                    tokio::net::lookup_host((host, port)),
                )
                .await
                .checked("deadline_exceeded")?
                .checked("unavailable")?
                .take(16)
                .collect();
                if addresses.is_empty()
                    || (!grant.allow_private_network
                        && addresses.iter().any(|a| !public_address(a.ip())))
                {
                    return Err(fail("permission_denied"));
                }
                let client = reqwest::Client::builder()
                    .no_proxy()
                    .redirect(reqwest::redirect::Policy::none())
                    .timeout(Duration::from_secs(5))
                    .resolve_to_addrs(host, &addresses)
                    .build()
                    .checked("unavailable")?;
                let mut request = client.get(url.clone());
                let secret = if let Some(reference) = &provider.credential_ref {
                    let vault = self.vault.clone();
                    let reference = reference.clone();
                    Some(
                        super::run_sync(move || vault.get(&reference).checked("permission_denied"))
                            .await?,
                    )
                } else {
                    None
                };
                if let Some(secret) = &secret {
                    let secret = std::str::from_utf8(secret).checked("permission_denied")?;
                    if provider.protocol == "messages" {
                        request = request.header("x-api-key", secret);
                    } else {
                        request = request.bearer_auth(secret);
                    }
                }
                self.alive_async(context).await?;
                let mut response = request.send().await.checked("unavailable")?;
                if !response.status().is_success()
                    || response
                        .content_length()
                        .is_some_and(|n| n > MAX_FRAME as u64 / 2)
                {
                    return Err(fail("unavailable"));
                }
                let mut bytes = Vec::new();
                while let Some(chunk) = response.chunk().await.checked("unavailable")? {
                    if bytes.len() + chunk.len() > MAX_FRAME / 2 {
                        return Err(fail("resource_limit"));
                    }
                    bytes.extend_from_slice(&chunk);
                }
                if secret
                    .as_ref()
                    .is_some_and(|secret| body_contains_secret(&bytes, secret))
                {
                    return Err(fail("permission_denied"));
                }
                let body = String::from_utf8(bytes).checked("invalid_input")?;
                json!({"body":body,"status":200})
            }
            _ => return Err(fail("unsupported")),
        };
        self.alive_async(context).await?;
        serde_json::to_string(&value).checked("invalid_input")
    }
    async fn exchange(
        self: &std::sync::Arc<Self>,
        p: &Installed,
        context: &mut Context,
        streams: (
            &mut tokio::process::ChildStdin,
            &mut BufReader<tokio::process::ChildStdout>,
        ),
        sequence: u32,
        method: &str,
        params: Value,
    ) -> Result<Value> {
        let (input, output) = streams;
        let id = format!("host-{sequence}");
        let mut request = serde_json::to_vec(&RpcRequest {
            jsonrpc: "2.0".into(),
            id: id.clone(),
            method: method.into(),
            params,
        })
        .checked("invalid_input")?;
        request.push(b'\n');
        if request.len() > MAX_FRAME {
            return Err(fail("resource_limit"));
        }
        input.write_all(&request).await.checked("worker_crashed")?;
        input.flush().await.checked("worker_crashed")?;
        loop {
            let pending = read_frame(output);
            tokio::pin!(pending);
            let line = loop {
                tokio::select! {
                    result=&mut pending=>break result?,
                    _=tokio::time::sleep(Duration::from_millis(50))=>{self.alive_async(context).await?;}
                }
            };
            let value: Value = serde_json::from_slice(&line).checked("worker_crashed")?;
            if value["jsonrpc"] != "2.0" {
                return Err(fail("worker_crashed"));
            }
            if value["method"] == "host.call" {
                let request: RpcRequest = serde_json::from_value(value).checked("invalid_input")?;
                context.broker_sequence += 1;
                if request.id != format!("runner-{}", context.broker_sequence)
                    || context.broker_sequence > 128
                {
                    return Err(fail("invalid_input"));
                }
                let result = if request.params["contextId"] != context.id {
                    Err(fail("permission_denied"))
                } else {
                    let method = request.params["method"]
                        .as_str()
                        .ok_or(fail("invalid_input"))?;
                    let payload = request.params["payload"]
                        .as_str()
                        .ok_or(fail("invalid_input"))?;
                    self.broker(context, p, method, payload).await
                };
                self.alive_async(context).await?;
                let result = result?;
                let response = json!({"jsonrpc":"2.0","id":request.id,"result":result});
                let mut encoded = serde_json::to_vec(&response).checked("invalid_input")?;
                encoded.push(b'\n');
                if encoded.len() > MAX_FRAME {
                    return Err(fail("resource_limit"));
                }
                input.write_all(&encoded).await.checked("worker_crashed")?;
                input.flush().await.checked("worker_crashed")?;
                continue;
            }
            if value["id"] != id || value.get("result").is_some() == value.get("error").is_some() {
                return Err(fail("worker_crashed"));
            }
            if value.get("error").is_some() {
                return Err(fail(match value["error"]["data"]["code"].as_str() {
                    Some("resource_limit") => "resource_limit",
                    Some("permission_denied") => "permission_denied",
                    _ => "worker_crashed",
                }));
            }
            self.alive_async(context).await?;
            return Ok(value["result"].clone());
        }
    }
    async fn invoke(
        self: &std::sync::Arc<Self>,
        p: &Installed,
        context: &mut Context,
        operation: Option<(&str, Value)>,
    ) -> Result<Value> {
        context.broker_sequence = 0;
        let mut child =
            tokio::process::Command::new(std::env::current_exe().checked("unavailable")?)
                .arg("--plugin-worker")
                .arg(&p.path)
                .arg(&p.digest)
                .arg(if self.development {
                    "development"
                } else {
                    "signed"
                })
                .env_clear()
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::null())
                .kill_on_drop(true)
                .spawn()
                .checked("worker_crashed")?;
        let pid = child.id().ok_or(fail("worker_crashed"))?;
        let mut input = child.stdin.take().ok_or(fail("worker_crashed"))?;
        let mut output = BufReader::new(child.stdout.take().ok_or(fail("worker_crashed"))?);
        let expected = context.selection.clone();
        let plugin_id = context.plugin_id.clone();
        let preflight = context.preflight;
        let revoked = async {
            loop {
                tokio::time::sleep(Duration::from_millis(50)).await;
                self.ensure_running()?;
                let manager = self.clone();
                let id = plugin_id.clone();
                let current = super::run_sync(move || manager.selection(&id)).await?;
                if current.digest != expected.digest
                    || current.epoch != expected.epoch
                    || current.revision != expected.revision
                    || !preflight && !current.enabled
                {
                    return Err(fail("grant_revoked"));
                }
            }
        };
        let work = async {
            let init = Initialize {
                settings: context.settings.clone(),
                api_version: API_VERSION.into(),
                capabilities: p.manifest.capabilities.clone(),
                package_digest: p.digest.clone(),
                activation_epoch: context.selection.epoch.to_string(),
                grant_revision: context.selection.revision.to_string(),
                context_id: context.id.clone(),
                fuel: "100000000".into(),
                max_memory_bytes: "134217728".into(),
            };
            self.exchange(
                p,
                context,
                (&mut input, &mut output),
                1,
                "plugin.initialize",
                serde_json::to_value(init).checked("invalid_input")?,
            )
            .await?;
            for (i, method) in ["plugin.prepare", "plugin.activate", "plugin.health"]
                .iter()
                .enumerate()
            {
                self.exchange(
                    p,
                    context,
                    (&mut input, &mut output),
                    2 + i as u32,
                    method,
                    json!({}),
                )
                .await?;
            }
            let value = if let Some((method, params)) = operation {
                self.exchange(p, context, (&mut input, &mut output), 5, method, params)
                    .await?
            } else {
                json!({"ready":true})
            };
            self.exchange(
                p,
                context,
                (&mut input, &mut output),
                6,
                "plugin.deactivate",
                json!({}),
            )
            .await?;
            self.exchange(
                p,
                context,
                (&mut input, &mut output),
                7,
                "plugin.dispose",
                json!({}),
            )
            .await?;
            Ok(value)
        };
        let monitored = async {
            tokio::select! {result=work=>result,result=monitor_rss(pid)=>result,result=revoked=>result}
        };
        let result = tokio::time::timeout(Duration::from_secs(30), monitored)
            .await
            .checked("deadline_exceeded")
            .and_then(|r| r);
        drop(input);
        if result.is_err() {
            let _ = child.kill().await;
        }
        let exit = tokio::time::timeout(Duration::from_secs(1), child.wait()).await;
        if !matches!(exit,Ok(Ok(status)) if status.success()) && result.is_ok() {
            let _ = child.kill().await;
            return Err(fail("worker_crashed"));
        }
        result
    }
    pub(super) async fn invoke_catalog(
        self: &std::sync::Arc<Self>,
        q: &local_agent_core::domain::provider_access::Query,
        method: &str,
        params: Value,
    ) -> Result<Value> {
        if !["provider.catalog.request", "provider.catalog.parse"].contains(&method) {
            return Err(fail("unsupported"));
        }
        let _permit = self.workers.try_acquire().checked("resource_limit")?;
        let manager = self.clone();
        let query = q.clone();
        let (p, mut context) = super::run_sync(move || {
            let (p, _) = manager.catalog_plugin(&query)?;
            let selected = manager.selection(&query.plugin_id)?;
            let context = manager.context(&p, &selected, false, None)?;
            Ok((p, context))
        })
        .await?;
        self.invoke(&p, &mut context, Some((method, params))).await
    }
    pub(crate) async fn refresh_external(
        self: &std::sync::Arc<Self>,
        id: &str,
        digest: &str,
        provider: Option<Provider>,
    ) -> Result<Value> {
        let manager = self.clone();
        let owned_id = id.to_owned();
        let gate = super::run_sync(move || manager.gate(&owned_id)).await?;
        let mut guard = Some(gate.try_lock_owned().checked("resource_limit")?);
        let _permit = self.workers.try_acquire().checked("resource_limit")?;
        let manager = self.clone();
        let owned_id = id.to_owned();
        let owned_digest = digest.to_owned();
        let (p, s, mut context) = super::run_sync(move || {
            let p = manager.verified(&owned_id, &owned_digest)?;
            let s = manager.selection(&owned_id)?;
            let context = manager.context(&p, &s, false, provider)?;
            Ok((p, s, context))
        })
        .await?;
        let mut committing = false;
        let result = async {
            let mut cached = Cached {
                observed_at: chrono::Utc::now().to_rfc3339(),
                report: None,
                collected_at: std::time::Instant::now(),
                digest: digest.into(),
                epoch: s.epoch.to_string(),
                revision: s.revision.to_string(),
                snapshot: uuid::Uuid::new_v4().to_string(),
                sessions: Vec::new(),
                source_revision: context.source_revision.clone(),
                quota: None,
            };
            if p.manifest
                .capabilities
                .iter()
                .any(|c| c == "session.source@1")
            {
                let mut cursor = None;
                let mut ids = BTreeSet::new();
                let mut batches = BTreeMap::new();
                for page in 0..40 {
                    let request = SourceRequest {
                        context_id: context.id.clone(),
                        source_ref: context.source_ref.clone(),
                        activation_epoch: s.epoch.to_string(),
                        grant_revision: s.revision.to_string(),
                        cursor: cursor.clone(),
                    };
                    let value = self
                        .invoke(
                            &p,
                            &mut context,
                            Some((
                                "session.scan",
                                serde_json::to_value(request).checked("invalid_input")?,
                            )),
                        )
                        .await?;
                    let encoded = serde_json::to_vec(&value).checked("invalid_input")?;
                    if encoded.len() > 524288 {
                        return Err(fail("resource_limit"));
                    }
                    let batch: SourceBatch =
                        serde_json::from_value(value).checked("invalid_input")?;
                    if batch.schema_version != 1
                        || batch.source_ref != context.source_ref
                        || Some(&batch.source_revision) != context.source_revision.as_ref()
                        || batch.upserts.len() > MAX_ITEMS
                        || batch.batch_id.len() > 100
                        || batch.complete != batch.next_cursor.is_none()
                    {
                        return Err(fail("invalid_input"));
                    }
                    if batches
                        .insert(batch.batch_id.clone(), package::hash(&encoded))
                        .is_some()
                    {
                        return Err(fail("invalid_input"));
                    }
                    for session in batch.upserts {
                        validate_session(&session)?;
                        if !ids.insert(session.native_id.clone()) {
                            return Err(fail("invalid_input"));
                        }
                        cached.sessions.push(session);
                    }
                    if batch.complete {
                        break;
                    }
                    if page == 39
                        || batch.next_cursor == cursor
                        || batch.next_cursor.as_ref().is_none_or(|s| s.len() > 128)
                    {
                        return Err(fail("resource_limit"));
                    }
                    cursor = batch.next_cursor;
                }
            } else if p
                .manifest
                .capabilities
                .iter()
                .any(|c| c == "account.quota-source@1")
            {
                let request = QuotaRequest {
                    context_id: context.id.clone(),
                    connection_ref: context.connection_ref.clone(),
                    observed_at: chrono::Utc::now().to_rfc3339(),
                };
                let value = self
                    .invoke(
                        &p,
                        &mut context,
                        Some((
                            "account.quota",
                            serde_json::to_value(request).checked("invalid_input")?,
                        )),
                    )
                    .await?;
                let mut quota: Quota = serde_json::from_value(value).checked("invalid_input")?;
                if !decimal_amount(&quota.remaining)
                    || quota.limit.as_ref().is_some_and(|s| !decimal_amount(s))
                    || !["tokens", "credits", "USD", "CNY"].contains(&quota.unit.as_str())
                    || chrono::DateTime::parse_from_rfc3339(&quota.observed_at).is_err()
                    || quota
                        .expires_at
                        .as_ref()
                        .is_some_and(|s| chrono::DateTime::parse_from_rfc3339(s).is_err())
                {
                    return Err(fail("invalid_input"));
                }
                quota.observed_at = chrono::Utc::now().to_rfc3339();
                cached.quota = Some(quota);
            } else {
                return Err(fail("unsupported"));
            }
            cached.observed_at = chrono::Utc::now().to_rfc3339();
            let manager = self.clone();
            let selected = s.clone();
            let id = id.to_owned();
            let guard = guard.take().ok_or(fail("unavailable"))?;
            committing = true;
            super::run_sync(move || {
                let _guard = guard;
                manager.publish_refresh(&id, &selected, cached)
            })
            .await
        };
        let result = tokio::time::timeout(Duration::from_secs(30), result)
            .await
            .checked("deadline_exceeded")
            .and_then(|value| value);
        if let Err(error) = &result
            && !committing
        {
            // Keep a last complete snapshot on parser/network failure. A revision
            // change still invalidates it; no partial scan is treated as deletion.
            let manager = self.clone();
            let code = error.code;
            let id = id.to_owned();
            let _ = super::run_sync(move || {
                let _guard = guard;
                if manager.alive(&context).is_ok() {
                    manager.db.lock().checked("unavailable")?.execute(
                        "UPDATE selections SET status='degraded',error=?2 WHERE plugin_id=?1 AND digest=?3 AND epoch=?4 AND revision=?5 AND enabled=1",
                        rusqlite::params![id,code,context.selection.digest,i64::try_from(context.selection.epoch).checked("resource_limit")?,i64::try_from(context.selection.revision).checked("resource_limit")?],
                    ).checked("unavailable")?;
                }
                Ok(())
            }).await;
        }
        result
    }
    fn publish_refresh(&self, id: &str, selected: &Selection, cached: Cached) -> Result<Value> {
        self.ensure_running()?;
        let db = self.db.lock().checked("unavailable")?;
        let changed = db.execute(
            "UPDATE selections SET status='active',error=NULL WHERE plugin_id=?1 AND digest=?2 AND epoch=?3 AND revision=?4 AND enabled=1",
            rusqlite::params![id, selected.digest, i64::try_from(selected.epoch).checked("resource_limit")?, i64::try_from(selected.revision).checked("resource_limit")?],
        ).checked("unavailable")?;
        if changed != 1 {
            return Err(fail("grant_revoked"));
        }
        self.cache
            .lock()
            .checked("unavailable")?
            .data
            .insert(id.into(), cached.into());
        drop(db);
        // New opt-in persistence cannot turn a successful legacy refresh into a failure.
        // The unified reader separately reports a persistence error.
        let _ = self.domain_cache_published(id);
        Ok(json!({"refreshed":true}))
    }
    pub(crate) fn query_view(&self, query: &ViewQuery) -> Result<Value> {
        let selected = self.selection(&query.plugin_id)?;
        if !selected.enabled {
            return Err(fail("plugin_disabled"));
        }
        let (_, bundle) = self.verified_package(&query.plugin_id, &selected.digest)?;
        if bundle
            .manifest
            .requires
            .iter()
            .any(|r| r == local_agent_core::plugin::object_contributions::FEATURE)
        {
            return Err(fail("object_context_required"));
        }
        self.require_view_dependencies(&bundle, &query.view_id)?;
        let view = bundle
            .manifest
            .views
            .iter()
            .find(|v| v.id == query.view_id)
            .ok_or(fail("not_found"))?;
        let status = self.status(&query.plugin_id, &selected.digest)?;
        if view.kind == ViewKind::Settings {
            return Ok(
                json!({"view":view,"status":status,"settings":status.settings,"confirmedEmpty":false}),
            );
        }
        if view.kind == ViewKind::Providers {
            return Ok(
                json!({"view":view,"status":status,"providers":bundle.manifest.providers,"confirmedEmpty":bundle.manifest.providers.is_empty()}),
            );
        }
        let cache = self.cache.lock().checked("unavailable")?;
        let saved = if view.kind == ViewKind::Report {
            cache
                .reports
                .get(&query.plugin_id)
                .and_then(|views| views.get(&query.view_id))
        } else {
            cache.data.get(&query.plugin_id).map(AsRef::as_ref)
        };
        let Some(saved) = saved.filter(|c| {
            status.enabled
                && (view.kind != ViewKind::Report || status.report_authorized)
                && c.digest == status.digest
                && c.epoch == status.activation_epoch
                && c.revision == status.grant_revision
        }) else {
            if query.cursor.is_some() {
                return Err(fail("stale_snapshot"));
            }
            return Ok(
                json!({"view":view,"status":status,"snapshot":null,"items":[],"quota":null,"report":null,"confirmedEmpty":false,"needsRefresh":true}),
            );
        };
        let offset = if let Some(cursor) = &query.cursor {
            let (snapshot, offset) = cursor.split_once('.').ok_or(fail("stale_snapshot"))?;
            if snapshot != saved.snapshot {
                return Err(fail("stale_snapshot"));
            }
            decimal_u64(offset)? as usize
        } else {
            0
        };
        if offset > saved.sessions.len() {
            return Err(fail("stale_snapshot"));
        }
        let items: Vec<_> = saved
            .sessions
            .iter()
            .skip(offset)
            .take(40)
            .map(|s| json!({"sessionId":format!("{}:{}",query.plugin_id,s.native_id),"record":s}))
            .collect();
        let total = saved.sessions.len();
        let end = (offset + 40).min(total);
        let next = if end < total {
            Some(format!("{}.{}", saved.snapshot, end))
        } else {
            None
        };
        let mut input = 0u64;
        let mut output = 0u64;
        let mut known = 0u64;
        for session in &saved.sessions {
            if let Some(usage) = &session.usage {
                input = input
                    .checked_add(decimal_u64(&usage.input_tokens)?)
                    .ok_or(fail("resource_limit"))?;
                output = output
                    .checked_add(decimal_u64(&usage.output_tokens)?)
                    .ok_or(fail("resource_limit"))?;
                known += 1;
            }
        }
        let total_tokens = input.checked_add(output).ok_or(fail("resource_limit"))?;
        let stale = saved.expired();
        Ok(json!({
            "view": view,
            "status": status,
            "snapshot": saved.snapshot,
            "items": items,
            "quota": saved.quota,
            "report": saved.report,
            "quotaExpired": stale,
            "reportExpired": stale,
            "total": total,
            "nextCursor": next,
            "confirmedEmpty": total == 0 && saved.quota.is_none() && saved.report.is_none(),
            "summary": {
                "knownUsageRecords": known.to_string(),
                "inputTokens": input.to_string(),
                "outputTokens": output.to_string(),
                "totalTokens": total_tokens.to_string()
            },
            "needsRefresh": false
        }))
    }
    pub(crate) async fn read_session(
        self: &std::sync::Arc<Self>,
        id: &str,
        digest: &str,
        session_id: &str,
        snapshot: &str,
    ) -> Result<Value> {
        let manager = self.clone();
        let owned_id = id.to_owned();
        let gate = super::run_sync(move || manager.gate(&owned_id)).await?;
        let _guard = gate.try_lock_owned().checked("resource_limit")?;
        let _permit = self.workers.try_acquire().checked("resource_limit")?;
        let native_id = session_id
            .strip_prefix(&format!("{id}:"))
            .ok_or(fail("permission_denied"))?
            .to_owned();
        let manager = self.clone();
        let id = id.to_owned();
        let digest = digest.to_owned();
        let snapshot = snapshot.to_owned();
        let (p, s, mut context, native_id) = super::run_sync(move || {
            let p = manager.verified(&id, &digest)?;
            let s = manager.selection(&id)?;
            let source_revision = {
                let cache = manager.cache.lock().checked("unavailable")?;
                let saved = cache.data.get(&id).ok_or(fail("stale_snapshot"))?;
                if saved.snapshot != snapshot
                    || saved.digest != digest
                    || saved.revision != s.revision.to_string()
                    || saved.epoch != s.epoch.to_string()
                    || !saved
                        .sessions
                        .iter()
                        .any(|item| item.native_id == native_id)
                {
                    return Err(fail("stale_snapshot"));
                }
                saved.source_revision.clone()
            };
            let context = manager.context(&p, &s, false, None)?;
            if context.source_revision != source_revision {
                return Err(fail("stale_snapshot"));
            }
            Ok((p, s, context, native_id))
        })
        .await?;
        let request = ConversationRequest {
            native_id: native_id.clone(),
            source: SourceRequest {
                context_id: context.id.clone(),
                source_ref: context.source_ref.clone(),
                activation_epoch: s.epoch.to_string(),
                grant_revision: s.revision.to_string(),
                cursor: None,
            },
        };
        let value = self
            .invoke(
                &p,
                &mut context,
                Some((
                    "session.read",
                    serde_json::to_value(request).checked("invalid_input")?,
                )),
            )
            .await?;
        let manager = self.clone();
        super::run_sync(move || {
            let conversation: Conversation =
                serde_json::from_value(value).checked("invalid_input")?;
            if conversation.native_id != native_id
                || conversation.messages.len() > 256
                || conversation.messages.iter().any(|m| {
                    !["user", "assistant", "tool"].contains(&m.role.as_str())
                        || m.text.len() > 65536
                })
            {
                return Err(fail("invalid_input"));
            }
            manager.alive(&context)?;
            serde_json::to_value(conversation).checked("invalid_input")
        })
        .await
    }
}
pub(super) async fn read_frame(reader: &mut (impl AsyncBufRead + Unpin)) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    loop {
        let chunk = reader.fill_buf().await.checked("worker_crashed")?;
        if chunk.is_empty() {
            return Err(fail("worker_crashed"));
        }
        let end = chunk.iter().position(|b| *b == b'\n');
        let count = end.map_or(chunk.len(), |n| n + 1);
        if bytes.len() + count > MAX_FRAME {
            return Err(fail("resource_limit"));
        }
        bytes.extend_from_slice(&chunk[..count]);
        reader.consume(count);
        if end.is_some() {
            return Ok(bytes);
        }
    }
}
pub(super) fn body_contains_secret(bytes: &[u8], secret: &[u8]) -> bool {
    if secret.is_empty() {
        return false;
    }
    if bytes.windows(secret.len()).any(|part| part == secret) {
        return true;
    }
    // JSON may escape slashes or Unicode while still handing the raw key to a
    // parser. Inspect decoded strings and object keys before crossing the ABI.
    fn decoded(value: &Value, secret: &str) -> bool {
        match value {
            Value::String(text) => text.contains(secret),
            Value::Array(items) => items.iter().any(|item| decoded(item, secret)),
            Value::Object(fields) => fields
                .iter()
                .any(|(key, value)| key.contains(secret) || decoded(value, secret)),
            _ => false,
        }
    }
    match (
        std::str::from_utf8(secret),
        serde_json::from_slice::<Value>(bytes),
    ) {
        (Ok(secret), Ok(value)) => decoded(&value, secret),
        _ => false,
    }
}
pub(super) async fn monitor_rss(pid: u32) -> Result<Value> {
    loop {
        tokio::time::sleep(Duration::from_millis(100)).await;
        // An observed RSS ceiling, not an exact instantaneous hard memory limit.
        let output = tokio::time::timeout(
            Duration::from_secs(1),
            tokio::process::Command::new("/bin/ps")
                .args(["-p", &pid.to_string(), "-o", "rss="])
                .output(),
        )
        .await
        .checked("deadline_exceeded")?
        .checked("unavailable")?;
        let text = String::from_utf8(output.stdout).checked("unavailable")?;
        if !output.status.success() || text.trim().is_empty() {
            // The protocol reader owns exit/EOF handling. A worker may already
            // have sent its successful dispose reply while its waiter is doing
            // the final lease check; do not race that reply with an RSS error.
            if unsafe { libc::kill(pid as libc::pid_t, 0) } != 0
                && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
            {
                return std::future::pending().await;
            }
            return Err(fail("worker_crashed"));
        }
        if text.trim().parse::<u64>().checked("unavailable")? > 512 * 1024 {
            return Err(fail("resource_limit"));
        }
    }
}
pub(crate) fn decimal_u64(text: &str) -> Result<u64> {
    let value = text.parse::<u64>().checked("invalid_input")?;
    if value.to_string() != text {
        return Err(fail("invalid_input"));
    }
    Ok(value)
}
fn decimal_amount(text: &str) -> bool {
    let mut parts = text.split('.');
    let integer = parts.next().unwrap_or("");
    !integer.is_empty()
        && integer.len() <= 26
        && integer.bytes().all(|b| b.is_ascii_digit())
        && (integer == "0" || !integer.starts_with('0'))
        && parts
            .next()
            .is_none_or(|p| !p.is_empty() && p.len() <= 12 && p.bytes().all(|b| b.is_ascii_digit()))
        && parts.next().is_none()
}
pub(super) fn validate_session(s: &SessionCandidate) -> Result<()> {
    if s.native_id.is_empty()
        || s.native_id.len() > 160
        || s.native_id.chars().any(char::is_control)
        || s.title.len() > 240
        || s.title.chars().any(char::is_control)
        || s.model.as_ref().is_some_and(|m| m.len() > 200)
        || s.created_at
            .as_ref()
            .is_some_and(|s| chrono::DateTime::parse_from_rfc3339(s).is_err())
    {
        return Err(fail("invalid_input"));
    }
    if let Some(u) = &s.usage {
        let input = decimal_u64(&u.input_tokens)?;
        let output = decimal_u64(&u.output_tokens)?;
        input.checked_add(output).ok_or(fail("resource_limit"))?;
        if u.cache_read_tokens
            .as_ref()
            .map(|s| decimal_u64(s))
            .transpose()?
            .is_some_and(|n| n > input)
            || u.reasoning_tokens
                .as_ref()
                .map(|s| decimal_u64(s))
                .transpose()?
                .is_some_and(|n| n > output)
        {
            return Err(fail("invalid_input"));
        }
    }
    Ok(())
}
pub(super) fn public_address(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(ip) => {
            let octets = ip.octets();
            !ip.is_private()
                && !ip.is_loopback()
                && !ip.is_link_local()
                && !ip.is_unspecified()
                && !ip.is_multicast()
                && !ip.is_broadcast()
                && !ip.is_documentation()
                && octets[0] != 0
                && octets[0] < 224
                && !(octets[0] == 100 && (64..128).contains(&octets[1]))
                && !(octets[0] == 198 && (18..20).contains(&octets[1]))
                && !(octets[0] == 192 && octets[1] == 0 && octets[2] == 0)
        }
        std::net::IpAddr::V6(ip) => {
            if let Some(v4) = ip.to_ipv4_mapped() {
                return public_address(v4.into());
            }
            let s = ip.segments();
            (s[0] & 0xe000) == 0x2000
                && s[0] != 0x2002
                && !(s[0] == 0x2001 && (s[1] < 0x200 || s[1] == 0xdb8))
        }
    }
}

#[cfg(test)]
mod isolation_tests {
    #[tokio::test]
    async fn rss_monitor_delegates_a_reaped_workers_exit_to_the_protocol_owner() {
        let mut child = tokio::process::Command::new("/usr/bin/true")
            .spawn()
            .unwrap();
        let pid = child.id().unwrap();
        assert!(child.wait().await.unwrap().success());
        assert!(
            tokio::time::timeout(
                std::time::Duration::from_millis(350),
                super::monitor_rss(pid)
            )
            .await
            .is_err()
        );
    }
    use super::super::tests::{install, manifest, new_host};
    use super::*;
    use std::{sync::Arc, time::Instant};

    #[tokio::test]
    async fn worker_frames_keep_fragmented_and_adjacent_replies_separate() {
        let bytes = b"{\"one\":1}\n{\"two\":2}\n";
        let mut reader = BufReader::with_capacity(3, bytes.as_slice());
        assert_eq!(read_frame(&mut reader).await.unwrap(), b"{\"one\":1}\n");
        assert_eq!(read_frame(&mut reader).await.unwrap(), b"{\"two\":2}\n");
        assert_eq!(
            read_frame(&mut reader).await.unwrap_err().code,
            "worker_crashed"
        );
    }

    #[tokio::test]
    async fn worker_frames_enforce_the_delimiter_capacity_and_eof_contract() {
        let mut exact = vec![b'x'; MAX_FRAME - 1];
        exact.push(b'\n');
        let mut reader = BufReader::new(exact.as_slice());
        assert_eq!(read_frame(&mut reader).await.unwrap(), exact);
        let mut oversized = vec![b'x'; MAX_FRAME];
        oversized.push(b'\n');
        for (bytes, code) in [
            (oversized.as_slice(), "resource_limit"),
            (b"partial".as_slice(), "worker_crashed"),
            (b"".as_slice(), "worker_crashed"),
        ] {
            let mut reader = BufReader::new(bytes);
            assert_eq!(read_frame(&mut reader).await.unwrap_err().code, code);
        }
    }

    async fn enabled(root: &std::path::Path) -> (Arc<Manager>, Status) {
        let manager = new_host(root);
        let installed = install(&manager, root, &manifest());
        let status = manager
            .change(StateRequest {
                plugin_id: installed.manifest.id,
                digest: installed.digest,
                action: StateAction::Enable,
            })
            .await
            .unwrap();
        (manager, status)
    }
    fn cached(selected: &Selection, snapshot: &str) -> Cached {
        Cached {
            observed_at: "2026-10-27T00:00:00Z".into(),
            report: None,
            collected_at: Instant::now(),
            digest: selected.digest.clone(),
            epoch: selected.epoch.to_string(),
            revision: selected.revision.to_string(),
            snapshot: snapshot.into(),
            sessions: vec![],
            source_revision: None,
            quota: None,
        }
    }

    #[tokio::test]
    async fn failed_status_commit_keeps_the_last_complete_cache_and_revocation_cannot_republish() {
        let temp = tempfile::tempdir().unwrap();
        let (manager, status) = enabled(temp.path()).await;
        let id = &status.manifest.id;
        let selected = manager.selection(id).unwrap();
        manager
            .cache
            .lock()
            .unwrap()
            .data
            .insert(id.clone(), cached(&selected, "old").into());
        manager.db.lock().unwrap().execute_batch("CREATE TRIGGER reject_refresh BEFORE UPDATE ON selections BEGIN SELECT RAISE(ABORT,'fixture'); END;").unwrap();
        assert_eq!(
            manager
                .publish_refresh(id, &selected, cached(&selected, "new"))
                .unwrap_err()
                .code,
            "unavailable"
        );
        assert_eq!(manager.cache.lock().unwrap().data[id].snapshot, "old");
        manager
            .db
            .lock()
            .unwrap()
            .execute_batch("DROP TRIGGER reject_refresh")
            .unwrap();
        manager
            .publish_refresh(id, &selected, cached(&selected, "new"))
            .unwrap();
        assert_eq!(manager.cache.lock().unwrap().data[id].snapshot, "new");
        manager.revoke(id, &status.digest, false).unwrap();
        assert_eq!(
            manager
                .publish_refresh(id, &selected, cached(&selected, "late"))
                .unwrap_err()
                .code,
            "grant_revoked"
        );
        assert!(!manager.cache.lock().unwrap().data.contains_key(id));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn authorization_wait_yields_and_shutdown_invalidates_the_result_before_delivery() {
        let temp = tempfile::tempdir().unwrap();
        let (manager, status) = enabled(temp.path()).await;
        let installed = manager
            .verified(&status.manifest.id, &status.digest)
            .unwrap();
        let selected = manager.selection(&status.manifest.id).unwrap();
        let context = manager.context(&installed, &selected, false, None).unwrap();
        let (entered, ready) = tokio::sync::oneshot::channel();
        let (release, gate) = std::sync::mpsc::channel();
        let holder = std::thread::spawn({
            let manager = manager.clone();
            move || {
                let _guard = manager.db.lock().unwrap();
                entered.send(()).unwrap();
                let _ = gate.recv_timeout(Duration::from_secs(4));
            }
        });
        ready.await.unwrap();
        let began = Instant::now();
        let request = tokio::spawn({
            let manager = manager.clone();
            async move { manager.alive_async(&context).await }
        });
        tokio::time::sleep(Duration::from_millis(20)).await;
        let elapsed = began.elapsed();
        manager.stop();
        release.send(()).unwrap();
        holder.join().unwrap();
        assert_eq!(request.await.unwrap().unwrap_err().code, "plugin_disabled");
        assert!(elapsed < Duration::from_secs(1));
    }
}
