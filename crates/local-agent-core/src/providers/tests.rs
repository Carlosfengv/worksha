use super::*;
use std::os::unix::fs::{PermissionsExt, symlink};

struct RedirectedCodex;
impl crate::plugin::BuiltinPlugin for RedirectedCodex {
    fn descriptor(&self) -> crate::plugin::PluginDescriptor {
        crate::plugin::PluginDescriptor {
            id: "fixture.redirected-codex",
            version: "1.0.0",
            provides: vec![crate::plugin::CapabilityKey {
                name: "agent.config-adapter",
                version: 1,
                instance: "codex",
            }],
            requires: vec![],
        }
    }
    fn prepare(&self) -> Result<crate::plugin::PluginResources, crate::plugin::PluginError> {
        Ok(crate::plugin::PluginResources::default())
    }
}
impl config::NativeConfigAdapter for RedirectedCodex {
    fn id(&self) -> &'static str {
        "codex"
    }
    fn protocol(&self) -> Option<&'static str> {
        Some("responses")
    }
    fn model_id(&self, _: &str, model: &str) -> String {
        model.into()
    }
    fn supports_model_catalog(&self) -> bool {
        true
    }
    fn requires_inline_credential(&self) -> bool {
        false
    }
    fn path(&self, home: &Path) -> Result<PathBuf, ConfigError> {
        Ok(home.join(".codex/different-config.toml"))
    }
    fn inspect(&self, home: &Path) -> config::NativeConfig {
        config::inspect(home, "codex")
    }
    fn render(
        &self,
        before: Option<&[u8]>,
        provider: Option<&Provider>,
        helper: &Path,
    ) -> Result<Vec<u8>, ConfigError> {
        config::render("codex", before, provider, helper)
    }
    fn models(
        &self,
        bytes: &mut Vec<u8>,
        provider: &Provider,
        _: &[String],
        _: Option<&str>,
    ) -> Result<(), ConfigError> {
        config::codex_models(bytes, provider)
    }
}

struct Fixture {
    _temp: tempfile::TempDir,
    home: PathBuf,
    data: PathBuf,
    vault: Arc<vault::MemoryVault>,
    manager: ProviderManager,
}
fn fixture() -> Fixture {
    let temp = tempfile::tempdir().unwrap();
    let base = temp.path().canonicalize().unwrap();
    let home = base.join("home");
    let data = base.join("data");
    fs::create_dir_all(&home).unwrap();
    let helper = base.join("test-helper");
    fs::write(&helper, b"isolated helper fixture").unwrap();
    let vault = Arc::new(vault::MemoryVault::default());
    let manager = ProviderManager::new(home.clone(), &data, helper, vault.clone()).unwrap();
    Fixture {
        _temp: temp,
        home,
        data,
        vault,
        manager,
    }
}
fn config(f: &Fixture, agent: &str, bytes: &[u8]) -> PathBuf {
    let path = config::path(&f.home, agent).unwrap();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, bytes).unwrap();
    path
}
fn p(f: &Fixture, protocol: &str) -> Provider {
    let credential = f
        .manager
        .credential(
            "isolated-test-key",
            Zeroizing::new("provider-secret-sentinel".into()),
        )
        .unwrap();
    serde_json::from_value(serde_json::json!({"id":"openai","name":"Test provider","endpoint":"https://example.invalid/v1","protocol":protocol,"model":"fixture-model","configured":true,"credentialRef":credential.credential_ref,"revision":1})).unwrap()
}
fn prepare(f: &Fixture, agent: &str, p: Option<&Provider>) -> Transaction {
    let current = config::inspect(&f.home, agent);
    assert_ne!(current.state, "unavailable", "{:?}", current.error);
    f.manager
        .prepare(
            BindingDraft {
                operation: None,
                agent_id: agent.into(),
                expected_revision: current.revision.unwrap(),
                provider_id: p.map(|p| p.id.clone()),
                expected_provider_revision: p.map(|p| p.revision),
                model: None,
            },
            p,
        )
        .unwrap()
}
fn tree_bytes(path: &Path, all: &mut Vec<Vec<u8>>) {
    for item in fs::read_dir(path).unwrap().flatten() {
        if item.path().is_dir() {
            tree_bytes(&item.path(), all);
        } else {
            all.push(fs::read(item.path()).unwrap());
        }
    }
}
#[test]
fn stopping_a_native_planner_in_flight_cannot_publish_a_late_preview() {
    use std::sync::{Mutex, mpsc};
    use std::time::Duration;
    struct BlockingConfig {
        entered: mpsc::Sender<()>,
        release: Mutex<mpsc::Receiver<()>>,
    }
    impl crate::plugin::BuiltinPlugin for BlockingConfig {
        fn descriptor(&self) -> crate::plugin::PluginDescriptor {
            crate::plugin::PluginDescriptor {
                id: "fixture.blocking-config",
                version: "1.0.0",
                provides: vec![crate::plugin::CapabilityKey {
                    name: "agent.config-adapter",
                    version: 1,
                    instance: "codex",
                }],
                requires: vec![],
            }
        }
        fn prepare(&self) -> Result<crate::plugin::PluginResources, crate::plugin::PluginError> {
            Ok(Default::default())
        }
    }
    impl config::NativeConfigAdapter for BlockingConfig {
        fn id(&self) -> &'static str {
            "codex"
        }
        fn protocol(&self) -> Option<&'static str> {
            Some("responses")
        }
        fn model_id(&self, _: &str, model: &str) -> String {
            model.into()
        }
        fn supports_model_catalog(&self) -> bool {
            true
        }
        fn requires_inline_credential(&self) -> bool {
            false
        }
        fn path(&self, home: &Path) -> Result<PathBuf, ConfigError> {
            config::path(home, "codex")
        }
        fn inspect(&self, home: &Path) -> config::NativeConfig {
            config::inspect(home, "codex")
        }
        fn render(
            &self,
            before: Option<&[u8]>,
            provider: Option<&Provider>,
            helper: &Path,
        ) -> Result<Vec<u8>, ConfigError> {
            self.entered.send(()).unwrap();
            self.release
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(5))
                .map_err(|_| ConfigError::new("unavailable", "owned fixture release timed out"))?;
            config::render("codex", before, provider, helper)
        }
        fn models(
            &self,
            bytes: &mut Vec<u8>,
            provider: &Provider,
            _: &[String],
            _: Option<&str>,
        ) -> Result<(), ConfigError> {
            config::codex_models(bytes, provider)
        }
    }
    let (entered, observed) = mpsc::channel();
    let (resume, release) = mpsc::channel();
    let adapter = Box::leak(Box::new(BlockingConfig {
        entered,
        release: Mutex::new(release),
    }));
    let mut f = fixture();
    f.manager.adapters =
        Arc::new(config::ConfigAdapters::from_contributions(vec![adapter]).unwrap());
    let original = b"# owned\nmodel='before'\n";
    let path = config(&f, "codex", original);
    let provider = p(&f, "responses");
    std::thread::scope(|scope| {
        let pending = scope.spawn(|| {
            f.manager.prepare(
                BindingDraft {
                    operation: None,
                    agent_id: "codex".into(),
                    expected_revision: files::hash(original),
                    provider_id: Some(provider.id.clone()),
                    expected_provider_revision: Some(provider.revision),
                    model: None,
                },
                Some(&provider),
            )
        });
        observed.recv_timeout(Duration::from_secs(5)).unwrap();
        f.manager.adapters.stop("codex").unwrap();
        resume.send(()).unwrap();
        assert_eq!(
            pending.join().unwrap().err().unwrap().code,
            "plugin_disabled"
        );
    });
    assert_eq!(fs::read(path).unwrap(), original);
    assert!(f.manager.journal().unwrap().transactions.is_empty());
}

