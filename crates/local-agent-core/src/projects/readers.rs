//! Trusted read-only project capabilities. Identity, visibility and aggregation stay in the domain.
use super::*;
use crate::plugin::{
    BuiltinPlugin, CapabilityKey, PluginDescriptor, PluginError, PluginHost, PluginLease,
    PluginResources,
};

fn key(name: &'static str) -> CapabilityKey {
    CapabilityKey {
        name,
        version: 1,
        instance: "native",
    }
}
fn descriptor(name: &'static str) -> PluginDescriptor {
    PluginDescriptor {
        id: name,
        version: "1.0.0",
        provides: vec![key(name)],
        requires: vec![],
    }
}
macro_rules! builtin {
    ($type:ident, $name:literal) => {
        struct $type;
        impl BuiltinPlugin for $type {
            fn descriptor(&self) -> PluginDescriptor {
                descriptor($name)
            }
            fn prepare(&self) -> Result<PluginResources, PluginError> {
                Ok(PluginResources::default())
            }
        }
    };
}
builtin!(SessionProjects, "project.source");
builtin!(NativeAssets, "project.assets");
builtin!(NativeGit, "project.git");
builtin!(NativeListeners, "project.listeners");

pub struct ProjectReaders {
    host: PluginHost,
    ports: ports::ProjectPorts,
    queries: std::sync::Mutex<super::query_cache::QueryCache>,
}
impl ProjectReaders {
    pub fn domain_capabilities(&self) -> Vec<crate::domain::CapabilityEntry> {
        use crate::domain as d;
        use d::ReadOperation::*;
        [
            (
                "project.source",
                "domain.project-source",
                "native",
                vec![CollectProjectEvidence],
            ),
            (
                "project.assets",
                "domain.project-source",
                "native-assets",
                vec![CollectAssets],
            ),
            (
                "project.git",
                "domain.git-reader",
                "native",
                vec![Snapshot, Diff],
            ),
            (
                "project.listeners",
                "domain.listener-source",
                "native",
                vec![CollectListeners],
            ),
        ]
        .into_iter()
        .map(|(legacy, name, instance, operations)| {
            let lease = self.lease(legacy);
            let active = lease.is_ok();
            d::CapabilityEntry {
                provider: d::Provider {
                    provider_key: format!("builtin:{name}@1:{instance}"),
                    plugin_id: legacy.into(),
                    implementation_version: env!("CARGO_PKG_VERSION").into(),
                    implementation_identity: format!(
                        "local-agent-core@{}:{legacy}",
                        env!("CARGO_PKG_VERSION")
                    ),
                    capability: d::Capability {
                        name: name.into(),
                        major: 1,
                        instance_id: instance.into(),
                    },
                    execution_kind: d::ExecutionKind::Builtin,
                },
                status: d::ProviderStatus {
                    enabled: active,
                    // Object visibility/workspace authority is still checked per request.
                    authorization: if active {
                        d::Authorization::Authorized
                    } else {
                        d::Authorization::Revoked
                    },
                    grant_revision: lease.map(|l| l.epoch).unwrap_or(0).into(),
                    runtime_active: active,
                    freshness: d::Freshness::Unread,
                },
                operations,
            }
        })
        .collect()
    }
    pub fn builtin() -> Result<Self, PluginError> {
        Self::with_ports(ports::ProjectPorts::default())
    }
    pub fn with_ports(ports: ports::ProjectPorts) -> Result<Self, PluginError> {
        let host = PluginHost::mount(&[
            &SessionProjects,
            &NativeAssets,
            &NativeGit,
            &NativeListeners,
        ])?;
        Ok(Self {
            host,
            ports,
            queries: Default::default(),
        })
    }
    fn lease(&self, name: &'static str) -> NativeResult<PluginLease> {
        self.host
            .acquire(&key(name))
            .map_err(|e| NativeError::new(e.code, &e.message))
    }
    fn check(lease: &PluginLease) -> NativeResult<()> {
        lease
            .ensure_active()
            .map_err(|e| NativeError::new(e.code, &e.message))
    }
    pub fn catalog(
        &self,
        broker: impl Fn() -> NativeResult<ProjectCatalog>,
    ) -> NativeResult<ProjectCatalog> {
        let lease = self.lease("project.source")?;
        let result = self.ports.source.catalog(&broker)?;
        Self::check(&lease)?;
        Ok(result)
    }
    pub fn query(
        &self,
        catalog: &ProjectCatalog,
        query: ProjectQuery,
        listeners: &listeners::ListenerSnapshot,
    ) -> NativeResult<ProjectPage> {
        let lease = self.lease("project.assets")?;
        let mut result = if self.ports.assets.native_cache() {
            self.queries
                .lock()
                .map_err(|_| NativeError::new("unavailable", "项目统计暂不可用"))?
                .query(catalog, query, listeners, |catalog, query, listeners| {
                    self.ports.assets.query(catalog, query, listeners)
                })?
        } else {
            self.ports.assets.query(catalog, query, listeners)?
        };
        if !self.listeners_active() {
            result.listeners = self.collect_listeners();
            for project in &mut result.items {
                project.listener_count = None;
                project.known_listener_count = 0;
            }
        }
        Self::check(&lease)?;
        Ok(result)
    }
    pub fn detail(
        &self,
        catalog: &ProjectCatalog,
        id: &str,
        query: ProjectQuery,
        listeners: &listeners::ListenerSnapshot,
    ) -> NativeResult<ProjectDetail> {
        let lease = self.lease("project.assets")?;
        let mut result = self
            .ports
            .assets
            .detail(catalog, id, query, listeners, &|project| {
                self.git_snapshot(project)
            })?;
        if !self.listeners_active() {
            result.listeners = self.collect_listeners();
            result.project.listener_count = None;
            result.project.known_listener_count = 0;
        }
        Self::check(&lease)?;
        Ok(result)
    }
    pub fn git_snapshot(&self, project: &ProjectIdentity) -> git::GitSnapshot {
        if let Ok(lease) = self.lease("project.git") {
            let snapshot = self.ports.git.snapshot(project);
            if lease.ensure_active().is_ok() {
                return snapshot;
            }
        }
        git::unavailable("Git 只读插件已停止")
    }
    pub fn diff(&self, project: &ProjectIdentity, file: &str) -> NativeResult<git::GitDiff> {
        let lease = self.lease("project.git")?;
        let result = self.ports.git.diff(project, file);
        Self::check(&lease)?;
        result
    }
    pub fn collect_listeners(&self) -> listeners::ListenerSnapshot {
        if let Ok(lease) = self.lease("project.listeners") {
            let result = self.ports.listeners.collect();
            if lease.ensure_active().is_ok() {
                return result;
            }
        }
        listeners::ListenerSnapshot {
            state: "unavailable".into(),
            error: Some("监听只读插件已停止".into()),
            ..Default::default()
        }
    }
    pub fn listeners_active(&self) -> bool {
        self.lease("project.listeners").is_ok()
    }
    pub fn stop(&self, capability: &'static str) -> Result<(), PluginError> {
        self.host.stop(capability)
    }
    pub fn stop_all(&self) {
        self.host.stop_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    #[derive(Default)]
    struct Replacement(std::sync::Mutex<Vec<&'static str>>);
    impl Replacement {
        fn record(&self, name: &'static str) -> usize {
            let mut calls = self.0.lock().unwrap();
            calls.push(name);
            calls.len()
        }
    }
    impl ports::ProjectSource for Replacement {
        fn catalog(
            &self,
            broker: &dyn Fn() -> NativeResult<ProjectCatalog>,
        ) -> NativeResult<ProjectCatalog> {
            self.record("source");
            let mut catalog = broker()?;
            catalog.revision += 1;
            Ok(catalog)
        }
    }
    impl ports::AssetReader for Replacement {
        fn query(
            &self,
            catalog: &ProjectCatalog,
            query: ProjectQuery,
            listeners: &listeners::ListenerSnapshot,
        ) -> NativeResult<ProjectPage> {
            let call = self.record("query");
            let mut page = super::super::query(catalog, query, listeners)?;
            page.coverage = format!("replacement-{call}");
            Ok(page)
        }
        fn detail(
            &self,
            catalog: &ProjectCatalog,
            id: &str,
            query: ProjectQuery,
            listeners: &listeners::ListenerSnapshot,
            git: &dyn Fn(&ProjectIdentity) -> git::GitSnapshot,
        ) -> NativeResult<ProjectDetail> {
            self.record("detail");
            super::super::detail_with_git(catalog, id, query, listeners, git)
        }
    }
    impl ports::GitReader for Replacement {
        fn snapshot(&self, _: &ProjectIdentity) -> git::GitSnapshot {
            self.record("git");
            git::unavailable("replacement-git")
        }
        fn diff(&self, _: &ProjectIdentity, _: &str) -> NativeResult<git::GitDiff> {
            self.record("diff");
            Err(NativeError::new("replacement_diff", "fixture"))
        }
    }
    impl ports::ListenerSource for Replacement {
        fn collect(&self) -> listeners::ListenerSnapshot {
            self.record("listeners");
            listeners::ListenerSnapshot {
                state: "unavailable".into(),
                error: Some("replacement-listeners".into()),
                ..Default::default()
            }
        }
    }
    #[test]
    fn consumers_use_replaced_ports_without_native_cache_or_side_reads() {
        let replacement = Arc::new(Replacement::default());
        let readers = ProjectReaders::with_ports(ports::ProjectPorts {
            source: replacement.clone(),
            assets: replacement.clone(),
            git: replacement.clone(),
            listeners: replacement.clone(),
        })
        .unwrap();
        let original = crate::associated_cost_tests::catalog();
        let catalog = readers.catalog(|| Ok(original.clone())).unwrap();
        assert_eq!(catalog.revision, original.revision + 1);
        let query = ProjectQuery {
            scope: Some("all".into()),
            ..Default::default()
        };
        let listeners = readers.collect_listeners();
        assert_eq!(listeners.error.as_deref(), Some("replacement-listeners"));
        let first = readers.query(&catalog, query.clone(), &listeners).unwrap();
        let second = readers.query(&catalog, query.clone(), &listeners).unwrap();
        assert_ne!(first.coverage, second.coverage);
        let project = &first.items[0].identity;
        let detail = readers
            .detail(&catalog, &project.id, query, &listeners)
            .unwrap();
        assert_eq!(detail.git.error.as_deref(), Some("replacement-git"));
        assert_eq!(
            readers.diff(project, "never-opened").err().unwrap().code,
            "replacement_diff"
        );
        assert_eq!(
            *replacement.0.lock().unwrap(),
            [
                "source",
                "listeners",
                "query",
                "query",
                "detail",
                "git",
                "diff"
            ]
        );
        readers.stop_all();
        let count = replacement.0.lock().unwrap().len();
        assert_eq!(
            readers
                .catalog(|| panic!("disabled broker"))
                .err()
                .unwrap()
                .code,
            "plugin_disabled"
        );
        assert_eq!(
            readers
                .query(&catalog, ProjectQuery::default(), &listeners)
                .err()
                .unwrap()
                .code,
            "plugin_disabled"
        );
        assert_eq!(
            readers.git_snapshot(project).error.as_deref(),
            Some("Git 只读插件已停止")
        );
        assert_eq!(
            readers.collect_listeners().error.as_deref(),
            Some("监听只读插件已停止")
        );
        assert_eq!(replacement.0.lock().unwrap().len(), count);
    }
    #[test]
    fn a_stopped_catalog_source_discards_a_broker_result_in_flight() {
        let readers = Arc::new(ProjectReaders::builtin().unwrap());
        let (ready, entered) = std::sync::mpsc::channel();
        let (resume, gate) = std::sync::mpsc::channel();
        let worker = std::thread::spawn({
            let readers = readers.clone();
            move || {
                readers.catalog(|| {
                    ready.send(()).unwrap();
                    gate.recv_timeout(std::time::Duration::from_secs(5))
                        .unwrap();
                    Ok(ProjectCatalog {
                        sessions: vec![],
                        calls: Default::default(),
                        writes: vec![],
                        revision: 1,
                        as_of: now(),
                        coverage: "native_history".into(),
                    })
                })
            }
        });
        entered
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        readers.stop("project.source").unwrap();
        resume.send(()).unwrap();
        assert_eq!(
            worker.join().unwrap().err().unwrap().code,
            "plugin_disabled"
        );
    }
    #[test]
    fn stopping_git_and_listener_capabilities_keeps_project_assets_available() {
        let readers = ProjectReaders::builtin().unwrap();
        let temp = tempfile::tempdir().unwrap();
        let project = identity(temp.path().to_str().unwrap()).unwrap();
        readers.stop("project.git").unwrap();
        assert_eq!(readers.git_snapshot(&project).state, "unavailable");
        assert_eq!(
            readers.diff(&project, "not-read").err().unwrap().code,
            "plugin_disabled"
        );
        readers.stop("project.listeners").unwrap();
        assert_eq!(readers.collect_listeners().state, "unavailable");
        assert!(readers.collect_listeners().items.is_empty());
        let catalog = ProjectCatalog {
            sessions: vec![],
            calls: Default::default(),
            writes: vec![],
            revision: 1,
            as_of: now(),
            coverage: "native_history".into(),
        };
        let cached = listeners::ListenerSnapshot {
            state: "ready".into(),
            ..Default::default()
        };
        assert_eq!(
            readers
                .query(&catalog, ProjectQuery::default(), &cached)
                .unwrap()
                .listeners
                .state,
            "unavailable"
        );
        assert_eq!(
            readers
                .query(&catalog, ProjectQuery::default(), &Default::default())
                .unwrap()
                .items
                .len(),
            0
        );
        readers.stop("project.assets").unwrap();
        assert_eq!(
            readers
                .query(&catalog, ProjectQuery::default(), &Default::default())
                .err()
                .unwrap()
                .code,
            "plugin_disabled"
        );
        assert!(readers.catalog(|| Ok(catalog.clone())).is_ok());
        readers.stop_all();
        assert_eq!(
            readers
                .catalog(|| panic!("stopped broker called"))
                .err()
                .unwrap()
                .code,
            "plugin_disabled"
        );
    }
}
