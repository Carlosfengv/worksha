//! Scheduled export reads committed telemetry and publishes only an explicit projection.
use std::{
    ffi::CString,
    fs::File,
    io::{Read, Write},
    os::fd::{AsRawFd, FromRawFd},
    os::unix::fs::MetadataExt,
    path::PathBuf,
    time::Duration,
};

use chrono::{DateTime, Utc};
use local_agent_core::{
    observation::content_epoch,
    plugin::platform::{LogFilter, LogFormat, RotatePolicy, SinkStatus},
};
use rusqlite::{Connection, OpenFlags, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};

use crate::AppState;

use super::{
    Checked, PlatformStore, Result, Selection, directory, fail, open_store, package, run_sync,
};

const MAX_FILE: usize = 512 * 1024;
const PAGE: i64 = 64;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Watermark {
    timestamp: String,
    id: String,
    sequence: i64,
    retention_floor: i64,
    epoch: Option<String>,
}
struct Grant {
    path: PathBuf,
    device: i64,
    inode: i64,
    format: LogFormat,
    status: SinkStatus,
    watermark: Watermark,
    legacy: Option<(String, String)>,
}

impl Grant {
    fn open_directory(&self) -> Result<File> {
        let directory = directory::open(&self.path)?;
        let metadata = directory.metadata().checked("permission_denied")?;
        if metadata.dev() as i64 != self.device || metadata.ino() as i64 != self.inode {
            return Err(fail("grant_revoked"));
        }
        Ok(directory)
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SanitizedGatewayLogRecord {
    timestamp: String,
    request_hash: String,
    agent_hash: String,
    model_hash: Option<String>,
    client_scope: &'static str,
    outcome: &'static str,
    http_status: Option<u16>,
    duration_ms: Option<f64>,
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Projection {
    agent_id: String,
    model: Option<String>,
    lan: Option<i64>,
    status: String,
    http_status: Option<u16>,
    duration_ms: Option<f64>,
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
}

impl SanitizedGatewayLogRecord {
    fn from_row(
        id: &str,
        row: Projection,
        time: DateTime<Utc>,
        filter: &LogFilter,
    ) -> Option<Self> {
        let client_scope = if row.lan == Some(1) { "lan" } else { "local" };
        let outcome = match row.status.as_str() {
            "success" => "success",
            "error" => "error",
            "cancelled" => "cancelled",
            "interrupted" => "interrupted",
            _ => "unknown",
        };
        let model = row.model.as_deref();
        if filter
            .client_scope
            .as_deref()
            .is_some_and(|value| value != client_scope)
            || filter
                .outcome
                .as_deref()
                .is_some_and(|value| value != outcome)
            || filter
                .model
                .as_deref()
                .is_some_and(|value| Some(value) != model)
            || filter
                .agent
                .as_deref()
                .is_some_and(|value| value != row.agent_id)
        {
            return None;
        }
        Some(Self {
            timestamp: time.to_rfc3339(),
            request_hash: package::hash(id.as_bytes()),
            agent_hash: package::hash(row.agent_id.as_bytes()),
            model_hash: model.map(|model| package::hash(model.as_bytes())),
            client_scope,
            outcome,
            http_status: row.http_status,
            duration_ms: row.duration_ms.filter(|ms| ms.is_finite() && *ms >= 0.0),
            input_tokens: row.input_tokens,
            output_tokens: row.output_tokens,
        })
    }

    fn encode(&self, format: LogFormat, buffer: &mut Vec<u8>) -> Result<()> {
        match format {
            LogFormat::Jsonl => {
                serde_json::to_writer(&mut *buffer, self).checked("invalid_record")?
            }
            LogFormat::Csv => {
                let option =
                    |value: Option<u64>| value.map(|value| value.to_string()).unwrap_or_default();
                writeln!(
                    buffer,
                    "{},{},{},{},{},{},{},{},{},{}",
                    self.timestamp,
                    self.request_hash,
                    self.agent_hash,
                    self.model_hash.as_deref().unwrap_or_default(),
                    self.client_scope,
                    self.outcome,
                    option(self.http_status.map(u64::from)),
                    self.duration_ms
                        .map(|value| value.to_string())
                        .unwrap_or_default(),
                    option(self.input_tokens),
                    option(self.output_tokens),
                )
                .checked("unavailable")?;
                return Ok(());
            }
            LogFormat::Wasm => return Err(fail("unsupported")),
        }
        buffer.push(b'\n');
        Ok(())
    }
}

fn grant(db: &Connection, plugin: &str, selected: &Selection) -> Result<Option<Grant>> {
    let raw = db
        .query_row(
            "SELECT g.dir_path, g.device, g.inode, g.format, g.rotate_policy, g.filter_json,
                g.quota_bytes, g.written_bytes, c.last_timestamp, c.last_request_id,
                COALESCE(c.exported_rows, 0), c.last_flushed_at, c.last_error
         FROM ext_sink_grants g LEFT JOIN ext_sink_cursors c USING (plugin_id)
         WHERE g.plugin_id = ?1 AND g.digest = ?2 AND g.host_revision = ?3 AND g.enabled = 1",
            params![plugin, selected.digest, selected.revision.to_string()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, i64>(7)?,
                    row.get::<_, Option<String>>(8)?,
                    row.get::<_, Option<String>>(9)?,
                    row.get::<_, i64>(10)?,
                    row.get::<_, Option<String>>(11)?,
                    row.get::<_, Option<String>>(12)?,
                ))
            },
        )
        .optional()
        .checked("invalid_record")?;
    let (progress, legacy) = db.query_row(
        "SELECT last_seq,retention_floor,epoch,legacy_time,legacy_id FROM ext_sink_progress WHERE plugin_id=?1",
        [plugin],|row| {
            let time:Option<String> = row.get(3)?;
            let id:Option<String> = row.get(4)?;
            Ok((Watermark { sequence:row.get(0)?, retention_floor:row.get(1)?, epoch:row.get(2)?, ..Default::default() },time.zip(id)))
        }).optional().checked("invalid_record")?.unwrap_or_default();
    raw.map(
        |(
            path,
            device,
            inode,
            format,
            rotate,
            filter,
            quota,
            used,
            timestamp,
            id,
            count,
            last,
            error,
        )| {
            if timestamp.is_some() != id.is_some() {
                return Err(fail("invalid_record"));
            }
            Ok(Grant {
                path: path.into(),
                device,
                inode,
                format: serde_json::from_value(serde_json::Value::String(format))
                    .checked("invalid_record")?,
                status: SinkStatus {
                    legacy_coverage_unknown: legacy.is_some(),
                    exported_rows: count.try_into().checked("invalid_record")?,
                    written_bytes: used.try_into().checked("invalid_record")?,
                    quota_bytes: quota.try_into().checked("invalid_record")?,
                    last_flushed_at: last,
                    last_error: error,
                    filter: serde_json::from_str(&filter).checked("invalid_record")?,
                    rotate_policy: serde_json::from_value(serde_json::Value::String(rotate))
                        .checked("invalid_record")?,
                },
                legacy,
                watermark: Watermark {
                    timestamp: timestamp.unwrap_or_default(),
                    id: id.unwrap_or_default(),
                    ..progress
                },
            })
        },
    )
    .transpose()
}

pub(super) fn status(
    db: &Connection,
    plugin: &str,
    selection: &Selection,
) -> Result<Option<SinkStatus>> {
    Ok(grant(db, plugin, selection)?.map(|grant| grant.status))
}

pub(super) fn configure(
    db: &Connection,
    plugin: &str,
    selected: &Selection,
    filter: LogFilter,
    rotate: RotatePolicy,
) -> Result<()> {
    if filter
        .client_scope
        .as_deref()
        .is_some_and(|scope| !matches!(scope, "local" | "lan"))
        || filter.outcome.as_deref().is_some_and(|outcome| {
            !matches!(
                outcome,
                "success" | "error" | "cancelled" | "interrupted" | "unknown"
            )
        })
        || [&filter.model, &filter.agent]
            .into_iter()
            .flatten()
            .any(|value| !super::valid_object_id(value))
    {
        return Err(fail("invalid_input"));
    }
    if pending(db, plugin)? {
        return Err(fail("busy"));
    }
    let rotation = serde_json::to_value(rotate).checked("invalid_input")?;
    let changed = db
        .execute(
            "UPDATE ext_sink_grants SET rotate_policy = ?4, filter_json = ?5
            WHERE plugin_id = ?1 AND digest = ?2 AND host_revision = ?3 AND enabled = 1",
            params![
                plugin,
                selected.digest,
                selected.revision.to_string(),
                rotation.as_str(),
                serde_json::to_string(&filter).checked("invalid_input")?
            ],
        )
        .checked("unavailable")?;
    if changed == 1 {
        Ok(())
    } else {
        Err(fail("permission_denied"))
    }
}

struct Batch {
    end: Watermark,
    records: Vec<SanitizedGatewayLogRecord>,
    window: String,
}

fn observation_db(state: &AppState, access: OpenFlags) -> Result<Connection> {
    let path = state
        .plugins
        .root
        .parent()
        .ok_or(fail("unavailable"))?
        .join("observation/observations.sqlite3");
    let db = Connection::open_with_flags(path, access | OpenFlags::SQLITE_OPEN_NOFOLLOW)
        .checked("unavailable")?;
    db.busy_timeout(Duration::from_millis(100))
        .checked("unavailable")?;
    Ok(db)
}

fn check_feed(db: &Connection, grant: &Grant) -> Result<(String, i64)> {
    let epoch = content_epoch::read(db).checked("invalid_record")?;
    let deleted: i64 = db
        .query_row(
            "SELECT last_deleted FROM observation_export_retention WHERE id=1",
            [],
            |r| r.get(0),
        )
        .checked("invalid_record")?;
    if let Some(expected) = &grant.watermark.epoch
        && (epoch.as_str() != expected
            || deleted
                > grant
                    .watermark
                    .sequence
                    .max(grant.watermark.retention_floor))
    {
        return Err(fail("source_changed"));
    }
    Ok((epoch.as_str().to_owned(), deleted))
}

fn read_batch(state: &AppState, grant: &Grant) -> Result<Batch> {
    read_page(
        &mut observation_db(state, OpenFlags::SQLITE_OPEN_READ_ONLY)?,
        grant,
    )
}

fn read_page(db: &mut Connection, grant: &Grant) -> Result<Batch> {
    let read = db.transaction().checked("unavailable")?;
    let (epoch, retention_floor) = check_feed(&read, grant)?;
    // Migrated baseline rows were inserted in old cursor order. Cross this
    // boundary once. Every subsequent live terminal row is eligible by seq.
    let mut after = grant.watermark.sequence;
    if grant.watermark.epoch.is_none()
        && let Some((time, id)) = &grant.legacy
    {
        after = read.query_row("SELECT coalesce(max(seq),0) FROM observation_export_feed WHERE baseline=1 AND (started_at,request_id)<=(?1,?2)",params![time,id],|r|r.get(0)).checked("invalid_record")?;
    }
    let mut stmt=read.prepare("SELECT seq,started_at,request_id,projection FROM observation_export_feed WHERE seq>?1 ORDER BY seq LIMIT ?2").checked("unavailable")?;
    let mut rows = stmt.query(params![after, PAGE]).checked("unavailable")?;
    let mut batch = Batch {
        end: Watermark {
            epoch: Some(epoch),
            sequence: after,
            retention_floor,
            ..grant.watermark.clone()
        },
        records: Vec::new(),
        window: String::new(),
    };
    let pattern = match grant.status.rotate_policy {
        RotatePolicy::Daily => "%Y-%m-%d",
        RotatePolicy::Hourly => "%Y-%m-%dT%H",
    };
    while let Some(row) = rows.next().checked("unavailable")? {
        let seq: i64 = row.get(0).checked("invalid_record")?;
        let started: String = row.get(1).checked("invalid_record")?;
        let id: String = row.get(2).checked("invalid_record")?;
        let projection: String = row.get(3).checked("invalid_record")?;
        let projection: Projection = serde_json::from_str(&projection).checked("invalid_record")?;
        let time = DateTime::parse_from_rfc3339(&started)
            .checked("invalid_record")?
            .with_timezone(&Utc);
        let window = time.format(pattern).to_string();
        if !batch.window.is_empty() && batch.window != window {
            break;
        }
        batch.window = window;
        batch.end.sequence = seq;
        batch.end.timestamp = started;
        batch.end.id = id;
        if let Some(record) = SanitizedGatewayLogRecord::from_row(
            &batch.end.id,
            projection,
            time,
            &grant.status.filter,
        ) {
            batch.records.push(record);
        }
    }
    Ok(batch)
}

struct Export {
    state: AppState,
    plugin: String,
    selection: Selection,
    revision: String,
    grant: Grant,
    batch: Batch,
}

pub(super) async fn flush(
    state: AppState,
    plugin: String,
    digest: String,
    revision: Option<String>,
) -> Result<SinkStatus> {
    let _permit = export_capacity().try_acquire().checked("busy")?;
    let exporter = state.clone();
    let id = plugin.clone();
    let initial = run_sync(move || {
        let (selected, definition) = exporter.plugins.active_platform(&id, &digest)?;
        definition.log_sink.ok_or(fail("not_found"))?;
        let store = open_store(&exporter)?;
        let current_revision = PlatformStore::revision(&store.0, &id, selected.revision)?;
        if revision
            .as_ref()
            .is_some_and(|revision| *revision != current_revision)
        {
            return Err(fail("grant_revoked"));
        }
        let grant = grant(&store.0, &id, &selected)?.ok_or(fail("permission_denied"))?;
        Ok((selected, current_revision, grant, pending(&store.0, &id)?))
    })
    .await?;
    let (selection, revision, grant, recover_pending) = initial;
    let work_state = state.clone();
    let work_plugin = plugin.clone();
    let work_selection = selection.clone();
    let work_revision = revision.clone();
    let result = async move {
        if recover_pending {
            return run_sync(move || {
                publish(&work_state, &work_plugin, &work_selection, &work_revision)
            }).await;
        }
        let export = run_sync(move || {
            grant.open_directory()?;
            let batch = read_batch(&work_state, &grant)?;
            Ok(Export { state: work_state, plugin: work_plugin, selection: work_selection,
                revision: work_revision, grant, batch })
        }).await?;
        let mut bytes = Vec::new();
        if matches!(export.grant.format, LogFormat::Wasm) && !export.batch.records.is_empty() {
            #[derive(Deserialize)]
            #[serde(deny_unknown_fields)]
            struct Output { lines: Vec<String> }
            let output = export.state.plugins.transform_sink(
                export.plugin.clone(), export.selection.clone(),
                serde_json::json!({"records":export.batch.records}),
            ).await?;
            let output: Output = serde_json::from_value(output).checked("invalid_record")?;
            if output.lines.len() != export.batch.records.len() {
                return Err(fail("invalid_record"));
            }
            for line in output.lines {
                if line.is_empty() || line.contains(['\r', '\n', '\0']) || line.len() > 8 * 1024 {
                    return Err(fail("invalid_record"));
                }
                bytes.extend_from_slice(line.as_bytes());
                bytes.push(b'\n');
            }
        } else {
            if matches!(export.grant.format, LogFormat::Csv) && !export.batch.records.is_empty() {
                bytes.extend_from_slice(b"timestamp,request_hash,agent_hash,model_hash,client_scope,outcome,http_status,duration_ms,input_tokens,output_tokens\n");
            }
            for model in &export.batch.records {
                model.encode(export.grant.format, &mut bytes)?;
            }
        }
        if bytes.len() > MAX_FILE {
            return Err(fail("resource_limit"));
        }
        run_sync(move || export.commit(&bytes)).await
    }.await;
    if let Err(error) = &result {
        let code = error.code;
        let _ = run_sync(move || {
            state.plugins.ensure_platform_current(&plugin, &selection)?;
            let mut store = open_store(&state)?;
            let tx = store
                .0
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .checked("unavailable")?;
            if revision == PlatformStore::revision(&tx, &plugin, selection.revision)? {
                tx.execute(
                    "INSERT INTO ext_sink_cursors (plugin_id, last_error) VALUES (?1, ?2)
                    ON CONFLICT (plugin_id) DO UPDATE SET last_error = excluded.last_error",
                    params![plugin, code],
                )
                .checked("unavailable")?;
            }
            tx.commit().checked("unavailable")
        })
        .await;
    }
    result
}

impl Export {
    fn commit(self, bytes: &[u8]) -> Result<SinkStatus> {
        let mut store = open_store(&self.state)?;
        let tx = store
            .0
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .checked("unavailable")?;
        let revision = PlatformStore::revision(&tx, &self.plugin, self.selection.revision)?;
        if self.revision != revision {
            return Err(fail("grant_revoked"));
        }
        let _commit = self
            .state
            .plugins
            .platform_commit
            .lock()
            .checked("unavailable")?;
        self.state
            .plugins
            .ensure_platform_current(&self.plugin, &self.selection)?;
        let current =
            grant(&tx, &self.plugin, &self.selection)?.ok_or(fail("permission_denied"))?;
        if current.watermark != self.grant.watermark {
            return Err(fail("stale_snapshot"));
        }
        current.open_directory()?;
        if self.batch.end == current.watermark
            && current.status.last_error.is_none()
            && !pending(&tx, &self.plugin)?
        {
            return Ok(current.status);
        }
        let filename = {
            let part = package::hash(
                &serde_json::to_vec(&[
                    &self.plugin,
                    &self.selection.digest,
                    self.batch
                        .end
                        .epoch
                        .as_ref()
                        .ok_or(fail("invalid_record"))?,
                    &self.batch.end.sequence.to_string(),
                    &self.grant.watermark.sequence.to_string(),
                ])
                .checked("invalid_record")?,
            );
            let suffix = match current.format {
                LogFormat::Csv => "csv",
                LogFormat::Jsonl => "jsonl",
                LogFormat::Wasm => "log",
            };
            format!("gateway-{}-{}.{}", self.batch.window, &part[..32], suffix)
        };
        if !pending(&tx, &self.plugin)? {
            if current
                .status
                .written_bytes
                .saturating_add(bytes.len() as u64)
                > current.status.quota_bytes
            {
                return Err(fail("resource_limit"));
            }
            tx.execute("INSERT INTO ext_sink_pending VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)",params![
                self.plugin, revision, self.selection.digest,current.device,current.inode,self.batch.end.epoch,
                current.watermark.sequence,self.batch.end.sequence,self.batch.end.retention_floor,self.batch.end.timestamp,self.batch.end.id,filename,bytes,self.batch.records.len() as i64
            ]).checked("unavailable")?;
        }
        tx.commit().checked("unavailable")?;
        drop(_commit);
        publish(&self.state, &self.plugin, &self.selection, &revision)
    }
}

fn pending(db: &Connection, plugin: &str) -> Result<bool> {
    db.query_row(
        "SELECT EXISTS(SELECT 1 FROM ext_sink_pending WHERE plugin_id=?1)",
        [plugin],
        |r| r.get(0),
    )
    .checked("unavailable")
}

fn publish(
    state: &AppState,
    plugin: &str,
    selection: &Selection,
    revision: &str,
) -> Result<SinkStatus> {
    let mut store = open_store(state)?;
    let tx = store
        .0
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .checked("unavailable")?;
    let _commit = state
        .plugins
        .platform_commit
        .lock()
        .checked("unavailable")?;
    state.plugins.ensure_platform_current(plugin, selection)?;
    let current = grant(&tx, plugin, selection)?.ok_or(fail("permission_denied"))?;
    if revision != PlatformStore::revision(&tx, plugin, selection.revision)? {
        return Err(fail("grant_revoked"));
    }
    let mut observations = observation_db(state, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
    publish_batch(tx, &mut observations, plugin, selection, revision, current)
}

fn publish_batch(
    tx: rusqlite::Transaction<'_>,
    observations: &mut Connection,
    plugin: &str,
    selection: &Selection,
    revision: &str,
    current: Grant,
) -> Result<SinkStatus> {
    let directory = current.open_directory()?;
    if !pending(&tx, plugin)? {
        return Ok(current.status);
    }
    let (epoch,seq,floor,time,id,name,bytes,count): (String,i64,i64,String,String,String,Vec<u8>,i64) = tx.query_row(
            "SELECT epoch,end_seq,retention_floor,end_time,end_id,filename,payload,record_count FROM ext_sink_pending
            WHERE plugin_id=?1 AND digest=?2 AND revision=?3 AND device=?4 AND inode=?5 AND start_seq=?6",
            params![plugin,selection.digest,revision,current.device,current.inode,current.watermark.sequence],
            |r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?,r.get(7)?))).checked("grant_revoked")?;
    // Reserve the observation writer through file publication and the cursor
    // commit, so retention cannot revoke these rows between the two.
    let read = observations
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .checked("busy")?;
    let (actual_epoch, deleted) = check_feed(&read, &current)?;
    if actual_epoch != epoch || deleted > current.watermark.sequence.max(floor) {
        return Err(fail("source_changed"));
    }
    if !bytes.is_empty() {
        if current
            .status
            .written_bytes
            .saturating_add(bytes.len() as u64)
            > current.status.quota_bytes
        {
            return Err(fail("resource_limit"));
        }
        write_batch(&directory, &name, &bytes)?;
    }
    tx.execute("INSERT INTO ext_sink_progress(plugin_id,epoch,last_seq,retention_floor) VALUES(?1,?2,?3,?4)
            ON CONFLICT(plugin_id) DO UPDATE SET epoch=excluded.epoch,last_seq=excluded.last_seq,retention_floor=excluded.retention_floor",
            params![plugin,epoch,seq,floor]).checked("unavailable")?;
    tx.execute("DELETE FROM ext_sink_pending WHERE plugin_id=?1", [plugin])
        .checked("unavailable")?;
    tx.execute(
        "UPDATE ext_sink_grants SET written_bytes = written_bytes + ?2 WHERE plugin_id = ?1",
        params![plugin, bytes.len() as i64],
    )
    .checked("unavailable")?;
    tx.execute(
            "INSERT INTO ext_sink_cursors VALUES (?1, ?2, ?3, ?4, strftime('%Y-%m-%dT%H:%M:%fZ','now'), NULL)
             ON CONFLICT(plugin_id) DO UPDATE SET last_timestamp = excluded.last_timestamp, last_request_id = excluded.last_request_id,
                exported_rows = exported_rows + excluded.exported_rows, last_flushed_at = excluded.last_flushed_at, last_error = NULL",
            params![plugin, time, id, count],
        ).checked("unavailable")?;
    let status = status(&tx, plugin, selection)?.ok_or(fail("invalid_record"))?;
    tx.commit().checked("unavailable")?;
    Ok(status)
}