#[test]
fn disabled_format_retains_pending_bytes_and_restart_applies_once() {
    let f = fixture();
    let original = b"# keep preferences\nmodel='before'\n";
    let path = config(&f, "codex", original);
    let provider = p(&f, "responses");
    let transaction = prepare(&f, "codex", Some(&provider));
    let planned = f.manager.plan(&transaction.id).unwrap().after.unwrap();
    f.manager.adapters.stop("codex").unwrap();
    assert!(
        f.manager
            .discover_imports(&[])
            .iter()
            .filter(|candidate| candidate.agent_id == "codex")
            .all(|candidate| !candidate.can_import)
    );
    let blocked = f
        .manager
        .apply_with_provider(&transaction.id, || Ok(Some(provider.clone())), true)
        .err()
        .unwrap();
    assert_eq!(blocked.code, "plugin_disabled");
    assert_eq!(fs::read(&path).unwrap(), original);
    assert_eq!(
        f.manager
            .journal()
            .unwrap()
            .transactions
            .last()
            .unwrap()
            .state,
        "pending"
    );
    let reopened = ProviderManager::new(
        f.home.clone(),
        &f.data,
        f.manager.helper_source.clone(),
        f.vault.clone(),
    )
    .unwrap();
    assert_eq!(
        reopened
            .apply_with_provider(&transaction.id, || Ok(Some(provider.clone())), true)
            .unwrap()
            .state,
        "applied"
    );
    assert_eq!(fs::read(&path).unwrap(), planned);
    assert_eq!(
        reopened
            .apply_with_provider(&transaction.id, || panic!("must not execute again"), true)
            .unwrap()
            .state,
        "applied"
    );
    assert_eq!(fs::read(&path).unwrap(), planned);
    let restore = reopened.prepare_restore(&transaction.id).unwrap();
    reopened
        .apply_with_provider(&restore.id, || Ok(None), true)
        .unwrap();
    assert_eq!(fs::read(&path).unwrap(), original);
}
#[test]
fn changed_adapter_path_cannot_redirect_a_pending_plan_even_when_both_files_are_absent() {
    let f = fixture();
    let provider = p(&f, "responses");
    let transaction = prepare(&f, "codex", Some(&provider));
    let redirected = ProviderManager::with_adapters(
        f.home.clone(),
        &f.data,
        f.manager.helper_source.clone(),
        f.vault.clone(),
        Arc::new(config::ConfigAdapters::from_contributions(vec![&RedirectedCodex]).unwrap()),
    )
    .unwrap();
    assert_eq!(
        redirected
            .apply_with_provider(&transaction.id, || Ok(Some(provider.clone())), true)
            .err()
            .unwrap()
            .code,
        "conflict"
    );
    assert_eq!(
        redirected
            .check_switch(&transaction.id, Some(&provider))
            .err()
            .unwrap()
            .code,
        "conflict"
    );
    assert!(!f.home.join(".codex/config.toml").exists());
    assert!(!f.home.join(".codex/different-config.toml").exists());
    assert_eq!(
        f.manager
            .journal()
            .unwrap()
            .transactions
            .last()
            .unwrap()
            .state,
        "pending"
    );
    f.manager
        .apply_with_provider(&transaction.id, || Ok(Some(provider.clone())), true)
        .unwrap();
    assert!(f.home.join(".codex/config.toml").exists());
    assert!(!f.home.join(".codex/different-config.toml").exists());
    assert_eq!(
        redirected
            .prepare_restore(&transaction.id)
            .err()
            .unwrap()
            .code,
        "conflict"
    );
}
#[test]
fn antigravity_app_rejects_provider_switch_before_touching_native_configuration() {
    let f = fixture();
    let path = f.home.join(".gemini/config/config.json");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let original = br#"{"userSettings":{"permissionPreset":"native-sentinel"}}"#;
    fs::write(&path, original).unwrap();
    let status = config::inspect(&f.home, "antigravity");
    assert_eq!(status.state, "unsupported");
    assert!(status.endpoint.is_none() && status.model.is_none() && status.revision.is_none());
    let error = f
        .manager
        .prepare(
            BindingDraft {
                operation: None,
                agent_id: "antigravity".into(),
                expected_revision: "absent".into(),
                provider_id: None,
                expected_provider_revision: None,
                model: None,
            },
            None,
        )
        .unwrap_err();
    assert_eq!(error.code, "unsupported");
    assert!(error.message.contains("CLI / SDK"));
    assert_eq!(fs::read(path).unwrap(), original);
    assert!(f.manager.status().unwrap().transactions.is_empty());
}
#[test]
fn deferred_switch_is_durable_and_never_writes_before_idle_commit() {
    for agent in ["claude", "claude-desktop", "codex", "opencode"] {
        let f = fixture();
        if agent == "claude-desktop" {
            let root = f
                .home
                .join("Library/Application Support/Claude-3p/configLibrary");
            fs::create_dir_all(&root).unwrap();
            fs::write(
                root.join("_meta.json"),
                br#"{"appliedId":"1bb72c52-51fa-421b-83da-d07fa5d02cf9"}"#,
            )
            .unwrap();
        }
        let mut provider = p(&f, "messages");
        provider.endpoint = "http://127.0.0.1:12345/isolated-route".into();
        let t = if agent == "opencode" || agent == "claude-desktop" {
            prepare(&f, agent, Some(&provider))
        } else {
            prepare(&f, agent, None)
        };
        let path = config::path(&f.home, agent).unwrap();
        let before = fs::read(&path).ok();
        let deferred = f.manager.activation(&t.id, "deferred").unwrap();
        assert_eq!(deferred.state, "pending");
        assert_eq!(fs::read(&path).ok(), before);
        assert_eq!(
            f.manager.status().unwrap().transactions[0]
                .activation
                .as_ref()
                .unwrap()
                .state,
            "deferred"
        );
        assert!(f.manager.activation(&t.id, "launch_requested").is_err());
        let p = t.provider.as_ref();
        f.manager.check_switch(&t.id, p).unwrap();
        assert_eq!(
            f.manager.apply(&t.id, p, false).unwrap_err().code,
            "agent_busy"
        );
        assert_eq!(fs::read(&path).ok(), before);
        let applied = f.manager.apply(&t.id, p, true).unwrap();
        assert_eq!(applied.activation.unwrap().state, "awaiting_restart");
        assert!(f.manager.activation(&t.id, "deferred").is_err());
        let launched = f.manager.activation(&t.id, "launch_requested").unwrap();
        assert_eq!(launched.activation.unwrap().state, "launch_requested");
        fs::write(&path, b"external-edit").unwrap();
        assert!(f.manager.check_switch(&t.id, p).is_err());
        assert!(f.manager.activation(&t.id, "launch_requested").is_err());
    }
}

