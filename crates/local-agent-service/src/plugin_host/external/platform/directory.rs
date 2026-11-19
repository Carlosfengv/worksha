use std::{
    ffi::CString,
    fs::{File, OpenOptions},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::{
            ffi::OsStrExt,
            fs::{MetadataExt, OpenOptionsExt},
        },
    },
    path::{Component, Path},
};

use local_agent_core::plugin::platform::*;
use rusqlite::{Connection, TransactionBehavior, params};

use super::{Checked, DirectorySelection, PlatformStore, Result, Selection, fail, open_store};
use crate::AppState;

pub(super) fn open(path: &Path) -> Result<File> {
    if !path.is_absolute() || path.parent().is_none_or(|parent| parent.parent().is_none()) {
        return Err(fail("permission_denied"));
    }
    let canonical = path.canonicalize().checked("permission_denied")?;
    if canonical != path
        || [
            "/System",
            "/usr",
            "/bin",
            "/sbin",
            "/private/etc",
            "/dev",
            "/Library/Keychains",
        ]
        .iter()
        .any(|blocked| path.starts_with(blocked))
    {
        return Err(fail("permission_denied"));
    }
    let flags = libc::O_NOFOLLOW | libc::O_DIRECTORY | libc::O_NONBLOCK | libc::O_CLOEXEC;
    let mut directory = OpenOptions::new()
        .read(true)
        .custom_flags(flags)
        .open("/")
        .checked("permission_denied")?;
    for part in path.components().skip(1) {
        let Component::Normal(part) = part else {
            return Err(fail("permission_denied"));
        };
        let name = CString::new(part.as_bytes()).checked("permission_denied")?;
        let child =
            unsafe { libc::openat(directory.as_raw_fd(), name.as_ptr(), flags | libc::O_RDONLY) };
        if child < 0 {
            return Err(fail("permission_denied"));
        }
        directory = unsafe { File::from_raw_fd(child) };
    }
    let metadata = directory.metadata().checked("permission_denied")?;
    if metadata.uid() != unsafe { libc::geteuid() } {
        return Err(fail("permission_denied"));
    }
    let selected = std::fs::symlink_metadata(path).checked("permission_denied")?;
    if selected.dev() != metadata.dev() || selected.ino() != metadata.ino() || !selected.is_dir() {
        return Err(fail("permission_denied"));
    }
    Ok(directory)
}

fn table(purpose: DirectoryPurpose) -> &'static str {
    match purpose {
        DirectoryPurpose::SourceRead => "ext_source_directory_grants",
        DirectoryPurpose::LogSink => "ext_sink_grants",
    }
}

pub(super) fn purposes(
    db: &Connection,
    plugin: &str,
    selection: &Selection,
) -> Result<Vec<DirectoryPurpose>> {
    let mut granted = Vec::new();
    for purpose in [DirectoryPurpose::SourceRead, DirectoryPurpose::LogSink] {
        let present: bool = db.query_row(
            &format!("SELECT EXISTS(SELECT 1 FROM {} WHERE plugin_id = ?1 AND digest = ?2 AND host_revision = ?3 {})", table(purpose),
                if matches!(purpose, DirectoryPurpose::LogSink) { " AND enabled = 1" } else { "" }),
            params![plugin, selection.digest, selection.revision.to_string()], |row| row.get(0),
        ).checked("invalid_record")?;
        if present {
            granted.push(purpose);
        }
    }
    Ok(granted)
}

pub(super) fn revoke(db: &Connection, plugin: &str, purpose: DirectoryPurpose) -> Result<()> {
    db.execute(
        match purpose {
            DirectoryPurpose::SourceRead => {
                "DELETE FROM ext_source_directory_grants WHERE plugin_id = ?1"
            }
            DirectoryPurpose::LogSink => {
                "UPDATE ext_sink_grants SET enabled = 0 WHERE plugin_id = ?1"
            }
        },
        [plugin],
    )
    .checked("unavailable")?;
    Ok(())
}

