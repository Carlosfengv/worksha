//! Product-owned excerpt files only. Counts describe event age, never deletion
//! eligibility or authorization to touch an original native transcript.
use super::Error;
use crate::sessions::activity_input_cache::CachedInput;
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::{collections::HashSet, fs::Metadata, io::Read, path::Path};

const FILE_BYTES: u64 = 2 * 1024 * 1024;
#[derive(Clone, Copy)]
pub enum ActivitySource {
    Gateway,
    Native,
}
impl ActivitySource {
    fn filename(self) -> &'static str {
        match self {
            Self::Gateway => "gateway-activity-inputs.json",
            Self::Native => "native-activity-inputs.json",
        }
    }
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityImpact {
    pub records: u64,
    pub older_than_cutoff: u64,
    pub within_window: u64,
    pub unknown_time: u64,
    pub file_bytes: u64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Saved {
    version: u8,
    rows: Vec<CachedInput>,
}

fn same(a: &Metadata, b: &Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        a.dev() == b.dev()
            && a.ino() == b.ino()
            && a.nlink() == 1
            && b.nlink() == 1
            && a.uid() == b.uid()
            && a.mode() == b.mode()
            && a.len() == b.len()
            && a.mtime() == b.mtime()
            && a.mtime_nsec() == b.mtime_nsec()
            && a.ctime() == b.ctime()
            && a.ctime_nsec() == b.ctime_nsec()
    }
    #[cfg(not(unix))]
    {
        a.len() == b.len() && a.modified().ok() == b.modified().ok()
    }
}
fn metadata(path: &Path) -> Result<Metadata, Error> {
    std::fs::symlink_metadata(path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            Error::SourceAbsent
        } else {
            Error::Unavailable
        }
    })
}
pub fn activity(
    directory: &Path,
    source: ActivitySource,
    days: u16,
    now: DateTime<Utc>,
) -> Result<ActivityImpact, Error> {
    if ![7, 30, 90].contains(&days) {
        return Err(Error::InvalidInput);
    }
    let cutoff = now
        .checked_sub_signed(Duration::days(i64::from(days)))
        .ok_or(Error::InvalidInput)?;
    let directory = directory.canonicalize().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            Error::SourceAbsent
        } else {
            Error::Unavailable
        }
    })?;
    let path = directory.join(source.filename());
    crate::providers::files::checked_path(&path).map_err(|_| Error::SourceChanged)?;
    let before = metadata(&path)?;
    if !before.is_file() || before.file_type().is_symlink() {
        return Err(Error::SourceChanged);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if before.nlink() != 1
            || before.mode() & 0o077 != 0
            || before.uid() != unsafe { libc::geteuid() }
        {
            return Err(Error::SourceChanged);
        }
    }
    if before.len() > FILE_BYTES {
        return Err(Error::Limit);
    }
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let mut file = options.open(&path).map_err(|_| Error::Unavailable)?;
    if !same(&before, &file.metadata().map_err(|_| Error::Unavailable)?) {
        return Err(Error::SourceChanged);
    }
    let mut bytes = Vec::new();
    (&mut file)
        .take(FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| Error::Unavailable)?;
    if bytes.len() as u64 > FILE_BYTES {
        return Err(Error::Limit);
    }
    if !same(&before, &file.metadata().map_err(|_| Error::Unavailable)?)
        || !same(&before, &metadata(&path).map_err(|_| Error::SourceChanged)?)
    {
        return Err(Error::SourceChanged);
    }
    let mut ignored = false;
    let mut json = serde_json::Deserializer::from_slice(&bytes);
    let saved: Saved = serde_ignored::deserialize(&mut json, |_| ignored = true)
        .map_err(|_| Error::UnsupportedSchema)?;
    json.end().map_err(|_| Error::UnsupportedSchema)?;
    if ignored || saved.version != 1 {
        return Err(Error::UnsupportedSchema);
    }
    if saved.rows.len() > 512 {
        return Err(Error::Limit);
    }
    let mut keys = HashSet::new();
    let mut result = ActivityImpact {
        records: saved.rows.len() as u64,
        older_than_cutoff: 0,
        within_window: 0,
        unknown_time: 0,
        file_bytes: bytes.len() as u64,
    };
    for row in saved.rows {
        if !keys.insert(row.key.clone())
            || row.key.is_empty()
            || row.key.len() > 2048
            || row.agent.is_empty()
            || row.agent.len() > 250
            || row.session_id.as_ref().is_some_and(|s| s.len() > 1024)
            || row.source_stamp.as_ref().is_some_and(|s| s.len() > 256)
        {
            return Err(Error::UnsupportedSchema);
        }
        if serde_json::to_vec(&row)
            .map_err(|_| Error::UnsupportedSchema)?
            .len()
            > 3072
        {
            return Err(Error::Limit);
        }
        // A native-cache row may carry legacy None. A gateway row must not be
        // silently interpreted using the native cache's different provenance.
        if matches!(source, ActivitySource::Gateway) && row.native.is_some() {
            return Err(Error::UnsupportedSchema);
        }
        let time = DateTime::parse_from_rfc3339(&row.input.at)
            .ok()
            .filter(|t| *t <= now);
        let other_time_invalid = row
            .started_at
            .as_ref()
            .is_some_and(|at| DateTime::parse_from_rfc3339(at).is_err())
            || row
                .native
                .as_ref()
                .is_some_and(|n| DateTime::parse_from_rfc3339(&n.event_at).is_err());
        match time.filter(|_| !other_time_invalid) {
            Some(at) if at < cutoff => result.older_than_cutoff += 1,
            Some(_) => result.within_window += 1,
            None => result.unknown_time += 1,
        }
    }
    // Atomic replacement after the read makes the inventory explicitly stale.
    if !same(&before, &metadata(&path).map_err(|_| Error::SourceChanged)?) {
        return Err(Error::SourceChanged);
    }
    Ok(result)
}

#[cfg(test)]
mod tests;
