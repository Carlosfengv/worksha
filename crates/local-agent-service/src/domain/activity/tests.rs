use super::*;
use std::sync::{Arc, Mutex};
struct Fixture {
    state: AppState,
    _root: tempfile::TempDir,
}
impl Fixture {
    async fn new() -> Self {
        use local_agent_core::{
            Store,
            providers::{ProviderManager, vault::MemoryVault},
            sessions::NativeSessions,
        };
        let root = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
        let home = root.path().join("home");
        let data = root.path().join("data");
        let files = home.join(".claude/projects/fixture");
        std::fs::create_dir_all(&files).unwrap();
        let at = chrono::Utc::now().to_rfc3339();
        let rows = [
            serde_json::json!({"type":"user","sessionId":"activity-fixture","uuid":"u1","promptId":"turn-1","entrypoint":"cli","cwd":home,"timestamp":at,"message":{"role":"user","content":"owned activity fixture"}}),
            serde_json::json!({"type":"assistant","sessionId":"activity-fixture","uuid":"a1","timestamp":at,"message":{"id":"r1","model":"fixture","content":[{"type":"text","text":"owned reply"}],"usage":{"input_tokens":10,"output_tokens":3}}}),
        ];
        std::fs::write(
            files.join("fixture.jsonl"),
            rows.iter()
                .map(|r| r.to_string() + "\n")
                .collect::<String>(),
        )
        .unwrap();
        let store = Store::open(&data).unwrap();
        let vault = Arc::new(MemoryVault::default());
        let helper = root.path().join("helper");
        std::fs::write(&helper, "offline").unwrap();
        let providers = ProviderManager::new(home.clone(), &data, helper, vault.clone()).unwrap();
        let (gateway, _listener) = crate::gateway::Gateway::bind(&data).await.unwrap();
        let state = AppState {
            provider_adapters: crate::provider_models::adapters::builtin(),
            provider_client: crate::state::provider_client().unwrap(),
            credentials: vault.clone(),
            plugins: Arc::new(crate::plugin_host::external::Manager::open(&data, vault).unwrap()),
            readonly: Arc::new(crate::plugin_host::ReadonlyRuntime::builtin().unwrap()),
            store: Arc::new(Mutex::new(store)),
            token: Arc::new("owned-activity-test".into()),
            host: Arc::new("127.0.0.1:9".into()),
            sessions: Arc::new(NativeSessions::new(home.clone(), &["claude".into()])),
            session_work: Arc::new(crate::sessions::work::SessionWork::default()),
            observation: Arc::new(
                crate::observation::Observation::new(&data, "127.0.0.1:9").unwrap(),
            ),
            listeners: Arc::new(Mutex::new(Default::default())),
            providers: Arc::new(Mutex::new(providers)),
            verification_capacity: Arc::new(tokio::sync::Semaphore::new(2)),
            query_capacity: crate::blocking::query_capacity(),
            billing: Arc::new(Mutex::new(crate::billing::Billing::new(home, &data))),
            gateway: Arc::new(gateway),
        };
        state.sessions.refresh().unwrap();
        state
            .observation
            .poll_native_guarded(&state.sessions, &|| true);
        Self { state, _root: root }
    }
}
fn input() -> a::UnifiedActivityQuery {
    a::UnifiedActivityQuery {
        scope: d::QueryScope {
            kind: d::ScopeKind::Builtin,
            source_ids: vec!["claude".into()],
        },
        range_seconds: 10800,
        product_id: None,
        session_id: None,
        include_gateway: false,
        include_historical: true,
    }
}

#[tokio::test]
async fn activity_reads_published_native_results_without_waiting_for_the_sampler_lock() {
    let f = Fixture::new().await;
    let expected = query(&f.state, input()).unwrap();
    assert_eq!(expected.items.len(), 1);
    assert_eq!(expected.items[0].kind, a::ActivityKind::NativeMessage);
    assert!(
        expected.items[0]
            .measurements
            .as_ref()
            .unwrap()
            .live_rate
            .is_none()
    );
    let encoded = serde_json::to_string(&expected).unwrap();
    assert!(!encoded.contains(f.state.sessions.home().to_str().unwrap()));
    assert!(!encoded.contains("owned reply"));
    assert!(!encoded.contains("userInput"));
    assert!(!encoded.contains("projectPath"));
    let held = f.state.observation.native_watch.lock().unwrap();
    let state = f.state.clone();
    let (tx, rx) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || tx.send(query(&state, input())).unwrap());
    let result = rx.recv_timeout(std::time::Duration::from_secs(1));
    drop(held);
    worker.join().unwrap();
    assert_eq!(result.unwrap().unwrap().items[0].id, expected.items[0].id);
}
#[tokio::test]
async fn activity_queries_do_not_restore_consent_and_inflight_revocation_rejects_the_result() {
    let f = Fixture::new().await;
    assert_eq!(
        query_with(&f.state, input(), &|| {
            f.state.sessions.set_enabled("claude", false).unwrap();
        })
        .err()
        .unwrap()
        .code,
        "stale_snapshot"
    );
    let page = query(&f.state, input()).unwrap();
    assert!(page.items.is_empty());
    assert_eq!(
        page.source_states[0].state.authorization,
        d::Authorization::Revoked
    );
    assert!(page.source_summaries[0].observed_records.is_none());
    f.state.sessions.set_enabled("claude", true).unwrap();
    // A newly granted source cannot resurrect the prior sampler publication.
    let page = query(&f.state, input()).unwrap();
    assert!(page.items.is_empty());
    assert!(page.source_summaries[0].observed_records.is_none());
}
#[tokio::test]
async fn stopping_collection_invalidates_an_inflight_query_without_fabricating_an_empty_success() {
    let f = Fixture::new().await;
    assert_eq!(
        query_with(&f.state, input(), &|| f.state.observation.pause_native())
            .err()
            .unwrap()
            .code,
        "stale_snapshot"
    );
    let page = query(&f.state, input()).unwrap();
    assert!(page.items.is_empty());
    assert_eq!(
        page.source_states[0].state.problem.as_deref(),
        Some("collector_stopped")
    );
}

#[test]
fn measured_rate_is_message_bound_and_expires_without_changing_the_historical_sample() {
    use local_agent_core::sessions::activity_performance::{ActivitySampling, RateSample};
    let now = chrono::Utc::now().timestamp_millis();
    let sample = ActivitySampling {
        at: now,
        window_ms: 5000,
        tokens_per_second: Some(42.0),
        recent: Some(RateSample {
            at: now - 10,
            window_ms: 5000,
            tokens_per_second: 42.0,
        }),
        reported: None,
    };
    let start = chrono::DateTime::from_timestamp_millis(now - 10000)
        .unwrap()
        .to_rfc3339();
    let (live, recent) = metrics::rate(Some(&sample), Some(&start), now, true);
    assert_eq!(live.unwrap().tokens_per_second, 42.0);
    assert!(recent.is_some());
    let (live, recent) = metrics::rate(Some(&sample), Some(&start), now + 6000, true);
    assert!(live.is_none());
    assert_eq!(recent.unwrap().at_ms.value(), (now - 10) as u64);
    let next = chrono::DateTime::from_timestamp_millis(now + 1)
        .unwrap()
        .to_rfc3339();
    let (live, recent) = metrics::rate(Some(&sample), Some(&next), now, true);
    assert!(live.is_none() && recent.is_none());
    assert!(
        metrics::rate(Some(&sample), Some(&start), now, false)
            .0
            .is_none()
    );
}
