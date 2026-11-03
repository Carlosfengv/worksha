//! Trusted host ports. Native paths and private catalogs are never guest inputs.
use super::*;

pub trait ProjectSource: Send + Sync {
    fn catalog(
        &self,
        broker: &dyn Fn() -> NativeResult<ProjectCatalog>,
    ) -> NativeResult<ProjectCatalog>;
}
pub trait AssetReader: Send + Sync {
    fn query(
        &self,
        catalog: &ProjectCatalog,
        query: ProjectQuery,
        listeners: &listeners::ListenerSnapshot,
    ) -> NativeResult<ProjectPage>;
    fn detail(
        &self,
        catalog: &ProjectCatalog,
        id: &str,
        query: ProjectQuery,
        listeners: &listeners::ListenerSnapshot,
        git: &dyn Fn(&ProjectIdentity) -> git::GitSnapshot,
    ) -> NativeResult<ProjectDetail>;
    /// Only the native aggregator follows QueryCache's private identity rules.
    /// Other readers may change independently of the native catalog revision.
    fn native_cache(&self) -> bool {
        false
    }
}
pub trait GitReader: Send + Sync {
    fn snapshot(&self, project: &ProjectIdentity) -> git::GitSnapshot;
    fn diff(&self, project: &ProjectIdentity, file: &str) -> NativeResult<git::GitDiff>;
}
pub trait ListenerSource: Send + Sync {
    fn collect(&self) -> listeners::ListenerSnapshot;
}

pub struct NativeProjects;
impl ProjectSource for NativeProjects {
    fn catalog(
        &self,
        broker: &dyn Fn() -> NativeResult<ProjectCatalog>,
    ) -> NativeResult<ProjectCatalog> {
        broker()
    }
}
pub struct NativeAssets;
impl AssetReader for NativeAssets {
    fn query(
        &self,
        catalog: &ProjectCatalog,
        query: ProjectQuery,
        listeners: &listeners::ListenerSnapshot,
    ) -> NativeResult<ProjectPage> {
        super::query(catalog, query, listeners)
    }
    fn detail(
        &self,
        catalog: &ProjectCatalog,
        id: &str,
        query: ProjectQuery,
        listeners: &listeners::ListenerSnapshot,
        git: &dyn Fn(&ProjectIdentity) -> git::GitSnapshot,
    ) -> NativeResult<ProjectDetail> {
        super::detail_with_git(catalog, id, query, listeners, git)
    }
    fn native_cache(&self) -> bool {
        true
    }
}
pub struct NativeGit;
impl GitReader for NativeGit {
    fn snapshot(&self, project: &ProjectIdentity) -> git::GitSnapshot {
        git::snapshot(project)
    }
    fn diff(&self, project: &ProjectIdentity, file: &str) -> NativeResult<git::GitDiff> {
        Ok(git::diff(project, file))
    }
}
pub struct NativeListeners;
impl ListenerSource for NativeListeners {
    fn collect(&self) -> listeners::ListenerSnapshot {
        listeners::collect()
    }
}

/// Replacement is a host construction decision, not an untrusted package API.
/// A worker adapter must first validate domain facts and never receive this broker.
pub struct ProjectPorts {
    pub source: std::sync::Arc<dyn ProjectSource>,
    pub assets: std::sync::Arc<dyn AssetReader>,
    pub git: std::sync::Arc<dyn GitReader>,
    pub listeners: std::sync::Arc<dyn ListenerSource>,
}
impl Default for ProjectPorts {
    fn default() -> Self {
        Self {
            source: std::sync::Arc::new(NativeProjects),
            assets: std::sync::Arc::new(NativeAssets),
            git: std::sync::Arc::new(NativeGit),
            listeners: std::sync::Arc::new(NativeListeners),
        }
    }
}
