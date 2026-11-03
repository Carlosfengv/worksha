use super::*;
use local_agent_core::{
    Store,
    providers::{
        ProviderManager,
        vault::{MemoryVault, Vault},
    },
};
use std::sync::{Mutex, atomic::AtomicUsize};
use tokio::sync::Notify;

const REFERENCE: &str = "10000000-0000-4000-8000-000000000001";

struct Fixture {
    state: AppState,
    account_id: String,
    revision: String,
    source: Arc<sources::ControlledSource>,
    _root: tempfile::TempDir,
}
impl Fixture {
    async fn new() -> Self {
        Self::with_vault(Arc::new(MemoryVault::default())).await
    }
    async fn with_vault(vault: Arc<dyn Vault>) -> Self {
        vault.set(REFERENCE, b"offline-account-key").unwrap();
        let root = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
        let home = root.path().join("home");
        let data = root.path().join("data");
        std::fs::create_dir_all(&home).unwrap();
        let store = Store::open(&data).unwrap();
        let provider_id = format!("custom-{}", uuid::Uuid::new_v4());
        store
            .save_provider(Provider {
                id: provider_id.clone(),
                name: "Controlled billing".into(),
                vendor: "openrouter".into(),
                endpoint: "https://openrouter.ai/api/v1".into(),
                protocol: "chat-completions".into(),
                model: "fixture".into(),
                configured: true,
                credential_ref: Some(REFERENCE.into()),
                credential_label: Some("offline fixture".into()),
                models: vec!["fixture".into()],
                disabled_models: vec![],
                model_catalog: None,
                revision: 0,
                category: "cloud".into(),
            })
            .unwrap();
        let helper = root.path().join("offline-helper");
        std::fs::write(&helper, b"offline fixture").unwrap();
        let manager = ProviderManager::new(home.clone(), &data, helper, vault.clone()).unwrap();
        let (gateway, _listener) = crate::gateway::Gateway::bind(&data).await.unwrap();
        let (sources, source) = sources::controlled();
        let state = AppState {
            provider_adapters: crate::provider_models::adapters::builtin(),
            provider_client: crate::state::provider_client().unwrap(),
            credentials: vault.clone(),
            plugins: Arc::new(crate::plugin_host::external::Manager::open(&data, vault).unwrap()),
            readonly: Arc::new(crate::plugin_host::ReadonlyRuntime::builtin().unwrap()),
            store: Arc::new(Mutex::new(store)),
            token: Arc::new("offline-billing-refresh".into()),
            host: Arc::new("127.0.0.1:9".into()),
            sessions: Arc::new(local_agent_core::sessions::NativeSessions::new(
                home.clone(),
                &[],
            )),
            session_work: Arc::new(crate::sessions::work::SessionWork::default()),
            observation: Arc::new(
                crate::observation::Observation::new(&data, "127.0.0.1:9").unwrap(),
            ),
            listeners: Arc::new(Mutex::new(Default::default())),
            providers: Arc::new(Mutex::new(manager)),
            verification_capacity: Arc::new(tokio::sync::Semaphore::new(2)),
            query_capacity: crate::blocking::query_capacity(),
            billing: Arc::new(Mutex::new(Billing::with_sources(home, &data, sources))),
            gateway: Arc::new(gateway),
        };
        let account_id = format!("provider-{provider_id}");
        let revision = catalog(&state)
            .unwrap()
            .into_iter()
            .find(|a| a.id == account_id)
            .unwrap()
            .source_revision;
        Self {
            state,
            account_id,
            revision,
            source,
            _root: root,
        }
    }
    fn start(&self) -> tokio::task::JoinHandle<ApiResult> {
        tokio::spawn(refresh(
            State(self.state.clone()),
            Path(self.account_id.clone()),
            Json(Refresh {
                expected_revision: self.revision.clone(),
            }),
        ))
    }
    async fn entered(&self) {
        tokio::time::timeout(Duration::from_secs(1), self.source.entered.notified())
            .await
            .unwrap();
    }
    fn active(&self) -> Arc<AtomicBool> {
        self.state.billing.lock().unwrap().in_flight[&self.account_id].clone()
    }
    async fn list(&self) -> Value {
        super::super::list(State(self.state.clone()))
            .await
            .unwrap()
            .0
    }
}

fn account<'a>(value: &'a Value, id: &str) -> &'a Value {
    value["accounts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["id"] == id)
        .unwrap()
}

async fn until(mut condition: impl FnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while !condition() {
        assert!(tokio::time::Instant::now() < deadline);
        tokio::task::yield_now().await;
    }
}

