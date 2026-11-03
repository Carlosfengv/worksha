//! Catalog choice accompanies the existing operation ledger; it never owns replay.
use super::{
    ConfigError, ProviderManager, files,
    operations::{self, Request, Status},
};
use crate::{
    Store,
    domain::{provider_connection::Selection, provider_source_operation::SourceRequest},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{os::unix::fs::MetadataExt, path::PathBuf};
#[cfg(test)]
mod tests;

const LIMIT: usize = operations::RECEIPT_LIMIT;
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Intent {
    version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    source_format: Option<String>,
    store_id: String,
    operation_id: String,
    sequence: u64,
    request_hash: String,
    catalog: Option<Selection>,
    terminal: bool,
    #[serde(default)]
    source_revision: Option<String>,
}
fn invalid() -> ConfigError {
    ConfigError::new(
        "catalog_operation_unavailable",
        "原目录选择无法核查，已保留原操作；未改用内置目录或重新探测",
    )
}
fn hash(request: &Request) -> Result<String, ConfigError> {
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(request).map_err(|_| invalid())?)
    ))
}
impl ProviderManager {
    fn catalog_intent_directory(&self, create: bool) -> Result<PathBuf, ConfigError> {
        let path = self.directory.join("catalog-operations");
        if create && !path.exists() {
            files::private_dir(&path)?;
        }
        if path.exists() {
            let m = std::fs::symlink_metadata(&path).map_err(|_| invalid())?;
            if !m.is_dir()
                || m.file_type().is_symlink()
                || m.uid() != unsafe { libc::geteuid() }
                || m.mode() & 0o077 != 0
            {
                return Err(invalid());
            }
        }
        Ok(path)
    }
    pub(super) fn catalog_intent_path(
        &self,
        r: &Request,
        create: bool,
    ) -> Result<PathBuf, ConfigError> {
        if !operations::valid_id(&r.store_id) || !operations::valid_id(&r.operation_id) {
            return Err(invalid());
        }
        Ok(self
            .catalog_intent_directory(create)?
            .join(format!("{}-{}.json", r.store_id, r.operation_id)))
    }
    fn read_catalog_intent(&self, r: &Request) -> Result<Intent, ConfigError> {
        let snapshot = files::read(&self.catalog_intent_path(r, false)?)?;
        if snapshot.stamp.mode & 0o077 != 0 {
            return Err(invalid());
        }
        let bytes = snapshot.bytes.ok_or_else(invalid)?;
        if bytes.len() > 16384 {
            return Err(invalid());
        }
        let intent: Intent = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
        if !matches!(intent.version, 1 | 2)
            || (intent.version == 1 && (intent.catalog.is_none() || intent.source_format.is_some()))
            || (intent.version == 2
                && intent.source_format.as_deref() != Some("source-operation-v1"))
            || intent.store_id != r.store_id
            || intent.operation_id != r.operation_id
            || intent.sequence != r.expected_sequence.checked_add(1).ok_or_else(invalid)?
            || intent.request_hash != hash(r)?
        {
            return Err(invalid());
        }
        Ok(intent)
    }
    pub fn catalog_operation_selection(&self, request: &Request) -> Result<Selection, ConfigError> {
        self.read_catalog_intent(request)?
            .catalog
            .ok_or_else(invalid)
    }
    pub(super) fn source_operation_is_explicit(
        &self,
        request: &Request,
    ) -> Result<bool, ConfigError> {
        Ok(self.read_catalog_intent(request)?.version == 2)
    }
    pub(super) fn catalog_operation_choice(
        &self,
        request: &Request,
    ) -> Result<Option<Selection>, ConfigError> {
        Ok(self.read_catalog_intent(request)?.catalog)
    }
    pub fn verify_source_operation(&self, request: &SourceRequest) -> Result<(), ConfigError> {
        let intent = self.read_catalog_intent(&request.operation)?;
        if intent.version != 2
            || intent.catalog.as_ref() != request.source.catalog()
            || intent.source_revision != request.expected_source_revision
        {
            return Err(invalid());
        }
        Ok(())
    }
    pub fn record_source_operation(
        &self,
        store: &Store,
        input: &SourceRequest,
    ) -> Result<(), ConfigError> {
        self.record_selected_operation(
            store,
            &input.operation,
            input.source.catalog(),
            Some(&input.expected_source_revision),
        )
    }
    pub(super) fn catalog_operation_source_revision(
        &self,
        request: &Request,
    ) -> Result<Option<String>, ConfigError> {
        Ok(self.read_catalog_intent(request)?.source_revision)
    }
    /// Connect requires a committed Pending. Manual Save is staged only inside
    /// the original SQL transaction, before its committed receipt is published.
    pub fn record_catalog_operation(
        &self,
        store: &Store,
        request: &Request,
        catalog: &Selection,
    ) -> Result<(), ConfigError> {
        self.record_selected_operation(store, request, Some(catalog), None)
    }
    fn record_selected_operation(
        &self,
        store: &Store,
        request: &Request,
        catalog: Option<&Selection>,
        expected: Option<&Option<String>>,
    ) -> Result<(), ConfigError> {
        if expected.is_some_and(|token| {
            token.as_ref().is_some_and(|s| {
                s.len() != 64
                    || !s
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            })
        }) {
            return Err(invalid());
        }
        let _guard = self.lock()?;
        let current = store
            .existing_provider_operation(request)?
            .ok_or_else(invalid)?;
        let admitted = match request.kind {
            operations::Kind::Connect => current.status == Status::Pending,
            operations::Kind::Save => {
                current.status == Status::Committed && !store.connection.is_autocommit()
            }
        };
        if !admitted {
            return Err(invalid());
        }
        let source = self.catalog_source_snapshot(store, &request.provider)?;
        if source.pending || expected.is_some_and(|token| *token != source.revision) {
            return Err(ConfigError::new(
                "catalog_source_changed",
                "目录来源已变化，本次未提交或重新探测",
            ));
        }
        let path = self.catalog_intent_path(request, true)?;
        if path.exists() {
            if request.kind == operations::Kind::Save {
                let saved = self.read_catalog_intent(request)?;
                if saved.catalog.as_ref() == catalog
                    && saved.source_revision == source.revision
                    && saved.version == if expected.is_some() { 2 } else { 1 }
                {
                    return Ok(());
                }
            }
            return Err(invalid());
        }
        let root = path.parent().ok_or_else(invalid)?;
        let entries = std::fs::read_dir(root)
            .map_err(|_| invalid())?
            .take(LIMIT + 1)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| invalid())?;
        if entries.len() > LIMIT {
            return Err(invalid());
        }
        if entries.len() == LIMIT {
            let mut reclaim = vec![];
            for entry in entries {
                let snapshot = files::read(&entry.path())?;
                let bytes = snapshot.bytes.ok_or_else(invalid)?;
                if bytes.len() > 16384 || snapshot.stamp.mode & 0o077 != 0 {
                    return Err(invalid());
                }
                let mut intent: Intent = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
                if !matches!(intent.version, 1 | 2)
                    || (intent.version == 1
                        && (intent.catalog.is_none() || intent.source_format.is_some()))
                    || (intent.version == 2
                        && intent.source_format.as_deref() != Some("source-operation-v1"))
                    || intent.store_id != request.store_id
                    || entry.file_name().to_str()
                        != Some(&format!("{}-{}.json", intent.store_id, intent.operation_id))
                {
                    return Err(invalid());
                }
                if !intent.terminal {
                    // Missing/unknown results do not prove a terminal operation.
                    intent.terminal = store
                        .provider_operation(&intent.store_id, &intent.operation_id)
                        .is_ok_and(|op| op.status != Status::Pending);
                }
                if intent.terminal {
                    reclaim.push((intent.sequence, entry.path()));
                }
            }
            reclaim.sort_by_key(|(sequence, _)| *sequence);
            let (_, old) = reclaim.into_iter().next().ok_or_else(|| {
                ConfigError::new("capacity", "目录操作恢复记录已满，请先核查原结果")
            })?;
            std::fs::remove_file(old).map_err(|_| invalid())?;
        }
        let intent = Intent {
            version: if expected.is_some() { 2 } else { 1 },
            source_format: expected.map(|_| "source-operation-v1".into()),
            store_id: request.store_id.clone(),
            operation_id: request.operation_id.clone(),
            sequence: current.sequence,
            request_hash: hash(request)?,
            catalog: catalog.cloned(),
            terminal: false,
            source_revision: source.revision,
        };
        files::atomic_write(
            &path,
            &serde_json::to_vec(&intent).map_err(|_| invalid())?,
            0o600,
        )
    }
    pub fn finish_catalog_operation_record(
        &self,
        store: &Store,
        request: &Request,
    ) -> Result<(), ConfigError> {
        let _guard = self.lock()?;
        if store
            .existing_provider_operation(request)?
            .is_none_or(|op| op.status == Status::Pending)
        {
            return Err(invalid());
        }
        let mut intent = self.read_catalog_intent(request)?;
        intent.terminal = true;
        files::atomic_write(
            &self.catalog_intent_path(request, false)?,
            &serde_json::to_vec(&intent).map_err(|_| invalid())?,
            0o600,
        )
    }
}
