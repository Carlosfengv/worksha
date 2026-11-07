use super::*;
use crate::plugin::{PluginDescriptor, PluginError, PluginResources};
use crate::sessions::{CallQuery, SessionQuery, opencode, source_plugins::test_fixture::Fixture};
use std::sync::{Mutex, mpsc};
use std::time::Duration;

#[derive(Clone, Copy, PartialEq)]
enum Phase {
    Scan,
    Read,
    Memory,
    Config,
    Progress,
}
struct PausedSource {
    inner: Arc<dyn ReadOnlySessionSource>,
    phase: Phase,
    entered: Mutex<Option<mpsc::Sender<()>>>,
    resume: Mutex<mpsc::Receiver<()>>,
}
impl PausedSource {
    fn pause(&self, phase: Phase) {
        if self.phase == phase
            && let Some(entered) = self.entered.lock().unwrap().take()
        {
            entered.send(()).unwrap();
            self.resume
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(10))
                .unwrap();
        }
    }
}
impl BuiltinPlugin for PausedSource {
    fn descriptor(&self) -> PluginDescriptor {
        self.inner.descriptor()
    }
    fn prepare(&self) -> Result<PluginResources, PluginError> {
        self.inner.prepare()
    }
}
impl ReadOnlySessionSource for PausedSource {
    fn agent(&self) -> &'static str {
        self.inner.agent()
    }
    fn scan(
        &self,
        context: &SourceContext<'_>,
        previous: &HashMap<PathBuf, IndexedFile>,
    ) -> NativeResult<SourceScan> {
        let result = self.inner.scan(context, previous)?;
        self.pause(Phase::Scan);
        if self.phase == Phase::Progress {
            self.pause(Phase::Progress);
            // Simulate a context check completing just before replacement.
            (context.progress)(99, 99)?;
        }
        Ok(result)
    }
    fn conversation(
        &self,
        context: &SourceContext<'_>,
        indexed: &IndexedFile,
        cursor: Option<&str>,
        anchor: Option<&str>,
    ) -> NativeResult<Conversation> {
        let result = self.inner.conversation(context, indexed, cursor, anchor)?;
        self.pause(Phase::Read);
        Ok(result)
    }
    fn memory(
        &self,
        context: &SourceContext<'_>,
        agent: &str,
        selected: Option<&str>,
        projects: &std::collections::BTreeSet<String>,
    ) -> NativeResult<super::super::MemorySnapshot> {
        let result = self.inner.memory(context, agent, selected, projects)?;
        self.pause(Phase::Memory);
        Ok(result)
    }
    fn mcp_config(
        &self,
        context: &SourceContext<'_>,
        phase: super::super::mcp::ConfigPhase,
        scope: &mut super::super::mcp::ConfigScope<'_>,
        inventory: &mut super::super::mcp::Inventory,
    ) -> NativeResult<()> {
        self.inner.mcp_config(context, phase, scope, inventory)?;
        self.pause(Phase::Config);
        Ok(())
    }
}
fn paused(
    fixture: &Fixture,
    phase: Phase,
) -> (Arc<NativeSessions>, mpsc::Receiver<()>, mpsc::Sender<()>) {
    paused_with(fixture, phase, Arc::new(opencode::OpenCodeSource))
}
fn paused_with(
    fixture: &Fixture,
    phase: Phase,
    inner: Arc<dyn ReadOnlySessionSource>,
) -> (Arc<NativeSessions>, mpsc::Receiver<()>, mpsc::Sender<()>) {
    let agent = inner.agent();
    let (entered, ready) = mpsc::channel();
    let (resume, receive) = mpsc::channel();
    let source = Arc::new(PausedSource {
        inner,
        phase,
        entered: Mutex::new(Some(entered)),
        resume: Mutex::new(receive),
    });
    let plugins = NativeSourcePlugins::mount(vec![source]).unwrap();
    let sessions =
        NativeSessions::with_source_plugins(fixture.home.clone(), &[agent.into()], plugins);
    (Arc::new(sessions), ready, resume)
}
fn count(sessions: &NativeSessions) -> usize {
    sessions
        .query(SessionQuery {
            agent: Some("opencode".into()),
            ..Default::default()
        })
        .unwrap()
        .total
}