#[test]
fn restart_preflight_rejects_stale_provider_and_external_edits_without_mutation() {
    let f = fixture();
    let path = config(&f, "claude", b"{}");
    let provider = p(&f, "messages");
    let t = prepare(&f, "claude", Some(&provider));
    let mut changed = provider.clone();
    changed.revision += 1;
    assert_eq!(
        f.manager
            .check_switch(&t.id, Some(&changed))
            .unwrap_err()
            .code,
        "conflict"
    );
    assert_eq!(fs::read(&path).unwrap(), b"{}");
    fs::write(&path, b"{\"new\":true}").unwrap();
    assert_eq!(
        f.manager
            .check_switch(&t.id, Some(&provider))
            .unwrap_err()
            .code,
        "conflict"
    );
    f.manager.cancel(&t.id).unwrap();
    assert!(f.manager.activation(&t.id, "deferred").is_err());
    assert!(f.manager.check_switch(&t.id, Some(&provider)).is_err());
}
#[test]
fn claude_switch_backup_restore_preserves_original_bytes_and_unrelated_config() {
    let f = fixture();
    let original=br#"{ "env":{"ANTHROPIC_AUTH_TOKEN":"original-secret-sentinel","KEEP_ME":"ok"}, "hooks":{"custom":[]},"permissions":{"deny":["Read(.env)"]},"mcpServers":{"x":{}},"unknown":42 }"#;
    let path = config(&f, "claude", original);
    let provider = p(&f, "messages");
    let t = prepare(&f, "claude", Some(&provider));
    assert_eq!(fs::read(&path).unwrap(), original);
    let mut stored = vec![];
    tree_bytes(&f.data, &mut stored);
    for bytes in stored {
        assert!(!bytes.windows(24).any(|w| w == b"original-secret-sentinel"));
        assert!(!String::from_utf8_lossy(&bytes).contains("provider-secret-sentinel"));
    }
    assert_eq!(
        fs::metadata(&t.backup_path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(
        f.manager.apply(&t.id, Some(&provider), true).unwrap().state,
        "applied"
    );
    let changed: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(changed["env"]["KEEP_ME"], "ok");
    assert_eq!(changed["unknown"], 42);
    assert_eq!(
        changed["env"]["ANTHROPIC_DEFAULT_HAIKU_MODEL"],
        "fixture-model"
    );
    assert_eq!(
        changed["env"]["CLAUDE_CODE_SUBAGENT_MODEL"],
        "fixture-model"
    );
    assert_eq!(changed["permissions"]["deny"][0], "Read(.env)");
    assert!(
        changed["apiKeyHelper"]
            .as_str()
            .unwrap()
            .contains("--credential")
    );
    assert!(
        !fs::read_to_string(&path)
            .unwrap()
            .contains("provider-secret-sentinel")
    );
    let restore = f.manager.prepare_restore(&t.id).unwrap();
    f.manager.apply(&restore.id, None, true).unwrap();
    assert_eq!(fs::read(&path).unwrap(), original);
    // A restore is itself backed up, so undoing the restore recovers the applied connection.
    let undo = f.manager.prepare_restore(&restore.id).unwrap();
    f.manager.apply(&undo.id, None, true).unwrap();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&fs::read(path).unwrap()).unwrap(),
        changed
    );
}
#[test]
fn codex_roundtrip_preserves_comments_mcp_profiles_and_does_not_read_auth_json() {
    let f = fixture();
    let original=b"# user preferences\nmodel = 'old'\n[profiles.review]\nmodel='review-model'\n[mcp_servers.safe]\ncommand='safe-server'\n[future]\nunknown=42\n";
    let path = config(&f, "codex", original);
    fs::write(
        f.home.join(".codex/auth.json"),
        b"not parsed credential sentinel",
    )
    .unwrap();
    let provider = p(&f, "responses");
    let t = prepare(&f, "codex", Some(&provider));
    f.manager.apply(&t.id, Some(&provider), true).unwrap();
    let text = fs::read_to_string(&path).unwrap();
    assert!(text.contains("# user preferences"));
    assert!(text.contains("[mcp_servers.safe]"));
    assert!(text.contains("[profiles.review]"));
    let doc = text.parse::<toml_edit::DocumentMut>().unwrap();
    let id = doc["model_provider"].as_str().unwrap();
    assert_eq!(
        doc["model_providers"][id]["auth"]["args"][0].as_str(),
        Some("--credential")
    );
    assert!(
        doc["model_providers"][id]
            .get("experimental_bearer_token")
            .is_none()
    );
    let restore = f.manager.prepare_restore(&t.id).unwrap();
    f.manager.apply(&restore.id, None, true).unwrap();
    assert_eq!(fs::read(path).unwrap(), original);
    assert_eq!(
        fs::read(f.home.join(".codex/auth.json")).unwrap(),
        b"not parsed credential sentinel"
    );
}
#[test]
fn busy_conflicts_and_changed_provider_never_overwrite_valid_config() {
    let f = fixture();
    let original = b"model='before'\n";
    let path = config(&f, "codex", original);
    let provider = p(&f, "responses");
    let t = prepare(&f, "codex", Some(&provider));
    assert_eq!(
        f.manager
            .apply(&t.id, Some(&provider), false)
            .unwrap_err()
            .code,
        "agent_busy"
    );
    assert_eq!(fs::read(&path).unwrap(), original);
    let mut changed = provider.clone();
    changed.endpoint = "https://changed.invalid/v1".into();
    assert_eq!(
        f.manager
            .apply(&t.id, Some(&changed), true)
            .unwrap_err()
            .code,
        "conflict"
    );
    fs::write(&path, b"model='external-edit'\n").unwrap();
    assert_eq!(
        f.manager
            .apply(&t.id, Some(&provider), true)
            .unwrap_err()
            .code,
        "conflict"
    );
    assert_eq!(fs::read(&path).unwrap(), b"model='external-edit'\n");
    f.manager.cancel(&t.id).unwrap();
    assert_eq!(
        f.manager
            .apply(&t.id, Some(&provider), true)
            .unwrap_err()
            .code,
        "conflict"
    );
}

