//! One bounded worker lifecycle per job. No optional-store mutex crosses an await.
use super::super::{Checked, Manager, Result, fail, package as legacy, worker::RpcRequest};
use super::{
    package,
    store::{Grant, Installed, Lease, Selection},
};
use local_agent_core::{
    domain as d,
    plugin::{external as common, full as f},
};
use serde_json::{Value, json};
use std::{collections::BTreeSet, path::PathBuf, sync::Arc, time::Duration};
use tokio::io::{AsyncWriteExt, BufReader};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct FileStamp {
    device: u64,
    inode: u64,
    bytes: u64,
    modified: i64,
    modified_ns: i64,
    changed: i64,
    changed_ns: i64,
}
impl FileStamp {
    pub(super) fn expired(&self, seconds: u32) -> bool {
        let now = chrono::Utc::now().timestamp();
        self.modified > now.saturating_add(2)
            || now.saturating_sub(self.modified) > i64::from(seconds)
    }
}
pub(super) fn stamp(grant: &Grant) -> Result<FileStamp> {
    use std::os::unix::fs::MetadataExt;
    let m = std::fs::symlink_metadata(&grant.path).checked("permission_denied")?;
    if !m.is_file()
        || m.file_type().is_symlink()
        || m.nlink() != 1
        || m.dev() != grant.device
        || m.ino() != grant.inode
        || m.len() > common::MAX_SOURCE
    {
        return Err(fail("grant_revoked"));
    }
    Ok(FileStamp {
        device: m.dev(),
        inode: m.ino(),
        bytes: m.len(),
        modified: m.mtime(),
        modified_ns: m.mtime_nsec(),
        changed: m.ctime(),
        changed_ns: m.ctime_nsec(),
    })
}
#[derive(Clone)]
pub(super) struct SourceData {
    pub text: Arc<String>,
    pub revision: String,
    pub stamp: FileStamp,
    pub grant: Grant,
}
pub(super) fn load_source(grant: &Grant) -> Result<SourceData> {
    let before = stamp(grant)?;
    let bytes =
        legacy::read_granted_file(&grant.path, common::MAX_SOURCE, grant.device, grant.inode)?;
    if before != stamp(grant)? {
        return Err(fail("stale_snapshot"));
    }
    let revision = legacy::hash(&bytes);
    Ok(SourceData {
        text: Arc::new(String::from_utf8(bytes).checked("invalid_input")?),
        revision,
        stamp: before,
        grant: grant.clone(),
    })
}
/// Complete transport and complete historical coverage are separate facts.
pub(super) fn verify_live_frame(source: &SourceData, published_records: usize) -> Result<()> {
    let mut lines = source.text.lines();
    let header = lines
        .next()
        .filter(|v| v.len() <= 256)
        .ok_or(fail("stale_snapshot"))?;
    let footer = lines
        .next_back()
        .filter(|v| v.len() <= 128)
        .ok_or(fail("stale_snapshot"))?;
    let head: Value = serde_json::from_str(header).checked("stale_snapshot")?;
    let tail: Value = serde_json::from_str(footer).checked("stale_snapshot")?;
    if !source.text.ends_with('\n')
        || head["snapshot"]
            .as_str()
            .is_none_or(|id| id.is_empty() || id.len() > 64)
        || tail["commit"] != head["snapshot"]
        || head["records"].as_u64() != Some(published_records as u64)
        || lines.count() != published_records
    {
        return Err(fail("stale_snapshot"));
    }
    Ok(())
}
#[derive(Clone)]
pub(super) struct Context {
    pub package: Installed,
    pub path: PathBuf,
    pub before: Selection,
    pub lease: Lease,
    pub preflight: bool,
    pub id: String,
    pub resource: String,
    pub source: Option<SourceData>,
}
impl Context {
    fn invocation(&self) -> Result<d::Invocation> {
        let source = self
            .package
            .manifest
            .source
            .as_ref()
            .ok_or(fail("invalid_record"))?;
        Ok(d::Invocation {
            invocation_id: self.id.clone(),
            provider_key: format!(
                "{}:domain.session-source@1:{}",
                self.package.manifest.id, source.id
            ),
            implementation_identity: self.package.digest.clone(),
            epoch: self.lease.epoch.into(),
            grant_revision: self.lease.revision.into(),
            catalog_revision: 1u64.into(),
            resource_ref: self.resource.clone(),
            deadline_ms: Some(30_000u64.into()),
            budget: d::Budget {
                batch_items: 256,
                max_records: 4096u64.into(),
                max_document_bytes: 10_485_760u64.into(),
            },
        })
    }
}
pub(super) enum Job {
    Preflight,
    Scan,
    Conversation {
        native_id: String,
        cursor: Option<String>,
        anchor: Option<String>,
        source_revision: String,
    },
}
pub(super) enum Output {
    Ready,
    Facts(d::facts::FactSnapshot),
    Conversation(d::ConversationPage),
}
impl Manager {
    pub(super) fn full_verified(&self, q: &f::PackageQuery) -> Result<(Installed, PathBuf)> {
        self.ensure_running()?;
        let (p, path) = {
            let host = self.full.lock().checked("unavailable")?;
            let p = host.installed(q)?;
            let path = host.package_path(&p);
            (p, path)
        };
        let checked = package::inspect(&legacy::read_file(&path, legacy::MAX_ARCHIVE)?)?;
        if checked.digest != p.digest
            || checked.manifest != p.manifest
            || checked.signing_key != p.signing_key
        {
            return Err(fail("signature_invalid"));
        }
        if !self.full_trusted(&p)? {
            return Err(fail("publisher_untrusted"));
        }
        Ok((p, path))
    }
    pub(super) fn full_check(&self, c: &Context) -> Result<()> {
        self.ensure_running()?;
        if !self.full_trusted(&c.package)? {
            return Err(fail("publisher_untrusted"));
        }
        if self.full_publisher_revision(&c.lease.publisher)? != c.lease.publisher_revision {
            return Err(fail("grant_revoked"));
        }
        let legacy_enabled: bool = self
            .db
            .lock()?
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM selections WHERE plugin_id=?1 AND enabled=1)",
                [&c.lease.id],
                |r| r.get(0),
            )
            .checked("unavailable")?;
        if legacy_enabled {
            return Err(fail("capability_conflict"));
        }
        {
            let host = self.full.lock().checked("unavailable")?;
            let s = host.selection(&c.lease.id)?;
            if c.preflight {
                if s != c.before {
                    return Err(fail("grant_revoked"));
                }
            } else if !s.enabled
                || s.digest != c.lease.digest
                || s.epoch != c.lease.epoch
                || s.revision != c.lease.revision
                || host.active.get(&c.lease.id) != Some(&c.lease)
            {
                return Err(fail("grant_revoked"));
            }
            if c.source.as_ref().is_some_and(|_| {
                host.grants
                    .get(&c.lease.id)
                    .is_none_or(|g| g.lease != c.lease)
            }) {
                return Err(fail("grant_revoked"));
            }
        }
        if !c.preflight {
            let marker = self
                .root
                .with_file_name("plugin-sources")
                .join("selected")
                .join(&c.lease.id);
            if !legacy::read_file(&marker, 64).is_ok_and(|bytes| bytes == c.lease.digest.as_bytes())
            {
                return Err(fail("grant_revoked"));
            }
        }
        if let Some(source) = &c.source {
            let current = stamp(&source.grant)?;
            // A live publisher may finish a new frame while its previous
            // immutable capture is being decoded. The file identity and
            // grant still have to match throughout the operation.
            let live = c
                .package
                .manifest
                .source
                .as_ref()
                .is_some_and(|s| s.max_age_seconds.is_some());
            if !live && current != source.stamp {
                return Err(fail("stale_snapshot"));
            }
        }
        Ok(())
    }
    async fn full_alive(self: &Arc<Self>, c: &Context) -> Result<()> {
        let manager = self.clone();
        let c = c.clone();
        super::super::run_sync(move || manager.full_check(&c)).await
    }
    pub(super) async fn full_run(self: &Arc<Self>, c: Context, job: Job) -> Result<Output> {
        let _permit = self.workers.try_acquire().checked("resource_limit")?;
        self.full_alive(&c).await?;
        let mut child =
            tokio::process::Command::new(std::env::current_exe().checked("unavailable")?)
                .arg("--plugin-worker")
                .arg(&c.path)
                .arg(&c.package.digest)
                .env_clear()
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::null())
                .kill_on_drop(true)
                .spawn()
                .checked("worker_crashed")?;
        let pid = child.id().ok_or(fail("worker_crashed"))?;
        let mut client = Client {
            input: child.stdin.take().ok_or(fail("worker_crashed"))?,
            output: BufReader::new(child.stdout.take().ok_or(fail("worker_crashed"))?),
            sequence: 0,
            broker_sequence: 0,
        };
        let work = async {
            let init = common::Initialize {
                api_version: f::API_VERSION.into(),
                capabilities: c.package.manifest.capabilities.clone(),
                package_digest: c.package.digest.clone(),
                activation_epoch: c.lease.epoch.to_string(),
                grant_revision: c.lease.revision.to_string(),
                context_id: c.id.clone(),
                fuel: "100000000".into(),
                max_memory_bytes: "134217728".into(),
                settings: Default::default(),
            };
            let init = serde_json::to_value(init).checked("invalid_input")?;
            if client
                .exchange(self, &c, "plugin.initialize", init.clone())
                .await?
                != init
            {
                return Err(fail("incompatible_api"));
            }
            for method in ["plugin.prepare", "plugin.activate", "plugin.health"] {
                if client.exchange(self, &c, method, json!({})).await? != json!({"ready":true}) {
                    return Err(fail("invalid_record"));
                }
            }
            let result = match job {
                Job::Preflight => Output::Ready,
                Job::Scan => {
                    let source = c.source.as_ref().ok_or(fail("permission_denied"))?;
                    let identity = d::facts::FactIdentity {
                        provider_key: c.invocation()?.provider_key,
                        source_id: c
                            .package
                            .manifest
                            .source_id()
                            .ok_or(fail("invalid_record"))?,
                        implementation_identity: c.package.digest.clone(),
                        epoch: c.lease.epoch.into(),
                        grant_revision: c.lease.revision.into(),
                        observed_at: chrono::Utc::now().to_rfc3339(),
                        evidence: d::facts::EvidenceLevel::SourceReported,
                    };
                    let mut publication = d::facts::publication::FactPublication::new(
                        identity,
                        c.package.manifest.product_ids(),
                        c.package.manifest.fact_kinds(),
                    )
                    .map_err(|_| fail("invalid_record"))?;
                    let mut cursor = None;
                    let mut cursors = BTreeSet::new();
                    let mut completed = None;
                    for _ in 0..32 {
                        let request = f::ScanRequest {
                            invocation: c.invocation()?,
                            source_id: c
                                .package
                                .manifest
                                .source_id()
                                .ok_or(fail("invalid_record"))?,
                            source_revision: source.revision.clone(),
                            cursor: cursor.clone(),
                        };
                        let page: f::ScanPage = serde_json::from_value(
                            client
                                .exchange(
                                    self,
                                    &c,
                                    "domain.scan",
                                    serde_json::to_value(request).checked("invalid_input")?,
                                )
                                .await?,
                        )
                        .checked("invalid_record")?;
                        if let Some(batch) = page.batch {
                            if batch.source_revision != source.revision {
                                return Err(fail("stale_snapshot"));
                            }
                            publication.accept(batch).map_err(|e| fact_error(e.code))?;
                        } else if page.next_cursor.is_some() {
                            return Err(fail("invalid_record"));
                        }
                        if let Some(checkpoint) = page.checkpoint {
                            if page.next_cursor.is_some()
                                || checkpoint.source_revision != source.revision
                            {
                                return Err(fail("stale_snapshot"));
                            }
                            completed = Some(
                                publication
                                    .finish(&checkpoint)
                                    .map_err(|e| fact_error(e.code))?,
                            );
                            break;
                        }
                        let next = page.next_cursor.ok_or(fail("invalid_record"))?;
                        if next.is_empty() || next.len() > 256 || !cursors.insert(next.clone()) {
                            return Err(fail("invalid_record"));
                        }
                        cursor = Some(next);
                    }
                    Output::Facts(completed.ok_or(fail("resource_limit"))?)
                }
                Job::Conversation {
                    native_id,
                    cursor,
                    anchor,
                    source_revision,
                } => {
                    let source = c.source.as_ref().ok_or(fail("permission_denied"))?;
                    if source.revision != source_revision {
                        return Err(fail("stale_snapshot"));
                    }
                    let request = f::ConversationRequest {
                        invocation: c.invocation()?,
                        source_id: c
                            .package
                            .manifest
                            .source_id()
                            .ok_or(fail("invalid_record"))?,
                        source_revision,
                        native_id: native_id.clone(),
                        cursor: cursor.clone(),
                        anchor,
                    };
                    let page: d::ConversationPage = serde_json::from_value(
                        client
                            .exchange(
                                self,
                                &c,
                                "domain.read-conversation",
                                serde_json::to_value(request).checked("invalid_input")?,
                            )
                            .await?,
                    )
                    .checked("invalid_record")?;
                    if page.session_id != native_id
                        || page.events.len() > 40
                        || page.complete != page.next_cursor.is_none()
                        || page.next_cursor.as_ref().is_some_and(|v| {
                            v.is_empty() || v.len() > 256 || Some(v) == cursor.as_ref()
                        })
                        || (page.events.is_empty() && page.next_cursor.is_some())
                        || page
                            .events
                            .iter()
                            .any(|e| e.native_id.is_empty() || e.native_id.len() > 256)
                        || page
                            .events
                            .iter()
                            .map(|e| &e.native_id)
                            .collect::<BTreeSet<_>>()
                            .len()
                            != page.events.len()
                    {
                        return Err(fail("invalid_record"));
                    }
                    Output::Conversation(page)
                }
            };
            for method in ["plugin.deactivate", "plugin.dispose"] {
                if client.exchange(self, &c, method, json!({})).await? != json!({"ready":true}) {
                    return Err(fail("invalid_record"));
                }
            }
            self.full_alive(&c).await?;
            Ok(result)
        };
        let monitored = async {
            tokio::select! {result=work=>result,result=super::super::broker::monitor_rss(pid)=>Err(result.err().unwrap_or(fail("worker_crashed")))}
        };
        let result = tokio::time::timeout(Duration::from_secs(30), monitored)
            .await
            .checked("deadline_exceeded")
            .and_then(|r| r);
        drop(client);
        if result.is_err() {
            let _ = child.kill().await;
        }
        let exited = tokio::time::timeout(Duration::from_secs(1), child.wait()).await;
        if !matches!(exited,Ok(Ok(status))if status.success()) && result.is_ok() {
            let _ = child.kill().await;
            return Err(fail("worker_crashed"));
        }
        result
    }
}
fn fact_error(code: d::ErrorCode) -> super::super::Error {
    fail(match code {
        d::ErrorCode::ResourceLimit => "resource_limit",
        d::ErrorCode::StaleSnapshot => "stale_snapshot",
        d::ErrorCode::Unavailable => "unavailable",
        _ => "invalid_record",
    })
}
struct Client {
    input: tokio::process::ChildStdin,
    output: BufReader<tokio::process::ChildStdout>,
    sequence: u32,
    broker_sequence: u32,
}
impl Client {
    async fn exchange(
        &mut self,
        m: &Arc<Manager>,
        c: &Context,
        method: &str,
        params: Value,
    ) -> Result<Value> {
        m.full_alive(c).await?;
        self.sequence += 1;
        let id = format!("host-{}", self.sequence);
        let mut request = serde_json::to_vec(&RpcRequest {
            jsonrpc: "2.0".into(),
            id: id.clone(),
            method: method.into(),
            params,
        })
        .checked("invalid_input")?;
        request.push(b'\n');
        if request.len() > common::MAX_FRAME {
            return Err(fail("resource_limit"));
        }
        self.input
            .write_all(&request)
            .await
            .checked("worker_crashed")?;
        self.input.flush().await.checked("worker_crashed")?;
        loop {
            let pending = super::super::broker::read_frame(&mut self.output);
            tokio::pin!(pending);
            let bytes = loop {
                tokio::select! {r=&mut pending=>break r?,_=tokio::time::sleep(Duration::from_millis(50))=>m.full_alive(c).await?,}
            };
            let value: Value = serde_json::from_slice(&bytes).checked("worker_crashed")?;
            if value["jsonrpc"] != "2.0" {
                return Err(fail("worker_crashed"));
            }
            if value["method"] == "host.call" {
                let request: RpcRequest =
                    serde_json::from_value(value).checked("invalid_record")?;
                self.broker_sequence += 1;
                if request.id != format!("runner-{}", self.broker_sequence)
                    || self.broker_sequence > 128
                    || request.params["contextId"] != c.id
                    || request.params["method"] != "source.read"
                    || c.preflight
                {
                    return Err(fail("permission_denied"));
                }
                let chunk: common::ReadChunk = serde_json::from_str(
                    request.params["payload"]
                        .as_str()
                        .ok_or(fail("invalid_record"))?,
                )
                .checked("invalid_record")?;
                let source = c.source.as_ref().ok_or(fail("permission_denied"))?;
                let offset = super::super::broker::decimal_u64(&chunk.offset)? as usize;
                if chunk.context_id != c.id
                    || chunk.source_ref != c.resource
                    || chunk.max_bytes == 0
                    || chunk.max_bytes as usize > common::MAX_CHUNK
                    || offset > source.text.len()
                    || !source.text.is_char_boundary(offset)
                {
                    return Err(fail("permission_denied"));
                }
                let mut end = offset
                    .saturating_add(chunk.max_bytes as usize)
                    .min(source.text.len());
                while end > offset && !source.text.is_char_boundary(end) {
                    end -= 1;
                }
                if end == offset && offset < source.text.len() {
                    return Err(fail("resource_limit"));
                }
                let chunk = common::SourceChunk {
                    text: source.text[offset..end].into(),
                    next_offset: end.to_string(),
                    complete: end == source.text.len(),
                    source_revision: source.revision.clone(),
                };
                let body = serde_json::to_string(&chunk).checked("invalid_record")?;
                let mut frame =
                    serde_json::to_vec(&json!({"jsonrpc":"2.0","id":request.id,"result":body}))
                        .checked("invalid_record")?;
                frame.push(b'\n');
                if frame.len() > common::MAX_FRAME {
                    return Err(fail("resource_limit"));
                }
                m.full_alive(c).await?;
                self.input
                    .write_all(&frame)
                    .await
                    .checked("worker_crashed")?;
                self.input.flush().await.checked("worker_crashed")?;
                continue;
            }
            if value["id"] != id || value.get("result").is_some() == value.get("error").is_some() {
                return Err(fail("worker_crashed"));
            }
            if value.get("error").is_some() {
                return Err(fail(match value["error"]["data"]["code"].as_str() {
                    Some("resource_limit") => "resource_limit",
                    Some("permission_denied") => "permission_denied",
                    Some("stale_snapshot") => "stale_snapshot",
                    _ => "worker_crashed",
                }));
            }
            m.full_alive(c).await?;
            return Ok(value["result"].clone());
        }
    }
}
