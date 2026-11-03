//! Typed, trusted read-only source seam. The catalog and consent stay in NativeSessions.
#[cfg(test)]
mod domain_tests;
use super::{Conversation, IndexedFile, NativeError, NativeResult, NativeSessions};
use crate::plugin::{BuiltinPlugin, CapabilityKey, PluginHost, PluginLease};
use std::{collections::BTreeMap, collections::HashMap, path::Path, path::PathBuf, sync::Arc};

pub(super) fn capability(agent: &'static str) -> CapabilityKey {
    CapabilityKey {
        name: "session.source",
        version: 1,
        instance: agent,
    }
}

pub(super) struct SourceContext<'a> {
    pub home: &'a Path,
    pub directory: &'a crate::agents::AgentDirectory,
    pub index_cache: &'a std::sync::Mutex<super::parser::IndexCache>,
    lease: PluginLease,
    grant_revision: u64,
    authorized: &'a dyn Fn() -> NativeResult<bool>,
    progress: &'a dyn Fn(usize, usize) -> NativeResult<()>,
    cancelled: &'a dyn Fn() -> bool,
}
impl SourceContext<'_> {
    pub(super) fn domain_cancelled(&self) -> bool {
        (self.cancelled)()
    }
    pub(super) fn domain_invocation(
        &self,
        provider: &crate::domain::Provider,
    ) -> crate::domain::Invocation {
        crate::domain::Invocation {
            invocation_id: uuid::Uuid::new_v4().to_string(),
            provider_key: provider.provider_key.clone(),
            implementation_identity: provider.implementation_identity.clone(),
            epoch: self.lease.epoch.into(),
            grant_revision: self.grant_revision.into(),
            catalog_revision: 1u64.into(),
            resource_ref: uuid::Uuid::new_v4().to_string(),
            deadline_ms: None,
            budget: crate::domain::Budget {
                batch_items: crate::domain::source::BATCH_ITEMS as u16,
                max_records: u64::MAX.into(),
                max_document_bytes: (512u64 * 1024 * 1024).into(),
            },
        }
    }
    pub fn check(&self) -> NativeResult<()> {
        self.lease
            .ensure_active()
            .map_err(|e| NativeError::new(e.code, &e.message))?;
        if !(self.authorized)()? {
            return Err(NativeError::new("unauthorized", "此会话来源已断开"));
        }
        Ok(())
    }
    pub fn progress(&self, total: usize, scanned: usize) -> NativeResult<()> {
        self.check()?;
        (self.progress)(total, scanned)
    }
}

pub(super) struct SourceScan {
    pub files: HashMap<PathBuf, IndexedFile>,
    pub total: usize,
    pub skipped: usize,
    pub error: Option<String>,
    pub scanned: usize,
    pub count: usize,
    pub versions: Vec<String>,
    pub revision_delta: u64,
}
impl SourceScan {
    pub fn database(
        files: HashMap<PathBuf, IndexedFile>,
        total: usize,
        skipped: usize,
        error: Option<String>,
        previous: &HashMap<PathBuf, IndexedFile>,
    ) -> Self {
        let changed = previous.len() != files.len()
            || files.iter().any(|(p, f)| {
                previous
                    .get(p)
                    .is_none_or(|old| old.database_revision != f.database_revision)
            });
        Self {
            count: files.len(),
            scanned: total.saturating_sub(skipped),
            files,
            total,
            skipped,
            error,
            versions: vec![],
            revision_delta: u64::from(changed),
        }
    }
}

pub(super) fn descriptor(agent: &'static str, id: &'static str) -> crate::plugin::PluginDescriptor {
    crate::plugin::PluginDescriptor {
        id,
        version: "1.0.0",
        requires: vec![],
        provides: [
            "session.source",
            "session.activity",
            "session.memory",
            "session.mcp",
            "session.writes",
        ]
        .into_iter()
        .map(|name| CapabilityKey {
            name,
            version: 1,
            instance: agent,
        })
        .collect(),
    }
}