#[test]
fn completed_transaction_releases_lock_even_while_a_fork_copy_remains_open() {
    let f = fixture();
    let guard = f.manager.provider_edit_guard().unwrap();
    // A child between fork and exec keeps the same open file description even
    // with CLOEXEC. A duplicate models that interval without forking the test runner.
    let inherited = guard.file.try_clone().unwrap();
    assert_eq!(f.manager.status().err().unwrap().code, "busy");
    drop(guard);
    let status = f.manager.status();
    drop(inherited);
    assert!(status.is_ok(), "finished transaction retained its lock");
}

#[test]
fn active_transaction_still_excludes_concurrent_managers_without_touching_config() {
    let f = fixture();
    let original = b"model='before'\n";
    let path = config(&f, "codex", original);
    let provider = p(&f, "responses");
    let transaction = prepare(&f, "codex", Some(&provider));
    let reopened = ProviderManager::new(
        f.home.clone(),
        &f.data,
        f.manager.helper_source.clone(),
        f.vault.clone(),
    )
    .unwrap();
    let guard = f.manager.provider_edit_guard().unwrap();
    let unrelated = fixture();
    std::thread::scope(|scope| {
        for manager in [&f.manager, &reopened] {
            let id = &transaction.id;
            let provider = &provider;
            scope.spawn(move || {
                assert_eq!(manager.status().err().unwrap().code, "busy");
                assert_eq!(
                    manager.apply(id, Some(provider), true).unwrap_err().code,
                    "busy"
                );
                assert_eq!(manager.cancel(id).unwrap_err().code, "busy");
            });
        }
        scope.spawn(|| assert!(unrelated.manager.status().is_ok()));
    });
    assert_eq!(fs::read(&path).unwrap(), original);
    assert_eq!(
        f.manager.journal().unwrap().transactions[0].state,
        "pending"
    );
    drop(guard);
    reopened
        .apply(&transaction.id, Some(&provider), true)
        .unwrap();
    let restore = reopened.prepare_restore(&transaction.id).unwrap();
    reopened.apply(&restore.id, None, true).unwrap();
    assert_eq!(fs::read(path).unwrap(), original);
}