fn write_batch(directory: &File, name: &str, bytes: &[u8]) -> Result<()> {
    if !name.starts_with("gateway-")
        || name.len() > 100
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.'))
    {
        return Err(fail("invalid_record"));
    }
    let name = CString::new(name).checked("invalid_input")?;
    let temp =
        CString::new(format!(".gateway-{}.tmp", uuid::Uuid::new_v4())).checked("invalid_input")?;
    let fd = directory.as_raw_fd();
    let file = unsafe {
        libc::openat(
            fd,
            temp.as_ptr(),
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            0o600,
        )
    };
    if file < 0 {
        return Err(fail("unavailable"));
    }
    let mut file = unsafe { File::from_raw_fd(file) };
    let result = (|| {
        file.write_all(bytes).checked("unavailable")?;
        file.sync_all().checked("unavailable")?;
        // linkat publishes the complete file without replacing an existing name.
        if unsafe { libc::linkat(fd, temp.as_ptr(), fd, name.as_ptr(), 0) } < 0 {
            if std::io::Error::last_os_error().kind() != std::io::ErrorKind::AlreadyExists {
                return Err(fail("unavailable"));
            }
            let existing = unsafe {
                libc::openat(
                    fd,
                    name.as_ptr(),
                    libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
                )
            };
            if existing < 0 {
                return Err(fail("permission_denied"));
            }
            let existing = unsafe { File::from_raw_fd(existing) };
            let meta = existing.metadata().checked("unavailable")?;
            if !meta.is_file()
                || meta.len() != bytes.len() as u64
                || meta.uid() != unsafe { libc::geteuid() }
            {
                return Err(fail("invalid_record"));
            }
            let mut saved = Vec::new();
            existing
                .take(MAX_FILE as u64 + 1)
                .read_to_end(&mut saved)
                .checked("unavailable")?;
            if saved != bytes {
                return Err(fail("invalid_record"));
            }
        }
        Ok(())
    })();
    let removed = unsafe { libc::unlinkat(fd, temp.as_ptr(), 0) };
    result?;
    if removed < 0 {
        return Err(fail("unavailable"));
    }
    directory.sync_all().checked("unavailable")
}

