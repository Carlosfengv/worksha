//! The legacy memory consumer is assembled from a common result and private native paths.
use super::{
    MemorySnapshot, NativeError, NativeResult,
    source_plugins::{ReadOnlySessionSource, SourceContext},
};
use crate::domain::{self as d, memory as m};
use m::MemorySource;
use std::collections::{BTreeMap, BTreeSet};

struct NativeRead<'a> {
    broker: &'a dyn Fn() -> NativeResult<MemorySnapshot>,
    context: &'a SourceContext<'a>,
    expected: m::MemoryRequest,
    paths: BTreeMap<String, (String, Option<String>)>,
    warnings: Vec<String>,
    error: Option<NativeError>,
}
impl NativeRead<'_> {
    fn capture<T>(&mut self, result: NativeResult<T>) -> d::Result<T> {
        result.map_err(|error| {
            let failure =
                super::domain_source::domain_failure(&error, self.context.domain_cancelled());
            self.error = Some(error);
            failure
        })
    }
}

impl MemorySource for NativeRead<'_> {
    fn read_memory(&mut self, request: &m::MemoryRequest) -> d::Result<m::MemorySnapshot> {
        if request != &self.expected {
            return Err(d::Failure::new(d::ErrorCode::StaleSnapshot));
        }
        self.capture(self.context.check())?;
        let native = self.capture((self.broker)())?;
        self.capture(self.context.check())?;
        if native.agent_id != request.product_id {
            return Err(d::Failure::new(d::ErrorCode::InvalidInput));
        }
        let mut files = Vec::with_capacity(native.files.len());
        for file in native.files {
            self.paths
                .insert(file.id.clone(), (file.path, file.project));
            files.push(m::MemoryFile {
                id: file.id,
                name: file.name,
                scope_label: file.scope,
                size_bytes: file.size_bytes.into(),
                modified_at: file.modified_at,
            });
        }
        self.warnings = native.warnings;
        let result = m::MemorySnapshot {
            source_id: request.source_id.clone(),
            product_id: native.agent_id,
            shared_product_ids: native.shared_agents,
            files,
            selected: native.selected.map(|p| m::MemoryPreview {
                id: p.id,
                text: p.text,
                truncated: p.truncated,
            }),
            observed_at: native.as_of,
            coverage: d::Coverage::Partial,
            limited: native.limited,
            loaded_in_client: None,
        };
        result.validate(request)?;
        Ok(result)
    }
}

