use crate::{billing, gateway, observation};
use local_agent_core::{Store, sessions::NativeSessions};
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub(crate) struct AppState {
    pub(crate) workbench: Option<Arc<crate::workbench::Workbench>>,
    pub(crate) provider_adapters: Arc<crate::provider_models::adapters::Adapters>,
    pub(crate) provider_client: reqwest::Client,
    pub(crate) credentials: Arc<dyn local_agent_core::providers::vault::Vault>,
    pub(crate) plugins: Arc<crate::plugin_host::external::Manager>,
    pub(crate) readonly: Arc<crate::plugin_host::ReadonlyRuntime>,
    pub(crate) iam: Arc<crate::iam::Iam>,
    pub(crate) store: Arc<Mutex<Store>>,
    pub(crate) token: Arc<String>,
    pub(crate) host: Arc<String>,
    pub(crate) sessions: Arc<NativeSessions>,
    pub(crate) session_work: Arc<crate::sessions::work::SessionWork>,
    pub(crate) observation: Arc<observation::Observation>,
    pub(crate) listeners: Arc<Mutex<local_agent_core::projects::listeners::ListenerSnapshot>>,
    pub(crate) providers: Arc<Mutex<local_agent_core::providers::ProviderManager>>,
    pub(crate) verification_capacity: Arc<tokio::sync::Semaphore>,
    pub(crate) query_capacity: Arc<tokio::sync::Semaphore>,
    pub(crate) billing: Arc<Mutex<billing::Billing>>,
    pub(crate) gateway: Arc<gateway::Gateway>,
}

pub(crate) fn provider_client() -> Result<reqwest::Client, reqwest::Error> {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(std::time::Duration::from_secs(3))
        .build()
}

#[cfg(test)]
mod tests;