#[test]
fn a_late_scan_cannot_change_progress_or_cache_after_same_source_reactivation() {
    let fixture = Fixture::new();
    let (sessions, entered, resume) = paused(&fixture, Phase::Progress);
    let worker = std::thread::spawn({
        let sessions = sessions.clone();
        move || sessions.refresh()
    });
    entered.recv_timeout(Duration::from_secs(10)).unwrap();
    let host = sessions.builtin_host();
    let factories: Vec<&dyn BuiltinPlugin> = sessions
        .source_plugins
        .sources
        .values()
        .map(|source| source.as_ref() as &dyn BuiltinPlugin)
        .collect();
    let enabled = factories
        .iter()
        .map(|factory| factory.descriptor().id.to_owned())
        .collect();
    host.reconfigure(
        &host.snapshot().unwrap().generation,
        &factories,
        &enabled,
        || Ok(()),
    )
    .unwrap();
    {
        let mut state = sessions.lock().unwrap();
        let source = state
            .sources
            .iter_mut()
            .find(|s| s.agent_id == "opencode")
            .unwrap();
        source.status = "ready".into();
        source.scanned_files = 7;
    }
    resume.send(()).unwrap();
    worker.join().unwrap().unwrap();
    let source = sessions
        .sources()
        .unwrap()
        .into_iter()
        .find(|s| s.agent_id == "opencode")
        .unwrap();
    assert_eq!(source.status, "ready");
    assert_eq!(source.scanned_files, 7);
    assert_eq!(count(&sessions), 0);
    sessions.refresh().unwrap();
    assert_eq!(count(&sessions), 1);
}

fn seed_claude(fixture: &Fixture) {
    let root = fixture.home.join(".claude/projects/fixture");
    std::fs::create_dir_all(&root).unwrap();
    let time = chrono::Utc::now().to_rfc3339();
    let user = serde_json::json!({"type":"user","sessionId":"registered-source","timestamp":time,"cwd":fixture.home,"message":{"content":"fixture"}});
    let reply = serde_json::json!({"type":"assistant","sessionId":"registered-source","uuid":"fixture-response","entrypoint":"cli","timestamp":time,"message":{"id":"fixture-response","model":"fixture-model","content":[{"type":"text","text":"fixture reply"}],"usage":{"input_tokens":10,"output_tokens":2}}});
    std::fs::write(root.join("fixture.jsonl"), format!("{user}\n{reply}\n")).unwrap();
    std::fs::write(fixture.home.join(".claude/CLAUDE.md"), "fixture memory").unwrap();
    std::fs::write(
        fixture.home.join(".claude.json"),
        r#"{"mcpServers":{"fixture":{"command":"never-executed-fixture-command"}}}"#,
    )
    .unwrap();
}

#[test]
fn collector_cancel_discards_jsonl_scan_and_manual_refresh_still_works() {
    let fixture = Fixture::new();
    seed_claude(&fixture);
    let (sessions, entered, resume) = paused_with(
        &fixture,
        Phase::Scan,
        Arc::new(super::super::jsonl::JsonlSource("claude")),
    );
    let active = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let worker = std::thread::spawn({
        let sessions = sessions.clone();
        let active = active.clone();
        move || sessions.refresh_guarded(&|| active.load(std::sync::atomic::Ordering::Acquire))
    });
    entered.recv_timeout(Duration::from_secs(10)).unwrap();
    active.store(false, std::sync::atomic::Ordering::Release);
    resume.send(()).unwrap();
    worker.join().unwrap().unwrap();
    assert_eq!(sessions.query(SessionQuery::default()).unwrap().total, 0);
    assert!(sessions.source_active("claude"));
    assert!(
        sessions
            .sources()
            .unwrap()
            .iter()
            .find(|s| s.agent_id == "claude")
            .unwrap()
            .enabled
    );
    sessions.refresh().unwrap();
    assert_eq!(sessions.query(SessionQuery::default()).unwrap().total, 1);
}

#[test]
fn memory_and_configuration_results_cannot_escape_revocation_or_plugin_stop() {
    for phase in [Phase::Memory, Phase::Config] {
        let fixture = Fixture::new();
        seed_claude(&fixture);
        let (sessions, entered, resume) = paused_with(
            &fixture,
            phase,
            Arc::new(super::super::jsonl::JsonlSource("claude")),
        );
        // Seed only the history index. Full refresh now observes configuration too;
        // the pause belongs to the explicit reader started below, not fixture setup.
        sessions.refresh_source_plugins(&|| true).unwrap();
        let worker = std::thread::spawn({
            let sessions = sessions.clone();
            move || {
                if phase == Phase::Memory {
                    sessions
                        .memory("claude-code-cli", None, &Default::default())
                        .map(|_| ())
                } else {
                    sessions.mcp(Default::default()).map(|_| ())
                }
            }
        });
        entered.recv_timeout(Duration::from_secs(10)).unwrap();
        if phase == Phase::Memory {
            sessions.set_enabled("claude", false).unwrap();
            sessions.set_enabled("claude", true).unwrap();
        } else {
            sessions.stop_source_plugin("claude").unwrap();
        }
        resume.send(()).unwrap();
        assert!(worker.join().unwrap().is_err());
    }
}