pub(super) fn read_native(
    source: &dyn ReadOnlySessionSource,
    context: &SourceContext<'_>,
    mut provider: d::Provider,
    product: &str,
    selected: Option<&str>,
    projects: &BTreeSet<String>,
) -> NativeResult<MemorySnapshot> {
    provider.provider_key = format!("builtin:domain.memory-source@1:{}", source.agent());
    provider.capability.name = "domain.memory-source".into();
    let expected = m::MemoryRequest {
        invocation: context.domain_invocation(&provider),
        source_id: source.agent().into(),
        product_id: product.into(),
        file_id: selected.map(Into::into),
    };
    let broker = || source.memory(context, product, selected, projects);
    let mut reader = NativeRead {
        broker: &broker,
        context,
        expected: expected.clone(),
        paths: BTreeMap::new(),
        warnings: vec![],
        error: None,
    };
    let result = reader.read_memory(&expected).map_err(|_| {
        reader
            .error
            .take()
            .unwrap_or_else(|| NativeError::new("unavailable", "记忆结果未通过领域契约核查"))
    })?;
    context.check()?;
    let files = result
        .files
        .into_iter()
        .map(|file| {
            let (path, project) = reader
                .paths
                .remove(&file.id)
                .ok_or_else(|| NativeError::new("unavailable", "记忆文件身份已变化"))?;
            Ok(super::memory::MemoryFile {
                id: file.id,
                name: file.name,
                path,
                scope: file.scope_label,
                project,
                size_bytes: file.size_bytes.value(),
                modified_at: file.modified_at,
            })
        })
        .collect::<NativeResult<_>>()?;
    Ok(MemorySnapshot {
        agent_id: result.product_id,
        shared_agents: result.shared_product_ids,
        files,
        selected: result.selected.map(|p| super::memory::MemoryPreview {
            id: p.id,
            text: p.text,
            truncated: p.truncated,
        }),
        as_of: result.observed_at,
        limited: result.limited,
        warnings: reader.warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sessions::{self as s, NativeSessions};
    use std::{cell::Cell, fs};

    fn request(
        context: &SourceContext<'_>,
        source: &dyn ReadOnlySessionSource,
    ) -> m::MemoryRequest {
        let mut provider = source.domain_provider().unwrap();
        provider.provider_key = format!("builtin:domain.memory-source@1:{}", source.agent());
        provider.capability.name = "domain.memory-source".into();
        m::MemoryRequest {
            invocation: context.domain_invocation(&provider),
            source_id: source.agent().into(),
            product_id: "codex-cli".into(),
            file_id: None,
        }
    }
    fn reader<'a>(
        broker: &'a dyn Fn() -> NativeResult<MemorySnapshot>,
        context: &'a SourceContext<'a>,
        expected: m::MemoryRequest,
    ) -> NativeRead<'a> {
        NativeRead {
            broker,
            context,
            expected,
            paths: BTreeMap::new(),
            warnings: vec![],
            error: None,
        }
    }

    #[test]
    fn common_memory_result_has_no_native_path_body_or_claim_of_client_loading() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().canonicalize().unwrap();
        fs::create_dir_all(home.join(".codex")).unwrap();
        fs::write(home.join(".codex/AGENTS.md"), "not-requested-body-sentinel").unwrap();
        let sessions = NativeSessions::new(home.clone(), &["codex".into()]);
        sessions
            .with_source_context("codex", None, true, |source, context| {
                let request = request(context, source);
                let projects = BTreeSet::new();
                let broker = || s::memory::read(context, "codex-cli", "codex", None, &projects);
                let mut reader = reader(&broker, context, request.clone());
                let result = reader.read_memory(&request).unwrap();
                assert_eq!(result.coverage, d::Coverage::Partial);
                assert_eq!(result.loaded_in_client, None);
                assert_eq!(result.shared_product_ids, ["codex-cli", "codex-desktop"]);
                assert!(result.selected.is_none());
                let encoded = serde_json::to_string(&result).unwrap();
                assert!(!encoded.contains("not-requested-body-sentinel"));
                assert!(!encoded.contains(home.to_str().unwrap()));
                assert!(!encoded.contains("\"path\""));
                assert!(
                    reader
                        .paths
                        .values()
                        .any(|(p, _)| p.ends_with("/AGENTS.md"))
                );
                let mut invalid = result.clone();
                invalid.files.push(invalid.files[0].clone());
                assert_eq!(
                    invalid.validate(&request).unwrap_err().code,
                    d::ErrorCode::InvalidInput
                );
                invalid = result;
                invalid.source_id = "claude".into();
                assert!(invalid.validate(&request).is_err());
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn forged_invocation_or_file_selection_never_reaches_memory_broker() {
        let temp = tempfile::tempdir().unwrap();
        let sessions = NativeSessions::new(temp.path().canonicalize().unwrap(), &["codex".into()]);
        sessions
            .with_source_context("codex", None, true, |source, context| {
                let expected = request(context, source);
                let calls = Cell::new(0);
                let broker = || {
                    calls.set(calls.get() + 1);
                    Err(NativeError::new("unavailable", "must not run"))
                };
                let mut reader = reader(&broker, context, expected.clone());
                for mutation in 0..4 {
                    let mut forged = expected.clone();
                    match mutation {
                        0 => forged.invocation.resource_ref = "other-resource".into(),
                        1 => {
                            forged.invocation.implementation_identity =
                                "other-implementation".into()
                        }
                        2 => forged.file_id = Some("other-file".into()),
                        _ => forged.product_id = "codex-desktop".into(),
                    }
                    assert_eq!(
                        reader.read_memory(&forged).unwrap_err().code,
                        d::ErrorCode::StaleSnapshot
                    );
                }
                assert_eq!(calls.get(), 0);
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn memory_revoked_during_read_does_not_publish_metadata_or_preview() {
        let temp = tempfile::tempdir().unwrap();
        let sessions = NativeSessions::new(temp.path().canonicalize().unwrap(), &["codex".into()]);
        let outcome = sessions.with_source_context("codex", None, true, |source, context| {
            let expected = request(context, source);
            let broker = || {
                sessions.set_enabled("codex", false)?;
                Ok(MemorySnapshot {
                    agent_id: "codex-cli".into(),
                    shared_agents: vec![],
                    files: vec![],
                    selected: None,
                    as_of: s::now(),
                    limited: false,
                    warnings: vec![],
                })
            };
            let mut reader = reader(&broker, context, expected.clone());
            assert_eq!(
                reader.read_memory(&expected).unwrap_err().code,
                d::ErrorCode::GrantRevoked
            );
            assert_eq!(reader.error.take().unwrap().code, "unauthorized");
            assert!(reader.paths.is_empty());
            Ok(())
        });
        assert_eq!(outcome.unwrap_err().code, "unauthorized");
    }
}
