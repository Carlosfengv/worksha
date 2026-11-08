//! One trusted graph for native sources, read-only ports and managed collectors.
use local_agent_core::{
    plugin::{
        BuiltinPlugin, PluginError, PluginHost,
        builtin::{Composition, Profile},
    },
    projects::readers::ProjectReaders,
    sessions::NativeSourcePlugins,
};
use std::sync::{Arc, Mutex, MutexGuard};
pub(crate) mod builtin;
pub(crate) mod external;
pub(crate) mod features;
pub(crate) mod intake;

pub(crate) struct RuntimeConfiguration {
    pub(crate) default_profile: Profile,
    pub(crate) effective_profile: Profile,
    pub(crate) problem: Option<String>,
}
pub(crate) struct ReadonlyRuntime {
    pub(crate) retain_request_views: Arc<std::sync::atomic::AtomicBool>,
    pub(crate) projects: Arc<ProjectReaders>,
    pub(crate) accounts: Arc<crate::billing::sources::AccountSources>,
    pub(crate) collectors: Arc<crate::tasks::CollectorScheduler>,
    pub(crate) host: Arc<PluginHost>,
    pub(crate) factories: Vec<Arc<dyn BuiltinPlugin>>,
    pub(crate) intake: Arc<intake::Intake>,
    configuration: Mutex<RuntimeConfiguration>,
}
impl ReadonlyRuntime {
    pub(crate) fn compose(
        default_profile: Profile,
        composition: &Composition,
    ) -> Result<(Self, NativeSourcePlugins), PluginError> {
        let sources = NativeSourcePlugins::definitions();
        let collectors = crate::tasks::CollectorScheduler::definitions();
        let mut factories = sources.registrations();
        factories.extend(crate::billing::sources::AccountSources::registrations());
        factories.extend(
            collectors
                .iter()
                .cloned()
                .map(|collector| collector as Arc<dyn BuiltinPlugin>),
        );
        let factories = features::register(factories);
        let enabled = builtin::selected(&factories, composition)?;
        let descriptors: Vec<&dyn BuiltinPlugin> = factories.iter().map(AsRef::as_ref).collect();
        let host = Arc::new(PluginHost::mount_enabled(&descriptors, &enabled)?);
        let plugins = sources.mount(host.clone())?;
        Ok((
            Self {
                retain_request_views: Arc::new(std::sync::atomic::AtomicBool::new(
                    enabled.contains("builtin.requests"),
                )),
                projects: Arc::new(ProjectReaders::with_host(host.clone())),
                accounts: Arc::new(crate::billing::sources::AccountSources::with_host(
                    host.clone(),
                )?),
                collectors: Arc::new(crate::tasks::CollectorScheduler::with_host(
                    host.clone(),
                    collectors,
                )),
                host,
                factories,
                intake: intake::Intake::new(composition.profile != Profile::Recovery),
                configuration: Mutex::new(RuntimeConfiguration {
                    default_profile,
                    effective_profile: composition.profile,
                    problem: None,
                }),
            },
            plugins,
        ))
    }
    #[cfg(test)]
    pub(crate) fn builtin() -> Result<Self, PluginError> {
        Self::compose(Profile::Desktop, &Composition::default()).map(|(runtime, _)| runtime)
    }
    pub(crate) fn configuration(
        &self,
    ) -> Result<MutexGuard<'_, RuntimeConfiguration>, PluginError> {
        self.configuration
            .lock()
            .map_err(|_| PluginError::new("unavailable", "内置组合状态暂不可用"))
    }
    pub(crate) fn bind_resources(&self, state: &crate::AppState) -> Result<(), PluginError> {
        self.bind_changed_resources(state, |_| true)
    }
    pub(crate) fn bind_changed_resources(
        &self,
        state: &crate::AppState,
        changed: impl Fn(&str) -> bool,
    ) -> Result<(), PluginError> {
        use local_agent_core::plugin::{CapabilityKey, PluginResources, ResourceKind};
        let key = CapabilityKey {
            name: "business.feature",
            version: 1,
            instance: "builtin.requests",
        };
        if changed("builtin.requests")
            && let Ok(lease) = self.host.acquire(&key)
        {
            let retained = self.retain_request_views.clone();
            let observation = Arc::downgrade(&state.observation);
            let mut resources = PluginResources::default();
            resources.own(ResourceKind::Cache, move || {
                if retained.load(std::sync::atomic::Ordering::Acquire) {
                    return;
                }
                if let Some(observation) = observation.upgrade() {
                    assert!(
                        observation.release_request_views().is_ok(),
                        "request views cleanup failed"
                    );
                }
            });
            self.host.own_resources(
                &CapabilityKey {
                    name: "business.feature",
                    version: 1,
                    instance: "builtin.requests",
                },
                lease.epoch,
                resources,
            )?;
        }
        let sessions = &state.sessions;
        for source in sessions.directory().sources() {
            let Some(instance) = sessions
                .source_plugin_id(&source.id)
                .filter(|id| changed(id))
                .and_then(|id| self.factories.iter().find(|p| p.descriptor().id == id))
                .and_then(|plugin| {
                    plugin
                        .descriptor()
                        .provides
                        .into_iter()
                        .find(|key| key.name == "session.source")
                        .map(|key| key.instance)
                })
            else {
                continue;
            };
            let key = CapabilityKey {
                name: "session.source",
                version: 1,
                instance,
            };
            if let Ok(lease) = self.host.acquire(&key) {
                let sessions = Arc::downgrade(sessions);
                let mut resources = PluginResources::default();
                resources.own(ResourceKind::Cache, move || {
                    if let Some(sessions) = sessions.upgrade() {
                        let _ = sessions.clear_stopped_sources();
                    }
                });
                let observation = Arc::downgrade(&state.observation);
                let agent = source.id.clone();
                resources.own(
                    if agent == "codex" {
                        ResourceKind::Connection
                    } else {
                        ResourceKind::Cache
                    },
                    move || {
                        if let Some(observation) = observation.upgrade() {
                            observation.forget_native_inputs(&agent);
                            observation.release_native_snapshots();
                        }
                    },
                );
                self.host.own_resources(&key, lease.epoch, resources)?;
            }
        }
        let key = CapabilityKey {
            name: "project.assets",
            version: 1,
            instance: "native",
        };
        if changed("project.assets")
            && let Ok(lease) = self.host.acquire(&key)
        {
            let readers = Arc::downgrade(&self.projects);
            let mut resources = PluginResources::default();
            resources.own(ResourceKind::Cache, move || {
                if let Some(readers) = readers.upgrade() {
                    readers.clear_cache();
                }
            });
            self.host.own_resources(&key, lease.epoch, resources)?;
        }
        Ok(())
    }
    pub(crate) fn stop(&self) {
        self.retain_request_views
            .store(false, std::sync::atomic::Ordering::Release);
        self.host.stop_all();
    }
}
