use super::stream::Update;
use local_agent_core::observation::{ModelObservation, ObservationStore};
use local_agent_core::sessions::activity_input::UserInput;
use local_agent_core::sessions::activity_input_cache::{CachedInput, InputCache};
use local_agent_core::sessions::activity_performance::{
    ActivitySampling, RATE_WINDOW_MS, RateSample,
};
use serde::Serialize;
use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::Mutex,
    time::{Duration, Instant},
};

fn read_warning(invalid: usize) -> Option<String> {
    (invalid > 0)
        .then(|| format!("有 {invalid} 条历史观测记录无法解析，已隔离显示；原数据库记录保持不变"))
}

#[cfg(all(test, unix))]
mod store_identity_tests;

type QueryLock<'a> = (
    super::query_queue::Permit<'a>,
    std::sync::MutexGuard<'a, Option<local_agent_core::observation::sql::History>>,
);

struct Live {
    user_input: Option<UserInput>,
    tool_names: Vec<String>,
    row: ModelObservation,
    start: Instant,
    last: Instant,
    phase: String,
    window: VecDeque<(Instant, f64)>,
    window_since: Option<Instant>,
    output_window: VecDeque<(Instant, f64)>,
    output_window_since: Option<Instant>,
    checkpoint: Instant,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn smooth_gateway_samples_are_historical_after_expiry_and_restart() {
        let dir = tempfile::tempdir().unwrap();
        let clock = Instant::now();
        let hub = Hub::new(ObservationStore::open(dir.path()).unwrap()).unwrap();
        let mut request = row("smooth", "claude", 0);
        request.status = "in_progress".into();
        request.ended_at = None;
        hub.start(request, clock, None);
        for (ms, tokens) in [(500, 10.0), (3000, 20.0)] {
            hub.update_at(
                "smooth",
                Update {
                    generated: true,
                    estimated_output_tokens: tokens,
                    ..Default::default()
                },
                clock + Duration::from_millis(ms),
            );
        }
        let view = |ms| {
            hub.activities_at(clock + Duration::from_millis(ms))
                .into_iter()
                .find(|a| a.request.id == "smooth")
                .unwrap()
        };
        assert_eq!(view(5000).sampling.tokens_per_second, Some(6.0));
        assert_eq!(view(5500).sampling.tokens_per_second, Some(4.0));
        assert!(view(8000).sampling.tokens_per_second.is_none());
        let recent = view(8000).sampling.recent.unwrap();
        assert_eq!(recent.tokens_per_second, 6.0);
        assert_eq!(view(9000).sampling.recent.unwrap().at, recent.at);
        hub.finish("smooth", "success", None);
        hub.flush(Duration::from_secs(3)).unwrap();
        drop(hub);
        let hub = Hub::new(ObservationStore::open(dir.path()).unwrap()).unwrap();
        let restored = hub
            .activities()
            .into_iter()
            .find(|a| a.request.id == "smooth")
            .unwrap();
        assert_eq!(restored.sampling.recent.unwrap(), recent);
        assert!(restored.sampling.tokens_per_second.is_none());
    }
    #[tokio::test]
    async fn reasoning_and_tool_arguments_have_live_output_rates_before_final_text() {
        let dir = tempfile::tempdir().unwrap();
        let hub = Hub::new(ObservationStore::open(dir.path()).unwrap()).unwrap();
        let start = Instant::now();
        let mut r = row("live-output", "claude", 0);
        r.status = "in_progress".into();
        r.ended_at = None;
        r.ttft_ms = None;
        r.first_text_ms = None;
        r.last_text_ms = None;
        r.estimated_text_tokens = None;
        hub.start(r, start, None);
        let snapshot = |ms| {
            hub.activities_at(start + Duration::from_millis(ms))
                .into_iter()
                .find(|a| a.request.id == "live-output")
                .unwrap()
        };
        assert!(snapshot(200).output_tokens_per_second.is_none());
        for (ms, wire, phase, rate) in [
            (
                500,
                r#"{"type":"content_block_delta","delta":{"type":"thinking_delta","thinking":"abcd"}}"#,
                "public_reasoning",
                1.0,
            ),
            (
                700,
                r#"{"type":"content_block_delta","delta":{"type":"input_json_delta","partial_json":"12345678"}}"#,
                "tool_arguments",
                3.0,
            ),
        ] {
            hub.update_at(
                "live-output",
                super::super::stream::decode(wire.as_bytes(), "messages"),
                start + Duration::from_millis(ms),
            );
            let a = snapshot(ms + 10);
            assert_eq!(a.phase, phase);
            assert_eq!(a.output_tokens_per_second, Some(rate));
            assert_eq!(a.request.ttft_ms, Some(500.0));
            assert!(a.request.first_text_ms.is_none());
            assert!(a.request.estimated_text_tokens.is_none());
            assert!(a.text_tokens_per_second.is_none());
            let json = serde_json::to_value(a).unwrap();
            assert_eq!(json["outputTokensPerSecond"], rate);
        }
        assert_eq!(snapshot(1800).output_tokens_per_second, Some(0.0));
        hub.update_at(
            "live-output",
            Update {
                issue: true,
                ..Default::default()
            },
            start + Duration::from_millis(2000),
        );
        assert!(snapshot(2100).output_tokens_per_second.is_none());
        let wire = br#"{"type":"content_block_delta","delta":{"type":"thinking_delta","thinking":"abcd"}}"#;
        hub.update_at(
            "live-output",
            super::super::stream::decode(wire, "messages"),
            start + Duration::from_millis(2200),
        );
        assert!(snapshot(2300).output_tokens_per_second.is_none());
        hub.update_at(
            "live-output",
            super::super::stream::decode(wire, "messages"),
            start + Duration::from_millis(3100),
        );
        assert_eq!(snapshot(3200).output_tokens_per_second, Some(2.0));
        hub.finish("live-output", "success", None);
        assert!(snapshot(3300).output_tokens_per_second.is_none());
        assert_eq!(snapshot(3300).request.ttft_ms, Some(500.0));
    }
    #[test]
    fn query_time_is_sampled_after_admission_not_when_a_waiting_request_arrives() {
        let dir = tempfile::tempdir().unwrap();
        let hub =
            std::sync::Arc::new(Hub::new(ObservationStore::open(dir.path()).unwrap()).unwrap());
        let held = hub.sql.lock().unwrap();
        let worker = std::thread::spawn({
            let hub = hub.clone();
            move || hub.query_now(|_, now| Ok(now))
        });
        let deadline = Instant::now() + Duration::from_secs(1);
        while hub.query_queue.issued() == 0 {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(1));
        }
        std::thread::sleep(Duration::from_millis(20));
        let released = chrono::Utc::now();
        drop(held);
        assert!(worker.join().unwrap().unwrap() >= released);
    }
    #[test]
    fn a_queued_history_read_survives_five_second_contention_within_the_read_deadline() {
        let dir = tempfile::tempdir().unwrap();
        let hub =
            std::sync::Arc::new(Hub::new(ObservationStore::open(dir.path()).unwrap()).unwrap());
        let held = hub.sql.lock().unwrap();
        let started = Instant::now();
        let worker = std::thread::spawn({
            let hub = hub.clone();
            move || hub.query(|_| Ok(7))
        });
        while hub.query_queue.issued() == 0 {
            assert!(started.elapsed() < Duration::from_secs(1));
            std::thread::sleep(Duration::from_millis(1));
        }
        std::thread::sleep(Duration::from_millis(5200));
        drop(held);
        assert_eq!(worker.join().unwrap().unwrap(), 7);
        assert!(started.elapsed() < Duration::from_secs(8));
    }
    #[tokio::test]
    async fn message_identity_and_first_request_time_survive_tool_requests_and_restart() {
        let dir = tempfile::tempdir().unwrap();
        let hub = Hub::new(ObservationStore::open(dir.path()).unwrap()).unwrap();
        let mut first = row("message-first", "claude", 0);
        first.session_id = Some("claude:s".into());
        first.activity_message = Some(local_agent_core::observation::ActivityMessage {
            id: "opaque-message".into(),
            started_at: first.started_at.clone(),
        });
        let started = first.started_at.clone();
        hub.start(first, Instant::now(), None);
        hub.finish("message-first", "success", None);
        for _ in 0..100 {
            if ObservationStore::open(dir.path())
                .unwrap()
                .load()
                .unwrap()
                .iter()
                .any(|r| r.id == "message-first" && r.status == "success")
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let restarted = Hub::new(ObservationStore::open(dir.path()).unwrap()).unwrap();
        let mut next = row("message-next-request", "claude", 0);
        next.session_id = Some("claude:s".into());
        next.activity_message = Some(local_agent_core::observation::ActivityMessage {
            id: "opaque-message".into(),
            started_at: "2099-01-01T00:00:00Z".into(),
        });
        restarted.start(next, Instant::now(), None);
        let rows = restarted.rows();
        assert_eq!(
            rows.iter()
                .find(|r| r.id == "message-next-request")
                .unwrap()
                .activity_message
                .as_ref()
                .unwrap()
                .started_at,
            started
        );
        assert_eq!(rows.len(), 2); // Request history stays request-level.
    }
    #[tokio::test]
    async fn bounded_activity_input_survives_restart_without_tool_bodies_in_request_history() {
        let dir = tempfile::tempdir().unwrap();
        let hub = Hub::new(ObservationStore::open(dir.path()).unwrap()).unwrap();
        let mut r = row("preview-memory", "claude", 0);
        r.status = "in_progress".into();
        r.ended_at = None;
        r.ttft_ms = None;
        hub.start(
            r,
            Instant::now(),
            Some(UserInput {
                text: "input-preview-sentinel".into(),
                at: chrono::Utc::now().to_rfc3339(),
            }),
        );
        hub.update(
            "preview-memory",
            Update {
                tool: true,
                tool_names: vec!["Read".into(), "Grep".into()],
                ..Default::default()
            },
        );
        let a = hub.activities().remove(0);
        assert_eq!(a.phase, "tool_arguments");
        assert_eq!(a.tool_names, vec!["Read", "Grep"]);
        assert!(a.request.ttft_ms.is_none());
        assert_eq!(a.user_input.unwrap().text, "input-preview-sentinel");
        hub.finish("preview-memory", "success", None);
        hub.flush(Duration::from_secs(3)).unwrap();
        for _ in 0..100 {
            let rows = ObservationStore::open(dir.path()).unwrap().load().unwrap();
            if rows.first().is_some_and(|r| r.status == "success") {
                let raw = serde_json::to_string(&rows).unwrap();
                assert!(!raw.contains("input-preview-sentinel"));
                assert!(!raw.contains("toolNames"));
                let restarted = Hub::new(ObservationStore::open(dir.path()).unwrap())
                    .unwrap()
                    .activities()
                    .remove(0);
                assert_eq!(
                    restarted.user_input.as_ref().unwrap().text,
                    "input-preview-sentinel"
                );
                assert!(restarted.tool_names.is_empty());
                assert!(restarted.tool_names.is_empty());
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("request metadata was not persisted");
    }
    #[test]
    fn corrupt_activity_summary_cache_recovers_without_a_permanent_persistence_error() {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().canonicalize().unwrap();
        let path = directory.join("gateway-activity-inputs.json");
        std::fs::write(&path, b"{broken").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        let hub = Hub::new(ObservationStore::open(&directory).unwrap()).unwrap();
        assert!(hub.error().is_some());
        hub.start(
            row("recovered-input", "claude", 0),
            Instant::now(),
            Some(UserInput {
                text: "恢复后的输入".into(),
                at: chrono::Utc::now().to_rfc3339(),
            }),
        );
        hub.finish("recovered-input", "success", None);
        hub.flush(Duration::from_secs(3)).unwrap();
        assert!(hub.error().is_none());
        let restarted = Hub::new(ObservationStore::open(&directory).unwrap()).unwrap();
        assert_eq!(
            restarted.activities()[0].user_input.as_ref().unwrap().text,
            "恢复后的输入"
        );
    }
    #[test]
    fn complete_history_survives_live_cache_limits_and_external_updates() {
        let dir = tempfile::tempdir().unwrap();
        let store = ObservationStore::open(dir.path()).unwrap();
        for begin in (0..100005).step_by(500) {
            let batch: Vec<_> = (begin..(begin + 500).min(100005))
                .map(|n| {
                    let mut value = row(
                        &format!("history-{n:06}"),
                        if n == 0 { "opencode" } else { "claude" },
                        0,
                    );
                    value.usage.input_tokens = Some(10);
                    value.usage.output_tokens = Some(2);
                    value.usage.total_tokens = Some(12);
                    value
                })
                .collect();
            store.save_batch(&batch).unwrap();
        }
        let hub = Hub::new(ObservationStore::open(dir.path()).unwrap()).unwrap();
        assert!(hub.rows().len() < 520);
        assert!(
            hub.activities()
                .iter()
                .any(|a| a.request.agent_id == "opencode")
        );
        let history = hub.history().unwrap();
        assert_eq!(history.len(), 100005);
        assert_eq!(
            history
                .iter()
                .filter_map(|r| r.usage.total_tokens)
                .sum::<u64>(),
            1200060
        );
        drop(history);
        let mut changed = row("history-100004", "claude", 0);
        changed.revision = 99;
        changed.status = "error".into();
        store.save(&changed).unwrap();
        let refreshed = hub.history().unwrap();
        assert_eq!(
            refreshed
                .iter()
                .find(|r| r.id == changed.id)
                .unwrap()
                .status,
            "error"
        );
        assert_eq!(refreshed.len(), 100005);
    }
    #[tokio::test]
    async fn damaged_events_preserve_first_output_and_a_clean_live_window_recovers() {
        let dir = tempfile::tempdir().unwrap();
        let hub = Hub::new(ObservationStore::open(dir.path()).unwrap()).unwrap();
        let start = Instant::now();
        for (id, damaged_first) in [("measured-first", false), ("missing-first", true)] {
            let mut r = row(id, "claude", 0);
            r.status = "in_progress".into();
            r.ended_at = None;
            r.ttft_ms = None;
            hub.start(r, start, None);
            let text = || Update {
                generated: true,
                text: true,
                estimated_tokens: 20.0,
                ..Default::default()
            };
            if !damaged_first {
                hub.update_at(id, text(), start + Duration::from_millis(500));
            }
            hub.update_at(
                id,
                Update {
                    issue: true,
                    ..Default::default()
                },
                start + Duration::from_secs(1),
            );
            let measured = || hub.rows().into_iter().find(|r| r.id == id).unwrap();
            assert_eq!(measured().ttft_ms, (!damaged_first).then_some(500.0));
            assert_eq!(measured().first_text_ms, (!damaged_first).then_some(500.0));
            hub.update_at(id, text(), start + Duration::from_millis(1500));
            let snapshot = |ms| {
                hub.activities_at(start + Duration::from_millis(ms))
                    .into_iter()
                    .find(|a| a.request.id == id)
                    .unwrap()
            };
            assert!(snapshot(1600).text_tokens_per_second.is_none());
            hub.update_at(id, text(), start + Duration::from_millis(2400));
            assert_eq!(snapshot(2500).text_tokens_per_second, Some(40.0));
            assert_eq!(snapshot(2600).text_tokens_per_second, Some(20.0));
            assert_eq!(measured().ttft_ms, (!damaged_first).then_some(500.0));
            assert_eq!(measured().first_text_ms, (!damaged_first).then_some(500.0));
            assert!(snapshot(2600).average_text_tokens_per_second.is_none());
            assert_eq!(measured().last_text_ms, Some(2400.0));
            assert!(measured().estimated_text_tokens.is_none());
            hub.finish(id, "interrupted", Some("连接中断"));
            let finished = snapshot(2700);
            assert!(finished.text_tokens_per_second.is_none());
            assert_eq!(finished.request.ttft_ms, (!damaged_first).then_some(500.0));
        }
        // A restart must keep measured TTFT without inventing live samples.
        for _ in 0..100 {
            let saved = ObservationStore::open(dir.path()).unwrap().load().unwrap();
            if saved.len() == 2 && saved.iter().all(|r| r.status == "interrupted") {
                let reloaded = Hub::new(ObservationStore::open(dir.path()).unwrap()).unwrap();
                let a = reloaded
                    .activities()
                    .into_iter()
                    .find(|a| a.request.id == "measured-first")
                    .unwrap();
                assert_eq!(a.request.ttft_ms, Some(500.0));
                assert!(a.text_tokens_per_second.is_none());
                assert!(a.average_text_tokens_per_second.is_none());
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("timing metadata was not persisted");
    }
    #[tokio::test]
    async fn all_parallel_requests_survive_history_limits_with_independent_metrics() {
        let dir = tempfile::tempdir().unwrap();
        let store = ObservationStore::open(dir.path()).unwrap();
        for n in 0..45 {
            store
                .save(&row(&format!("completed-{n}"), "claude", 0))
                .unwrap();
        }
        let hub = Hub::new(store).unwrap();
        let now = Instant::now();
        for n in 0..35 {
            let id = format!("parallel-{n}");
            let mut r = row(&id, "codex", 120);
            r.agent_product_id = Some("codex-cli".into());
            r.status = "in_progress".into();
            r.ended_at = None;
            r.ttft_ms = None;
            hub.start(
                r,
                now - Duration::from_secs(if n == 0 { 1 } else { 3 }),
                None,
            );
            hub.update_at(
                &id,
                Update {
                    generated: true,
                    text: true,
                    estimated_tokens: if n == 0 { 20.0 } else { 40.0 },
                    ..Default::default()
                },
                now,
            );
        }
        let active: Vec<_> = hub
            .activities()
            .into_iter()
            .filter(|a| a.request.status == "in_progress")
            .collect();
        assert_eq!(active.len(), 35);
        assert!(active.iter().all(|a| !a.stale));
        let first = active
            .iter()
            .find(|a| a.request.id == "parallel-0")
            .unwrap();
        assert_eq!(first.request.ttft_ms, Some(1000.0));
        assert_eq!(first.text_tokens_per_second, Some(20.0));
        let second = active
            .iter()
            .find(|a| a.request.id == "parallel-1")
            .unwrap();
        assert_eq!(second.request.ttft_ms, Some(3000.0));
        assert_eq!(second.text_tokens_per_second, Some(40.0));
    }
    #[tokio::test]
    async fn only_documented_official_endpoint_cost_is_persisted_and_reloads() {
        let dir = tempfile::tempdir().unwrap();
        let hub = Hub::new(ObservationStore::open(dir.path()).unwrap()).unwrap();
        for (id, endpoint) in [
            ("actual", "https://openrouter.ai/api/v1"),
            ("unknown", "https://other.test/v1"),
        ] {
            let mut r = row(id, "opencode", 0);
            r.endpoint = endpoint.into();
            r.status = "in_progress".into();
            hub.start(r, Instant::now(), None);
            hub.update(
                id,
                Update {
                    reported_cost: Some(0.002),
                    ..Default::default()
                },
            );
            hub.finish(id, "success", None);
        }
        // Persistence is asynchronous; wait only for our two metadata fixtures.
        for _ in 0..100 {
            let rows = ObservationStore::open(dir.path()).unwrap().load().unwrap();
            if rows.len() == 2 && rows.iter().all(|r| r.status == "success") {
                assert_eq!(
                    rows.iter()
                        .find(|r| r.id == "actual")
                        .unwrap()
                        .reported_cost
                        .as_ref()
                        .unwrap()
                        .amount,
                    0.002
                );
                assert!(
                    rows.iter()
                        .find(|r| r.id == "unknown")
                        .unwrap()
                        .reported_cost
                        .is_none()
                );
                let reloaded = Hub::new(ObservationStore::open(dir.path()).unwrap()).unwrap();
                assert_eq!(
                    reloaded.rows.lock().unwrap()["actual"]
                        .row
                        .reported_cost
                        .as_ref()
                        .unwrap()
                        .currency,
                    "USD"
                );
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("cost metadata was not persisted");
    }
    fn row(id: &str, agent: &str, minutes: i64) -> ModelObservation {
        let time = (chrono::Utc::now() - chrono::Duration::minutes(minutes)).to_rfc3339();
        serde_json::from_value(serde_json::json!({
            "id":id,"agentId":agent,"provider":"fixture","endpoint":"http://localhost/v1",
            "bindingRevision":1,"startedAt":time,"endedAt":time,"status":"success",
            "protocol":"messages","revision":1,"usage":{},"ttftMs":400.0
        }))
        .unwrap()
    }
    #[tokio::test]
    async fn latest_per_agent_and_every_active_request_survive_age_and_global_feed_limits() {
        let dir = tempfile::tempdir().unwrap();
        let store = ObservationStore::open(dir.path()).unwrap();
        store.save(&row("old-opencode", "opencode", 120)).unwrap();
        store.save(&row("old-codex", "codex", 90)).unwrap();
        for index in 0..45 {
            store
                .save(&row(&format!("recent-{index}"), "claude", 0))
                .unwrap();
        }
        let hub = Hub::new(store).unwrap();
        let mut live = row("running", "claude", 180);
        live.status = "in_progress".into();
        live.ended_at = None;
        hub.start(live, Instant::now(), None);
        let activities = hub.activities();
        assert!(
            activities
                .iter()
                .any(|a| a.request.id == "old-opencode" && a.request.ttft_ms == Some(400.0))
        );
        assert!(activities.iter().any(|a| a.request.id == "old-codex"));
        assert!(activities.iter().any(|a| a.request.id == "running"));
        assert_eq!(activities.len(), 48);
        assert!(
            activities
                .iter()
                .all(|a| a.average_text_tokens_per_second.is_none())
        );
    }
}
pub struct Hub {
    input_recovery_error: Option<String>,
    message_key: Option<zeroize::Zeroizing<String>>,
    rows: Mutex<HashMap<String, Live>>,
    directory: std::path::PathBuf,
    writer: super::persistence::Writer,
    error: Mutex<Option<String>>,
    #[cfg(test)]
    history: Mutex<History>,
    sql: Mutex<Option<local_agent_core::observation::sql::History>>,
    query_queue: super::query_queue::Queue,
}
#[cfg(test)]
#[derive(Default)]
struct History {
    store: Option<ObservationStore>,
    identity: Option<std::fs::Metadata>,
    version: Option<i64>,
    rows: Vec<ModelObservation>,
    invalid: usize,
}
#[cfg(test)]
impl History {
    fn read(&mut self, directory: &std::path::Path) -> Result<(), String> {
        let identity = std::fs::metadata(directory.join("observations.sqlite3")).ok();
        let same = self
            .identity
            .as_ref()
            .zip(identity.as_ref())
            .is_some_and(|(a, b)| {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::MetadataExt;
                    a.dev() == b.dev() && a.ino() == b.ino()
                }
                #[cfg(not(unix))]
                {
                    a.created().ok() == b.created().ok()
                }
            });
        if !same {
            self.store = None;
            self.version = None;
        }
        if self.store.is_none() {
            self.store = Some(ObservationStore::open(directory)?);
        }
        let store = self.store.as_ref().ok_or("请求历史暂不可用")?;
        let version = store.data_version()?;
        if self.version != Some(version) {
            let read = store.read(None)?;
            let after = store.data_version()?;
            self.rows = read.rows;
            self.invalid = read.invalid_records;
            self.version = (version == after).then_some(after);
            self.identity = identity;
        }
        Ok(())
    }
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GatewayActivity {
    pub sampling: ActivitySampling,
    pub user_input: Option<UserInput>,
    pub tool_names: Vec<String>,
    pub request: ModelObservation,
    pub phase: String,
    pub text_tokens_per_second: Option<f64>,
    pub output_tokens_per_second: Option<f64>,
    pub average_text_tokens_per_second: Option<f64>,
    pub throughput: Option<f64>,
    pub elapsed_ms: f64,
    pub stale: bool,
}
impl Hub {
    pub fn release_native_snapshots(&self) {
        if let Ok(mut saved) = self.sql.lock()
            && let Some(history) = saved.as_mut()
        {
            history.release_native_snapshots();
        }
    }
    pub fn warm(self: &std::sync::Arc<Self>) {
        let hub = self.clone();
        let _ = std::thread::Builder::new()
            .name("observation-query-index".into())
            .spawn(move || {
                let _ = hub.query(|_| Ok(()));
            });
    }
    pub fn query_now<T>(
        &self,
        read: impl FnOnce(
            &mut local_agent_core::observation::sql::History,
            chrono::DateTime<chrono::Utc>,
        ) -> Result<T, String>,
    ) -> Result<T, String> {
        self.query(|history| read(history, chrono::Utc::now()))
    }
    pub fn cached_query<T>(
        &self,
        read: impl FnOnce(&mut local_agent_core::observation::sql::History) -> Result<T, String>,
    ) -> Result<T, String> {
        let (_permit, mut saved) = self.query_lock()?;
        self.check_query_scope(&mut saved)?;
        let result = read(saved.as_mut().ok_or("请求快照已过期，请刷新后重试")?);
        self.check_query_scope(&mut saved)?;
        result
    }
    pub fn query<T>(
        &self,
        read: impl FnOnce(&mut local_agent_core::observation::sql::History) -> Result<T, String>,
    ) -> Result<T, String> {
        let (_permit, mut saved) = self.query_lock()?;
        // Ordinary reads may rebuild; frozen reads above must only expire.
        let _ = self.check_query_scope(&mut saved);
        if saved.is_none() {
            *saved = Some(local_agent_core::observation::sql::History::open(
                &self.directory,
            )?);
        }
        let history = saved.as_mut().ok_or("历史查询索引暂不可用")?;
        let pending = self.writer.pending_for_epoch(history.content_epoch());
        let live = if pending.is_some() {
            self.rows()
        } else {
            Vec::new()
        };
        let result = history.refresh_pending(&live, &pending.unwrap_or_default());
        if let Ok(mut error) = self.error.lock() {
            *error = result
                .as_ref()
                .err()
                .cloned()
                .or_else(|| read_warning(history.invalid));
        }
        if !history.initialized() {
            result?;
        }
        let result = read(history);
        self.check_query_scope(&mut saved)?;
        result
    }
    fn check_query_scope(
        &self,
        saved: &mut Option<local_agent_core::observation::sql::History>,
    ) -> Result<(), String> {
        if saved.as_ref().is_some_and(|history| {
            !history.same_file(&self.directory)
                || !history.content_is_current().is_ok_and(|current| current)
        }) {
            // Dropping the connection also releases its TEMP payloads, frozen
            // positions and export leases. Never return a late old-store reply.
            *saved = None;
            return Err("历史存储或内容版本不可确认，旧快照已停止提供，请刷新后重试".into());
        }
        Ok(())
    }
    fn query_lock(&self) -> Result<QueryLock<'_>, String> {
        // FIFO admission prevents hot readers from repeatedly overtaking waiters.
        // Measured snapshot refreshes can contend for 5–6 seconds. Keep that
        // bounded wait separate from callers' eight-second end-to-end deadline.
        let permit = self.query_queue.enter(Duration::from_secs(7))?;
        let deadline = permit.deadline();
        loop {
            match self.sql.try_lock() {
                Ok(lock) => return Ok((permit, lock)),
                Err(std::sync::TryLockError::Poisoned(_)) => {
                    return Err("历史查询索引暂不可用".into());
                }
                Err(_) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(10))
                }
                Err(_) => return Err("历史查询正在更新，请稍后刷新".into()),
            }
        }
    }
    pub fn downstream(&self, id: &str, status: &str) {
        let row = if let Ok(mut rows) = self.rows.lock()
            && let Some(l) = rows.get_mut(id)
        {
            if l.row
                .downstream_status
                .as_deref()
                .is_some_and(|s| ["timeout", "cancelled", "body_read"].contains(&s))
            {
                return;
            }
            l.row.downstream_status = Some(status.into());
            l.row.revision += 1;
            Some(l.row.clone())
        } else {
            None
        };
        if let Some(row) = row {
            self.persist(row);
        }
    }
    pub fn flush(&self, timeout: Duration) -> Result<(), String> {
        self.writer.flush(timeout)
    }
    pub fn interrupt_active(&self) {
        let ids: Vec<_> = self
            .rows()
            .into_iter()
            .filter(|r| r.status == "in_progress")
            .map(|r| r.id)
            .collect();
        for id in ids {
            self.finish(
                &id,
                "interrupted",
                Some("服务停止等待超时，未确认上游完整结果"),
            );
        }
    }
    pub fn health(&self) -> super::persistence::Health {
        self.writer.health()
    }
    pub fn client_response_id(&self, id: &str, response: &str) {
        if let Ok(mut rows) = self.rows.lock()
            && let Some(l) = rows.get_mut(id)
        {
            l.row.client_response_id = Some(response.to_owned());
        }
    }
    pub fn sent(&self, id: &str) {
        if let Ok(mut rows) = self.rows.lock()
            && let Some(l) = rows.get_mut(id)
        {
            l.row.sent_ms = Some(l.start.elapsed().as_secs_f64() * 1000.0);
        }
    }
    pub fn upstream_id(&self, id: &str, value: Option<&str>) {
        if let Ok(mut rows) = self.rows.lock()
            && let Some(l) = rows.get_mut(id)
        {
            l.row.upstream_request_id = value.map(super::label);
        }
    }
    #[cfg(test)]
    pub fn history(&self) -> Result<Vec<ModelObservation>, String> {
        let mut history = self.history.lock().map_err(|_| "请求历史暂不可用")?;
        let result = history.read(&self.directory);
        if let Ok(mut error) = self.error.lock() {
            *error = result.err().or_else(|| read_warning(history.invalid));
        }
        let saved = history.rows.clone();
        drop(history);
        let mut rows: HashMap<_, _> = saved.into_iter().map(|r| (r.id.clone(), r)).collect();
        let live = self.rows();
        let live_ids: std::collections::HashSet<_> = live.iter().map(|r| r.id.clone()).collect();
        for row in rows
            .values_mut()
            .filter(|r| r.status == "in_progress" && !live_ids.contains(&r.id))
        {
            row.status = "interrupted".into();
            row.timing_issue = Some("当前进程没有该历史请求的活跃时序，结果未确认".into());
        }
        for row in live {
            if rows
                .get(&row.id)
                .is_none_or(|old| old.revision <= row.revision)
            {
                rows.insert(row.id.clone(), row);
            }
        }
        let mut rows: Vec<_> = rows.into_values().collect();
        rows.sort_by(|a, b| b.started_at.cmp(&a.started_at).then(b.id.cmp(&a.id)));
        Ok(rows)
    }
    #[cfg(test)]
    pub fn new(store: ObservationStore) -> Result<Self, String> {
        let directory = store.directory();
        Self::initialize(directory, Ok(store))
    }
    pub fn open(directory: &std::path::Path) -> Result<Self, String> {
        Self::initialize(directory.to_path_buf(), ObservationStore::open(directory))
    }
    fn initialize(
        directory: std::path::PathBuf,
        store: Result<ObservationStore, String>,
    ) -> Result<Self, String> {
        let inputs = InputCache::open(directory.join("gateway-activity-inputs.json"));
        let writer = super::persistence::Writer::new(
            directory.clone(),
            store.as_ref().ok().map(|store| store.write_epoch().clone()),
        )?;
        let message_key = super::message::key(&directory);
        let (history, initial_error) = match store.and_then(|store| {
            let mut report = store.read(Some(512))?;
            let ids: HashSet<_> = report.rows.iter().map(|r| r.id.clone()).collect();
            report.rows.extend(
                store
                    .latest_by_agent()?
                    .into_iter()
                    .filter(|r| !ids.contains(&r.id)),
            );
            Ok::<_, String>(report)
        }) {
            Ok(report) => (report.rows, read_warning(report.invalid_records)),
            Err(message) => (vec![], Some(message)),
        };
        let mut rows = HashMap::new();
        for mut row in history {
            if row.status == "in_progress" {
                row.status = "interrupted".into();
                row.timing_issue = Some("应用退出或采集器中断，未确认请求结果".into());
                row.revision += 1;
                writer.enqueue(row.clone());
            }
            let now = Instant::now();
            rows.insert(
                row.id.clone(),
                Live {
                    user_input: inputs
                        .get(&row.id)
                        .filter(|saved| {
                            saved.agent == row.agent_id && saved.session_id == row.session_id
                        })
                        .map(|saved| saved.input.clone()),
                    tool_names: Vec::new(),
                    row,
                    start: now,
                    last: now,
                    phase: "ended".into(),
                    window: VecDeque::new(),
                    window_since: None,
                    output_window: VecDeque::new(),
                    output_window_since: None,
                    checkpoint: now,
                },
            );
        }
        let message_error = message_key.as_ref().err().cloned();
        Ok(Self {
            input_recovery_error: inputs.error,
            message_key: message_key.ok(),
            rows: Mutex::new(rows),
            directory,
            writer,
            error: Mutex::new(initial_error.or(message_error)),
            #[cfg(test)]
            history: Mutex::new(History::default()),
            sql: Mutex::new(None),
            query_queue: Default::default(),
        })
    }
    fn persist(&self, row: ModelObservation) {
        self.writer.enqueue(row);
    }
    pub(super) fn message_identity(
        &self,
        body: &serde_json::Value,
        session: Option<&str>,
        product: Option<&str>,
    ) -> Option<String> {
        super::message::identity(body, session, product, self.message_key.as_ref()?.as_str())
    }
    pub fn start(&self, mut row: ModelObservation, clock: Instant, user_input: Option<UserInput>) {
        if let Some(input) = &user_input {
            self.writer.enqueue_input(CachedInput {
                key: row.id.clone(),
                agent: row.agent_id.clone(),
                session_id: row.session_id.clone(),
                started_at: row.activity_message.as_ref().map(|m| m.started_at.clone()),
                source_stamp: None,
                native: None,
                input: input.clone(),
            });
        }
        if let Ok(mut rows) = self.rows.lock() {
            if let Some(message) = row.activity_message.as_mut()
                && let Some(started) = rows
                    .values()
                    .filter(|old| old.row.session_id == row.session_id)
                    .filter_map(|old| old.row.activity_message.as_ref())
                    .filter(|old| old.id == message.id)
                    .map(|old| &old.started_at)
                    .min()
            {
                message.started_at = started.clone();
            }
            self.persist(row.clone());
            let mut latest: HashMap<String, (&str, &str)> = HashMap::new();
            for (id, live) in rows.iter() {
                let group = local_agent_core::agents::group(
                    &live.row.agent_id,
                    live.row.agent_product_id.as_deref(),
                );
                if latest
                    .get(&group)
                    .is_none_or(|(_, at)| *at < live.row.started_at.as_str())
                {
                    latest.insert(group, (id, &live.row.started_at));
                }
            }
            let protected: HashSet<_> = latest.values().map(|(id, _)| id.to_string()).collect();
            if rows.len() >= 512
                && let Some(oldest) = rows
                    .iter()
                    .filter(|(id, l)| l.row.status != "in_progress" && !protected.contains(*id))
                    .min_by_key(|(_, l)| &l.row.started_at)
                    .map(|(id, _)| id.clone())
            {
                rows.remove(&oldest);
            }
            let now = Instant::now();
            rows.insert(
                row.id.clone(),
                Live {
                    user_input,
                    tool_names: Vec::new(),
                    row,
                    start: clock,
                    last: now,
                    phase: "waiting_first".into(),
                    window: VecDeque::new(),
                    window_since: None,
                    output_window: VecDeque::new(),
                    output_window_since: None,
                    checkpoint: now,
                },
            );
        } else {
            self.persist(row);
        }
    }
    pub fn status(&self, id: &str, http: u16) {
        if let Ok(mut rows) = self.rows.lock()
            && let Some(l) = rows.get_mut(id)
        {
            l.row.http_status = Some(http);
        }
    }
    pub fn touch(&self, id: &str) {
        if let Ok(mut rows) = self.rows.lock()
            && let Some(l) = rows.get_mut(id)
        {
            l.last = Instant::now();
        }
    }
    pub fn update(&self, id: &str, u: Update) {
        self.update_at(id, u, Instant::now());
    }
    pub fn update_at(&self, id: &str, mut u: Update, now: Instant) {
        if let Ok(mut rows) = self.rows.lock()
            && let Some(l) = rows.get_mut(id)
        {
            let accounting = u.usage.is_some() || u.reported_cost.is_some();
            if l.row.status != "in_progress" {
                // A bounded tail read can recover reported accounting after client
                // cancellation/conversion failure, but cannot repair delivery or timing.
                u.generated = false;
                u.text = false;
                u.tool = false;
                u.tool_names.clear();
                u.issue = false;
                u.terminal = None;
                if accounting {
                    // The conservative billing interval includes recovered tail usage.
                    l.row.ended_at = Some(chrono::Utc::now().to_rfc3339());
                    l.row.duration_ms = Some(l.start.elapsed().as_secs_f64() * 1000.0);
                }
            }
            if !u.tool_names.is_empty() && !u.issue {
                l.tool_names = u.tool_names.iter().take(8).cloned().collect();
                l.phase = "tool_arguments".into();
                l.last = now;
            } else if u.generated && !u.tool {
                l.tool_names.clear();
            }
            if u.generated && !u.issue {
                if l.row.ttft_ms.is_none() && l.row.timing_issue.is_none() {
                    l.row.first_output_kind = Some(
                        if u.tool {
                            "tool_arguments"
                        } else if u.text {
                            "text"
                        } else {
                            "public_reasoning"
                        }
                        .into(),
                    );
                    l.row.ttft_ms =
                        Some(now.saturating_duration_since(l.start).as_secs_f64() * 1000.0);
                }
                l.last = now;
                l.phase = if u.tool {
                    "tool_arguments"
                } else if u.text {
                    "streaming"
                } else {
                    "public_reasoning"
                }
                .into();
                if u.estimated_output_tokens > 0.0 {
                    l.output_window_since.get_or_insert(now);
                    l.output_window.push_back((now, u.estimated_output_tokens));
                }
            }
            if u.text && !u.issue {
                let elapsed = now.saturating_duration_since(l.start).as_secs_f64() * 1000.0;
                l.row.last_text_ms = Some(elapsed);
                if l.row.timing_issue.is_none() {
                    l.row.first_text_ms.get_or_insert(elapsed);
                    *l.row.estimated_text_tokens.get_or_insert(0.0) += u.estimated_tokens;
                }
                // A later clean window is measurable even when a previous
                // event was lost. It cannot repair whole-request timings.
                l.window_since.get_or_insert(now);
                l.window.push_back((now, u.estimated_tokens));
            }
            while l
                .window
                .front()
                .is_some_and(|(t, _)| now.duration_since(*t) > Duration::from_secs(1))
            {
                l.window.pop_front();
            }
            while l.output_window.front().is_some_and(|(t, _)| {
                now.saturating_duration_since(*t)
                    >= Duration::from_millis(u64::from(RATE_WINDOW_MS))
            }) {
                l.output_window.pop_front();
            }
            if u.generated
                && !u.issue
                && u.estimated_output_tokens > 0.0
                && l.output_window_since.is_some_and(|since| {
                    l.row.timing_issue.is_none()
                        || now.saturating_duration_since(since)
                            >= Duration::from_millis(u64::from(RATE_WINDOW_MS))
                })
            {
                let rate = l
                    .output_window
                    .iter()
                    .map(|(_, tokens)| tokens)
                    .sum::<f64>()
                    / (f64::from(RATE_WINDOW_MS) / 1000.0);
                let at = chrono::DateTime::parse_from_rfc3339(&l.row.started_at)
                    .ok()
                    .and_then(|at| {
                        at.timestamp_millis().checked_add(
                            now.saturating_duration_since(l.start)
                                .as_millis()
                                .try_into()
                                .ok()?,
                        )
                    });
                if let Some(at) = at {
                    let sample = RateSample {
                        at,
                        tokens_per_second: rate,
                        window_ms: RATE_WINDOW_MS,
                    };
                    if sample.valid() {
                        l.row.last_rate_sample = Some(sample);
                    }
                }
            }
            if local_agent_core::provider_cost::openrouter_endpoint(&l.row.endpoint)
                && let Some(amount) = u.reported_cost
            {
                l.row.reported_cost = Some(local_agent_core::provider_cost::ReportedCost {
                    amount,
                    currency: "USD".into(),
                    source: "openrouter.usage.cost".into(),
                });
            }
            if let Some(response_id) = u.response_id {
                // Never overwrite a conflicting identity with another stream event.
                if l.row
                    .timing_issue
                    .as_deref()
                    .is_some_and(|s| s.contains("响应身份发生冲突"))
                {
                    l.row.response_id = None;
                    l.row.upstream_response_id = None;
                } else if l
                    .row
                    .response_id
                    .as_ref()
                    .is_none_or(|old| old == &response_id)
                {
                    l.row.upstream_response_id = Some(response_id.clone());
                    if l.row
                        .client_protocol
                        .as_deref()
                        .is_none_or(|p| p == l.row.protocol)
                    {
                        l.row.client_response_id = Some(response_id.clone());
                    }
                    l.row.response_id = Some(response_id);
                } else {
                    l.row.response_id = None;
                    l.row.upstream_response_id = None;
                    l.row.client_response_id = None;
                    l.row.timing_issue = Some("响应身份发生冲突，跨源关联不可用".into());
                }
            }
            if let Some(reason) = u.stop_reason {
                l.row.stop_reason = Some(super::label(&reason));
            }
            if let Some(model) = u.model {
                l.row.reported_model = Some(super::label(&model));
                l.row.model = Some(super::label(&model));
            }
            if let Some(usage) = u.usage {
                let a = &mut l.row.usage;
                a.input_tokens = usage.input_tokens.or(a.input_tokens);
                a.output_tokens = usage.output_tokens.or(a.output_tokens);
                a.cached_input_tokens = usage.cached_input_tokens.or(a.cached_input_tokens);
                a.cache_write_input_tokens = usage
                    .cache_write_input_tokens
                    .or(a.cache_write_input_tokens);
                a.reasoning_output_tokens =
                    usage.reasoning_output_tokens.or(a.reasoning_output_tokens);
                a.total_tokens = a
                    .input_tokens
                    .zip(a.output_tokens)
                    .map(|(a, b)| a.saturating_add(b));
            }
            if u.issue {
                l.row.timing_issue = Some("部分流事件无法解析；未补算未知时序或用量".into());
                // A later damaged event cannot invalidate an earlier measured
                // first output. If the first event was lost, TTFT stays unknown.
                l.row.estimated_text_tokens = None;
                l.window.clear();
                l.window_since = None;
                l.output_window.clear();
                l.output_window_since = None;
            }
            if let Some(status) = u.terminal {
                l.phase = status.into();
            }
            // Reported usage/identity survives a crash without persisting bodies.
            // Every snapshot advances the revision so delayed writes cannot regress it.
            if now.saturating_duration_since(l.checkpoint) >= Duration::from_secs(1)
                || u.terminal.is_some()
                || (l.row.status != "in_progress" && accounting)
            {
                l.row.revision += 1;
                l.checkpoint = now;
                self.persist(l.row.clone());
            }
        }
    }
    pub fn finish(&self, id: &str, status: &str, issue: Option<&str>) {
        let row = if let Ok(mut rows) = self.rows.lock()
            && let Some(l) = rows.get_mut(id)
        {
            if l.row.status != "in_progress" {
                return;
            }
            l.row.status = status.into();
            l.row.ended_at = Some(chrono::Utc::now().to_rfc3339());
            l.row.duration_ms = Some(l.start.elapsed().as_secs_f64() * 1000.0);
            l.row.revision += 1;
            if let Some(issue) = issue {
                l.row.timing_issue = Some(issue.into());
            }
            l.phase = "ended".into();
            l.window.clear();
            Some(l.row.clone())
        } else {
            None
        };
        if let Some(row) = row {
            self.persist(row);
        }
    }
    pub fn rows(&self) -> Vec<ModelObservation> {
        let mut rows: Vec<_> = self
            .rows
            .lock()
            .map(|r| r.values().map(|l| l.row.clone()).collect())
            .unwrap_or_default();
        rows.sort_by(|a, b| b.started_at.cmp(&a.started_at));
        rows
    }
    pub fn activities(&self) -> Vec<GatewayActivity> {
        self.activities_at(Instant::now())
    }
    pub(crate) fn try_activities(&self) -> Result<Vec<GatewayActivity>, String> {
        if self.rows.is_poisoned() {
            return Err("网关活动快照不可用".into());
        }
        let rows = self.activities();
        if self.rows.is_poisoned() {
            return Err("网关活动快照不可用".into());
        }
        Ok(rows)
    }
    fn activities_at(&self, now: Instant) -> Vec<GatewayActivity> {
        let mut rows: Vec<_> = self
            .rows
            .lock()
            .map(|r| {
                let mut entries: Vec<_> = r.values().collect();
                entries.sort_by(|a, b| b.row.started_at.cmp(&a.row.started_at));
                let mut latest = HashSet::new();
                entries
                    .into_iter()
                    .filter(|l| {
                        if l.row.status == "in_progress"
                            || latest.insert(local_agent_core::agents::group(
                                &l.row.agent_id,
                                l.row.agent_product_id.as_deref(),
                            ))
                        {
                            return true;
                        }
                        l.row
                            .ended_at
                            .as_ref()
                            .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
                            .is_some_and(|t| {
                                chrono::Utc::now().signed_duration_since(t).num_seconds()
                                    <= 3 * 60 * 60
                            })
                    })
                    .map(|l| {
                        let active = l.row.status == "in_progress";
                        let speed = (active
                            && l.phase == "streaming"
                            && l.window_since.is_some_and(|since| {
                                l.row.timing_issue.is_none()
                                    || now.saturating_duration_since(since)
                                        >= Duration::from_secs(1)
                            }))
                        .then(|| {
                            l.window
                                .iter()
                                .filter(|(t, _)| now.duration_since(*t) <= Duration::from_secs(1))
                                .map(|(_, v)| v)
                                .sum()
                        });
                        GatewayActivity {
                            sampling: ActivitySampling {
                                at: chrono::DateTime::parse_from_rfc3339(&l.row.started_at)
                                    .ok()
                                    .and_then(|at| {
                                        at.timestamp_millis().checked_add(
                                            now.saturating_duration_since(l.start)
                                                .as_millis()
                                                .try_into()
                                                .ok()?,
                                        )
                                    })
                                    .unwrap_or(0),
                                window_ms: RATE_WINDOW_MS,
                                tokens_per_second: (active
                                    && ["streaming", "public_reasoning", "tool_arguments"]
                                        .contains(&l.phase.as_str())
                                    && l.output_window_since.is_some_and(|since| {
                                        l.row.timing_issue.is_none()
                                            || now.saturating_duration_since(since)
                                                >= Duration::from_millis(u64::from(RATE_WINDOW_MS))
                                    }))
                                .then(|| {
                                    l.output_window
                                        .iter()
                                        .filter(|(at, _)| {
                                            now.saturating_duration_since(*at)
                                                < Duration::from_millis(u64::from(RATE_WINDOW_MS))
                                        })
                                        .map(|(_, tokens)| tokens)
                                        .sum::<f64>()
                                        / (f64::from(RATE_WINDOW_MS) / 1000.0)
                                })
                                .filter(|value| value.is_finite() && *value > 0.0),
                                recent: l.row.last_rate_sample.clone().filter(RateSample::valid),
                                reported: None,
                            },
                            user_input: l.user_input.clone(),
                            tool_names: l.tool_names.clone(),
                            request: l.row.clone(),
                            phase: l.phase.clone(),
                            text_tokens_per_second: speed,
                            output_tokens_per_second: (active
                                && ["streaming", "public_reasoning", "tool_arguments"]
                                    .contains(&l.phase.as_str())
                                && l.output_window_since.is_some_and(|since| {
                                    l.row.timing_issue.is_none()
                                        || now.saturating_duration_since(since)
                                            >= Duration::from_secs(1)
                                }))
                            .then(|| {
                                l.output_window
                                    .iter()
                                    .filter(|(t, _)| {
                                        now.saturating_duration_since(*t) <= Duration::from_secs(1)
                                    })
                                    .map(|(_, v)| v)
                                    .sum()
                            }),
                            average_text_tokens_per_second: l.row.average_text_speed(),
                            throughput: l.row.throughput(),
                            elapsed_ms: l
                                .row
                                .duration_ms
                                .unwrap_or_else(|| l.start.elapsed().as_secs_f64() * 1000.0),
                            stale: active && now.duration_since(l.last) > Duration::from_secs(30),
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
        rows.sort_by(|a, b| b.request.started_at.cmp(&a.request.started_at));
        rows
    }
    pub fn error(&self) -> Option<String> {
        let health = self.writer.health();
        self.error
            .lock()
            .ok()
            .and_then(|e| e.clone())
            .or(health.error)
            .or_else(|| {
                health
                    .last_success_at
                    .is_none()
                    .then(|| self.input_recovery_error.clone())
                    .flatten()
            })
    }
    pub fn active_count(&self, agent: &str) -> usize {
        self.rows
            .lock()
            .map(|r| {
                r.values()
                    .filter(|l| {
                        l.row.client.is_none()
                            && l.row.agent_id == agent
                            && l.row.status == "in_progress"
                    })
                    .count()
            })
            .unwrap_or(0)
    }
}