pub(super) trait ReadOnlySessionSource: BuiltinPlugin {
    fn agent(&self) -> &'static str;
    fn domain_provider(&self) -> Option<crate::domain::Provider> {
        None
    }
    fn locations(&self, home: &Path) -> Vec<PathBuf> {
        super::source_locations(home, self.agent())
    }
    fn capabilities(&self) -> Vec<super::Capability> {
        super::source_capabilities(self.agent())
    }
    fn scan(
        &self,
        context: &SourceContext<'_>,
        previous: &HashMap<PathBuf, IndexedFile>,
    ) -> NativeResult<SourceScan>;
    fn conversation(
        &self,
        context: &SourceContext<'_>,
        indexed: &IndexedFile,
        cursor: Option<&str>,
        anchor: Option<&str>,
    ) -> NativeResult<Conversation>;

    fn activity_roots(&self, home: &Path) -> Vec<PathBuf> {
        super::roots(home, self.agent())
    }
    fn memory(
        &self,
        _context: &SourceContext<'_>,
        _agent: &str,
        _selected: Option<&str>,
        _projects: &std::collections::BTreeSet<String>,
    ) -> NativeResult<super::MemorySnapshot> {
        Err(NativeError::new(
            "unsupported",
            "此来源尚未提供记忆读取能力",
        ))
    }
    fn activities(
        &self,
        _context: &SourceContext<'_>,
        _files: &[IndexedFile],
        _now: chrono::DateTime<chrono::Utc>,
    ) -> NativeResult<Option<(Vec<super::live::NativeActivity>, bool)>> {
        Ok(None)
    }
    fn has_database_activity(&self) -> bool {
        false
    }
    fn mcp_config(
        &self,
        _context: &SourceContext<'_>,
        _phase: super::mcp::ConfigPhase,
        _scope: &mut super::mcp::ConfigScope<'_>,
        _inventory: &mut super::mcp::Inventory,
    ) -> NativeResult<()> {
        Ok(())
    }
}

/// An in-memory built-in composition, not an external plugin SDK or a persisted grant.
pub struct NativeSourcePlugins {
    host: PluginHost,
    pub(super) sources: BTreeMap<&'static str, Arc<dyn ReadOnlySessionSource>>,
    pub(super) directory: Arc<crate::agents::AgentDirectory>,
}
impl NativeSourcePlugins {
    pub fn builtin() -> Result<Self, crate::plugin::PluginError> {
        Self::mount(Self::builtins())
    }

    fn builtins() -> Vec<Arc<dyn ReadOnlySessionSource>> {
        vec![
            Arc::new(super::domain_source::BuiltinSessionAdapter::claude()),
            Arc::new(super::domain_source::BuiltinSessionAdapter::wrap(Arc::new(
                super::jsonl::JsonlSource("codex"),
            ))),
            Arc::new(super::domain_source::BuiltinSessionAdapter::wrap(Arc::new(
                super::opencode::OpenCodeSource,
            ))),
            Arc::new(super::domain_source::BuiltinSessionAdapter::wrap(Arc::new(
                super::antigravity::AntigravitySource,
            ))),
        ]
    }

    fn mount(
        sources: Vec<Arc<dyn ReadOnlySessionSource>>,
    ) -> Result<Self, crate::plugin::PluginError> {
        let builtins = crate::agents::builtin_directory();
        let directory = crate::agents::AgentDirectory::new(
            builtins
                .sources()
                .iter()
                .filter(|descriptor| sources.iter().any(|s| s.agent() == descriptor.id))
                .cloned()
                .collect(),
            builtins
                .products()
                .iter()
                .filter(|descriptor| sources.iter().any(|s| s.agent() == descriptor.family))
                .cloned()
                .collect(),
        )
        .map_err(|message| crate::plugin::PluginError {
            code: "invalid_input",
            message: message.into(),
        })?;
        Self::mount_with_directory(sources, Arc::new(directory))
    }

