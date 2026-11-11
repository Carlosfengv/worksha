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
        Self::with_source(vault, "openrouter").await
    }
    async fn with_source(vault: Arc<dyn Vault>, kind: &'static str) -> Self {
        let (secret, endpoint, protocol, model) = match kind {
            local_agent_core::providers::subscription::MINIMAX_PLAN => (
                "sk-cp-offline-account-key",
                "https://api.minimax.io/anthropic",
                "messages",
                "MiniMax-M3.1-Flash",
            ),
            "deepseek" => (
                "offline-account-key",
                "https://api.deepseek.com/v1",
                "chat-completions",
                "deepseek-flash",
            ),
            "kimi-cn" => (
                "offline-account-key",
                "https://api.moonshot.cn/v1",
                "chat-completions",
                "kimi-k2.6",
            ),
            "kimi-global" => (
                "offline-account-key",
                "https://api.moonshot.ai/v1",
                "chat-completions",
                "kimi-k2.6",
            ),
            "openrouter" => (
                "offline-account-key",
                "https://openrouter.ai/api/v1",
                "chat-completions",
                "fixture",
            ),
            _ => panic!("unsupported fixture source"),
        };
        vault.set(REFERENCE, secret.as_bytes()).unwrap();
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
                vendor: match kind {
                    "kimi-cn" | "kimi-global" => "kimi",
                    _ => kind,
                }
                .into(),
                endpoint: endpoint.into(),
                protocol: protocol.into(),
                model: model.into(),
                configured: true,
                credential_ref: Some(REFERENCE.into()),
                credential_label: Some("offline fixture".into()),
                models: vec![model.into()],
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
        let (sources, source) = sources::controlled_kind(kind);
        let state = AppState {
            iam: Default::default(),
            workbench: None,
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
        self.start_for(&self.account_id, &self.revision)
    }
    fn start_for(&self, account_id: &str, revision: &str) -> tokio::task::JoinHandle<ApiResult> {
        tokio::spawn(refresh(
            State(self.state.clone()),
            Path(account_id.into()),
            Json(Refresh {
                expected_revision: revision.into(),
            }),
        ))
    }
    async fn entered(&self) {
        tokio::time::timeout(Duration::from_secs(1), self.source.entered.notified())
            .await
            .unwrap();
    }
    fn active(&self) -> Arc<AtomicBool> {
        self.state
            .billing
            .lock()
            .unwrap()
            .in_flight
            .iter()
            .find(|(key, _)| key.account == self.account_id && key.revision == self.revision)
            .unwrap()
            .1
            .clone()
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

#[tokio::test]
async fn default_and_pool_aliases_share_refresh_and_retry_ownership() {
    use local_agent_core::providers::upstream_accounts::AccountEdit;

    let fixture = Fixture::new().await;
    let other = "10000000-0000-4000-8000-000000000002";
    fixture
        .state
        .credentials
        .set(other, b"other-offline-key")
        .unwrap();
    let register = |label: &str, credential_ref: &str| {
        let store = fixture.state.store.lock().unwrap();
        let provider = store
            .provider_by_id(fixture.account_id.strip_prefix("provider-").unwrap())
            .unwrap()
            .unwrap();
        let saved = store
            .save_upstream_account(
                &provider,
                &AccountEdit {
                    id: uuid::Uuid::new_v4().to_string(),
                    expected_revision: 0,
                    provider_revision: provider.revision,
                    label: label.into(),
                    credential_ref: credential_ref.into(),
                },
            )
            .unwrap();
        format!("upstream-{}", saved.reference().id)
    };
    let alias = register("Same Key", REFERENCE);
    let distinct = register("Separate Key", other);
    let list = fixture.list().await;
    let revision = |id| account(&list, id)["sourceRevision"].as_str().unwrap();

    let request = fixture.start();
    fixture.entered().await;
    let duplicate = fixture.start_for(&alias, revision(&alias));
    let (status, Json(blocked)) = tokio::time::timeout(Duration::from_secs(2), async {
        tokio::select! {
            response = duplicate => response.unwrap().unwrap_err(),
            _ = fixture.source.entered.notified() => panic!("a pool alias sent a duplicate provider read"),
        }
    })
    .await
    .unwrap();
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(blocked["message"].as_str().unwrap().contains("正在刷新"));
    assert_eq!(
        account(&fixture.list().await, &alias)["state"],
        "refreshing"
    );

    let independent = fixture.start_for(&distinct, revision(&distinct));
    fixture.entered().await;
    assert_eq!(fixture.source.calls.load(Ordering::Acquire), 2);
    fixture.source.resume.notify_waiters();
    let _ = request.await.unwrap().unwrap();
    let _ = independent.await.unwrap().unwrap();

    let retry = fixture.start_for(&alias, revision(&alias));
    let (_, Json(throttled)) = tokio::time::timeout(Duration::from_secs(2), async {
        tokio::select! {
            response = retry => response.unwrap().unwrap_err(),
            _ = fixture.source.entered.notified() => panic!("a pool alias bypassed the retry window"),
        }
    })
    .await
    .unwrap();
    assert!(throttled["message"].as_str().unwrap().contains("10 秒"));
    let list = fixture.start().await.unwrap().unwrap().0;
    assert_eq!(account(&list, &fixture.account_id)["state"], "ready");
    assert_eq!(account(&list, &alias)["state"], "not_loaded");
    assert!(account(&list, &alias)["snapshot"].is_null());
    assert_eq!(fixture.source.calls.load(Ordering::Acquire), 2);
}

#[tokio::test]
async fn newer_reads_retire_the_ranking_snapshot_of_credential_aliases() {
    use local_agent_core::{
        model_services::policy::AccountPlanningHint,
        providers::{subscription::MINIMAX_PLAN, upstream_accounts::AccountEdit},
        routing::{ALIAS, Turn},
    };

    for kind in [MINIMAX_PLAN, "openrouter"] {
        let fixture = Fixture::with_source(Arc::new(MemoryVault::default()), kind).await;
        let state = &fixture.state;
        let other = uuid::Uuid::new_v4().to_string();
        state
            .credentials
            .set(&other, b"sk-cp-other-fixture")
            .unwrap();
        let register = |credential_ref: &str| {
            let store = state.store.lock().unwrap();
            let provider = store
                .provider_by_id(fixture.account_id.strip_prefix("provider-").unwrap())
                .unwrap()
                .unwrap();
            let reference = store
                .save_upstream_account(
                    &provider,
                    &AccountEdit {
                        id: uuid::Uuid::new_v4().to_string(),
                        expected_revision: 0,
                        provider_revision: provider.revision,
                        label: "Quota fixture".into(),
                        credential_ref: credential_ref.into(),
                    },
                )
                .unwrap()
                .reference();
            let target = store
                .upstream_account_target(&provider, &reference)
                .unwrap()
                .0;
            let id = format!("upstream-{}", reference.id);
            (id, target, reference)
        };
        let (alias, target, reference) = register(REFERENCE);
        let (distinct, other_target, other_reference) = register(&other);
        let binding = {
            let store = state.store.lock().unwrap();
            let model_id = uuid::Uuid::new_v4().to_string();
            let draft = json!({
                "id": uuid::Uuid::new_v4().to_string(), "revision": 0,
                "name": "Alias quota fixture", "defaultModelId": model_id,
                "models": [{"id":model_id,"name":"Quota model","providerId":target.id,
                    "model":target.model,"capabilities":{"contextWindow":96000,"maxOutputTokens":4096,
                    "tools":true,"thinking":false,"inputModalities":["text"]}}],
                "routingPolicy":{"enabled":true,"accountPools":[{"modelId":model_id,
                    "accounts":[other_reference, reference],"strategy":"quota-remaining"}]}
            });
            let service = store
                .save_model_service(serde_json::from_value(draft).unwrap())
                .unwrap();
            store
                .create_model_service_binding(
                    "claude",
                    "fixture",
                    &uuid::Uuid::new_v4().to_string(),
                    &service,
                )
                .unwrap()
                .0
        };
        let preferred_account = || {
            let turn_id = uuid::Uuid::new_v4().to_string();
            let turn = Turn {
                session: Some(turn_id.clone()),
                digest: turn_id.clone(),
                chain: vec![turn_id],
                continuation: false,
                portable: true,
            };
            let store = state.store.lock().unwrap();
            let selected = store
                .model_service_admit(&binding.id, &turn, ALIAS)
                .unwrap();
            let billing = state.billing.lock().unwrap();
            let plan = store
                .service_dispatch(
                    &binding.id,
                    &turn,
                    &selected,
                    |_| Default::default(),
                    |target, reference, _| AccountPlanningHint {
                        quota: billing.upstream_quota(target, Some(reference), chrono::Utc::now()),
                        blocked: None,
                    },
                )
                .unwrap()
                .unwrap();
            plan.candidates[0]
                .service
                .as_ref()
                .unwrap()
                .upstream_account
                .clone()
                .unwrap()
        };
        let list = fixture.list().await;
        let plan = |window| json!({"base_resp":{"status_code":0},"model_remains":[window]});
        let full = |remaining| {
            if kind == MINIMAX_PLAN {
                plan(
                    json!({"model_name":"general","current_interval_remaining_percent":remaining,
                    "current_weekly_remaining_percent":95}),
                )
            } else {
                json!({"data":{"limit":100,"limit_remaining":remaining,"limit_reset":null}})
            }
        };
        let partial = if kind == MINIMAX_PLAN {
            plan(json!({"model_name":"general","current_interval_remaining_percent":10}))
        } else {
            json!({"data":{"usage":1.25}})
        };
        let read = async |id: &str, response: Option<Value>| {
            state.billing.lock().unwrap().attempted.clear();
            let failed = response.is_none();
            fixture.source.fail.store(failed, Ordering::Release);
            *fixture.source.budget_response.lock().unwrap() = response;
            let revision = account(&list, id)["sourceRevision"].as_str().unwrap();
            let request = fixture.start_for(id, revision);
            fixture.entered().await;
            fixture.source.resume.notify_one();
            assert_eq!(request.await.unwrap().is_err(), failed);
        };
        let alias_ranked = || {
            state
                .billing
                .lock()
                .unwrap()
                .upstream_quota(&target, Some(&reference), chrono::Utc::now())
                .is_some()
        };
        let independent_ranked = || {
            state
                .billing
                .lock()
                .unwrap()
                .upstream_quota(&other_target, Some(&other_reference), chrono::Utc::now())
                .is_some()
        };

        read(&alias, Some(full(90))).await;
        read(&distinct, Some(full(40))).await;
        assert!(alias_ranked() && independent_ranked());
        assert_eq!(preferred_account(), reference);
        read(&fixture.account_id, Some(partial)).await;
        assert!(
            !alias_ranked(),
            "{kind}: a newer partial read must retire older alias amounts"
        );
        assert!(independent_ranked());
        assert_eq!(preferred_account(), other_reference);
        let history = fixture.list().await;
        assert_eq!(account(&history, &alias)["state"], "stale");
        assert_eq!(
            account(&history, &alias)["snapshot"]["windows"][0]["remainingPercent"],
            90.
        );

        read(&alias, Some(full(75))).await;
        assert!(
            alias_ranked(),
            "a fresh read of the alias restores its ranking"
        );
        assert_eq!(preferred_account(), reference);
        read(&fixture.account_id, None).await;
        assert!(
            !alias_ranked(),
            "{kind}: a newer failed read must retire older alias amounts"
        );
        assert!(independent_ranked());
        assert_eq!(preferred_account(), other_reference);

        read(&alias, Some(full(65))).await;
        state.billing.lock().unwrap().attempted.clear();
        fixture.source.fail.store(false, Ordering::Release);
        *fixture.source.budget_response.lock().unwrap() = Some(full(55));
        let obsolete = fixture.start();
        fixture.entered().await;
        {
            let store = state.store.lock().unwrap();
            let mut provider = store.provider_by_id(&target.id).unwrap().unwrap();
            provider.credential_ref = Some(other);
            store.save_provider(provider).unwrap();
        }
        fixture.source.resume.notify_one();
        assert!(obsolete.await.unwrap().is_err());
        assert!(alias_ranked() && independent_ranked());
    }
}

#[tokio::test]
async fn upstream_refresh_and_routing_use_the_exact_account_version() {
    use local_agent_core::{
        providers::upstream_accounts::AccountEdit,
        routing::{ALIAS, Turn},
    };
    let fixture = Fixture::new().await;
    let state = &fixture.state;
    let provider_id = fixture.account_id.strip_prefix("provider-").unwrap();
    let add = |reference: &str, label: &str| {
        state.credentials.set(reference, label.as_bytes()).unwrap();
        let store = state.store.lock().unwrap();
        let provider = store.provider_by_id(provider_id).unwrap().unwrap();
        let input = AccountEdit {
            id: uuid::Uuid::new_v4().to_string(),
            expected_revision: 0,
            provider_revision: provider.revision,
            label: format!("{label} fixture"),
            credential_ref: reference.into(),
        };
        store
            .save_upstream_account(&provider, &input)
            .unwrap()
            .reference()
    };
    let a = add("10000000-0000-4000-8000-000000000002", "alpha");
    let b = add("10000000-0000-4000-8000-000000000003", "beta");
    let a_id = format!("upstream-{}", a.id);
    let a_row = catalog(state)
        .unwrap()
        .into_iter()
        .find(|row| row.id == a_id)
        .unwrap();
    let (target, secret) =
        api_credentials(state, &a_row, &fixture._root.path().join("home")).unwrap();
    assert_eq!(
        &**secret, b"alpha",
        "the Provider default Key cannot be substituted"
    );
    *fixture.source.budget_response.lock().unwrap() =
        Some(json!({"data":{"limit":100,"limit_remaining":0,"limit_reset":null}}));
    let refresh_a = || {
        tokio::spawn(refresh(
            State(state.clone()),
            Path(a_id.clone()),
            Json(Refresh {
                expected_revision: a_row.source_revision.clone(),
            }),
        ))
    };
    let refresh_request = refresh_a();
    fixture.entered().await;
    fixture.source.resume.notify_one();
    let response = refresh_request.await.unwrap().unwrap().0;
    assert_eq!(
        account(&response, &a_id)["snapshot"]["windows"][0]["remainingPercent"],
        0.
    );
    let now = chrono::Utc::now();
    let (binding, selected, turn, target_b) = {
        let store = state.store.lock().unwrap();
        let model_id = uuid::Uuid::new_v4().to_string();
        let service = store.save_model_service(serde_json::from_value(serde_json::json!({
            "id":uuid::Uuid::new_v4().to_string(), "revision":0,"name":"Quota fixture","defaultModelId":model_id,
            "models":[{"id":model_id,"name":"Quota model","providerId":provider_id,"model":"fixture","capabilities":{"contextWindow":96000,"maxOutputTokens":4096,"tools":true,"thinking":false,"inputModalities":["text"]}}],
            "routingPolicy":{"enabled":true,"accountPools":[{"modelId":model_id,"accounts":[a,b],"strategy":"quota-remaining"}]}
        })).unwrap()).unwrap();
        let source = &service.models[0].provider;
        let binding = store
            .dynamic_create(
                "claude",
                "quota fixture",
                &uuid::Uuid::new_v4().to_string(),
                source,
            )
            .unwrap();
        store.attach_model_service(&binding.id, &service).unwrap();
        let turn = Turn {
            session: Some("quota-session".into()),
            digest: "quota-turn".into(),
            chain: vec!["quota-turn".into()],
            continuation: false,
            portable: true,
        };
        let selected = store
            .model_service_admit(&binding.id, &turn, ALIAS)
            .unwrap();
        let (target_b, _) = store.upstream_account_target(source, &b).unwrap();
        (binding, selected, turn, target_b)
    };
    {
        let billing = state.billing.lock().unwrap();
        assert!(
            billing
                .upstream_quota(&target, Some(&a), now)
                .unwrap()
                .exhausted()
        );
        assert!(
            billing.upstream_quota(&target_b, Some(&a), now).is_none(),
            "a snapshot cannot follow another Key"
        );
        assert!(billing.upstream_quota(&target_b, Some(&b), now).is_none());
        assert!(
            billing
                .upstream_quota(&target, Some(&a), now + chrono::Duration::seconds(301))
                .is_none()
        );
    }
    let plan = state
        .store
        .lock()
        .unwrap()
        .service_dispatch(
            &binding.id,
            &turn,
            &selected,
            |_| Default::default(),
            |target, reference, _| local_agent_core::model_services::policy::AccountPlanningHint {
                quota: state
                    .billing
                    .lock()
                    .unwrap()
                    .upstream_quota(target, Some(reference), now),
                blocked: None,
            },
        )
        .unwrap()
        .unwrap();
    assert_eq!(
        plan.candidates[0]
            .service
            .as_ref()
            .unwrap()
            .upstream_account
            .as_ref(),
        Some(&b)
    );
    assert!(super::super::upstream_exhausted(state, &target));
    assert!(!super::super::upstream_exhausted(state, &target_b));

    // A failed read removes stale amounts from ranking but does not clear a
    // confirmed budget block for this same credential and activation.
    state
        .billing
        .lock()
        .unwrap()
        .attempted
        .retain(|key, _| key.account != a_id);
    fixture.source.fail.store(true, Ordering::Release);
    let failed = refresh_a();
    fixture.entered().await;
    fixture.source.resume.notify_one();
    assert!(failed.await.unwrap().is_err());
    assert_eq!(
        account(&fixture.list().await, &a_id)["state"],
        "refresh_failed"
    );
    assert!(super::super::upstream_exhausted(state, &target));
    assert!(
        state
            .billing
            .lock()
            .unwrap()
            .upstream_quota(&target, Some(&a), chrono::Utc::now())
            .is_none()
    );

    state
        .billing
        .lock()
        .unwrap()
        .attempted
        .retain(|key, _| key.account != a_id);
    fixture.source.fail.store(false, Ordering::Release);
    *fixture.source.budget_response.lock().unwrap() = Some(json!({"data":{"usage":1.25}}));
    let recovered = refresh_a();
    fixture.entered().await;
    fixture.source.resume.notify_one();
    let recovered = recovered.await.unwrap().unwrap().0;
    assert_eq!(account(&recovered, &a_id)["state"], "ready");
    assert_eq!(account(&recovered, &a_id)["snapshot"]["windows"], json!([]));
    assert!(super::super::upstream_exhausted(state, &target));
    let data = fixture._root.path().join("data");
    let reopened = Store::open(&data).unwrap();
    let stored = reopened.openrouter_admission_evidence().unwrap().unwrap();
    assert!(!stored.contains("1.25") && !stored.contains("remainingPercent"));
    assert!(!stored.contains("000000000002") && !stored.contains("alpha"));
    let mut cold = Billing::new(fixture._root.path().join("home"), &data);
    cold.restore_openrouter(&reopened);
    assert!(cold.upstream_budget(&target, chrono::Utc::now()).exhausted);
    assert!(
        cold.upstream_quota(&target, Some(&a), chrono::Utc::now())
            .is_none()
    );
    assert!(
        cold.cached(
            &a_row,
            Some(cold.sources.lease("openrouter").unwrap().epoch)
        )
        .is_none()
    );
    assert!(
        !cold
            .upstream_budget(&target_b, chrono::Utc::now())
            .exhausted
    );
    let rotated_target = {
        let store = state.store.lock().unwrap();
        let provider = store.provider_by_id(provider_id).unwrap().unwrap();
        let next = store
            .save_upstream_account(
                &provider,
                &AccountEdit {
                    id: a.id.clone(),
                    expected_revision: a.revision,
                    provider_revision: provider.revision,
                    label: "Rotated alpha fixture".into(),
                    credential_ref: "10000000-0000-4000-8000-000000000004".into(),
                },
            )
            .unwrap()
            .reference();
        store.upstream_account_target(&provider, &next).unwrap().0
    };
    assert!(
        !super::super::upstream_exhausted(state, &rotated_target),
        "rotation invalidates the old observation"
    );
    assert!(
        !cold
            .upstream_budget(&rotated_target, chrono::Utc::now())
            .exhausted
    );
    let sources = state.billing.lock().unwrap().sources.clone();
    sources.stop("openrouter");
    sources.restart();
    assert!(
        !super::super::upstream_exhausted(state, &target),
        "new source activation requires a fresh read"
    );
}

#[tokio::test]
async fn openrouter_refresh_reconciles_confirmed_reply_budgets_only_with_matching_evidence() {
    use local_agent_core::model_services::policy::{
        OpenRouterBudgetRejection, OpenRouterBudgetScope,
    };

    let fixture = Fixture::new().await;
    let provider_id = fixture.account_id.strip_prefix("provider-").unwrap();
    let now = chrono::Utc::now();
    let (credential, key) = {
        let store = fixture.state.store.lock().unwrap();
        let provider = store.provider_by_id(provider_id).unwrap().unwrap();
        let credential = super::super::openrouter_scope(&provider).unwrap();
        let key = OpenRouterBudgetRejection {
            credential: credential.clone(),
            scope: OpenRouterBudgetScope::KeyLimit,
            observed_at: now,
            reset_at: None,
        };
        store.record_openrouter_budget_rejection(&key).unwrap();
        store
            .record_openrouter_budget_rejection(&OpenRouterBudgetRejection {
                scope: OpenRouterBudgetScope::DefaultMember,
                reset_at: Some(now.timestamp() + 3600),
                ..key.clone()
            })
            .unwrap();
        store
            .record_openrouter_budget_rejection(&OpenRouterBudgetRejection {
                scope: OpenRouterBudgetScope::DefaultMember,
                reset_at: Some(now.timestamp() + 60),
                observed_at: now + chrono::Duration::microseconds(1),
                ..key.clone()
            })
            .unwrap();
        (credential, key)
    };
    let holds = || {
        fixture
            .state
            .store
            .lock()
            .unwrap()
            .openrouter_reply_holds(&credential)
            .unwrap()
    };
    let refresh = async |data| {
        fixture.state.billing.lock().unwrap().attempted.clear();
        *fixture.source.budget_response.lock().unwrap() = Some(json!({"data":data}));
        let request = fixture.start();
        fixture.entered().await;
        fixture.source.resume.notify_one();
        let response = request.await.unwrap().unwrap().0;
        assert_eq!(account(&response, &fixture.account_id)["state"], "ready");
    };
    refresh(json!({"usage":1.25})).await;
    assert_eq!(holds().len(), 2);
    refresh(json!({"limit":100,"limit_remaining":25,"limit_reset":null})).await;
    let member_hold = [(
        "workspace_default_member".into(),
        Some(now.timestamp() + 3600),
    )];
    assert_eq!(holds(), member_hold);
    fixture
        .state
        .store
        .lock()
        .unwrap()
        .record_openrouter_budget_rejection(&key)
        .unwrap();
    assert_eq!(
        holds(),
        member_hold,
        "a late rejection cannot override a newer trusted refresh"
    );
    refresh(json!({"budgets":[],"effective_budget":null})).await;
    assert!(holds().is_empty());
}

#[tokio::test]
async fn provider_balance_refresh_reconciles_only_with_official_availability_evidence() {
    use local_agent_core::model_services::policy::BalanceState;

    let deepseek_wallet = json!([{"currency":"CNY","total_balance":"10"}]);
    let kimi_balance = |data| json!({"status":true,"code":0,"data":data});
    for (kind, unavailable, ambiguous, available) in [
        (
            "deepseek",
            json!({"is_available":false,"balance_infos":deepseek_wallet}),
            json!({"balance_infos":deepseek_wallet}),
            json!({"is_available":true,"balance_infos":[]}),
        ),
        (
            "kimi-cn",
            kimi_balance(json!({"available_balance":0,"cash_balance":5})),
            kimi_balance(json!({"available_balance":"10","cash_balance":10})),
            kimi_balance(json!({"available_balance":10})),
        ),
        (
            "kimi-global",
            kimi_balance(json!({"available_balance":-1})),
            kimi_balance(json!({"cash_balance":10})),
            kimi_balance(json!({"available_balance":10})),
        ),
    ] {
        let fixture = Fixture::with_source(Arc::new(MemoryVault::default()), kind).await;
        let provider_id = fixture.account_id.strip_prefix("provider-").unwrap();
        let (target, credential) = {
            let store = fixture.state.store.lock().unwrap();
            let target = store.provider_by_id(provider_id).unwrap().unwrap();
            let (_, credential) = super::super::balance_target(&target).unwrap();
            (target, credential)
        };
        let availability = || {
            fixture
                .state
                .store
                .lock()
                .unwrap()
                .upstream_balance_state(&credential)
                .unwrap()
        };
        let budget = |target: &Provider| {
            let store = fixture.state.store.lock().unwrap();
            fixture
                .state
                .billing
                .lock()
                .unwrap()
                .route_budget(&store, target, chrono::Utc::now())
        };
        let refresh = async |wire| {
            fixture.state.billing.lock().unwrap().attempted.clear();
            *fixture.source.budget_response.lock().unwrap() = Some(wire);
            let request = fixture.start();
            fixture.entered().await;
            fixture.source.resume.notify_one();
            request.await.unwrap()
        };
        let earliest = chrono::Utc::now();
        let response = refresh(unavailable.clone()).await.unwrap().0;
        assert_eq!(account(&response, &fixture.account_id)["state"], "ready");
        assert_eq!(availability(), BalanceState::Exhausted, "{kind}");
        assert!(budget(&target).exhausted);
        let reopened = Store::open(&fixture._root.path().join("data")).unwrap();
        assert_eq!(
            reopened.upstream_balance_state(&credential).unwrap(),
            BalanceState::Exhausted
        );
        let mut rotated = target.clone();
        rotated.credential_ref = Some(uuid::Uuid::new_v4().to_string());
        assert!(!budget(&rotated).exhausted);

        let _ambiguous = refresh(ambiguous).await;
        assert_eq!(availability(), BalanceState::Exhausted, "{kind}");
        let response = refresh(available.clone()).await.unwrap().0;
        assert_eq!(account(&response, &fixture.account_id)["state"], "ready");
        let BalanceState::Recovery(observed) = availability() else {
            panic!("{kind} did not enter recovery");
        };
        let recovered = budget(&target);
        assert!(!recovered.exhausted);
        assert_eq!(recovered.key_reset.unwrap().reset_at, observed);
        reopened
            .record_upstream_balance(&credential, earliest, false)
            .unwrap();
        let _ = refresh(available).await.unwrap();
        assert_eq!(availability(), BalanceState::Recovery(observed));
        let _ = refresh(unavailable).await.unwrap();
        assert_eq!(availability(), BalanceState::Exhausted);
    }
}

#[tokio::test]
async fn default_openrouter_refresh_shares_admission_and_restores_legacy_pool_evidence() {
    use local_agent_core::providers::upstream_accounts::AccountEdit;

    let fixture = Fixture::new().await;
    let (provider, target, reference) = {
        let store = fixture.state.store.lock().unwrap();
        let provider = store
            .provider_by_id(fixture.account_id.strip_prefix("provider-").unwrap())
            .unwrap()
            .unwrap();
        let reference = store
            .save_upstream_account(
                &provider,
                &AccountEdit {
                    id: uuid::Uuid::new_v4().to_string(),
                    expected_revision: 0,
                    provider_revision: provider.revision,
                    label: "Same Key in pool".into(),
                    credential_ref: REFERENCE.into(),
                },
            )
            .unwrap()
            .reference();
        let target = store
            .upstream_account_target(&provider, &reference)
            .unwrap()
            .0;
        (provider, target, reference)
    };
    let row_id = format!("upstream-{}", reference.id);
    let pool_revision = account(&fixture.list().await, &row_id)["sourceRevision"]
        .as_str()
        .unwrap()
        .to_owned();
    let read = async |id: &str, revision: &str, value: Value| {
        fixture.state.billing.lock().unwrap().attempted.clear();
        *fixture.source.budget_response.lock().unwrap() = Some(value);
        let request = fixture.start_for(id, revision);
        fixture.entered().await;
        fixture.source.resume.notify_one();
        request.await.unwrap().unwrap().0
    };
    let exhausted = json!({"data":{"limit":100,"limit_remaining":0,"limit_reset":null}});
    read(&fixture.account_id, &fixture.revision, exhausted.clone()).await;
    let blocked = |target: &Provider| super::super::upstream_exhausted(&fixture.state, target);
    assert!(blocked(&provider) && blocked(&target));
    assert!(
        fixture
            .state
            .billing
            .lock()
            .unwrap()
            .upstream_quota(&provider, None, chrono::Utc::now())
            .unwrap()
            .exhausted()
    );
    read(&row_id, &pool_revision, json!({"data":{"usage":1.25}})).await;
    assert!(blocked(&provider) && blocked(&target));

    let data = fixture._root.path().join("data");
    let store = Store::open(&data).unwrap();
    let mut evidence: Value =
        serde_json::from_str(&store.openrouter_admission_evidence().unwrap().unwrap()).unwrap();
    let records = evidence["accounts"].as_object_mut().unwrap();
    assert_eq!(records.len(), 1, "aliases share a single durable record");
    let mut legacy = records.values().next().unwrap().clone();
    legacy["revision"] = json!(pool_revision);
    records.clear();
    records.insert(row_id.clone(), legacy);
    store
        .save_openrouter_admission_evidence(&evidence.to_string())
        .unwrap();
    store
        .set_upstream_account_state(&provider.id, &reference, "enabled", "paused")
        .unwrap();
    let cold_home = fixture._root.path().join("home");
    let mut cold = Billing::new(cold_home.clone(), &data);
    cold.restore_openrouter(&store);
    let now = chrono::Utc::now();
    assert!(
        cold.upstream_budget(&provider, now).exhausted,
        "an old pool row still protects the default Key after upgrade"
    );
    assert!(cold.upstream_quota(&provider, None, now).is_none());
    let mut rotated = provider.clone();
    rotated.credential_ref = Some(uuid::Uuid::new_v4().to_string());
    assert!(!cold.upstream_budget(&rotated, now).exhausted);
    store
        .set_upstream_account_state(&provider.id, &reference, "paused", "enabled")
        .unwrap();

    read(
        &row_id,
        &pool_revision,
        json!({"data":{"limit":100,"limit_remaining":25,"limit_reset":null}}),
    )
    .await;
    assert!(!blocked(&provider) && !blocked(&target));
    {
        let mut billing = fixture.state.billing.lock().unwrap();
        assert!(
            billing
                .upstream_quota(&provider, None, chrono::Utc::now())
                .is_none()
        );
        billing.seed_openrouter_budget_snapshot(
            &target,
            &reference,
            now - chrono::Duration::seconds(1),
            &exhausted,
        );
    }
    assert!(
        !blocked(&provider) && !blocked(&target),
        "an older alias response cannot resurrect a cleared hold"
    );
    let mut cold = Billing::new(cold_home, &data);
    cold.restore_openrouter(&store);
    assert!(
        !cold
            .upstream_budget(&provider, chrono::Utc::now())
            .exhausted
    );
}

#[tokio::test]
async fn upstream_refresh_publication_serializes_with_account_revocation() {
    use local_agent_core::providers::upstream_accounts::AccountEdit;

    let fixture = Fixture::new().await;
    let state = &fixture.state;
    let provider_id = fixture.account_id.strip_prefix("provider-").unwrap();
    let reference = {
        let store = state.store.lock().unwrap();
        let provider = store.provider_by_id(provider_id).unwrap().unwrap();
        store
            .save_upstream_account(
                &provider,
                &AccountEdit {
                    id: uuid::Uuid::new_v4().to_string(),
                    expected_revision: 0,
                    provider_revision: provider.revision,
                    label: "Revocation fixture".into(),
                    credential_ref: REFERENCE.into(),
                },
            )
            .unwrap()
            .reference()
    };
    let id = format!("upstream-{}", reference.id);
    let revision = catalog(state)
        .unwrap()
        .into_iter()
        .find(|account| account.id == id)
        .unwrap()
        .source_revision;
    *fixture.source.budget_response.lock().unwrap() =
        Some(json!({"data":{"limit":100,"limit_remaining":0,"limit_reset":null}}));
    let request = tokio::spawn(refresh(
        State(state.clone()),
        Path(id.clone()),
        Json(Refresh {
            expected_revision: revision,
        }),
    ));
    fixture.entered().await;
    let (release, holder) = held_billing(state).await;
    fixture.source.resume.notify_one();
    // The publisher can verify the account while the billing cache is held,
    // but must keep the store guard until the cache update can commit.
    until(|| state.store.try_lock().is_err()).await;
    let entered = Arc::new(Notify::new());
    let mut revoke = tokio::task::spawn_blocking({
        let store = state.store.clone();
        let entered = entered.clone();
        let provider_id = provider_id.to_owned();
        move || {
            entered.notify_one();
            store
                .lock()
                .unwrap()
                .set_upstream_account_state(&provider_id, &reference, "enabled", "revoked")
                .unwrap()
        }
    });
    tokio::time::timeout(Duration::from_secs(1), entered.notified())
        .await
        .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(100), &mut revoke)
            .await
            .is_err(),
        "revocation must wait for the checked publication to finish"
    );
    release.send(()).unwrap();
    holder.join().unwrap();
    let Json(result) = request.await.unwrap().unwrap();
    assert!(result["accounts"].is_array());
    assert_eq!(revoke.await.unwrap().state, "revoked");
    assert!(
        catalog(state)
            .unwrap()
            .iter()
            .all(|account| account.id != id)
    );
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
    let list = fixture.list().await;
    let current = account(&list, &fixture.account_id);
    assert_eq!(current["state"], "not_loaded");
    assert!(current["snapshot"].is_null());
    let revision = current["sourceRevision"].as_str().unwrap();
    assert_ne!(revision, fixture.revision);
    let refreshed = fixture.start_for(&fixture.account_id, revision);
    fixture.entered().await;
    assert_eq!(fixture.source.calls.load(Ordering::Acquire), 2);
    assert!(
        fixture
            .start_for(&fixture.account_id, revision)
            .await
            .unwrap()
            .is_err()
    );
    fixture.source.resume.notify_waiters();
    assert!(request.await.unwrap().is_err());
    let list = refreshed.await.unwrap().unwrap().0;
    assert_eq!(account(&list, &fixture.account_id)["state"], "ready");
    assert_eq!(
        account(&list, &fixture.account_id)["sourceRevision"],
        revision
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
    let epoch = fixture
        .state
        .billing
        .lock()
        .unwrap()
        .sources
        .lease("openrouter")
        .unwrap()
        .epoch;
    fixture.state.billing.lock().unwrap().cache.insert(
        fixture.account_id.clone(),
        Cached {
            source_epoch: Some(epoch),
            current_read_scope: None,
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

#[tokio::test]
async fn subscription_refresh_persists_each_confirmed_hold_for_the_credential() {
    use local_agent_core::providers::subscription::MINIMAX_PLAN;

    let fixture = Fixture::with_source(Arc::new(MemoryVault::default()), MINIMAX_PLAN).await;
    let now = chrono::Utc::now();
    let reset = now.timestamp() + 3600;
    let refresh = async |value| {
        fixture.state.billing.lock().unwrap().attempted.clear();
        *fixture.source.budget_response.lock().unwrap() = Some(json!({
            "base_resp":{"status_code":0}, "model_remains":[value],
        }));
        let read = fixture.start();
        fixture.entered().await;
        fixture.source.resume.notify_one();
        let response = read.await.unwrap().unwrap().0;
        assert_eq!(account(&response, &fixture.account_id)["state"], "ready");
    };
    refresh(
        json!({"model_name":"general", "current_interval_status":2, "end_time":reset,
        "current_weekly_status":2}),
    )
    .await;
    refresh(
        json!({"model_name":"general", "current_interval_remaining_percent":99,
        "end_time":now.timestamp()-1}),
    )
    .await;
    let data = fixture._root.path().join("data");
    let store = Store::open(&data).unwrap();
    let provider = store
        .provider_by_id(fixture.account_id.strip_prefix("provider-").unwrap())
        .unwrap()
        .unwrap();
    let cold = Billing::new(fixture._root.path().join("home"), &data);
    assert!(cold.route_budget(&store, &provider, now).exhausted);
    assert!(
        cold.route_budget(&store, &provider, now + chrono::Duration::days(8))
            .exhausted,
        "a weekly hold without a known reset needs new evidence"
    );
    let rotated = Provider {
        credential_ref: Some(uuid::Uuid::new_v4().to_string()),
        ..provider.clone()
    };
    assert!(!cold.route_budget(&store, &rotated, now).exhausted);

    refresh(json!({"model_name":"general", "current_weekly_remaining_percent":12})).await;
    assert!(cold.route_budget(&store, &provider, now).exhausted);
    let due = cold.route_budget(&store, &provider, now + chrono::Duration::hours(1));
    assert!(!due.exhausted && due.track_recovery);
    refresh(
        json!({"model_name":"general", "current_interval_remaining_percent":12, "end_time":reset}),
    )
    .await;
    let stale = local_agent_core::billing::api_at(MINIMAX_PLAN, &json!({
        "base_resp":{"status_code":0}, "model_remains":[{"model_name":"general", "current_interval_status":2, "end_time":reset}],
    }), now - chrono::Duration::seconds(1)).unwrap();
    store
        .record_minimax_allowance(
            &local_agent_core::model_services::policy::scopes(&provider).0,
            &stale.windows,
            (now - chrono::Duration::seconds(1)).fixed_offset(),
        )
        .unwrap();
    let open = cold.route_budget(&store, &provider, now);
    assert!(!open.exhausted && !open.track_recovery);
}

#[tokio::test]
async fn subscription_refresh_distinguishes_authorization_and_recovers_with_a_new_key() {
    use local_agent_core::providers::{subscription::MINIMAX_PLAN, upstream_accounts::AccountEdit};
    let fixture = Fixture::with_source(Arc::new(MemoryVault::default()), MINIMAX_PLAN).await;
    let state = &fixture.state;
    let provider_id = catalog_provider_id(&fixture.account_id).unwrap();
    let provider = state
        .store
        .lock()
        .unwrap()
        .provider_by_id(&provider_id)
        .unwrap()
        .unwrap();
    let mut edit = AccountEdit {
        id: uuid::Uuid::new_v4().to_string(),
        expected_revision: 0,
        provider_revision: provider.revision,
        label: "Subscription fixture".into(),
        credential_ref: REFERENCE.into(),
    };
    let original = state
        .store
        .lock()
        .unwrap()
        .save_upstream_account(&provider, &edit)
        .unwrap()
        .reference();
    let id = format!("upstream-{}", original.id);
    let row = || {
        catalog(state)
            .unwrap()
            .into_iter()
            .find(|row| row.id == id)
            .unwrap()
    };
    let original_revision = row().source_revision;
    let set_response = |value| *fixture.source.budget_response.lock().unwrap() = Some(value);
    let budget = |remaining| {
        json!({"base_resp":{"status_code":0},"model_remains":[{
            "model_name":"general","current_interval_status":1,"current_interval_remaining_percent":remaining,
            "current_weekly_status":1,"current_weekly_remaining_percent":90
        }]})
    };
    let expire_retry_window = || {
        state
            .billing
            .lock()
            .unwrap()
            .attempted
            .retain(|key, _| key.account != id)
    };
    set_response(budget(18));
    let first = fixture.start_for(&id, &original_revision);
    fixture.entered().await;
    assert_eq!(row().state, "refreshing");
    assert!(
        fixture
            .start_for(&id, &original_revision)
            .await
            .unwrap()
            .is_err()
    );
    fixture.source.resume.notify_one();
    assert!(first.await.unwrap().is_ok());
    assert_eq!(
        row().snapshot.unwrap().windows[0].remaining_percent,
        Some(18.0)
    );
    assert_eq!(fixture.source.calls.load(Ordering::Acquire), 1);

    expire_retry_window();
    set_response(json!({"base_resp":{"status_code":2049,"status_msg":"do-not-show-fixture-key"}}));
    let expired = fixture.start_for(&id, &original_revision);
    fixture.entered().await;
    fixture.source.resume.notify_one();
    let (_, Json(error)) = expired.await.unwrap().unwrap_err();
    assert!(!error.to_string().contains("do-not-show-fixture-key"));
    let unauthorized = row();
    assert_eq!(unauthorized.state, "subscription_authorization_failed");
    assert!(unauthorized.snapshot.is_some());
    let (target, _) = state
        .store
        .lock()
        .unwrap()
        .upstream_account_target(&provider, &original)
        .unwrap();
    assert!(
        state
            .billing
            .lock()
            .unwrap()
            .upstream_quota(&target, Some(&original), chrono::Utc::now())
            .is_none()
    );

    edit.expected_revision = original.revision;
    edit.credential_ref = uuid::Uuid::new_v4().to_string();
    state
        .credentials
        .set(&edit.credential_ref, b"sk-cp-new-fixture-key")
        .unwrap();
    state
        .store
        .lock()
        .unwrap()
        .save_upstream_account(&provider, &edit)
        .unwrap();
    let replacement = row();
    assert_eq!(replacement.state, "not_loaded");
    assert!(replacement.snapshot.is_none() && replacement.error.is_none());
    assert_ne!(replacement.source_revision, original_revision);
    set_response(json!({"base_resp":{"status_code":2062}}));
    let unconfirmed = fixture.start_for(&id, &replacement.source_revision);
    fixture.entered().await;
    fixture.source.resume.notify_one();
    assert!(unconfirmed.await.unwrap().is_err());
    assert_eq!(row().state, "subscription_unconfirmed");
    assert!(row().snapshot.is_none());

    expire_retry_window();
    set_response(budget(71));
    let recovered = fixture.start_for(&id, &replacement.source_revision);
    fixture.entered().await;
    fixture.source.resume.notify_one();
    assert!(recovered.await.unwrap().is_ok());
    let current = row();
    assert_eq!(current.state, "ready");
    assert!(current.error.is_none());
    assert_eq!(
        current.snapshot.unwrap().windows[0].remaining_percent,
        Some(71.0)
    );
    assert_eq!(fixture.source.calls.load(Ordering::Acquire), 4);
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

#[tokio::test]
async fn a_completed_read_cannot_publish_into_a_new_activation_of_the_same_account_source() {
    let fixture = Fixture::new().await;
    let Prepared::Read(reading) = prepare(
        &fixture.state,
        fixture.account_id.clone(),
        Refresh {
            expected_revision: fixture.revision.clone(),
        },
    )
    .unwrap() else {
        panic!("fresh fixture must read")
    };
    let Reading {
        account: prior_account,
        sources,
        flight,
        lease,
        ..
    } = *reading;
    let old_key = flight.key.clone();
    sources.restart();
    assert!(sources.active("openrouter"));
    assert!(
        publish(
            &fixture.state,
            prior_account,
            flight,
            Ok(AccountSnapshot::new("api", "old-generation")),
            lease
        )
        .is_err()
    );
    {
        let billing = fixture.state.billing.lock().unwrap();
        assert!(!billing.cache.contains_key(&fixture.account_id));
        assert!(!billing.failures.contains_key(&fixture.account_id));
        assert!(!billing.in_flight[&old_key].load(Ordering::Acquire));
    }
    let old_read = tokio::time::timeout(
        Duration::from_secs(1),
        sources.api(
            "openrouter",
            "https://openrouter.ai/api/v1/key",
            "fixture",
            old_key.epoch,
        ),
    )
    .await
    .unwrap();
    assert!(old_read.is_err());
    assert_eq!(fixture.source.calls.load(Ordering::Acquire), 0);
    let current = fixture.start();
    fixture.entered().await;
    fixture.source.resume.notify_one();
    assert_eq!(
        account(&current.await.unwrap().unwrap().0, &fixture.account_id)["state"],
        "ready"
    );
}
