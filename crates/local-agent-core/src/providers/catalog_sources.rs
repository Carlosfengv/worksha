//! Remembered directory choice, independently of network grants and old SQL DTOs.
use super::{
    ConfigError, ProviderManager, files,
    operations::{Kind, Operation, Request, Status},
};
use crate::{
    Provider, Store,
    domain::{provider_connection::Selection, provider_source_operation::SourceRequest},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{os::unix::fs::MetadataExt, path::PathBuf};
#[cfg(test)]
mod tests;

#[derive(Clone)]
pub struct CatalogSourceSnapshot {
    pub revision: Option<String>,
    pub catalog: Option<Selection>,
    pub scope_matches: bool,
    pub pending: bool,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Selected {
    catalog: Selection,
    scope: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Pending {
    operation_id: String,
    sequence: u64,
    request_hash: String,
    provider_revision: u64,
    next: Option<Selected>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    scope: Option<String>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Source {
    version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    source_format: Option<String>,
    change_id: String,
    store_id: String,
    provider_id: String,
    active: Option<Selected>,
    pending: Option<Pending>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceMarker {
    version: u32,
    store_id: String,
    provider_id: String,
}
struct SaveBefore {
    source: files::FileSnapshot,
    marker: files::FileSnapshot,
}
fn unavailable() -> ConfigError {
    ConfigError::new(
        "catalog_source_unavailable",
        "目录来源记录尚不可核查；原文件保留，没有改用内置目录",
    )
}
fn changed() -> ConfigError {
    ConfigError::new(
        "catalog_source_changed",
        "目录来源已变化，请重新读取；本次未回退或重新发送",
    )
}
fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn scope(provider: &Provider) -> String {
    digest(
        serde_json::json!([
            provider.id,
            provider.endpoint,
            provider.protocol,
            provider.vendor,
            provider.credential_ref
        ])
        .to_string()
        .as_bytes(),
    )
}
fn request_hash(request: &Request) -> Result<String, ConfigError> {
    Ok(digest(
        &serde_json::to_vec(request).map_err(|_| unavailable())?,
    ))
}
impl ProviderManager {
    /// A missing SQL result never lets a legacy caller take over an operation
    /// already bound to a plugin. Known SQL outcomes remain independently read.
    pub fn require_native_provider_operation(
        &self,
        store: &Store,
        request: &Request,
    ) -> Result<(), ConfigError> {
        let unknown = || {
            ConfigError::new(
                "catalog_operation_unavailable",
                "原操作包含插件目录选择或尚不可核查，请保留原请求核查；未改用内置或重新探测",
            )
        };
        let marker = self
            .catalog_intent_path(request, false)
            .map_err(|_| unknown())?;
        if files::read(&marker).map_err(|_| unknown())?.bytes.is_some() {
            return Err(unknown());
        }
        let (source, _) = self
            .read_catalog_source(store, &request.provider)
            .map_err(|_| unknown())?;
        if source
            .pending
            .as_ref()
            .is_some_and(|p| p.operation_id == request.operation_id)
        {
            return Err(unknown());
        }
        Ok(())
    }
    fn catalog_source_path(
        &self,
        store_id: &str,
        provider_id: &str,
        create: bool,
    ) -> Result<PathBuf, ConfigError> {
        self.catalog_metadata_path("catalog-sources", store_id, provider_id, create)
    }
    fn catalog_marker_path(
        &self,
        store_id: &str,
        provider_id: &str,
        create: bool,
    ) -> Result<PathBuf, ConfigError> {
        self.catalog_metadata_path("catalog-source-markers", store_id, provider_id, create)
    }
    fn catalog_metadata_path(
        &self,
        namespace: &str,
        store_id: &str,
        provider_id: &str,
        create: bool,
    ) -> Result<PathBuf, ConfigError> {
        let root = self.directory.join(namespace);
        if create && !root.exists() {
            files::private_dir(&root)?;
        }
        if root.exists() {
            let m = std::fs::symlink_metadata(&root).map_err(|_| unavailable())?;
            if !m.is_dir()
                || m.file_type().is_symlink()
                || m.uid() != unsafe { libc::geteuid() }
                || m.mode() & 0o077 != 0
            {
                return Err(unavailable());
            }
        }
        Ok(root.join(format!(
            "{}.json",
            digest(
                serde_json::json!([store_id, provider_id])
                    .to_string()
                    .as_bytes()
            )
        )))
    }
    fn read_catalog_marker(
        &self,
        store_id: &str,
        provider_id: &str,
    ) -> Result<files::FileSnapshot, ConfigError> {
        let snapshot = files::read(&self.catalog_marker_path(store_id, provider_id, false)?)?;
        if snapshot.stamp.mode & 0o077 != 0 {
            return Err(unavailable());
        }
        if let Some(bytes) = &snapshot.bytes {
            if bytes.len() > 4096 {
                return Err(unavailable());
            }
            let marker: SourceMarker = serde_json::from_slice(bytes).map_err(|_| unavailable())?;
            if marker.version != 1
                || marker.store_id != store_id
                || marker.provider_id != provider_id
            {
                return Err(unavailable());
            }
        }
        Ok(snapshot)
    }
    fn ensure_catalog_marker(&self, store_id: &str, provider_id: &str) -> Result<(), ConfigError> {
        if self
            .read_catalog_marker(store_id, provider_id)?
            .bytes
            .is_none()
        {
            let marker = SourceMarker {
                version: 1,
                store_id: store_id.into(),
                provider_id: provider_id.into(),
            };
            files::atomic_write(
                &self.catalog_marker_path(store_id, provider_id, true)?,
                &serde_json::to_vec(&marker).map_err(|_| unavailable())?,
                0o600,
            )?;
        }
        Ok(())
    }
    fn write_catalog_source(&self, source: &Source) -> Result<(), ConfigError> {
        let before = self.read_catalog_marker(&source.store_id, &source.provider_id)?;
        let path = self.catalog_source_path(&source.store_id, &source.provider_id, true)?;
        self.ensure_catalog_marker(&source.store_id, &source.provider_id)?;
        let result = files::atomic_write(
            &path,
            &serde_json::to_vec(source).map_err(|_| unavailable())?,
            0o600,
        );
        if result.is_err() && before.bytes.is_none() {
            // Only undo a newly created marker when no source was published.
            // A crash or unreadable result remains explicitly unavailable.
            if files::read(&path)?.bytes.is_none() {
                let marker =
                    self.catalog_marker_path(&source.store_id, &source.provider_id, false)?;
                let current = files::read(&marker)?;
                files::replace(&marker, &current.stamp, None, 0o600)?;
            }
        }
        result
    }
    fn read_catalog_source(
        &self,
        store: &Store,
        provider: &Provider,
    ) -> Result<(Source, Option<String>), ConfigError> {
        let store_id = store.provider_write_protocol()?.store_id;
        let snapshot = files::read(&self.catalog_source_path(&store_id, &provider.id, false)?)?;
        if snapshot.stamp.mode & 0o077 != 0 {
            return Err(unavailable());
        }
        let revision = snapshot.stamp.digest;
        let marker = self.read_catalog_marker(&store_id, &provider.id)?;
        if snapshot.bytes.is_none() && marker.bytes.is_some() {
            return Err(unavailable());
        }
        let source = if let Some(bytes) = snapshot.bytes {
            if bytes.len() > 16384 {
                return Err(unavailable());
            }
            let source: Source = serde_json::from_slice(&bytes).map_err(|_| unavailable())?;
            if !matches!(source.version, 1 | 2)
                || (source.version == 1 && source.source_format.is_some())
                || (source.version == 2
                    && source.source_format.as_deref() != Some("source-operation-v1"))
                || source
                    .pending
                    .as_ref()
                    .is_some_and(|p| match source.version {
                        1 => p.next.is_none() || p.scope.is_some(),
                        _ => p.scope.as_ref().is_none_or(|scope| {
                            scope.len() != 64 || p.next.as_ref().is_some_and(|n| &n.scope != scope)
                        }),
                    })
                || uuid::Uuid::parse_str(&source.change_id).is_err()
                || source.store_id != store_id
                || source.provider_id != provider.id
            {
                return Err(unavailable());
            }
            // Backfill healthy v1 records without rewriting their source bytes
            // or granting permission. Future file loss can then be detected.
            self.ensure_catalog_marker(&store_id, &provider.id)?;
            source
        } else {
            Source {
                version: 1,
                source_format: None,
                change_id: uuid::Uuid::new_v4().to_string(),
                store_id,
                provider_id: provider.id.clone(),
                active: None,
                pending: None,
            }
        };
        Ok((source, revision))
    }
    fn resolve_catalog_source(
        &self,
        store: &Store,
        mut source: Source,
    ) -> Result<Source, ConfigError> {
        if let Some(pending) = source.pending.take() {
            let operation = store
                .provider_operation(&source.store_id, &pending.operation_id)
                .map_err(|_| unavailable())?;
            if operation.sequence != pending.sequence
                || request_hash(&operation.request)? != pending.request_hash
                || operation.request.provider.id != source.provider_id
            {
                return Err(unavailable());
            }
            match operation.status {
                Status::Pending => source.pending = Some(pending),
                Status::Committed => {
                    let provider = operation.provider.as_ref().ok_or_else(unavailable)?;
                    if provider.revision != pending.provider_revision
                        || Some(scope(provider)).as_ref()
                            != pending
                                .scope
                                .as_ref()
                                .or_else(|| pending.next.as_ref().map(|s| &s.scope))
                    {
                        return Err(unavailable());
                    }
                    source.active = pending.next;
                }
                Status::Failed | Status::Cancelled => {}
            }
        }
        Ok(source)
    }
    pub fn catalog_source_snapshot(
        &self,
        store: &Store,
        provider: &Provider,
    ) -> Result<CatalogSourceSnapshot, ConfigError> {
        let (source, revision) = self.read_catalog_source(store, provider)?;
        let source = self.resolve_catalog_source(store, source)?;
        let pending = source.pending.is_some();
        let selected = source.pending.as_ref().and_then(|p| p.next.as_ref()).or({
            if pending {
                None
            } else {
                source.active.as_ref()
            }
        });
        Ok(CatalogSourceSnapshot {
            revision,
            catalog: selected.map(|s| s.catalog.clone()),
            scope_matches: selected.is_none_or(|s| s.scope == scope(provider)),
            pending,
        })
    }
    /// Choice changes are explicit CAS operations; unknown records are preserved.
    pub fn select_catalog_source(
        &self,
        store: &Store,
        provider: &Provider,
        expected: Option<&str>,
        catalog: Option<Selection>,
    ) -> Result<(), ConfigError> {
        let _guard = self.lock()?;
        let (source, revision) = self.read_catalog_source(store, provider)?;
        if revision.as_deref() != expected {
            return Err(changed());
        }
        let source = self.resolve_catalog_source(store, source)?;
        if source.pending.is_some() {
            return Err(ConfigError::new(
                "catalog_source_pending",
                "连接操作仍待核查，未变更目录来源",
            ));
        }
        let source = Source {
            change_id: uuid::Uuid::new_v4().to_string(),
            active: catalog.map(|catalog| Selected {
                catalog,
                scope: scope(provider),
            }),
            pending: None,
            ..source
        };
        self.write_catalog_source(&source)
    }
    pub fn verify_catalog_source_intent(
        &self,
        store: &Store,
        request: &Request,
    ) -> Result<(), ConfigError> {
        let (_, revision) = self.read_catalog_source(store, &request.provider)?;
        if revision != self.catalog_operation_source_revision(request)? {
            return Err(changed());
        }
        Ok(())
    }
    /// Stage before the SQL completion. Reads reconcile using the original
    /// receipt after a crash; they never infer commitment from matching fields.
    pub fn stage_catalog_source(
        &self,
        store: &Store,
        request: &Request,
        detected: &Provider,
    ) -> Result<(), ConfigError> {
        let _guard = self.lock()?;
        self.verify_catalog_source_intent(store, request)?;
        let op = store
            .existing_provider_operation(request)?
            .ok_or_else(unavailable)?;
        let provider = match request.kind {
            Kind::Connect if op.status == Status::Pending => {
                store.normalize_provider(detected.clone())?
            }
            Kind::Save if op.status == Status::Committed && !store.connection.is_autocommit() => {
                let saved = op.provider.as_ref().ok_or_else(unavailable)?;
                if serde_json::to_vec(saved).map_err(|_| unavailable())?
                    != serde_json::to_vec(detected).map_err(|_| unavailable())?
                {
                    return Err(changed());
                }
                saved.clone()
            }
            _ => return Err(changed()),
        };
        let (source, _) = self.read_catalog_source(store, &request.provider)?;
        let source = self.resolve_catalog_source(store, source)?;
        if source.pending.is_some() {
            return Err(unavailable());
        }
        let catalog = self.catalog_operation_choice(request)?;
        let explicit = self.source_operation_is_explicit(request)?;
        let source = Source {
            version: if explicit { 2 } else { source.version },
            source_format: if explicit {
                Some("source-operation-v1".into())
            } else {
                source.source_format.clone()
            },
            change_id: uuid::Uuid::new_v4().to_string(),
            pending: Some(Pending {
                operation_id: request.operation_id.clone(),
                sequence: op.sequence,
                request_hash: request_hash(request)?,
                provider_revision: provider.revision,
                next: catalog.map(|catalog| Selected {
                    catalog,
                    scope: scope(&provider),
                }),
                scope: (explicit || source.version == 2).then(|| scope(&provider)),
            }),
            ..source
        };
        self.write_catalog_source(&source)
    }
    pub fn finish_catalog_source(
        &self,
        store: &Store,
        request: &Request,
    ) -> Result<(), ConfigError> {
        let _guard = self.lock()?;
        let (source, _) = self.read_catalog_source(store, &request.provider)?;
        if source
            .pending
            .as_ref()
            .is_none_or(|p| p.operation_id != request.operation_id)
        {
            return Ok(());
        }
        let source = self.resolve_catalog_source(store, source)?;
        if source.pending.is_some() {
            return Err(unavailable());
        }
        self.write_catalog_source(&source)
    }

    /// Manual metadata save uses the unchanged Save receipt. No verification is
    /// invented. The choice is staged before SQL commit and grants do not grow.
    pub fn save_catalog_provider(
        &self,
        store: &Store,
        request: &Request,
        catalog: &Selection,
        authorize: impl FnOnce() -> Result<(), ConfigError>,
    ) -> Result<Operation, ConfigError> {
        self.save_selected_provider(store, request, Some(catalog), None, authorize)
    }
    pub fn save_source_provider(
        &self,
        store: &Store,
        input: &SourceRequest,
        authorize: impl FnOnce() -> Result<(), ConfigError>,
    ) -> Result<Operation, ConfigError> {
        self.save_selected_provider(
            store,
            &input.operation,
            input.source.catalog(),
            Some(input),
            authorize,
        )
    }
    fn save_selected_provider(
        &self,
        store: &Store,
        request: &Request,
        catalog: Option<&Selection>,
        input: Option<&SourceRequest>,
        authorize: impl FnOnce() -> Result<(), ConfigError>,
    ) -> Result<Operation, ConfigError> {
        if request.kind != Kind::Save {
            return Err(changed());
        }
        let path = self.catalog_source_path(&request.store_id, &request.provider.id, false)?;
        let mut before = None;
        let result = store.start_provider_operation_before_commit(request, |operation| {
            if let Some(input) = input {
                self.record_source_operation(store, input)?;
            } else {
                self.record_catalog_operation(store, request, catalog.ok_or_else(unavailable)?)?;
            }
            let snapshot = files::read(&path)?;
            if snapshot.stamp.digest != self.catalog_operation_source_revision(request)? {
                return Err(changed());
            }
            before = Some(SaveBefore {
                source: snapshot,
                marker: self.read_catalog_marker(&request.store_id, &request.provider.id)?,
            });
            self.stage_catalog_source(
                store,
                request,
                operation.provider.as_ref().ok_or_else(unavailable)?,
            )?;
            authorize()
        });
        let operation = match result {
            Ok(start) => start.operation,
            Err(error) => {
                // A confirmed original receipt is authoritative even if COMMIT
                // acknowledgement failed. Otherwise restore only our own stage,
                // and only after proving the SQL clock/metadata rolled back.
                if let Ok(Some(op)) = store.existing_provider_operation(request) {
                    if op.status == Status::Committed
                        && self
                            .catalog_operation_choice(request)
                            .is_ok_and(|s| s.as_ref() == catalog)
                        && input.is_none_or(|i| self.verify_source_operation(i).is_ok())
                    {
                        op
                    } else {
                        return Err(unavailable());
                    }
                } else {
                    if let Some(snapshot) = before {
                        self.undo_catalog_save(store, request, snapshot)
                            .map_err(|_| unavailable())?;
                    }
                    return Err(error);
                }
            }
        };
        if operation.status == Status::Cancelled {
            return Ok(operation);
        }
        if self.catalog_operation_choice(request)?.as_ref() != catalog
            || input.is_some_and(|i| self.verify_source_operation(i).is_err())
        {
            return Err(unavailable());
        }
        let _ = self.finish_catalog_source(store, request);
        let _ = self.finish_catalog_operation_record(store, request);
        Ok(operation)
    }
    fn undo_catalog_save(
        &self,
        store: &Store,
        request: &Request,
        before: SaveBefore,
    ) -> Result<(), ConfigError> {
        let _guard = self.lock()?;
        if store.existing_provider_operation(request)?.is_some()
            || store.provider_write_protocol()?.sequence != request.expected_sequence
            || store
                .provider_by_id(&request.provider.id)
                .map_err(|_| unavailable())?
                .as_ref()
                .map_or(0, |p| p.revision)
                != request.provider.revision
        {
            return Err(unavailable());
        }
        let path = self.catalog_source_path(&request.store_id, &request.provider.id, false)?;
        if before.source.bytes.is_none() && files::read(&path)?.bytes.is_none() {
            if before.marker.bytes.is_none() {
                let marker =
                    self.catalog_marker_path(&request.store_id, &request.provider.id, false)?;
                let current = self.read_catalog_marker(&request.store_id, &request.provider.id)?;
                files::replace(&marker, &current.stamp, None, 0o600)?;
            }
            return Ok(());
        }
        let (source, revision) = self.read_catalog_source(store, &request.provider)?;
        if let Some(pending) = &source.pending {
            if pending.operation_id != request.operation_id
                || pending.sequence != request.expected_sequence + 1
                || pending.request_hash != request_hash(request)?
            {
                return Err(unavailable());
            }
            let path = self.catalog_source_path(&request.store_id, &request.provider.id, false)?;
            let current = files::read(&path)?;
            if current.stamp.digest != revision {
                return Err(unavailable());
            }
            files::replace(
                &path,
                &current.stamp,
                before.source.bytes.as_deref(),
                before.source.stamp.mode,
            )?;
            if before.marker.bytes.is_none() {
                let marker =
                    self.catalog_marker_path(&request.store_id, &request.provider.id, false)?;
                let current = self.read_catalog_marker(&request.store_id, &request.provider.id)?;
                files::replace(&marker, &current.stamp, None, 0o600)?;
            }
        } else if self.read_catalog_source(store, &request.provider)?.1
            != before.source.stamp.digest
        {
            return Err(unavailable());
        }
        Ok(())
    }
}