#[test]
fn missing_original_file_is_restored_to_absence_and_restart_recovers_interrupted_commit() {
    let f = fixture();
    let t = prepare(&f, "codex", None);
    let path = config::path(&f.home, "codex").unwrap();
    let plan = f.manager.plan(&t.id).unwrap();
    let mut journal = f.manager.journal().unwrap();
    journal.transactions[0].state = "applying".into();
    f.manager.save(&journal).unwrap();
    files::replace(
        &path,
        &plan.before_stamp,
        plan.after.as_deref(),
        plan.after_mode,
    )
    .unwrap();
    let restarted = ProviderManager::new(
        f.home.clone(),
        &f.data,
        f.manager.helper_source.clone(),
        f.vault.clone(),
    )
    .unwrap();
    assert_eq!(restarted.status().unwrap().transactions[0].state, "applied");
    let restore = restarted.prepare_restore(&t.id).unwrap();
    restarted.apply(&restore.id, None, true).unwrap();
    assert!(!path.exists());
}
#[test]
fn corrupt_or_lost_backup_blocks_mutation_and_restore_rejects_external_edit() {
    let f = fixture();
    let path = config(&f, "codex", b"model='old'\n");
    let t = prepare(&f, "codex", None);
    fs::write(&t.backup_path, b"corrupt").unwrap();
    assert_eq!(
        f.manager.apply(&t.id, None, true).unwrap_err().code,
        "backup_failed"
    );
    assert_eq!(fs::read(&path).unwrap(), b"model='old'\n");
    f.manager.cancel(&t.id).unwrap();
    let t = prepare(&f, "codex", None);
    f.manager.apply(&t.id, None, true).unwrap();
    fs::write(&path, b"model='new-user-edit'\n").unwrap();
    assert_eq!(
        f.manager.prepare_restore(&t.id).unwrap_err().code,
        "conflict"
    );
    f.vault.0.lock().unwrap().clear();
    assert!(f.manager.prepare_restore(&t.id).is_err());
    assert_eq!(fs::read(&path).unwrap(), b"model='new-user-edit'\n");
}
#[test]
fn invalid_symlinks_profiles_and_login_restrictions_are_not_rewritten() {
    let f = fixture();
    let path = config(&f, "codex", b"profile='work'\n");
    let p = p(&f, "responses");
    let revision = config::inspect(&f.home, "codex").revision.unwrap();
    assert_eq!(
        f.manager
            .prepare(
                BindingDraft {
                    operation: None,
                    agent_id: "codex".into(),
                    expected_revision: revision,
                    provider_id: Some(p.id.clone()),
                    expected_provider_revision: Some(p.revision),
                    model: None,
                },
                Some(&p)
            )
            .unwrap_err()
            .code,
        "unsupported"
    );
    fs::remove_file(&path).unwrap();
    symlink(f.home.join("target"), &path).unwrap();
    assert_eq!(files::read(&path).err().unwrap().code, "unsafe_path");
    assert!(
        config::render(
            "claude",
            Some(br#"{"forceLoginMethod":"claudeai"}"#),
            Some(&p),
            Path::new("/helper")
        )
        .is_err()
    );
    assert!(config::render("claude", Some(b"broken"), None, Path::new("/helper")).is_err());
    assert!(validate("claude", &p).is_err());
    assert!(config::render("codex", Some(b"model=5"), None, Path::new("/helper")).is_err());
    assert!(
        config::render(
            "codex",
            Some(b"forced_login_method='api'"),
            None,
            Path::new("/helper")
        )
        .is_err()
    );
    assert!(
        config::render(
            "claude",
            Some(br#"{"forceLoginMethod":"console"}"#),
            None,
            Path::new("/helper")
        )
        .is_err()
    );
}
#[test]
fn verification_binds_to_full_provider_version_and_expiry() {
    let f = fixture();
    let mut p = p(&f, "responses");
    assert!(f.manager.require_verification(&p).is_err());
    let mut v = Verification {
        provider_id: p.id.clone(),
        model: None,
        fingerprint: fingerprint(&p),
        state: "verified".into(),
        checked_at: chrono::Utc::now().to_rfc3339(),
        message: "test".into(),
    };
    f.manager.save_verification(v.clone()).unwrap();
    f.manager.require_verification(&p).unwrap();
    p.model = "different".into();
    assert!(f.manager.require_verification(&p).is_err());
    p.model = "fixture-model".into();
    v.checked_at = (chrono::Utc::now() - chrono::Duration::minutes(16)).to_rfc3339();
    f.manager.save_verification(v).unwrap();
    assert!(f.manager.require_verification(&p).is_err());
}

#[test]
fn opencode_keeps_other_groups_uses_local_auth_and_restores_exactly() {
    for grouped in [false, true] {
        let f = fixture();
        let original=br#"{ "model":"old/model", "provider":{"old":{"options":{"apiKey":"old-native-secret"}}}, "mcp":{"keep":{}}, "enabled_providers":["old"] }"#;
        let path = config(&f, "opencode", original);
        let mut upstream = p(&f, "messages");
        upstream.name = "MyDeepSeek".into();
        upstream.vendor = "deepseek".into();
        upstream.models = vec!["another-model".into(), "disabled-model".into()];
        upstream.disabled_models = vec!["disabled-model".into()];
        let mut rendered = upstream.clone();
        rendered.endpoint = "http://127.0.0.1:12345/gateway/fixture".into();
        rendered.credential_ref = Some(
            f.manager
                .credential("local", Zeroizing::new("local-route-secret".into()))
                .unwrap()
                .credential_ref,
        );
        let before = config::inspect(&f.home, "opencode");
        let transaction = f
            .manager
            .prepare_routed(
                BindingDraft {
                    operation: None,
                    agent_id: "opencode".into(),
                    expected_revision: before.revision.unwrap(),
                    provider_id: Some(upstream.id.clone()),
                    expected_provider_revision: Some(upstream.revision),
                    model: None,
                },
                Some(&upstream),
                Some((
                    &rendered,
                    Routing {
                        id: uuid::Uuid::new_v4().to_string(),
                        client_protocol: "messages".into(),
                        upstream_protocol: "messages".into(),
                        local_endpoint: rendered.endpoint.clone(),
                        models: if grouped {
                            upstream.enabled_model_ids()
                        } else {
                            vec![]
                        },
                    },
                )),
            )
            .unwrap();
        f.manager
            .apply(&transaction.id, Some(&upstream), true)
            .unwrap();
        let bytes = fs::read(&path).unwrap();
        let doc: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            doc["provider"]["old"]["options"]["apiKey"],
            "old-native-secret"
        );
        assert_eq!(doc["mcp"]["keep"], serde_json::json!({}));
        assert_eq!(doc["enabled_providers"].as_array().unwrap().len(), 2);
        let group = doc["model"].as_str().unwrap().split_once('/').unwrap().0;
        assert_eq!(doc["provider"][group]["name"], "MyDeepSeek · Local Agent");
        let published = doc["provider"][group]["models"].as_object().unwrap();
        assert_eq!(
            published.len(),
            if grouped { 2 } else { 1 },
            "legacy single-model and grouped snapshots have separate publication scopes"
        );
        assert!(published.contains_key("fixture-model"));
        assert_eq!(published.contains_key("another-model"), grouped);
        assert!(!published.contains_key("disabled-model"));
        assert!(String::from_utf8_lossy(&bytes).contains("local-route-secret"));
        assert!(!String::from_utf8_lossy(&bytes).contains("provider-secret-sentinel"));
        assert!(
            !serde_json::to_string(&f.manager.status().unwrap())
                .unwrap()
                .contains("local-route-secret")
        );
        let restored = f.manager.prepare_restore(&transaction.id).unwrap();
        f.manager.apply(&restored.id, None, true).unwrap();
        assert_eq!(fs::read(path).unwrap(), original);
    }
}

#[test]
fn per_agent_model_selection_preserves_provider_default_and_encrypted_backup() {
    for (agent, protocol, original) in [
        (
            "claude",
            "messages",
            br#"{"permissions":{"deny":["Read(.env)"]}}"#.as_slice(),
        ),
        (
            "codex",
            "responses",
            b"# original preferences\nmodel='old'\n".as_slice(),
        ),
    ] {
        let f = fixture();
        let path = config(&f, agent, original);
        let mut source = p(&f, protocol);
        source.models = vec!["alternate-model".into()];
        let selected = select_model(&source, Some("alternate-model")).unwrap();
        assert_eq!(source.model, "fixture-model");
        assert!(select_model(&source, Some("unregistered-model")).is_err());
        assert!(select_model(&source, Some("")).is_err());
        let verification = |p: &Provider| Verification {
            provider_id: p.id.clone(),
            model: Some(p.model.clone()),
            fingerprint: fingerprint(p),
            state: "verified".into(),
            checked_at: chrono::Utc::now().to_rfc3339(),
            message: "fixture".into(),
        };
        f.manager.save_verification(verification(&source)).unwrap();
        assert!(f.manager.require_verification(&selected).is_err());
        f.manager
            .save_verification(verification(&selected))
            .unwrap();
        f.manager.require_verification(&source).unwrap();
        f.manager.require_verification(&selected).unwrap();
        assert_eq!(f.manager.verifications().unwrap().len(), 2);
        let transaction = f
            .manager
            .prepare(
                BindingDraft {
                    operation: None,
                    agent_id: agent.into(),
                    expected_revision: config::inspect(&f.home, agent).revision.unwrap(),
                    provider_id: Some(source.id.clone()),
                    expected_provider_revision: Some(source.revision),
                    model: Some("alternate-model".into()),
                },
                Some(&source),
            )
            .unwrap();
        assert_eq!(fs::read(&path).unwrap(), original);
        assert_eq!(
            transaction.provider.as_ref().unwrap().model,
            "fixture-model"
        );
        assert_eq!(transaction.model.as_deref(), Some("alternate-model"));
        assert!(transaction.target_name.contains("alternate-model"));
        let backup = fs::read(&transaction.backup_path).unwrap();
        assert!(backup.starts_with(b"LAC2"));
        assert_eq!(
            f.manager.plan(&transaction.id).unwrap().before.as_deref(),
            Some(original)
        );
        f.manager
            .apply(&transaction.id, Some(&source), true)
            .unwrap();
        let actual = config::inspect(&f.home, agent);
        assert_eq!(actual.model.as_deref(), Some("alternate-model"));
        let restore = f.manager.prepare_restore(&transaction.id).unwrap();
        f.manager.apply(&restore.id, None, true).unwrap();
        assert_eq!(fs::read(&path).unwrap(), original);
    }
}

#[test]
fn disabled_models_cannot_be_selected_or_prepared_even_as_default() {
    let f = fixture();
    let path = config(&f, "codex", b"model='before'\n");
    let original = fs::read(&path).unwrap();
    let mut source = p(&f, "responses");
    source.models = vec!["alternate-model".into()];
    source.disabled_models = vec![source.model.clone(), "alternate-model".into()];
    assert_eq!(
        select_model(&source, None).unwrap_err().code,
        "invalid_model"
    );
    assert!(select_model(&source, Some("alternate-model")).is_err());
    assert!(
        f.manager
            .prepare(
                BindingDraft {
                    operation: None,
                    agent_id: "codex".into(),
                    expected_revision: config::inspect(&f.home, "codex").revision.unwrap(),
                    provider_id: Some(source.id.clone()),
                    expected_provider_revision: Some(source.revision),
                    model: Some("alternate-model".into()),
                },
                Some(&source)
            )
            .is_err()
    );
    assert_eq!(fs::read(&path).unwrap(), original);
    assert!(f.manager.status().unwrap().transactions.is_empty());
    source.disabled_models.pop();
    assert!(select_model(&source, Some("alternate-model")).is_ok());
    assert!(select_model(&source, None).is_err());
}

#[test]
fn claude_desktop_updates_applied_profile_and_refuses_unknown_profile_paths() {
    let f = fixture();
    let root = f
        .home
        .join("Library/Application Support/Claude-3p/configLibrary");
    fs::create_dir_all(&root).unwrap();
    let id = uuid::Uuid::new_v4().to_string();
    fs::write(
        root.join("_meta.json"),
        serde_json::json!({"appliedId":id}).to_string(),
    )
    .unwrap();
    let original=br#"{"inferenceProvider":"gateway","inferenceGatewayBaseUrl":"http://127.0.0.1:15721/old","inferenceGatewayApiKey":"old-desktop-secret","coworkEgressAllowedHosts":["internal.example"]}"#;
    let path = config(&f, "claude-desktop", original);
    let mut provider = p(&f, "messages");
    provider.name = "MyDeepSeek".into();
    provider.vendor = "deepseek".into();
    provider.endpoint = "http://127.0.0.1:12345/desktop-route".into();
    let transaction = prepare(&f, "claude-desktop", Some(&provider));
    f.manager
        .apply(&transaction.id, Some(&provider), true)
        .unwrap();
    let doc: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(doc["inferenceModels"][0]["name"], "claude-sonnet-5");
    assert_eq!(
        doc["inferenceModels"][0]["labelOverride"],
        "MyDeepSeek · fixture-model"
    );
    assert!(
        doc["inferenceModels"][0]["labelOverride"]
            .as_str()
            .unwrap()
            .contains(&provider.model)
    );
    assert_eq!(doc["inferenceCredentialKind"], "helper-script");
    assert!(doc.get("inferenceGatewayApiKey").is_none());
    assert_eq!(
        doc["coworkEgressAllowedHosts"],
        serde_json::json!(["internal.example"])
    );
    let restore = f.manager.prepare_restore(&transaction.id).unwrap();
    f.manager.apply(&restore.id, None, true).unwrap();
    assert_eq!(fs::read(&path).unwrap(), original);
    fs::write(root.join("_meta.json"), br#"{"appliedId":"../../outside"}"#).unwrap();
    assert!(config::path(&f.home, "claude-desktop").is_err());
    assert!(config::inspect(&f.home, "claude-desktop").error.is_some());
}

#[test]
fn claude_desktop_group_maps_every_enabled_model_and_restores_profile_exactly() {
    let f = fixture();
    let root = f
        .home
        .join("Library/Application Support/Claude-3p/configLibrary");
    fs::create_dir_all(&root).unwrap();
    let id = uuid::Uuid::new_v4().to_string();
    let meta = serde_json::json!({"appliedId":id}).to_string();
    fs::write(root.join("_meta.json"), &meta).unwrap();
    let original=br#"{"inferenceProvider":"gateway","inferenceGatewayApiKey":"original-secret","managedMcpServers":[{"name":"keep"}],"coworkEgressAllowedHosts":["internal.example"]}"#;
    let path = config(&f, "claude-desktop", original);
    let mut source = p(&f, "chat-completions");
    source.name = "MyDeepSeek".into();
    source.model = "z-default".into();
    source.models = vec!["another-model".into(), "disabled-model".into()];
    source.disabled_models = vec!["disabled-model".into()];
    let mut rendered = source.clone();
    rendered.protocol = "messages".into();
    rendered.endpoint = "http://127.0.0.1:12345/group".into();
    let routing = Routing {
        id: uuid::Uuid::new_v4().to_string(),
        client_protocol: "messages".into(),
        upstream_protocol: source.protocol.clone(),
        local_endpoint: rendered.endpoint.clone(),
        models: source.enabled_model_ids(),
    };
    let draft = BindingDraft {
        operation: None,
        agent_id: "claude-desktop".into(),
        expected_revision: config::inspect(&f.home, "claude-desktop").revision.unwrap(),
        provider_id: Some(source.id.clone()),
        expected_provider_revision: Some(source.revision),
        model: None,
    };
    let mut incomplete = routing.clone();
    incomplete.models.pop();
    assert_eq!(
        f.manager
            .prepare_routed(draft.clone(), Some(&source), Some((&rendered, incomplete)))
            .unwrap_err()
            .code,
        "invalid_model"
    );
    assert_eq!(fs::read(&path).unwrap(), original);
    let t = f
        .manager
        .prepare_routed(draft, Some(&source), Some((&rendered, routing)))
        .unwrap();
    f.manager.apply(&t.id, Some(&source), true).unwrap();
    let doc: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    let published = doc["inferenceModels"].as_array().unwrap();
    assert_eq!(published.len(), 2);
    for (entry, model) in published.iter().zip(["z-default", "another-model"]) {
        assert_eq!(entry["name"], claude_desktop_model_id(&source.id, model));
        assert_eq!(entry["labelOverride"], format!("MyDeepSeek · {model}"));
        assert_eq!(entry["maxEffort"], "low");
    }
    assert_ne!(published[0]["name"], published[1]["name"]);
    assert_ne!(
        claude_desktop_model_id("other", "z-default"),
        published[0]["name"]
    );
    assert_eq!(
        doc["managedMcpServers"],
        serde_json::json!([{"name":"keep"}])
    );
    assert_eq!(
        doc["coworkEgressAllowedHosts"],
        serde_json::json!(["internal.example"])
    );
    assert_eq!(doc["modelDiscoveryEnabled"], false);
    assert!(doc.get("inferenceGatewayApiKey").is_none());
    assert!(
        !fs::read_to_string(&path)
            .unwrap()
            .contains("provider-secret-sentinel")
    );
    assert_eq!(fs::read_to_string(root.join("_meta.json")).unwrap(), meta);
    let restore = f.manager.prepare_restore(&t.id).unwrap();
    f.manager.apply(&restore.id, None, true).unwrap();
    assert_eq!(fs::read(path).unwrap(), original);
    assert!(
        desktop_clients::claude_models(&mut b"{}".to_vec(), &rendered, &vec!["model".into(); 201])
            .is_err()
    );
}

#[test]
fn v2_backup_authenticates_reference_and_ciphertext() {
    let f = fixture();
    let path = config(&f, "claude", br#"{"keep":42}"#);
    let provider = p(&f, "messages");
    let transaction = prepare(&f, "claude", Some(&provider));
    let mut encrypted = fs::read(&transaction.backup_path).unwrap();
    assert!(encrypted.starts_with(b"LAC2"));
    encrypted[45] ^= 1;
    fs::write(&transaction.backup_path, encrypted).unwrap();
    assert_eq!(
        f.manager
            .apply(&transaction.id, Some(&provider), true)
            .unwrap_err()
            .code,
        "backup_failed"
    );
    assert_eq!(fs::read(path).unwrap(), br#"{"keep":42}"#);
}

#[test]
fn legacy_backups_remain_readable_and_new_restore_uses_v2() {
    use vault::Vault;
    let f = fixture();
    let path = config(&f, "claude", br#"{"keep":42}"#);
    let provider = p(&f, "messages");
    let transaction = prepare(&f, "claude", Some(&provider));
    let plan = f.manager.plan(&transaction.id).unwrap();
    let key = vault::random_key();
    f.vault
        .set(
            &format!(
                "backup-{}",
                files::hash(f.data.join("provider-configs").to_string_lossy().as_bytes())
            ),
            &key,
        )
        .unwrap();
    let legacy = vault::seal(&key, &serde_json::to_vec(&plan).unwrap(), &transaction.id).unwrap();
    fs::write(&transaction.backup_path, &legacy).unwrap();
    f.manager
        .apply(&transaction.id, Some(&provider), true)
        .unwrap();
    let restore = f.manager.prepare_restore(&transaction.id).unwrap();
    assert!(fs::read(&restore.backup_path).unwrap().starts_with(b"LAC2"));
    assert_eq!(fs::read(&transaction.backup_path).unwrap(), legacy);
    f.manager.apply(&restore.id, None, true).unwrap();
    assert_eq!(fs::read(path).unwrap(), br#"{"keep":42}"#);
}

#[test]
fn connection_metadata_separates_login_api_and_unresolved_profiles_without_vault_access() {
    let temporary = tempfile::tempdir().unwrap();
    let home = temporary.path().canonicalize().unwrap();
    let claude = home.join(".claude");
    fs::create_dir_all(&claude).unwrap();
    fs::write(
        claude.join("settings.json"),
        br#"{"env":{"CLAUDE_CODE_OAUTH_TOKEN":"fixture-oauth"}}"#,
    )
    .unwrap();
    let inspected = config::inspect(&home, "claude");
    assert_eq!(inspected.state, "ready");
    assert_eq!(inspected.mode, "native");
    fs::write(
        claude.join("settings.json"),
        br#"{"forceLoginMethod":"console"}"#,
    )
    .unwrap();
    assert_eq!(config::inspect(&home, "claude").mode, "api");
    fs::write(
        claude.join("settings.json"),
        br#"{"env":{"ANTHROPIC_BASE_URL":"http://127.0.0.1:9/gateway/route?key=fixture-private"}}"#,
    )
    .unwrap();
    let invalid = config::inspect(&home, "claude");
    assert_eq!(invalid.state, "unavailable");
    assert!(invalid.endpoint.is_none());
    assert!(
        !serde_json::to_string(&invalid)
            .unwrap()
            .contains("fixture-private")
    );
    let codex = home.join(".codex");
    fs::create_dir_all(&codex).unwrap();
    fs::write(
        codex.join("config.toml"),
        "[model_providers.openai]\nenv_key='FIXTURE_KEY'\n",
    )
    .unwrap();
    assert_eq!(config::inspect(&home, "codex").mode, "api");
    fs::write(
        codex.join("config.toml"),
        "profile='custom'\n[profiles.custom]\nmodel_provider='other'\n",
    )
    .unwrap();
    assert_eq!(config::inspect(&home, "codex").mode, "profile");
}

#[cfg(target_os = "macos")]
#[test]
#[ignore = "explicit disposable macOS Keychain and packaged credential-helper smoke test"]
fn macos_keychain_and_service_helper_roundtrip_disposable_key() {
    use super::vault::Vault;
    let reference = uuid::Uuid::new_v4().to_string();
    struct Cleanup(String);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = vault::SystemVault.delete(&self.0);
        }
    }
    let _cleanup = Cleanup(reference.clone());
    let secret = Zeroizing::new(format!("disposable-{}", uuid::Uuid::new_v4()));
    vault::SystemVault
        .set(&reference, secret.as_bytes())
        .unwrap();
    assert!(vault::SystemVault.get(&reference).unwrap().as_slice() == secret.as_bytes());
    let binary =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/local-agent-service");
    let output = std::process::Command::new(binary)
        .args(["--credential", &reference])
        .output()
        .unwrap();
    assert!(output.status.success(), "credential helper unavailable");
    assert!(output.stdout == secret.as_bytes());
    assert!(output.stderr.is_empty());
    vault::SystemVault.delete(&reference).unwrap();
    assert_eq!(
        vault::SystemVault.get(&reference).err().unwrap().code,
        "credential_not_found"
    );
}

#[test]
fn codex_group_uses_authenticated_catalog_preserves_mcp_and_restores_shared_config() {
    let f = fixture();
    let original = b"# shared by CLI and desktop\nmodel='official-model'\nmodel_reasoning_effort='high'\nweb_search='cached'\n[mcp_servers.keep]\ncommand='safe'\n";
    let path = config(&f, "codex", original);
    let mut source = p(&f, "chat-completions");
    source.models = vec!["other-model".into(), "disabled".into()];
    source.disabled_models = vec!["disabled".into()];
    let mut rendered = source.clone();
    rendered.protocol = "responses".into();
    rendered.endpoint = "http://127.0.0.1:12345/gateway/test/v1".into();
    let draft = BindingDraft {
        operation: None,
        agent_id: "codex".into(),
        expected_revision: config::inspect(&f.home, "codex").revision.unwrap(),
        provider_id: Some(source.id.clone()),
        expected_provider_revision: Some(source.revision),
        model: None,
    };
    let route = Routing {
        id: uuid::Uuid::new_v4().to_string(),
        client_protocol: "responses".into(),
        upstream_protocol: source.protocol.clone(),
        local_endpoint: rendered.endpoint.clone(),
        models: source.enabled_model_ids(),
    };
    let mut incomplete = route.clone();
    incomplete.models.pop();
    assert_eq!(
        f.manager
            .prepare_routed(draft.clone(), Some(&source), Some((&rendered, incomplete)))
            .unwrap_err()
            .code,
        "invalid_model"
    );
    let transaction = f
        .manager
        .prepare_routed(draft, Some(&source), Some((&rendered, route)))
        .unwrap();
    assert_eq!(fs::read(&path).unwrap(), original);
    f.manager
        .apply(&transaction.id, Some(&source), true)
        .unwrap();
    let bytes = fs::read(&path).unwrap();
    let doc = config::parse_toml(Some(&bytes)).unwrap();
    let id = doc["model_provider"].as_str().unwrap();
    assert_eq!(
        doc["model_providers"][id]["model_catalog_url"].as_str(),
        Some("http://127.0.0.1:12345/gateway/test/v1/codex/models.json")
    );
    assert!(
        doc["model_providers"][id]["auth"]["command"]
            .as_str()
            .is_some()
    );
    assert_eq!(doc["model_reasoning_effort"].as_str(), Some("none"));
    assert_eq!(doc["web_search"].as_str(), Some("disabled"));
    assert_eq!(doc["mcp_servers"]["keep"]["command"].as_str(), Some("safe"));
    assert!(!String::from_utf8_lossy(&bytes).contains("provider-secret-sentinel"));
    let restore = f.manager.prepare_restore(&transaction.id).unwrap();
    f.manager.apply(&restore.id, None, true).unwrap();
    assert_eq!(fs::read(&path).unwrap(), original);
    let mut foreign = b"model_catalog_json='/existing/catalog.json'\n".to_vec();
    assert_eq!(
        config::codex_models(&mut foreign, &rendered)
            .unwrap_err()
            .code,
        "unsupported"
    );
    assert_eq!(foreign, b"model_catalog_json='/existing/catalog.json'\n");
}

#[test]
fn connected_provider_saves_the_normalized_verification_and_rolls_back_on_record_failure() {
    let f = fixture();
    let store = crate::Store::open(&f.data).unwrap();
    let mut provider = p(&f, "chat-completions");
    provider.id = format!("custom-{}", uuid::Uuid::new_v4());
    provider.vendor = "custom".into();
    provider.revision = 0;
    let (saved, verified) = f
        .manager
        .save_connected_provider(&store, provider.clone(), chrono::Utc::now().to_rfc3339())
        .unwrap();
    assert_eq!(saved.revision, 1);
    assert_eq!(verified.fingerprint, fingerprint(&saved));
    f.manager.require_verification(&saved).unwrap();
    assert!(
        f.manager
            .save_connected_provider(&store, provider.clone(), chrono::Utc::now().to_rfc3339())
            .is_err()
    );
    let before = serde_json::to_value(store.providers().unwrap()).unwrap();
    fs::write(
        f.manager.directory.join("verifications.json"),
        b"malformed-record",
    )
    .unwrap();
    provider.id = format!("custom-{}", uuid::Uuid::new_v4());
    assert!(
        f.manager
            .save_connected_provider(&store, provider, chrono::Utc::now().to_rfc3339())
            .is_err()
    );
    assert_eq!(
        before,
        serde_json::to_value(store.providers().unwrap()).unwrap()
    );
}
