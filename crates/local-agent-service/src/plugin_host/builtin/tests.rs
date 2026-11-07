use super::*;
use crate::plugin_host::ReadonlyRuntime;
use local_agent_core::{
    providers::{ProviderManager, vault::MemoryVault},
    sessions::NativeSessions,
};
use std::sync::Mutex;

struct Fixture {
    state: AppState,
    root: tempfile::TempDir,
}

#[tokio::test]
async fn request_view_leases_survive_unrelated_changes_and_release_when_requests_stop() {
    let f = Fixture::new().await;
    let id = uuid::Uuid::new_v4().to_string();
    f.state
        .observation
        .sql_query(|history| {
            let now = chrono::Utc::now();
            let page = history.request_page(
                &local_agent_core::requests::Query::default(),
                now,
                serde_json::Value::Null,
                None,
            )?;
            history
                .begin_csv_export(&id, page["snapshot"].as_str().unwrap(), now)
                .map_err(|_| "fixture export failed")?;
            Ok(())
        })
        .unwrap();
    let mut composition = Composition {
        profile: Profile::Desktop,
        disabled_plugin_ids: vec!["builtin.projects".into()],
        removed_plugin_ids: vec![],
    };
    apply(&f.state, f.change(composition.clone())).unwrap();
    assert!(
        f.state
            .observation
            .cached_sql_query(|history| Ok(history
                .csv_export_report(&id, chrono::Utc::now())
                .is_ok()))
            .unwrap()
    );
    composition.disabled_plugin_ids = vec![
        "builtin.overview".into(),
        "builtin.projects".into(),
        "builtin.requests".into(),
    ];
    apply(&f.state, f.change(composition)).unwrap();
    assert!(
        f.state
            .observation
            .cached_sql_query(|history| Ok(history
                .csv_export_report(&id, chrono::Utc::now())
                .is_err()))
            .unwrap()
    );
}

#[tokio::test]
async fn business_removal_persists_and_owned_support_stops_without_revoking_unrelated_sources() {
    let f = Fixture::new().await;
    let disabled = vec![
        "builtin.agents".into(),
        "builtin.model-services".into(),
        "builtin.providers".into(),
        "builtin.workbench".into(),
    ];
    let mut change = f.change(Composition {
        profile: Profile::Desktop,
        disabled_plugin_ids: disabled,
        removed_plugin_ids: vec!["builtin.providers".into()],
    });
    change.expected_impact = Some(read(&f.state).unwrap().impact);
    let reply = apply(&f.state, change.clone()).unwrap();
    for id in [
        "builtin.providers",
        "builtin.agents",
        "builtin.model-services",
        "builtin.workbench",
    ] {
        assert!(!super::super::features::active(&f.state, id));
    }
    assert!(super::super::features::active(&f.state, "builtin.sessions"));
    assert!(f.state.sessions.source_active("codex"));
    assert!(
        reply
            .catalog
            .host
            .plugins
            .iter()
            .filter(|p| p
                .capabilities
                .iter()
                .any(|c| c.address.name == "account.source"))
            .all(|p| !p.runtime_active && p.resources.is_empty())
    );
    assert_eq!(apply(&f.state, change).unwrap().receipt, reply.receipt);
    let saved = f
        .state
        .store
        .lock()
        .unwrap()
        .builtin_composition(Profile::Desktop)
        .unwrap();
    assert_eq!(
        saved.composition.removed_plugin_ids,
        vec!["builtin.providers"]
    );
    let (restarted, _) = ReadonlyRuntime::compose(Profile::Desktop, &saved.composition).unwrap();
    assert!(
        !restarted
            .host
            .snapshot()
            .unwrap()
            .plugins
            .iter()
            .find(|p| p.id == "builtin.providers")
            .unwrap()
            .runtime_active
    );
    let restored = apply(&f.state, f.change(Composition::default())).unwrap();
    assert!(restored.catalog.composition.removed_plugin_ids.is_empty());
    assert!(super::super::features::active(
        &f.state,
        "builtin.providers"
    ));
}

#[tokio::test]
async fn business_dependencies_and_changed_impact_fail_before_any_mutation() {
    let f = Fixture::new().await;
    let before = read(&f.state).unwrap();
    let change = f.change(Composition {
        profile: Profile::Desktop,
        disabled_plugin_ids: vec!["builtin.providers".into()],
        removed_plugin_ids: vec![],
    });
    assert_eq!(
        apply(&f.state, change).unwrap_err().code,
        "dependency_missing"
    );
    let mut change = f.change(Composition::default());
    change.expected_impact = Some(ImpactContext {
        active_tasks: false,
        model_services: vec!["a service not present in the reviewed store".into()],
    });
    assert_eq!(apply(&f.state, change).unwrap_err().code, "stale_snapshot");
    assert_eq!(read(&f.state).unwrap(), before);
}

