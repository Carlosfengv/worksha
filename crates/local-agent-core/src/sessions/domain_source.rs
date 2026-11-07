//! Built-in sources execute shared domain operations. Native
//! index artifacts and the full legacy response remain private to this facade.
use super::source_plugins::{ReadOnlySessionSource, SourceContext, SourceScan};
use super::{Catalog, Conversation, IndexedFile, NativeError, NativeResult, jsonl::JsonlSource};
use crate::{
    domain as d,
    plugin::{BuiltinPlugin, PluginDescriptor, PluginError, PluginResources},
};
use d::source::SessionSource;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
};

pub(super) const PROVIDER_KEY: &str = "builtin:domain.session-source@1:claude";
pub(super) struct BuiltinSessionAdapter(Arc<dyn ReadOnlySessionSource>);

pub(super) fn domain_failure(error: &NativeError, cancelled: bool) -> d::Failure {
    let code = if cancelled {
        d::ErrorCode::Cancelled
    } else {
        match error.code {
            "unauthorized" => d::ErrorCode::GrantRevoked,
            "plugin_disabled" => d::ErrorCode::PluginDisabled,
            "stale_snapshot" => d::ErrorCode::StaleSnapshot,
            "invalid_input" => d::ErrorCode::InvalidInput,
            "not_found" => d::ErrorCode::NotFound,
            "unsupported" => d::ErrorCode::Unsupported,
            "resource_limit" => d::ErrorCode::ResourceLimit,
            "deadline_exceeded" => d::ErrorCode::DeadlineExceeded,
            _ => d::ErrorCode::Unavailable,
        }
    };
    d::Failure::new(code)
}
impl BuiltinSessionAdapter {
    pub fn claude() -> Self {
        Self::wrap(Arc::new(JsonlSource("claude")))
    }
    pub fn wrap(source: Arc<dyn ReadOnlySessionSource>) -> Self {
        Self(source)
    }
    fn provider_key(&self) -> String {
        if self.agent() == "claude" {
            PROVIDER_KEY.into()
        } else {
            format!("builtin:domain.session-source@1:{}", self.agent())
        }
    }
}
impl BuiltinPlugin for BuiltinSessionAdapter {
    fn display_name(&self) -> String {
        let name = crate::agents::builtin_directory()
            .source(self.agent())
            .map(|source| source.name.as_str())
            .unwrap_or(self.agent());
        format!("{name} 本机来源")
    }
    fn descriptor(&self) -> PluginDescriptor {
        self.0.descriptor()
    }
    fn prepare(&self) -> Result<PluginResources, PluginError> {
        self.0.prepare()
    }
    fn capability_metadata(
        &self,
        key: &crate::plugin::CapabilityKey,
    ) -> crate::plugin::builtin::CapabilityMetadata {
        use crate::plugin::builtin::{CapabilityKind, CapabilityMetadata, ResourceScope};
        CapabilityMetadata {
            kind: if key.name == "session.activity" {
                CapabilityKind::Stream
            } else {
                CapabilityKind::Query
            },
            resource_scope: ResourceScope::NativeSource,
        }
    }
}

struct NativeBridge<'a> {
    source: &'a dyn ReadOnlySessionSource,
    context: &'a SourceContext<'a>,
    expected: d::Invocation,
    previous: Option<&'a HashMap<PathBuf, IndexedFile>>,
    indexed: Option<&'a IndexedFile>,
    scan: Option<SourceScan>,
    conversation: Option<Conversation>,
    native_error: Option<NativeError>,
}
impl NativeBridge<'_> {
    fn check(&mut self, invocation: &d::Invocation, source_id: &str) -> d::Result<()> {
        if invocation != &self.expected || source_id != self.source.agent() {
            return Err(d::Failure::new(d::ErrorCode::StaleSnapshot));
        }
        self.capture(self.context.check())
    }
    fn capture<T>(&mut self, result: NativeResult<T>) -> d::Result<T> {
        result.map_err(|error| {
            let failure = domain_failure(&error, self.context.domain_cancelled());
            self.native_error = Some(error);
            failure
        })
    }
    fn legacy_error(&mut self, failure: d::Failure) -> NativeError {
        self.native_error.take().unwrap_or_else(|| {
            NativeError::new(
                match failure.code {
                    d::ErrorCode::StaleSnapshot => "stale_snapshot",
                    _ => "unavailable",
                },
                "来源结果未通过领域契约核查，未发布新结果",
            )
        })
    }
}