    fn mount_with_directory(
        sources: Vec<Arc<dyn ReadOnlySessionSource>>,
        directory: Arc<crate::agents::AgentDirectory>,
    ) -> Result<Self, crate::plugin::PluginError> {
        let registered: std::collections::BTreeSet<_> = sources.iter().map(|s| s.agent()).collect();
        if registered.len() != sources.len()
            || registered.len() != directory.sources().len()
            || directory
                .sources()
                .iter()
                .any(|s| !registered.contains(s.id.as_str()))
            || sources
                .iter()
                .any(|s| !s.descriptor().provides.contains(&capability(s.agent())))
        {
            return Err(crate::plugin::PluginError {
                code: "invalid_input",
                message: "来源目录与能力注册不一致".into(),
            });
        }
        let plugins: Vec<&dyn BuiltinPlugin> = sources
            .iter()
            .map(|source| source.as_ref() as &dyn BuiltinPlugin)
            .collect();
        let host = PluginHost::mount(&plugins)?;
        Ok(Self {
            host,
            sources: sources.into_iter().map(|s| (s.agent(), s)).collect(),
            directory,
        })
    }

    pub(super) fn active(&self, agent: &'static str) -> bool {
        self.host.acquire(&capability(agent)).is_ok()
    }
    pub(super) fn lease(&self, agent: &str) -> Option<crate::plugin::PluginLease> {
        self.sources
            .get(agent)
            .and_then(|source| self.host.acquire(&capability(source.agent())).ok())
    }
}