fn export_capacity() -> &'static tokio::sync::Semaphore {
    static EXPORT: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);
    &EXPORT
}

pub(crate) async fn run(state: AppState, runtime: crate::runtime::Runtime) {
    let mut timer = tokio::time::interval(Duration::from_secs(15));
    timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            _ = runtime.stopped() => return,
            _ = timer.tick() => {}
        }
        if !state.plugins.root.join("platform.sqlite3").is_file() {
            continue;
        }
        let Some(permit) = state.readonly.intake.acquire_for(None) else {
            continue;
        };
        crate::plugin_host::intake::ACTIVE_JOB
            .scope(std::sync::Arc::new(permit), flush_sinks(&state, &runtime))
            .await;
    }
}

async fn flush_sinks(state: &AppState, runtime: &crate::runtime::Runtime) {
    let snapshot = state.clone();
    let Ok(plugins) = run_sync(move || {
        let store = open_store(&snapshot)?;
        let mut stmt = store
            .0
            .prepare("SELECT plugin_id, digest FROM ext_sink_grants WHERE enabled = 1")
            .checked("unavailable")?;
        stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .checked("unavailable")?
        .collect::<rusqlite::Result<Vec<_>>>()
        .checked("unavailable")
    })
    .await
    else {
        return;
    };
    for (plugin, digest) in plugins {
        tokio::select! {
            _ = runtime.stopped() => return,
            _ = flush(state.clone(), plugin, digest, None) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recovery_reuses_file_after_publish_and_keeps_late_terminal_requests() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().canonicalize().unwrap();
        let mut store = PlatformStore::open(&path, "test-store").unwrap();
        let epoch_dir = path.join("observations");
        let obs = local_agent_core::observation::ObservationStore::open(&epoch_dir).unwrap();
        let mut db = Connection::open(epoch_dir.join("observations.sqlite3")).unwrap();
        let selected = Selection {
            report: false,
            digest: "package".into(),
            previous: None,
            enabled: true,
            status: "active".into(),
            error: None,
            epoch: 1,
            revision: 1,
            source: None,
            connection: None,
        };
        let meta = path.metadata().unwrap();
        store.0.execute("INSERT INTO ext_sink_grants VALUES('sink','package','1',?1,?2,?3,'jsonl','daily','{}',1000000,0,1,'now')",params![path.to_str(),meta.dev() as i64,meta.ino() as i64]).unwrap();
        let grant_now =
            |store: &PlatformStore| grant(&store.0, "sink", &selected).unwrap().unwrap();
        let first = local_agent_core::observation::ModelObservation {
            id: "a".into(),
            agent_id: "claude".into(),
            status: "in_progress".into(),
            started_at: "2026-11-19T00:00:00Z".into(),
            ..Default::default()
        };
        let second = local_agent_core::observation::ModelObservation {
            id: "b".into(),
            status: "success".into(),
            started_at: "2026-11-19T00:01:00Z".into(),
            ..first.clone()
        };
        obs.save_batch(&[first.clone(), second]).unwrap();
        let page = read_page(&mut db, &grant_now(&store)).unwrap();
        assert_eq!(page.end.id, "b");
        assert_eq!(page.records.len(), 1);
        let mut bytes = Vec::new();
        page.records[0]
            .encode(LogFormat::Jsonl, &mut bytes)
            .unwrap();
        store.0.execute("INSERT INTO ext_sink_pending VALUES('sink','1.0','package',?1,?2,?3,0,?4,0,?5,'b','gateway-fixture.jsonl',?6,1)",params![meta.dev() as i64, meta.ino() as i64,page.end.epoch,page.end.sequence,page.end.timestamp,bytes]).unwrap();
        store
            .0
            .execute_batch(
                "CREATE TRIGGER fail_cursor BEFORE INSERT ON ext_sink_cursors
             BEGIN SELECT RAISE(ABORT, 'interrupted cursor commit'); END;",
            )
            .unwrap();
        let binding = grant_now(&store);
        let tx = store
            .0
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        assert_eq!(
            publish_batch(tx, &mut db, "sink", &selected, "1.0", binding)
                .err()
                .unwrap()
                .code,
            "unavailable"
        );
        assert!(pending(&store.0, "sink").unwrap());
        assert_eq!(grant_now(&store).status.written_bytes, 0);
        assert_eq!(
            std::fs::read(path.join("gateway-fixture.jsonl")).unwrap(),
            bytes
        );
        drop(store);
        store = PlatformStore::open(&path, "test-store").unwrap();
        store.0.execute_batch("DROP TRIGGER fail_cursor").unwrap();
        obs.save(&local_agent_core::observation::ModelObservation {
            status: "success".into(),
            revision: 2,
            ..first
        })
        .unwrap();
        let binding = grant_now(&store);
        let tx = store
            .0
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        let status = publish_batch(tx, &mut db, "sink", &selected, "1.0", binding).unwrap();
        assert_eq!(status.exported_rows, 1);
        assert_eq!(status.written_bytes, bytes.len() as u64);
        assert!(!pending(&store.0, "sink").unwrap());
        assert_eq!(
            std::fs::read(path.join("gateway-fixture.jsonl")).unwrap(),
            bytes
        );
        let next = read_page(&mut db, &grant_now(&store)).unwrap();
        assert_eq!(
            (
                next.end.id.as_str(),
                next.window.as_str(),
                next.records.len()
            ),
            ("a", "2026-11-19", 1)
        );
        let binding = grant_now(&store);
        let tx = store
            .0
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        let repeat = publish_batch(tx, &mut db, "sink", &selected, "1.0", binding).unwrap();
        assert_eq!(repeat.exported_rows, 1);
        assert_eq!(repeat.written_bytes, bytes.len() as u64);
    }
}
