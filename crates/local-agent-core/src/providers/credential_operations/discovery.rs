//! Bounded metadata discovery. Never queries Vault or changes an operation.
use super::{Operation, Status, legacy};
use crate::providers::{
    ConfigError, ProviderManager, credential_scope::CredentialScope, fail, files,
    operations::valid_id,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    fs,
    os::unix::{ffi::OsStrExt, fs::MetadataExt},
    path::Path,
};

pub const PAGE_SIZE: usize = 32;
pub const MAX_RESPONSE_BYTES: usize = 65536;
const MAX_CANDIDATES: usize = 3 * 4096;
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Request {
    pub store_id: String,
    pub namespace_id: String,
    pub cursor: Option<String>,
}
impl Request {
    pub fn validate(&self) -> Result<(), ConfigError> {
        if !valid_id(&self.store_id) || !valid_id(&self.namespace_id) {
            return Err(invalid());
        }
        if let Some(cursor) = &self.cursor {
            if cursor.len() > 128 {
                return Err(invalid());
            }
            let mut fields = cursor.split(':');
            if !fields.next().is_some_and(|value| {
                value.len() == 64
                    && value
                        .bytes()
                        .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
            }) || !matches!(fields.next(), Some("1" | "2" | "3"))
                || !fields.next().is_some_and(valid_id)
                || fields.next().is_some()
            {
                return Err(invalid());
            }
        }
        Ok(())
    }
}
#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Legacy,
    ScopedV2,
    ScopedV3,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Item {
    pub kind: Kind,
    pub operation_id: String,
    pub digest: Option<String>,
    pub label: Option<String>,
    pub phase: Option<Status>,
    pub legacy_stage: Option<legacy::Stage>,
    pub issue: Option<Issue>,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Issue {
    Unavailable,
    Invalid,
    SourceUnverified,
    Conflict,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Page {
    pub version: u32,
    pub store_id: String,
    pub namespace_id: String,
    pub snapshot: String,
    pub page_size: usize,
    pub total_candidates: usize,
    pub scanned: usize,
    pub merged: usize,
    pub unrecognized: usize,
    pub items: Vec<Item>,
    pub next_cursor: Option<String>,
}
fn unavailable() -> ConfigError {
    ConfigError::new(
        "credential_discovery_unavailable",
        "后台记录暂时无法完整读取，请保留已有提示后重试",
    )
}
fn changed() -> ConfigError {
    ConfigError::new(
        "credential_discovery_changed",
        "后台记录已变化，请重新查找；已有提示和凭据均已保留",
    )
}
fn invalid() -> ConfigError {
    ConfigError::new(
        "invalid_credential_operation",
        "凭据发现来源或游标无效，未重建任何提示",
    )
}
fn metadata(path: &Path) -> Result<Option<fs::Metadata>, ConfigError> {
    match fs::symlink_metadata(path) {
        Ok(m) => Ok(Some(m)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err(unavailable()),
    }
}
// ctime detects in-place changes even if a writer restores the original mtime.
#[derive(Serialize)]
struct FileStamp {
    device: u64,
    inode: u64,
    bytes: u64,
    mode: u32,
    owner: u32,
    modified: (i64, i64),
    changed: (i64, i64),
}
fn stamp(m: Option<&fs::Metadata>) -> Option<FileStamp> {
    m.map(|m| FileStamp {
        device: m.dev(),
        inode: m.ino(),
        bytes: m.len(),
        mode: m.mode(),
        owner: m.uid(),
        modified: (m.mtime(), m.mtime_nsec()),
        changed: (m.ctime(), m.ctime_nsec()),
    })
}
fn legacy_inventory(
    manager: &ProviderManager,
) -> Result<(Vec<String>, String, usize), ConfigError> {
    let directory = manager.directory.join("credential-operations-v1");
    files::checked_path(&directory)?;
    let root = metadata(&manager.directory)?;
    let dir = metadata(&directory)?;
    let key = metadata(&manager.directory.join("credential-operation-key-v1.json"))?;
    let mut entries = Vec::new();
    let mut ids = Vec::new();
    let mut unknown = 0;
    if let Some(m) = &dir {
        if !m.is_dir() || m.uid() != unsafe { libc::geteuid() } {
            return Err(unavailable());
        }
        for (index, entry) in fs::read_dir(&directory)
            .map_err(|_| unavailable())?
            .enumerate()
        {
            if index >= 4096 {
                return fail(
                    "credential_discovery_capacity",
                    "旧记录目录超过可安全检查的容量，请保留材料后人工整理；未将部分记录当成完整列表",
                );
            }
            let entry = entry.map_err(|_| unavailable())?;
            let name = entry.file_name();
            let m = fs::symlink_metadata(entry.path()).map_err(|_| changed())?;
            entries.push((name.as_bytes().to_vec(), stamp(Some(&m))));
            match name
                .to_str()
                .and_then(|n| n.strip_suffix(".json"))
                .filter(|id| valid_id(id))
            {
                Some(id) => ids.push(format!("1:{id}")),
                None => unknown += 1,
            }
        }
    }
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    let digest = files::hash(
        &serde_json::to_vec(&(
            root.as_ref().map(|m| (m.dev(), m.ino(), m.mode(), m.uid())),
            stamp(dir.as_ref()),
            stamp(key.as_ref()),
            entries,
        ))
        .map_err(|_| unavailable())?,
    );
    Ok((ids, digest, unknown))
}
fn candidates(
    manager: &ProviderManager,
    target: &ProviderManager,
) -> Result<(Vec<String>, String, usize), ConfigError> {
    let journal = target.credential_journal.as_ref().ok_or_else(unavailable)?;
    let (revision, records) = journal.discovery_inventory()?;
    let (old, digest, mut unknown) = legacy_inventory(manager)?;
    let mut keys = old.into_iter().collect::<BTreeSet<_>>();
    for (domain, name) in records {
        if let Some(id) = name
            .strip_prefix("operations/")
            .and_then(|n| n.strip_suffix(".json"))
            .filter(|id| valid_id(id))
        {
            keys.insert(format!("{domain}:{id}"));
        } else {
            unknown += 1;
        }
    }
    if keys.len() > MAX_CANDIDATES {
        return Err(unavailable());
    }
    let keys = keys.into_iter().collect::<Vec<_>>();
    let snapshot = files::hash(
        &serde_json::to_vec(&(
            1,
            &target.credential_store_id,
            &target.credential_namespace_id,
            revision,
            digest,
            &keys,
            unknown,
        ))
        .map_err(|_| unavailable())?,
    );
    Ok((keys, snapshot, unknown))
}
fn item(
    manager: &ProviderManager,
    target: &ProviderManager,
    scope: &CredentialScope,
    key: &str,
) -> Result<Option<Item>, ConfigError> {
    let (kind, id) = key.split_once(':').ok_or_else(invalid)?;
    if kind == "1" {
        return legacy::discovery_preview(manager, target, id, true).map(|p| {
            p.map(|p| Item {
                kind: Kind::Legacy,
                operation_id: id.into(),
                digest: Some(p.source_digest),
                label: p.label,
                phase: Some(p.phase),
                legacy_stage: Some(p.stage),
                issue: None,
            })
        });
    }
    let legacy_manager;
    let reader = if kind == "2" {
        legacy_manager = manager.credential_scoped_manager(scope, None)?;
        &legacy_manager
    } else {
        target
    };
    let migration = if kind == "3" {
        legacy::discovery_preview(manager, target, id, false)?
    } else {
        None
    };
    let raw = reader
        .read_credential_bytes(
            &format!("operations/{id}.json"),
            &reader.credential_operation_path(id)?,
        )?
        .ok_or_else(changed)?;
    let operation = reader.read_credential_operation(id)?.ok_or_else(changed)?;
    let (phase, label) = match operation {
        Operation::Pending { label, .. } => (Status::Pending, Some(label)),
        Operation::Ready { label, .. } => (Status::Ready, Some(label)),
        Operation::CancelledPending { label, .. } => (Status::CancelledPending, Some(label)),
        Operation::Cancelled => (Status::Cancelled, None),
        Operation::Migrating => {
            let p = migration.as_ref().ok_or_else(invalid)?;
            (p.phase.clone(), p.label.clone())
        }
    };
    Ok(Some(Item {
        kind: if kind == "2" {
            Kind::ScopedV2
        } else {
            Kind::ScopedV3
        },
        operation_id: id.into(),
        digest: Some(
            migration
                .as_ref()
                .map_or_else(|| files::hash(&raw), |m| m.source_digest.clone()),
        ),
        label,
        phase: Some(phase),
        legacy_stage: migration.map(|m| m.stage),
        issue: None,
    }))
}
fn checked_item(value: Item) -> Result<Item, ConfigError> {
    if value.label.as_ref().is_some_and(|label| {
        label.trim().is_empty()
            || label.trim() != label
            || label.len() > 200
            || label.chars().any(char::is_control)
    }) {
        return Err(invalid());
    }
    Ok(value)
}
impl ProviderManager {
    pub fn discover_credentials(
        &self,
        scope: &CredentialScope,
        input: &Request,
    ) -> Result<Page, ConfigError> {
        input.validate()?;
        if input.store_id != scope.store_id || !valid_id(&input.namespace_id) {
            return Err(invalid());
        }
        let target = self.credential_scoped_manager(scope, Some(&input.namespace_id))?;
        let _root_guard = target.lock()?;
        files::checked_path(&self.directory)?;
        let _legacy_guard = if metadata(&self.directory)?.is_some() {
            Some(self.lock()?)
        } else {
            None
        };
        let (keys, snapshot, unknown) = candidates(self, &target)?;
        let start = match &input.cursor {
            None => 0,
            Some(cursor) => {
                if cursor.len() > 128 {
                    return Err(invalid());
                }
                let (expected, key) = cursor.split_once(':').ok_or_else(invalid)?;
                if expected.len() != 64
                    || !expected
                        .bytes()
                        .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
                {
                    return Err(invalid());
                }
                if expected != snapshot {
                    return Err(changed());
                }
                keys.binary_search(&key.to_string())
                    .map_err(|_| invalid())?
                    + 1
            }
        };
        let end = (start + PAGE_SIZE).min(keys.len());
        let mut items = Vec::new();
        let mut merged = 0;
        for key in &keys[start..end] {
            let (kind, id) = key.split_once(':').ok_or_else(invalid)?;
            match item(self, &target, scope, key).and_then(|i| i.map(checked_item).transpose()) {
                Ok(Some(row)) => items.push(row),
                Ok(None) => merged += 1,
                Err(error) => items.push(Item {
                    kind: match kind {
                        "1" => Kind::Legacy,
                        "2" => Kind::ScopedV2,
                        _ => Kind::ScopedV3,
                    },
                    operation_id: id.into(),
                    digest: None,
                    label: None,
                    phase: None,
                    legacy_stage: None,
                    issue: Some(match error.code.as_str() {
                        "credential_namespace_unverified" => Issue::SourceUnverified,
                        "operation_conflict" => Issue::Conflict,
                        "unavailable"
                        | "credential_discovery_unavailable"
                        | "credential_recovery_unavailable"
                        | "credential_legacy_not_found" => Issue::Unavailable,
                        _ => Issue::Invalid,
                    }),
                }),
            }
        }
        if candidates(self, &target)?.1 != snapshot {
            return Err(changed());
        }
        Ok(Page {
            version: 1,
            store_id: input.store_id.clone(),
            namespace_id: input.namespace_id.clone(),
            snapshot: snapshot.clone(),
            page_size: PAGE_SIZE,
            total_candidates: keys.len(),
            scanned: end - start,
            merged,
            unrecognized: unknown,
            items,
            next_cursor: (end < keys.len()).then(|| format!("{snapshot}:{}", keys[end - 1])),
        })
    }
}

#[cfg(test)]
mod tests;
