//! Compose only trusted built-in sources. Existing domain APIs remain unchanged.
use local_agent_core::sessions::{NativeSessions, NativeSourcePlugins};
use std::path::PathBuf;
use std::sync::Arc;
pub(crate) mod external;

pub(crate) struct ReadonlyRuntime {
    pub(crate) projects: Arc<local_agent_core::projects::readers::ProjectReaders>,
    pub(crate) accounts: Arc<crate::billing::sources::AccountSources>,
    pub(crate) collectors: Arc<crate::tasks::CollectorScheduler>,
}
impl ReadonlyRuntime {
    pub(crate) fn builtin() -> Result<Self, local_agent_core::plugin::PluginError> {
        Ok(Self {
            projects: Arc::new(local_agent_core::projects::readers::ProjectReaders::builtin()?),
            accounts: Arc::new(crate::billing::sources::AccountSources::builtin()?),
            collectors: Arc::new(crate::tasks::CollectorScheduler::builtin()?),
        })
    }
    pub(crate) fn stop(&self) {
        self.collectors.stop_all();
        self.accounts.stop_all();
        self.projects.stop_all();
    }
}

pub(crate) fn sessions(
    home: PathBuf,
    enabled: &[String],
) -> Result<NativeSessions, Box<dyn std::error::Error>> {
    let plugins = NativeSourcePlugins::builtin()?;
    Ok(NativeSessions::with_source_plugins(home, enabled, plugins))
}