impl NativeSessions {
    pub fn directory(&self) -> &crate::agents::AgentDirectory {
        &self.source_plugins.directory
    }
    pub fn source_active(&self, agent: &str) -> bool {
        self.source_plugins
            .sources
            .get(agent)
            .is_some_and(|s| self.source_plugins.active(s.agent()))
    }
    pub fn pause_background_scan(&self) -> NativeResult<()> {
        let mut state = self.lock()?;
        let mut changed = false;
        for source in state
            .sources
            .iter_mut()
            .filter(|s| s.enabled && self.source_active(&s.agent_id))
        {
            changed |= source.error.as_deref() != Some("后台会话采集已停止，可手动重新检查");
            source.status = "partial".into();
            source.error = Some("后台会话采集已停止，可手动重新检查".into());
        }
        if changed {
            state.revision += 1;
            state.as_of = super::now();
        }
        Ok(())
    }
    pub(super) fn active_sources(&self) -> NativeResult<Vec<super::SourceStatus>> {
        Ok(self
            .sources()?
            .into_iter()
            .map(|mut source| {
                source.enabled &= self.source_active(&source.agent_id);
                source
            })
            .collect())
    }
    pub(super) fn activity_roots(&self, agent: &str) -> Vec<PathBuf> {
        self.source_plugins
            .sources
            .get(agent)
            .filter(|_| self.source_active(agent))
            .map(|source| source.activity_roots(&self.home))
            .unwrap_or_default()
    }
    pub(super) fn database_activities(
        &self,
        source: &super::SourceStatus,
    ) -> NativeResult<Option<(Vec<super::live::NativeActivity>, bool)>> {
        if !self
            .source_plugins
            .sources
            .get(source.agent_id.as_str())
            .is_some_and(|p| p.has_database_activity())
        {
            return Ok(None);
        }
        let files = self
            .lock()?
            .files
            .values()
            .filter(|file| file.agent == source.agent_id)
            .cloned()
            .collect::<Vec<_>>();
        self.with_source_context(
            &source.agent_id,
            Some(source.authorization_revision),
            true,
            |plugin, context| plugin.activities(context, &files, chrono::Utc::now()),
        )
    }
    pub(super) fn with_source_context<T>(
        &self,
        agent: &str,
        expected: Option<u64>,
        consent: bool,
        read: impl FnOnce(&dyn ReadOnlySessionSource, &SourceContext<'_>) -> NativeResult<T>,
    ) -> NativeResult<T> {
        let source = self
            .source_plugins
            .sources
            .get(agent)
            .ok_or_else(|| NativeError::new("unsupported", "来源插件未注册"))?;
        let revision = self
            .lock()?
            .sources
            .iter()
            .find(|s| s.agent_id == agent)
            .map(|s| s.authorization_revision)
            .ok_or_else(|| NativeError::new("unsupported", "来源未注册"))?;
        if expected.is_some_and(|expected| revision != expected) {
            return Err(NativeError::new("unauthorized", "此会话来源已断开"));
        }
        let lease = self
            .source_plugins
            .host
            .acquire(&capability(source.agent()))
            .map_err(|e| NativeError::new(e.code, &e.message))?;
        let authorized = || {
            Ok(self.lock()?.sources.iter().any(|s| {
                s.agent_id == agent
                    && s.authorization_revision == revision
                    && (!consent || s.enabled)
            }))
        };
        let context = SourceContext {
            home: &self.home,
            directory: self.directory(),
            index_cache: &self.index_cache,
            lease,
            grant_revision: revision,
            authorized: &authorized,
            progress: &|_, _| Ok(()),
            cancelled: &|| false,
        };
        context.check()?;
        let result = read(source.as_ref(), &context)?;
        context.check()?;
        Ok(result)
    }
    fn source_authorized(&self, agent: &str, revision: u64) -> NativeResult<bool> {
        Ok(self.lock()?.sources.iter().any(|source| {
            source.agent_id == agent && source.enabled && source.authorization_revision == revision
        }))
    }

    pub(super) fn refresh_source_plugins(&self, proceed: &dyn Fn() -> bool) -> NativeResult<()> {
        for descriptor in self.directory().sources() {
            let Some(plugin) = self.source_plugins.sources.get(descriptor.id.as_str()) else {
                continue;
            };
            let agent = plugin.agent();
            let (authorization_revision, previous) = {
                let state = self.lock()?;
                let Some(source) = state
                    .sources
                    .iter()
                    .find(|s| s.agent_id == agent && s.enabled)
                else {
                    continue;
                };
                let previous = state
                    .files
                    .iter()
                    .filter(|(_, f)| f.agent == agent)
                    .map(|(path, file)| (path.clone(), file.clone()))
                    .collect();
                (source.authorization_revision, previous)
            };
            let lease = match self.source_plugins.host.acquire(&capability(agent)) {
                Ok(lease) => lease,
                Err(error) => {
                    let mut state = self.lock()?;
                    if let Some(source) = state
                        .sources
                        .iter_mut()
                        .find(|s| s.agent_id == agent && s.enabled)
                    {
                        source.status = "partial".into();
                        source.error = Some(error.message);
                    }
                    continue;
                }
            };
            let authorized =
                || Ok(proceed() && self.source_authorized(agent, authorization_revision)?);
            let progress = |total, scanned| {
                let mut state = self.lock()?;
                if self.source_active(agent)
                    && let Some(source) = state.sources.iter_mut().find(|s| {
                        s.agent_id == agent
                            && s.enabled
                            && s.authorization_revision == authorization_revision
                    })
                {
                    source.status = "scanning".into();
                    source.total_files = total;
                    source.scanned_files = scanned;
                    source.error = None;
                }
                Ok(())
            };
            let context = SourceContext {
                home: &self.home,
                directory: self.directory(),
                index_cache: &self.index_cache,
                lease,
                grant_revision: authorization_revision,
                authorized: &authorized,
                progress: &progress,
                cancelled: &|| !proceed(),
            };
            let result = context
                .check()
                .and_then(|()| plugin.scan(&context, &previous));
            let mut state = self.lock()?;
            if !proceed()
                || context.lease.ensure_active().is_err()
                || !state.sources.iter().any(|s| {
                    s.agent_id == agent
                        && s.enabled
                        && s.authorization_revision == authorization_revision
                })
            {
                if !proceed()
                    && let Some(source) = state.sources.iter_mut().find(|s| {
                        s.agent_id == agent
                            && s.enabled
                            && s.authorization_revision == authorization_revision
                    })
                {
                    source.status = "partial".into();
                    source.error = Some("后台会话采集已停止，可手动重新检查".into());
                }
                drop(state);
                if let Ok(mut cache) = self.index_cache.lock() {
                    cache.revoke(agent);
                }
                continue;
            }
            let result = match result {
                Ok(result) => result,
                Err(error) => {
                    let source = state
                        .sources
                        .iter_mut()
                        .find(|s| s.agent_id == agent)
                        .unwrap();
                    source.status = "partial".into();
                    source.error = Some(error.message);
                    source.scanned_files = 0;
                    source.skipped_files = 0;
                    source.last_scan = Some(super::now());
                    state.as_of = super::now();
                    continue;
                }
            };
            state.files.retain(|_, file| file.agent != agent);
            state.files.extend(result.files);
            state.revision += result.revision_delta;
            let source = state
                .sources
                .iter_mut()
                .find(|s| s.agent_id == agent)
                .unwrap();
            source.session_count = result.count;
            source.total_files = result.total;
            source.scanned_files = result.scanned;
            source.skipped_files = result.skipped;
            source.versions = result.versions;
            source.last_scan = Some(super::now());
            source.status = if result.error.is_some() || result.skipped > 0 {
                "partial"
            } else {
                "ready"
            }
            .into();
            source.error = result.error;
            if source.status == "ready" {
                source.last_successful_scan = source.last_scan.clone();
            }
            state.as_of = super::now();
        }
        Ok(())
    }

    pub(super) fn plugin_conversation(
        &self,
        indexed: &IndexedFile,
        revision: u64,
        cursor: Option<&str>,
        anchor: Option<&str>,
    ) -> NativeResult<Conversation> {
        let plugin = &self.source_plugins.sources[indexed.agent.as_str()];
        let lease = self
            .source_plugins
            .host
            .acquire(&capability(plugin.agent()))
            .map_err(|error| NativeError::new(error.code, &error.message))?;
        let authorized = || self.source_authorized(&indexed.agent, revision);
        let context = SourceContext {
            home: &self.home,
            directory: self.directory(),
            index_cache: &self.index_cache,
            lease,
            grant_revision: revision,
            authorized: &authorized,
            progress: &|_, _| Ok(()),
            cancelled: &|| false,
        };
        context.check()?;
        let result = plugin.conversation(&context, indexed, cursor, anchor)?;
        context.check()?;
        Ok(result)
    }

    /// Internal service shutdown boundary. No product toggle or additional IPC.
    pub fn stop_source_plugins(&self) -> NativeResult<()> {
        self.source_plugins.host.stop_all();
        self.clear_stopped_sources()
    }
    pub fn stop_source_plugin(&self, agent: &str) -> NativeResult<()> {
        let source = self
            .source_plugins
            .sources
            .get(agent)
            .ok_or_else(|| NativeError::new("unsupported", "来源插件未注册"))?;
        self.source_plugins
            .host
            .stop(source.descriptor().id)
            .map_err(|error| NativeError::new(error.code, &error.message))?;
        self.clear_stopped_sources()
    }
    fn clear_stopped_sources(&self) -> NativeResult<()> {
        let mut state = self.lock()?;
        let before = state.files.len();
        state
            .files
            .retain(|_, file| self.source_active(&file.agent));
        let mut changed = before != state.files.len();
        for source in &mut state.sources {
            if source.enabled && !self.source_active(&source.agent_id) {
                changed |= source.status != "partial"
                    || source.error.as_deref() != Some("只读来源插件已停止")
                    || source.session_count != 0;
                source.status = "partial".into();
                source.error = Some("只读来源插件已停止".into());
                source.session_count = 0;
            }
        }
        if changed {
            state.revision += 1;
            state.as_of = super::now();
        }
        drop(state);
        if let Ok(mut cache) = self.index_cache.try_lock() {
            for source in self
                .directory()
                .sources()
                .iter()
                .filter(|s| !self.source_active(&s.id))
            {
                cache.revoke(&source.id);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod directory_tests;
#[cfg(test)]
mod tests;
