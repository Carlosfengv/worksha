use super::{Marker, journal::lock_at};
use crate::providers::{ConfigError, credential_operations, files, operations::valid_id};
use std::{fs, path::Path};

pub(super) struct Archive {
    pub values: Vec<(String, Vec<u8>)>,
    pub digest: String,
    _guards: Vec<crate::providers::ConfigTransactionGuard>,
}
fn invalid() -> ConfigError {
    ConfigError::new(
        "credential_namespace_unverified",
        "旧凭据恢复材料不完整或来源无法核查；原文件已保留",
    )
}
fn read_file(path: &Path, limit: usize) -> Result<Vec<u8>, ConfigError> {
    if fs::symlink_metadata(path).map_err(|_| invalid())?.len() > limit as u64 {
        return Err(invalid());
    }
    let value = files::read_protected(path)?;
    if value.stamp.mode != 0o600 {
        return Err(invalid());
    }
    value
        .bytes
        .filter(|bytes| bytes.len() <= limit)
        .ok_or_else(invalid)
}
fn values(directory: &Path, store_id: &str) -> Result<Archive, ConfigError> {
    let marker = read_file(&directory.join("scope.json"), 1024)?;
    let saved: Marker = serde_json::from_slice(&marker).map_err(|_| invalid())?;
    if saved.version != 2 || saved.store_id != store_id {
        return Err(invalid());
    }
    let mut result = vec![];
    let mut needs_key = false;
    for entry in fs::read_dir(directory).map_err(|_| invalid())? {
        let entry = entry.map_err(|_| invalid())?;
        let name = entry.file_name();
        match name.to_str().ok_or_else(invalid)? {
            "scope.json" | "transaction.lock" => {}
            "protection-key.json" => {
                let bytes = read_file(&entry.path(), 4096)?;
                credential_operations::validate_archive_value(
                    "protection-key.json",
                    &bytes,
                    store_id,
                )?;
                result.push(("protection-key.json".into(), bytes));
            }
            "operations" => {
                files::checked_path(&entry.path())?;
                for (index, file) in fs::read_dir(entry.path())
                    .map_err(|_| invalid())?
                    .enumerate()
                {
                    if index >= 4096 {
                        return Err(invalid());
                    }
                    let file = file.map_err(|_| invalid())?;
                    let name = file.file_name().into_string().map_err(|_| invalid())?;
                    if !name.strip_suffix(".json").is_some_and(valid_id) {
                        return Err(invalid());
                    }
                    let bytes = read_file(&file.path(), 4096)?;
                    needs_key |=
                        credential_operations::validate_archive_value(&name, &bytes, store_id)?;
                    result.push((format!("operations/{name}"), bytes));
                }
            }
            _ => return Err(invalid()),
        }
    }
    if needs_key && !result.iter().any(|(name, _)| name == "protection-key.json") {
        return Err(invalid());
    }
    result.sort_by(|a, b| a.0.cmp(&b.0));
    let fingerprints: Vec<_> = result
        .iter()
        .map(|(name, bytes)| (name, files::hash(bytes)))
        .collect();
    let digest = files::hash(
        &serde_json::to_vec(&(files::hash(&marker), fingerprints)).map_err(|_| invalid())?,
    );
    Ok(Archive {
        values: result,
        digest,
        _guards: vec![],
    })
}
pub(super) fn read(root: &Path, store_id: &str) -> Result<Option<Archive>, ConfigError> {
    let parent = root.join("provider-configs");
    let directory = parent.join("credential-operations-v2");
    files::checked_path(&directory)?;
    if !directory.exists() {
        return Ok(None);
    }
    let parent_guard = lock_at(&parent.join("transaction.lock"))?;
    let operation_guard = lock_at(&directory.join("transaction.lock"))?;
    let mut snapshot = values(&directory, store_id)?;
    if values(&directory, store_id)?.digest != snapshot.digest {
        return Err(invalid());
    }
    snapshot._guards = vec![parent_guard, operation_guard];
    Ok(Some(snapshot))
}