#[test]
fn configuration_publication_does_not_hold_the_history_lock_or_escape_source_stop() {
    let fixture = Fixture::new();
    seed_claude(&fixture);
    let (sessions, entered, resume) = paused_with(
        &fixture,
        Phase::Config,
        Arc::new(super::super::jsonl::JsonlSource("claude")),
    );
    let worker = std::thread::spawn({
        let sessions = sessions.clone();
        move || sessions.refresh()
    });
    entered.recv_timeout(Duration::from_secs(10)).unwrap();
    // Stopping and state reads finish while configuration is deliberately paused.
    sessions.stop_source_plugin("claude").unwrap();
    assert_eq!(sessions.query(SessionQuery::default()).unwrap().total, 0);
    assert!(
        sessions
            .sources()
            .unwrap()
            .iter()
            .any(|s| s.agent_id == "claude" && s.enabled)
    );
    resume.send(()).unwrap();
    worker.join().unwrap().unwrap();
    let revision: serde_json::Value =
        serde_json::from_str(&sessions.domain_fact_catalog_revision().unwrap()).unwrap();
    assert_eq!(revision[0], true);
    assert!(revision[1].is_null());
    assert!(
        !sessions
            .mcp(Default::default())
            .unwrap()
            .services
            .iter()
            .any(|s| s.agent_id == "claude")
    );
}

#[test]
fn stopping_one_source_stops_all_its_readers_and_keeps_other_sources_and_consent() {
    let fixture = Fixture::new();
    seed_claude(&fixture);
    let sessions = NativeSessions::new(fixture.home.clone(), &["claude".into(), "opencode".into()]);
    sessions.refresh().unwrap();
    let id = sessions
        .query(SessionQuery {
            agent: Some("claude-code-cli".into()),
            ..Default::default()
        })
        .unwrap()
        .items[0]
        .id
        .clone();
    assert!(
        !sessions
            .memory("claude-code-cli", None, &Default::default())
            .unwrap()
            .files
            .is_empty()
    );
    assert!(
        sessions
            .mcp(Default::default())
            .unwrap()
            .services
            .iter()
            .any(|s| s.name == "fixture")
    );
    let mut watch = super::super::live::NativeWatch::default();
    let _ = watch.poll(&sessions);
    sessions.stop_source_plugin("claude").unwrap();
    assert!(sessions.conversation(&id, None, None).is_err());
    assert_eq!(
        sessions
            .memory("claude-code-cli", None, &Default::default())
            .err()
            .unwrap()
            .code,
        "plugin_disabled"
    );
    assert!(
        !sessions
            .mcp(Default::default())
            .unwrap()
            .services
            .iter()
            .any(|s| s.agent_id == "claude")
    );
    assert!(!watch.poll(&sessions).iter().any(|a| a.agent_id == "claude"));
    assert_eq!(
        watch.context_samples(&sessions, [id]).unwrap()[0].state,
        "unavailable"
    );
    assert_eq!(count(&sessions), 1);
    let source = sessions
        .sources()
        .unwrap()
        .into_iter()
        .find(|s| s.agent_id == "claude")
        .unwrap();
    assert!(source.enabled);
    assert_eq!(source.status, "partial");
    sessions.refresh().unwrap();
    assert_eq!(count(&sessions), 1);
}

#[test]
fn preparing_a_source_does_not_discover_or_read_unconsented_files() {
    let fixture = Fixture::new();
    let sessions = NativeSessions::new(fixture.home.clone(), &[]);
    // Even an unreadable known source must not be touched without consent.
    std::fs::remove_file(super::super::opencode::path(&fixture.home)).unwrap();
    sessions.refresh().unwrap();
    let source = sessions
        .sources()
        .unwrap()
        .into_iter()
        .find(|s| s.agent_id == "opencode")
        .unwrap();
    assert!(!source.enabled);
    assert_eq!(source.status, "disabled");
    assert!(source.last_scan.is_none());
    assert!(source.error.is_none());
}