pub(super) fn grant(state: &AppState, selection: DirectorySelection) -> Result<serde_json::Value> {
    let request = selection.request;
    let (active, definition) = state
        .plugins
        .active_platform(&request.plugin_id, &request.digest)?;
    if !match request.purpose {
        DirectoryPurpose::SourceRead => {
            matches!(definition.adapter, Some(AdapterDef::AgentAdapter { .. }))
        }
        DirectoryPurpose::LogSink => definition.log_sink.is_some(),
    } {
        return Err(fail("permission_denied"));
    }
    let directory = open(&selection.path)?;
    let meta = directory.metadata().checked("permission_denied")?;
    let mut store = open_store(state)?;
    let tx = store
        .0
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .checked("unavailable")?;
    let plugin = &request.plugin_id;
    if selection.expected_grant_revision != PlatformStore::revision(&tx, plugin, active.revision)? {
        return Err(fail("grant_revoked"));
    }
    let path = selection.path.to_str().ok_or(fail("invalid_input"))?;
    let identity = params![
        plugin,
        request.digest,
        active.revision.to_string(),
        path,
        meta.dev() as i64,
        meta.ino() as i64
    ];
    match request.purpose {
        DirectoryPurpose::SourceRead => {
            tx.execute(
                "INSERT INTO ext_source_directory_grants VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(plugin_id) DO UPDATE SET digest = excluded.digest,
                 host_revision = excluded.host_revision, dir_path = excluded.dir_path,
                 device = excluded.device, inode = excluded.inode",
                identity,
            )
            .checked("unavailable")?;
        }
        DirectoryPurpose::LogSink => {
            let changed: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM ext_sink_grants WHERE plugin_id=?1 AND (device<>?2 OR inode<>?3))", params![plugin,meta.dev() as i64,meta.ino() as i64],|r|r.get(0)).checked("unavailable")?;
            if changed {
                for table in ["ext_sink_pending", "ext_sink_progress", "ext_sink_cursors"] {
                    tx.execute(&format!("DELETE FROM {table} WHERE plugin_id=?1"), [plugin])
                        .checked("unavailable")?;
                }
                tx.execute(
                    "UPDATE ext_sink_grants SET written_bytes=0 WHERE plugin_id=?1",
                    [plugin],
                )
                .checked("unavailable")?;
            }
            let sink = definition.log_sink.ok_or(fail("invalid_input"))?;
            let format = serde_json::to_value(sink.format).checked("invalid_input")?;
            let rotate = serde_json::to_value(sink.rotate_policy).checked("invalid_input")?;
            tx.execute(
                "INSERT INTO ext_sink_grants (plugin_id, digest, host_revision, dir_path, device, inode, format, rotate_policy, filter_json, enabled, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 1, strftime('%Y-%m-%dT%H:%M:%fZ','now'))
                 ON CONFLICT(plugin_id) DO UPDATE SET digest = excluded.digest,
                 host_revision = excluded.host_revision, dir_path = excluded.dir_path,
                 device = excluded.device, inode = excluded.inode, format = excluded.format,
                 rotate_policy = excluded.rotate_policy, filter_json = excluded.filter_json,
                 enabled = 1, updated_at = excluded.updated_at",
                params![plugin, request.digest, active.revision.to_string(), path, meta.dev() as i64, meta.ino() as i64,
                    format.as_str(), rotate.as_str(), serde_json::to_string(&sink.filter).checked("invalid_input")?],
            ).checked("unavailable")?;
        }
    }
    PlatformStore::bump_revision(&tx, plugin)?;
    let revision = PlatformStore::revision(&tx, plugin, active.revision)?;
    if matches!(request.purpose, DirectoryPurpose::LogSink) {
        tx.execute("UPDATE ext_sink_pending SET revision=?2 WHERE plugin_id=?1 AND digest=?3 AND device=?4 AND inode=?5",
            params![plugin,revision,request.digest,meta.dev() as i64,meta.ino() as i64]).checked("unavailable")?;
    }
    let _commit = state
        .plugins
        .platform_commit
        .lock()
        .checked("unavailable")?;
    state.plugins.ensure_platform_current(plugin, &active)?;
    tx.commit().checked("unavailable")?;
    Ok(serde_json::json!({"grantRevision": revision}))
}