impl SessionSource for NativeBridge<'_> {
    fn scan(
        &mut self,
        request: &d::ScanRequest,
        emit: &mut dyn FnMut(d::SourceBatch) -> d::Result<()>,
    ) -> d::Result<d::Checkpoint> {
        self.check(&request.invocation, &request.source_id)?;
        let previous = self
            .previous
            .ok_or_else(|| d::Failure::new(d::ErrorCode::InvalidInput))?;
        let result = self.source.scan(self.context, previous);
        let mut scan = self.capture(result)?;
        self.check(&request.invocation, &request.source_id)?;
        let catalog = Catalog {
            sources: vec![],
            files: std::mem::take(&mut scan.files),
            revision: 0,
            as_of: String::new(),
        };
        let mut records = Vec::with_capacity(d::source::BATCH_ITEMS);
        let mut count = 0u64;
        let mut batches = 0u64;
        let revision = request.invocation.invocation_id.clone();
        for file in super::unique_files(&catalog) {
            let s = &file.summary;
            records.push(d::SessionRecord {
                id: s.id.clone(),
                native_id: s.native_id.clone(),
                product_ids: s.agent_products.clone(),
                unknown_client: s.has_unknown_client,
                title: s.title.clone(),
                created_at: s.created_at.clone(),
                updated_at: s.updated_at.clone(),
                project_ref: s.project_id.clone(),
                usage: (&s.usage).into(),
                partial: s.partial,
            });
            count += 1;
            if records.len() == d::source::BATCH_ITEMS {
                self.check(&request.invocation, &request.source_id)?;
                emit(d::SourceBatch {
                    source_id: request.source_id.clone(),
                    source_revision: revision.clone(),
                    batch_id: batches.into(),
                    records: std::mem::take(&mut records),
                })?;
                batches += 1;
            }
        }
        if !records.is_empty() {
            self.check(&request.invocation, &request.source_id)?;
            emit(d::SourceBatch {
                source_id: request.source_id.clone(),
                source_revision: revision.clone(),
                batch_id: batches.into(),
                records,
            })?;
            batches += 1;
        }
        scan.files = catalog.files;
        self.check(&request.invocation, &request.source_id)?;
        let coverage = if scan.error.is_some() || scan.skipped > 0 {
            d::Coverage::Partial
        } else {
            d::Coverage::Complete
        };
        self.scan = Some(scan);
        Ok(d::Checkpoint {
            source_revision: revision,
            batches: batches.into(),
            records: count.into(),
            complete: true,
            coverage,
        })
    }
    fn read_conversation(
        &mut self,
        request: &d::ConversationRequest,
    ) -> d::Result<d::ConversationPage> {
        self.check(&request.invocation, &request.source_id)?;
        let indexed = self
            .indexed
            .filter(|f| f.summary.id == request.session_id)
            .ok_or_else(|| d::Failure::new(d::ErrorCode::InvalidInput))?;
        let result = self.source.conversation(
            self.context,
            indexed,
            request.cursor.as_deref(),
            request.anchor.as_deref(),
        );
        let result = self.capture(result)?;
        self.check(&request.invocation, &request.source_id)?;
        let page = d::ConversationPage {
            session_id: request.session_id.clone(),
            events: result
                .items
                .iter()
                .map(|e| d::ConversationEvent {
                    id: e.id.clone(),
                    native_id: e.native_id.clone(),
                    turn_id: e.turn_id.clone(),
                    kind: e.kind.clone(),
                    role: e.role.clone(),
                    status: e.status.clone(),
                    text: e.text.clone(),
                    input: e.input.clone(),
                    output: e.output.clone(),
                    model: e.model.clone(),
                    response_id: e.model_response_id.clone(),
                    usage: e.usage.as_ref().map(Into::into),
                })
                .collect(),
            next_cursor: result.next_cursor.clone(),
            complete: result.next_cursor.is_none(),
        };
        self.conversation = Some(result);
        Ok(page)
    }
}
impl ReadOnlySessionSource for BuiltinSessionAdapter {
    fn domain_provider(&self) -> Option<d::Provider> {
        Some(d::Provider {
            provider_key: self.provider_key(),
            plugin_id: self.descriptor().id.into(),
            implementation_version: env!("CARGO_PKG_VERSION").into(),
            implementation_identity: format!(
                "local-agent-core@{}:{}",
                env!("CARGO_PKG_VERSION"),
                self.agent()
            ),
            capability: d::Capability {
                name: "domain.session-source".into(),
                major: 1,
                instance_id: self.agent().into(),
            },
            execution_kind: d::ExecutionKind::Builtin,
        })
    }
    fn agent(&self) -> &'static str {
        self.0.agent()
    }
    fn locations(&self, home: &Path) -> Vec<PathBuf> {
        self.0.locations(home)
    }
    fn capabilities(&self) -> Vec<super::Capability> {
        self.0.capabilities()
    }
    fn activity_roots(&self, home: &Path) -> Vec<PathBuf> {
        self.0.activity_roots(home)
    }
    fn has_database_activity(&self) -> bool {
        self.0.has_database_activity()
    }
    fn activities(
        &self,
        context: &SourceContext<'_>,
        files: &[IndexedFile],
        now: chrono::DateTime<chrono::Utc>,
    ) -> NativeResult<Option<(Vec<super::live::NativeActivity>, bool)>> {
        self.0.activities(context, files, now)
    }
    fn scan(
        &self,
        context: &SourceContext<'_>,
        previous: &HashMap<PathBuf, IndexedFile>,
    ) -> NativeResult<SourceScan> {
        let request = d::ScanRequest {
            invocation: context.domain_invocation(
                &self
                    .domain_provider()
                    .ok_or_else(|| NativeError::new("unavailable", "领域来源描述不可用"))?,
            ),
            source_id: self.agent().into(),
        };
        let mut bridge = NativeBridge {
            source: self.0.as_ref(),
            context,
            expected: request.invocation.clone(),
            previous: Some(previous),
            indexed: None,
            scan: None,
            conversation: None,
            native_error: None,
        };
        let mut publication = d::source::ScanPublication::new(self.agent());
        let checkpoint = bridge
            .scan(&request, &mut |batch| publication.accept(batch))
            .map_err(|e| bridge.legacy_error(e))?;
        publication
            .finish(&checkpoint)
            .map_err(|e| bridge.legacy_error(e))?;
        context.check()?;
        bridge
            .scan
            .take()
            .ok_or_else(|| NativeError::new("unavailable", "来源未完成扫描"))
    }
    fn conversation(
        &self,
        context: &SourceContext<'_>,
        indexed: &IndexedFile,
        cursor: Option<&str>,
        anchor: Option<&str>,
    ) -> NativeResult<Conversation> {
        let request = d::ConversationRequest {
            invocation: context.domain_invocation(
                &self
                    .domain_provider()
                    .ok_or_else(|| NativeError::new("unavailable", "领域来源描述不可用"))?,
            ),
            source_id: self.agent().into(),
            session_id: indexed.summary.id.clone(),
            cursor: cursor.map(Into::into),
            anchor: anchor.map(Into::into),
        };
        let mut bridge = NativeBridge {
            source: self.0.as_ref(),
            context,
            expected: request.invocation.clone(),
            previous: None,
            indexed: Some(indexed),
            scan: None,
            conversation: None,
            native_error: None,
        };
        bridge
            .read_conversation(&request)
            .map_err(|e| bridge.legacy_error(e))?;
        context.check()?;
        bridge
            .conversation
            .take()
            .ok_or_else(|| NativeError::new("unavailable", "来源未完成正文读取"))
    }
    fn memory(
        &self,
        context: &SourceContext<'_>,
        agent: &str,
        selected: Option<&str>,
        projects: &std::collections::BTreeSet<String>,
    ) -> NativeResult<super::MemorySnapshot> {
        super::domain_memory::read_native(
            self.0.as_ref(),
            context,
            self.domain_provider()
                .ok_or_else(|| NativeError::new("unavailable", "领域来源描述不可用"))?,
            agent,
            selected,
            projects,
        )
    }
    fn mcp_config(
        &self,
        context: &SourceContext<'_>,
        phase: super::mcp::ConfigPhase,
        scope: &mut super::mcp::ConfigScope<'_>,
        inventory: &mut super::mcp::Inventory,
    ) -> NativeResult<()> {
        self.0.mcp_config(context, phase, scope, inventory)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn domain_failures_keep_cancellation_and_revocation_nonretryable_without_exposing_native_text()
    {
        for (native, expected) in [
            ("unauthorized", d::ErrorCode::GrantRevoked),
            ("not_found", d::ErrorCode::NotFound),
            ("plugin_disabled", d::ErrorCode::PluginDisabled),
            ("stale_snapshot", d::ErrorCode::StaleSnapshot),
        ] {
            let error = NativeError::new(native, "private source and native diagnostic");
            let failure = domain_failure(&error, false);
            assert_eq!(failure.code, expected);
            assert!(!failure.retryable);
            assert!(!serde_json::to_string(&failure).unwrap().contains("private"));
            assert_eq!(domain_failure(&error, true).code, d::ErrorCode::Cancelled);
        }
    }
}