#[test]
fn completed_scan_cannot_publish_after_revocation_or_reauthorization_or_stop() {
    for action in ["revoke", "reauthorize", "stop"] {
        let fixture = Fixture::new();
        let (sessions, entered, resume) = paused(&fixture, Phase::Scan);
        let worker = {
            let sessions = sessions.clone();
            std::thread::spawn(move || sessions.refresh())
        };
        entered.recv_timeout(Duration::from_secs(10)).unwrap();
        if action == "stop" {
            sessions.stop_source_plugins().unwrap();
        } else {
            sessions.set_enabled("opencode", false).unwrap();
            if action == "reauthorize" {
                sessions.set_enabled("opencode", true).unwrap();
            }
        }
        resume.send(()).unwrap();
        worker.join().unwrap().unwrap();
        assert_eq!(count(&sessions), 0, "{action}");
        assert_eq!(sessions.calls(CallQuery::default()).unwrap().total, 0);
        assert!(sessions.analysis_rows().unwrap().is_empty());
        if action == "reauthorize" {
            sessions.refresh().unwrap();
            assert_eq!(count(&sessions), 1);
        } else if action == "stop" {
            sessions.refresh().unwrap();
            assert_eq!(count(&sessions), 0);
            let source = sessions
                .sources()
                .unwrap()
                .into_iter()
                .find(|s| s.agent_id == "opencode")
                .unwrap();
            assert!(
                source.enabled,
                "stopping implementation must not change consent"
            );
            assert_eq!(source.status, "partial");
            assert!(source.error.unwrap().contains("插件已停止"));
        }
    }
}

#[test]
fn loaded_conversation_is_discarded_after_reauthorization_or_stop() {
    for stop in [false, true] {
        let fixture = Fixture::new();
        let (sessions, entered, resume) = paused(&fixture, Phase::Read);
        sessions.refresh().unwrap();
        let id = sessions.query(SessionQuery::default()).unwrap().items[0]
            .id
            .clone();
        let worker = {
            let sessions = sessions.clone();
            std::thread::spawn(move || sessions.conversation(&id, None, None))
        };
        entered.recv_timeout(Duration::from_secs(10)).unwrap();
        if stop {
            sessions.stop_source_plugins().unwrap();
        } else {
            sessions.set_enabled("opencode", false).unwrap();
            sessions.set_enabled("opencode", true).unwrap();
        }
        resume.send(()).unwrap();
        let error = match worker.join().unwrap() {
            Ok(_) => panic!("revoked conversation was published"),
            Err(error) => error,
        };
        assert_eq!(
            error.code,
            if stop {
                "plugin_disabled"
            } else {
                "unauthorized"
            }
        );
        assert_eq!(count(&sessions), 0);
    }
}

#[test]
fn stopping_the_pilot_keeps_other_sources_and_neutral_usage_serialization() {
    let fixture = Fixture::new();
    let claude = fixture.home.join(".claude/projects/fixture");
    std::fs::create_dir_all(&claude).unwrap();
    std::fs::write(claude.join("fixture.jsonl"), "{\"type\":\"user\",\"sessionId\":\"claude-fixture\",\"timestamp\":\"2026-10-22T00:00:00Z\",\"message\":{\"role\":\"user\",\"content\":\"fixture\"}}\n").unwrap();
    let sessions = NativeSessions::new(fixture.home.clone(), &["claude".into(), "opencode".into()]);
    sessions.refresh().unwrap();
    assert_eq!(count(&sessions), 1);
    sessions.stop_source_plugin("opencode").unwrap();
    assert_eq!(count(&sessions), 0);
    let stopped_revision = sessions.revision().unwrap();
    sessions.stop_source_plugin("opencode").unwrap();
    assert_eq!(sessions.revision().unwrap(), stopped_revision);
    assert_eq!(
        sessions
            .query(SessionQuery {
                agent: Some("claude".into()),
                ..Default::default()
            })
            .unwrap()
            .total,
        1
    );
    let usage = crate::usage::Usage {
        input_tokens: Some(u64::MAX),
        output_tokens: Some(0),
        ..Default::default()
    };
    let old_path: crate::sessions::Usage = usage;
    let expected = serde_json::json!({ "inputTokens": u64::MAX, "outputTokens": 0,
        "cachedInputTokens": null, "cacheWriteInputTokens": null,
        "reasoningOutputTokens": null, "totalTokens": null });
    assert_eq!(serde_json::to_value(old_path).unwrap(), expected);
    let decoded: crate::usage::Usage = serde_json::from_value(expected).unwrap();
    assert_eq!(decoded.input_tokens, Some(u64::MAX));
    assert_eq!(decoded.output_tokens, Some(0));
    assert_eq!(decoded.total_tokens, None);
}