#[tokio::test]
async fn disabled_business_routes_are_rejected_but_receipts_and_cancellation_remain_available() {
    let f = Fixture::new().await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let app = axum::Router::new()
        .fallback(|| async { axum::Json(serde_json::json!({"executed":true})) })
        .layer(axum::middleware::from_fn_with_state(f.state.clone(), guard));
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    apply(
        &f.state,
        f.change(Composition {
            profile: Profile::Desktop,
            disabled_plugin_ids: vec![
                "builtin.agents".into(),
                "builtin.model-services".into(),
                "builtin.projects".into(),
                "builtin.providers".into(),
                "builtin.workbench".into(),
            ],
            removed_plugin_ids: vec!["builtin.projects".into()],
        }),
    )
    .unwrap();
    let client = reqwest::Client::new();
    for path in [
        "/api/projects",
        "/api/projects/example",
        "/api/domain/project-visibility",
        "/api/providers",
        "/api/provider-models",
        "/api/model-services",
        "/api/model-service-members/verify",
        "/api/workbench",
        "/gateway/fixture/v1/messages",
        "/v1/messages",
    ] {
        let response = client.get(format!("{base}{path}")).send().await.unwrap();
        assert_eq!(response.status(), 409, "{path}");
        assert_eq!(
            response.json::<serde_json::Value>().await.unwrap()["code"],
            "plugin_disabled"
        );
    }
    for path in [
        "/api/builtin-plugins",
        "/api/sessions",
        "/api/provider-operations/fixture",
        "/api/model-service-operations/fixture",
    ] {
        assert_eq!(
            client
                .get(format!("{base}{path}"))
                .send()
                .await
                .unwrap()
                .json::<serde_json::Value>()
                .await
                .unwrap()["executed"],
            true,
            "{path}"
        );
    }
    assert_eq!(
        client
            .post(format!("{base}/api/provider-operations/cancel"))
            .send()
            .await
            .unwrap()
            .json::<serde_json::Value>()
            .await
            .unwrap()["executed"],
        true
    );
    server.abort();
}
impl Fixture {
    async fn new() -> Self {
        let root = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
        let home = root.path().join("home");
        let data = root.path().join("data");
        std::fs::create_dir_all(&home).unwrap();
        let store = Store::open(&data).unwrap();
        let vault = Arc::new(MemoryVault::default());
        let helper = root.path().join("helper");
        std::fs::write(&helper, b"offline helper").unwrap();
        let providers = ProviderManager::new(home.clone(), &data, helper, vault.clone()).unwrap();
        let (readonly, plugins) =
            ReadonlyRuntime::compose(Profile::Desktop, &Composition::default()).unwrap();
        let sources = readonly.accounts.clone();
        let (gateway, _listener) = crate::gateway::Gateway::bind(&data).await.unwrap();
        let state = AppState {
            workbench: None,
            provider_adapters: crate::provider_models::adapters::builtin(),
            provider_client: crate::state::provider_client().unwrap(),
            credentials: vault.clone(),
            plugins: Arc::new(crate::plugin_host::external::Manager::open(&data, vault).unwrap()),
            readonly: Arc::new(readonly),
            store: Arc::new(Mutex::new(store)),
            token: Arc::new("offline-composition".into()),
            host: Arc::new("127.0.0.1:9".into()),
            sessions: Arc::new(NativeSessions::with_source_plugins(
                home.clone(),
                &[],
                plugins,
            )),
            session_work: Arc::new(crate::sessions::work::SessionWork::default()),
            observation: Arc::new(
                crate::observation::Observation::new(&data, "127.0.0.1:9").unwrap(),
            ),
            listeners: Arc::new(Mutex::new(Default::default())),
            providers: Arc::new(Mutex::new(providers)),
            verification_capacity: Arc::new(tokio::sync::Semaphore::new(2)),
            query_capacity: crate::blocking::query_capacity(),
            billing: Arc::new(Mutex::new(crate::billing::Billing::with_sources(
                home, &data, sources,
            ))),
            gateway: Arc::new(gateway),
        };
        state.readonly.bind_resources(&state).unwrap();
        Self { state, root }
    }
    fn change(&self, composition: Composition) -> CompositionChange {
        let catalog = read(&self.state).unwrap();
        CompositionChange {
            expected_impact: None,
            operation_id: uuid::Uuid::new_v4().to_string(),
            store_id: catalog.store_id,
            expected_revision: catalog.revision,
            expected_generation: catalog.host.generation,
            composition,
        }
    }
}
#[test]
fn profiles_select_one_shared_graph_and_registration_never_grants_source_access() {
    for (profile, count) in [
        (Profile::Desktop, 25),
        (Profile::Background, 22),
        (Profile::Recovery, 0),
    ] {
        let (runtime, sources) = ReadonlyRuntime::compose(
            profile,
            &Composition {
                profile,
                disabled_plugin_ids: vec![],
                removed_plugin_ids: vec![],
            },
        )
        .unwrap();
        let root = tempfile::tempdir().unwrap();
        let sessions = NativeSessions::with_source_plugins(root.path().into(), &[], sources);
        assert!(Arc::ptr_eq(&runtime.host, &sessions.builtin_host()));
        let catalog = runtime.host.snapshot().unwrap();
        assert_eq!(catalog.plugins.len(), 25);
        assert_eq!(
            catalog
                .plugins
                .iter()
                .filter(|plugin| plugin.runtime_active)
                .count(),
            count
        );
        assert!(
            catalog
                .plugins
                .iter()
                .all(|plugin| plugin.resources.is_empty())
        );
        assert!(
            sessions
                .sources()
                .unwrap()
                .iter()
                .all(|source| !source.enabled)
        );
        assert_eq!(
            runtime.intake.acquire().is_some(),
            profile != Profile::Recovery
        );
    }
}
#[tokio::test]
async fn source_disable_revokes_saved_and_memory_consent_and_replay_cannot_restore_it() {
    let f = Fixture::new().await;
    f.state
        .store
        .lock()
        .unwrap()
        .save_native_source("codex", true)
        .unwrap();
    f.state.sessions.set_enabled("codex", true).unwrap();
    let id = f
        .state
        .sessions
        .source_plugin_id("codex")
        .unwrap()
        .to_owned();
    let request = f.change(Composition {
        profile: Profile::Desktop,
        disabled_plugin_ids: vec![id],
        removed_plugin_ids: vec![],
    });
    let first = apply(&f.state, request.clone()).unwrap();
    assert!(!f.state.sessions.source_active("codex"));
    assert!(
        !f.state
            .sessions
            .sources()
            .unwrap()
            .iter()
            .find(|source| source.agent_id == "codex")
            .unwrap()
            .enabled
    );
    assert!(
        f.state
            .store
            .lock()
            .unwrap()
            .native_sources()
            .unwrap()
            .is_empty()
    );
    let on = apply(&f.state, f.change(Composition::default())).unwrap();
    assert!(f.state.sessions.source_active("codex"));
    assert!(
        f.state
            .store
            .lock()
            .unwrap()
            .native_sources()
            .unwrap()
            .is_empty()
    );
    assert_eq!(apply(&f.state, request).unwrap().receipt, first.receipt);
    assert_eq!(read(&f.state).unwrap().host, on.catalog.host);
    let store = Store::open(&f.root.path().join("data")).unwrap();
    assert_eq!(
        store
            .builtin_composition(Profile::Background)
            .unwrap()
            .composition,
        Composition::default()
    );
    assert!(store.native_sources().unwrap().is_empty());
}
#[tokio::test]
async fn rejected_dependency_and_stale_or_foreign_operations_preserve_generation_and_consent() {
    let f = Fixture::new().await;
    let before = read(&f.state).unwrap();
    let invalid = f.change(Composition {
        profile: Profile::Desktop,
        disabled_plugin_ids: vec!["project.source".into()],
        removed_plugin_ids: vec![],
    });
    assert_eq!(
        apply(&f.state, invalid).unwrap_err().code,
        "dependency_missing"
    );
    let mut request = f.change(Composition::default());
    request.expected_generation = uuid::Uuid::new_v4().to_string();
    assert_eq!(apply(&f.state, request).unwrap_err().code, "stale_snapshot");
    let mut request = f.change(Composition::default());
    request.store_id = uuid::Uuid::new_v4().to_string();
    assert_eq!(
        apply(&f.state, request).unwrap_err().code,
        "operation_store_changed"
    );
    assert_eq!(read(&f.state).unwrap(), before);
    assert!(f.state.readonly.intake.acquire().is_some());
}
#[tokio::test]
async fn recovery_is_reversible_without_automatic_source_authorization_or_reference_cycles() {
    let f = Fixture::new().await;
    let sessions = Arc::downgrade(&f.state.sessions);
    let runtime = Arc::downgrade(&f.state.readonly);
    apply(
        &f.state,
        f.change(Composition {
            profile: Profile::Recovery,
            disabled_plugin_ids: vec![],
            removed_plugin_ids: vec![],
        }),
    )
    .unwrap();
    assert!(f.state.readonly.intake.acquire().is_none());
    assert!(f.state.sessions.set_enabled("codex", true).is_err());
    let restored = apply(&f.state, f.change(Composition::default())).unwrap();
    assert_eq!(
        restored
            .catalog
            .host
            .plugins
            .iter()
            .filter(|p| p.runtime_active)
            .count(),
        25
    );
    assert!(
        f.state
            .sessions
            .sources()
            .unwrap()
            .iter()
            .all(|source| !source.enabled)
    );
    assert!(f.state.readonly.intake.acquire().is_some());
    drop(f);
    assert!(sessions.upgrade().is_none());
    assert!(runtime.upgrade().is_none());
}