async fn held_billing(
    state: &AppState,
) -> (std::sync::mpsc::Sender<()>, std::thread::JoinHandle<()>) {
    let entered = Arc::new(Notify::new());
    let (release, gate) = std::sync::mpsc::channel();
    let thread = std::thread::spawn({
        let billing = state.billing.clone();
        let entered = entered.clone();
        move || {
            let _held = billing.lock().unwrap();
            entered.notify_one();
            gate.recv_timeout(Duration::from_secs(3)).unwrap();
        }
    });
    tokio::time::timeout(Duration::from_secs(1), entered.notified())
        .await
        .unwrap();
    (release, thread)
}

#[tokio::test]
async fn successful_refresh_returns_ready_and_recent_replay_does_not_query_again() {
    let fixture = Fixture::new().await;
    let request = fixture.start();
    fixture.entered().await;
    fixture.source.resume.notify_one();
    let response = request.await.unwrap().unwrap().0;
    assert_eq!(account(&response, &fixture.account_id)["state"], "ready");
    assert_eq!(
        account(&response, &fixture.account_id)["snapshot"]["source"],
        "controlled-account"
    );
    let cached = fixture.start().await.unwrap().unwrap().0;
    assert_eq!(account(&cached, &fixture.account_id)["state"], "ready");
    assert_eq!(fixture.source.calls.load(Ordering::Acquire), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn cancelled_preparation_cannot_leave_registration_without_a_guard() {
    let fixture = Fixture::new().await;
    let (release, thread) = held_billing(&fixture.state).await;
    let request = fixture.start();
    until(|| fixture.state.query_capacity.available_permits() == 7).await;
    request.abort();
    assert!(
        tokio::time::timeout(Duration::from_millis(100), request)
            .await
            .unwrap()
            .unwrap_err()
            .is_cancelled()
    );
    release.send(()).unwrap();
    thread.join().unwrap();
    until(|| fixture.state.query_capacity.available_permits() == 8).await;
    let list = fixture.list().await;
    assert_ne!(account(&list, &fixture.account_id)["state"], "refreshing");
    assert_eq!(fixture.source.calls.load(Ordering::Acquire), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn cancellation_during_network_never_waits_for_the_billing_mutex() {
    let fixture = Fixture::new().await;
    let request = fixture.start();
    fixture.entered().await;
    let active = fixture.active();
    let (release, thread) = held_billing(&fixture.state).await;
    request.abort();
    assert!(
        tokio::time::timeout(Duration::from_millis(100), request)
            .await
            .unwrap()
            .unwrap_err()
            .is_cancelled()
    );
    assert!(!active.load(Ordering::Acquire));
    release.send(()).unwrap();
    thread.join().unwrap();
    assert_ne!(
        account(&fixture.list().await, &fixture.account_id)["state"],
        "refreshing"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn cancelled_publish_waiter_keeps_the_worker_owner_until_cache_is_published() {
    let fixture = Fixture::new().await;
    let request = fixture.start();
    fixture.entered().await;
    let active = fixture.active();
    let (release, thread) = held_billing(&fixture.state).await;
    fixture.source.resume.notify_one();
    until(|| fixture.state.query_capacity.available_permits() == 7).await;
    request.abort();
    assert!(
        tokio::time::timeout(Duration::from_millis(100), request)
            .await
            .unwrap()
            .unwrap_err()
            .is_cancelled()
    );
    assert!(active.load(Ordering::Acquire));
    release.send(()).unwrap();
    thread.join().unwrap();
    until(|| {
        !active.load(Ordering::Acquire) && fixture.state.query_capacity.available_permits() == 8
    })
    .await;
    assert_eq!(
        account(&fixture.list().await, &fixture.account_id)["state"],
        "ready"
    );
    assert_eq!(fixture.source.calls.load(Ordering::Acquire), 1);
}

#[tokio::test]
async fn rejected_publication_releases_the_owner_without_querying_again() {
    let fixture = Fixture::new().await;
    let request = fixture.start();
    fixture.entered().await;
    let active = fixture.active();
    let held = fixture
        .state
        .query_capacity
        .clone()
        .acquire_many_owned(8)
        .await
        .unwrap();
    fixture.source.resume.notify_one();
    let (status, Json(value)) = request.await.unwrap().unwrap_err();
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(value["code"], "busy");
    assert!(!active.load(Ordering::Acquire));
    drop(held);
    assert_ne!(
        account(&fixture.list().await, &fixture.account_id)["state"],
        "refreshing"
    );
    assert_eq!(fixture.source.calls.load(Ordering::Acquire), 1);
}

#[tokio::test]
async fn edited_provider_rejects_late_data_without_attaching_it_to_new_credentials() {
    let fixture = Fixture::new().await;
    let request = fixture.start();
    fixture.entered().await;
    let replacement = "10000000-0000-4000-8000-000000000002";
    fixture
        .state
        .credentials
        .set(replacement, b"replacement-offline-account-key")
        .unwrap();
    {
        let store = fixture.state.store.lock().unwrap();
        let mut provider = store
            .providers()
            .unwrap()
            .into_iter()
            .find(|p| Some(&p.id) == catalog_provider_id(&fixture.account_id).as_ref())
            .unwrap();
        provider.name = "Changed during read".into();
        provider.credential_ref = Some(replacement.into());
        provider.credential_label = Some("replacement fixture".into());
        store.save_provider(provider).unwrap();
    }
    fixture.source.resume.notify_one();
    assert!(request.await.unwrap().is_err());
    let list = fixture.list().await;
    assert!(account(&list, &fixture.account_id)["snapshot"].is_null());
    assert_ne!(
        account(&list, &fixture.account_id)["sourceRevision"],
        fixture.revision
    );
}

fn catalog_provider_id(account_id: &str) -> Option<String> {
    account_id.strip_prefix("provider-").map(str::to_owned)
}

#[tokio::test]
async fn stopped_source_rejects_late_result_and_releases_registration() {
    let fixture = Fixture::new().await;
    let request = fixture.start();
    fixture.entered().await;
    let active = fixture.active();
    fixture
        .state
        .billing
        .lock()
        .unwrap()
        .sources
        .stop("openrouter");
    fixture.source.resume.notify_one();
    assert!(request.await.unwrap().is_err());
    assert!(!active.load(Ordering::Acquire));
    assert_eq!(
        account(&fixture.list().await, &fixture.account_id)["state"],
        "not_available"
    );
}

#[tokio::test]
async fn failed_read_preserves_the_previous_snapshot_and_marks_its_revision() {
    let fixture = Fixture::new().await;
    fixture.state.billing.lock().unwrap().cache.insert(
        fixture.account_id.clone(),
        Cached {
            revision: fixture.revision.clone(),
            snapshot: AccountSnapshot::new("api", "previous-account"),
            error: None,
        },
    );
    fixture.source.fail.store(true, Ordering::Release);
    let request = fixture.start();
    fixture.entered().await;
    fixture.source.resume.notify_one();
    assert!(request.await.unwrap().is_err());
    let list = fixture.list().await;
    assert_eq!(
        account(&list, &fixture.account_id)["state"],
        "refresh_failed"
    );
    assert_eq!(
        account(&list, &fixture.account_id)["snapshot"]["source"],
        "previous-account"
    );
    assert_eq!(fixture.source.calls.load(Ordering::Acquire), 1);
}

struct SlowVault {
    memory: MemoryVault,
    entered: Notify,
    gate: Mutex<std::sync::mpsc::Receiver<()>>,
    gets: AtomicUsize,
}
impl Vault for SlowVault {
    fn get(
        &self,
        reference: &str,
    ) -> Result<Zeroizing<Vec<u8>>, local_agent_core::providers::ConfigError> {
        if reference == REFERENCE {
            self.gets.fetch_add(1, Ordering::Release);
            self.entered.notify_one();
            self.gate
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(3))
                .unwrap();
        }
        self.memory.get(reference)
    }
    fn set(
        &self,
        reference: &str,
        bytes: &[u8],
    ) -> Result<(), local_agent_core::providers::ConfigError> {
        self.memory.set(reference, bytes)
    }
    fn delete(&self, reference: &str) -> Result<(), local_agent_core::providers::ConfigError> {
        self.memory.delete(reference)
    }
}

#[tokio::test(flavor = "current_thread")]
async fn slow_vault_does_not_hold_provider_manager_or_block_async_cancellation() {
    let (release, gate) = std::sync::mpsc::channel();
    let vault = Arc::new(SlowVault {
        memory: Default::default(),
        entered: Default::default(),
        gate: Mutex::new(gate),
        gets: Default::default(),
    });
    let fixture = Fixture::with_vault(vault.clone()).await;
    let request = fixture.start();
    tokio::time::timeout(Duration::from_secs(1), vault.entered.notified())
        .await
        .unwrap();
    assert!(fixture.state.providers.try_lock().is_ok());
    assert!(fixture.state.store.try_lock().is_ok());
    request.abort();
    assert!(
        tokio::time::timeout(Duration::from_millis(100), request)
            .await
            .unwrap()
            .unwrap_err()
            .is_cancelled()
    );
    release.send(()).unwrap();
    until(|| fixture.state.query_capacity.available_permits() == 8).await;
    assert_eq!(vault.gets.load(Ordering::Acquire), 1);
    assert_eq!(fixture.source.calls.load(Ordering::Acquire), 0);
}
