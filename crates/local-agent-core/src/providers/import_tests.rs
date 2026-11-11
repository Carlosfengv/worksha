use super::*;
use crate::providers::vault::{MemoryVault, Vault};
use crate::storage::default_providers;
use std::{fs, path::PathBuf, sync::Arc};

pub(super) struct Fixture {
    _temp: tempfile::TempDir,
    pub(super) home: PathBuf,
    pub(super) data: PathBuf,
    pub(super) vault: Arc<MemoryVault>,
    pub(super) manager: ProviderManager,
    pub(super) store: Store,
}
pub(super) fn fixture() -> Fixture {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let home = root.join("home");
    let data = root.join("app");
    fs::create_dir_all(home.join(".claude")).unwrap();
    fs::create_dir_all(home.join(".codex")).unwrap();
    let vault = Arc::new(MemoryVault::default());
    let helper = root.join("helper");
    fs::write(&helper, b"test-helper").unwrap();
    let manager = ProviderManager::new(home.clone(), &data, helper, vault.clone()).unwrap();
    let store = Store::open(&data).unwrap();
    Fixture {
        _temp: temp,
        home,
        data,
        vault,
        manager,
        store,
    }
}
pub(super) fn write(f: &Fixture, agent: &str, text: &str) {
    fs::write(config::path(&f.home, agent).unwrap(), text).unwrap();
}
pub(super) fn claude(f: &Fixture) -> Candidate {
    f.manager
        .discover_imports(&f.store.providers().unwrap())
        .into_iter()
        .find(|c| c.agent_id == "claude")
        .unwrap()
}
fn import(f: &Fixture, c: &Candidate, model: Option<&str>) -> Result<Imported, ConfigError> {
    f.manager.import_provider(
        &f.store,
        ImportRequest {
            candidate_id: c.id.clone(),
            expected_revision: c.revision.clone(),
            model: model.map(str::to_owned),
            operation: None,
        },
    )
}

#[test]
fn ambiguous_native_import_write_keeps_a_durable_owner_before_returning_unknown() {
    struct LostReply {
        inner: Arc<MemoryVault>,
    }
    impl Vault for LostReply {
        fn get(&self, account: &str) -> Result<Zeroizing<Vec<u8>>, ConfigError> {
            self.inner.get(account)
        }
        fn set(&self, account: &str, bytes: &[u8]) -> Result<(), ConfigError> {
            self.inner.set(account, bytes)?;
            if bytes == b"native-import-lost-reply-sentinel" {
                return fail("credential_unavailable", "测试：写入回复丢失");
            }
            Ok(())
        }
        fn delete(&self, account: &str) -> Result<(), ConfigError> {
            self.inner.delete(account)
        }
    }
    let f = fixture();
    write(
        &f,
        "claude",
        r#"{"env":{"ANTHROPIC_API_KEY":"native-import-lost-reply-sentinel","ANTHROPIC_MODEL":"model"}}"#,
    );
    let manager = ProviderManager::new(
        f.home.clone(),
        &f.data,
        f.manager.helper_source.clone(),
        Arc::new(LostReply {
            inner: f.vault.clone(),
        }),
    )
    .unwrap();
    let candidate = claude(&f);
    let error = manager
        .import_provider(
            &f.store,
            ImportRequest {
                candidate_id: candidate.id,
                expected_revision: candidate.revision,
                model: None,
                operation: None,
            },
        )
        .err()
        .unwrap();
    println!("native import lost-reply observed code: {}", error.code);
    assert_eq!(error.code, "native_import_result_unknown");
    assert_eq!(
        f.store.providers().unwrap().len(),
        default_providers().len()
    );
    let db = rusqlite::Connection::open(f.data.join("local-agent.sqlite3")).unwrap();
    let count: i64 = db
        .query_row(
            "SELECT count(*) FROM native_import_operations WHERE state='pending'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        count, 1,
        "the external write must already have a durable owner"
    );
    let pending = manager
        .native_import_protocol(&f.store)
        .unwrap()
        .pending_requests;
    assert_eq!(pending.len(), 1);
    let request = pending[0].clone();
    let operation = manager
        .native_import_operation(&f.store, &request, false)
        .unwrap();
    assert_eq!(operation.status, operations::Status::Pending);
    let expected_reference = operation
        .provider
        .as_ref()
        .unwrap()
        .credential_ref
        .clone()
        .unwrap();
    let accounts_before = f.vault.0.lock().unwrap().clone();
    let reopened = Store::open(&f.data).unwrap();
    let restored = manager.import_provider(&reopened, request.clone()).unwrap();
    assert_eq!(
        restored.provider.credential_ref.as_deref(),
        Some(expected_reference.as_str())
    );
    assert_eq!(
        f.vault.0.lock().unwrap().clone(),
        accounts_before,
        "resume must only verify the already written credential"
    );
    assert_eq!(
        reopened.providers().unwrap().len(),
        default_providers().len() + 1
    );
    assert_eq!(
        manager
            .native_import_operation(&reopened, &request, false)
            .unwrap()
            .status,
        operations::Status::Committed
    );
    assert!(
        manager
            .native_import_protocol(&reopened)
            .unwrap()
            .pending_requests
            .is_empty()
    );
    let values: String = db
        .query_row(
            "SELECT group_concat(value) FROM native_import_operations",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(!values.contains("native-import-lost-reply-sentinel"));
}
#[test]
fn claude_import_is_private_idempotent_and_never_changes_source() {
    let f = fixture();
    let original = r#"{"env":{"ANTHROPIC_BASE_URL":"https://api.deepseek.com/anthropic","ANTHROPIC_AUTH_TOKEN":"import-secret-sentinel","ANTHROPIC_API_KEY":"lower-priority-key","ANTHROPIC_MODEL":"sonnet","ANTHROPIC_DEFAULT_SONNET_MODEL":"deepseek-model"},"permissions":{"deny":["Read(.env)"]}}"#;
    write(&f, "claude", original);
    let candidate = claude(&f);
    assert!(candidate.can_import);
    assert_eq!(candidate.model.as_deref(), Some("deepseek-model"));
    assert_eq!(candidate.vendor, "deepseek");
    let encoded = serde_json::to_string(&candidate).unwrap();
    for key in ["import-secret-sentinel", "lower-priority-key"] {
        assert!(!encoded.contains(key));
    }
    assert!(
        f.vault.0.lock().unwrap().is_empty(),
        "scan must not save secrets"
    );
    let result = import(&f, &candidate, None).unwrap();
    assert!(!result.reused);
    assert_eq!(result.provider.revision, 1);
    assert_eq!(result.provider.category, "cloud");
    assert_eq!(
        &**f.manager
            .secret(result.provider.credential_ref.as_ref().unwrap())
            .unwrap(),
        b"import-secret-sentinel"
    );
    assert_eq!(
        f.store.providers().unwrap().len(),
        default_providers().len() + 1
    );
    assert_eq!(
        claude(&f).existing_provider_id.as_deref(),
        Some(result.provider.id.as_str())
    );
    let again = import(&f, &candidate, None).unwrap();
    assert!(again.reused);
    assert_eq!(again.provider.id, result.provider.id);
    assert_eq!(
        f.vault.0.lock().unwrap().len(),
        2,
        "one protection key and one imported API key"
    );
    assert_eq!(
        fs::read_to_string(config::path(&f.home, "claude").unwrap()).unwrap(),
        original
    );
    let db = fs::read(f.data.join("local-agent.sqlite3")).unwrap();
    assert!(
        !db.windows(b"import-secret-sentinel".len())
            .any(|b| b == b"import-secret-sentinel")
    );
}
#[test]
fn source_changes_are_rejected_before_any_keychain_write() {
    let f = fixture();
    write(
        &f,
        "claude",
        r#"{"env":{"ANTHROPIC_API_KEY":"old-key","ANTHROPIC_MODEL":"model"}}"#,
    );
    let c = claude(&f);
    write(
        &f,
        "claude",
        r#"{"env":{"ANTHROPIC_API_KEY":"new-key","ANTHROPIC_MODEL":"model"}}"#,
    );
    assert_eq!(import(&f, &c, None).err().unwrap().code, "conflict");
    assert!(f.vault.0.lock().unwrap().is_empty());
    let new = import(&f, &claude(&f), None).unwrap();
    assert_eq!(
        &**f.manager
            .secret(new.provider.credential_ref.as_ref().unwrap())
            .unwrap(),
        b"new-key"
    );
}
#[test]
fn codex_imports_provider_definitions_and_only_current_model() {
    let f = fixture();
    write(
        &f,
        "codex",
        "model='actual-model'\nmodel_provider='proxy'\n[model_providers.proxy]\nname='My proxy'\nbase_url='https://example.invalid/v1'\nwire_api='responses'\nexperimental_bearer_token='codex-import-secret'\n[model_providers.local]\nbase_url='http://localhost:11434/v1'\nwire_api='chat'\n",
    );
    let list = f.manager.discover_imports(&[]);
    let c = list.iter().find(|c| c.source_label == "proxy").unwrap();
    assert!(c.can_import && c.active);
    assert_eq!(c.model.as_deref(), Some("actual-model"));
    let p = import(&f, c, None).unwrap().provider;
    assert_eq!(p.protocol, "responses");
    let c = list.iter().find(|c| c.source_label == "local").unwrap();
    assert!(c.can_import && !c.active);
    assert!(c.model.is_none());
    assert!(import(&f, c, None).is_err());
    let p = import(&f, c, Some("local-model")).unwrap().provider;
    assert_eq!(p.vendor, "ollama");
    assert_eq!(p.protocol, "chat-completions");
    assert!(p.credential_ref.is_none());
    assert!(
        !serde_json::to_string(&list)
            .unwrap()
            .contains("codex-import-secret")
    );
}
#[test]
fn codex_explicit_api_auth_file_is_bound_to_revision_and_oauth_is_never_imported() {
    let f = fixture();
    write(&f, "codex", "model='model'\n");
    let auth_path = f.home.join(".codex/auth.json");
    fs::write(&auth_path, r#"{"auth_mode":"chatgpt","tokens":{"access_token":"oauth-sentinel"},"OPENAI_API_KEY":"stale-api-key"}"#).unwrap();
    let official = f
        .manager
        .discover_imports(&[])
        .into_iter()
        .find(|c| c.agent_id == "codex")
        .unwrap();
    assert!(!official.can_import);
    assert!(import(&f, &official, None).is_err());
    fs::write(
        &auth_path,
        r#"{"auth_mode":"apikey","OPENAI_API_KEY":"explicit-file-key"}"#,
    )
    .unwrap();
    let c = f
        .manager
        .discover_imports(&[])
        .into_iter()
        .find(|c| c.agent_id == "codex")
        .unwrap();
    assert!(c.can_import);
    assert!(c.credential_source.contains("auth.json"));
    fs::write(
        &auth_path,
        r#"{"auth_mode":"apikey","OPENAI_API_KEY":"rotated-file-key"}"#,
    )
    .unwrap();
    assert_eq!(import(&f, &c, None).err().unwrap().code, "conflict");
    let c = f
        .manager
        .discover_imports(&[])
        .into_iter()
        .find(|c| c.agent_id == "codex")
        .unwrap();
    import(&f, &c, None).unwrap();
    assert!(
        !serde_json::to_string(&c)
            .unwrap()
            .contains("rotated-file-key")
    );
}
#[test]
fn env_key_resolves_only_supplied_process_environment_and_revision_tracks_rotation() {
    let f = fixture();
    write(
        &f,
        "codex",
        "model='model'\nmodel_provider='proxy'\n[model_providers.proxy]\nbase_url='https://example.invalid'\nenv_key='PROVIDER_IMPORT_KEY'\n",
    );
    let find = |key: Option<&str>| {
        f.manager
            .import_materials(&|name| {
                if name == "PROVIDER_IMPORT_KEY" {
                    key.map(str::to_owned)
                } else {
                    None
                }
            })
            .into_iter()
            .find(|m| m.public.source_label == "proxy")
            .unwrap()
    };
    assert!(!find(None).public.can_import);
    let first = find(Some("env-secret-one"));
    let second = find(Some("env-secret-two"));
    assert!(first.public.can_import);
    assert_ne!(first.public.revision, second.public.revision);
    assert!(
        !serde_json::to_string(&first.public)
            .unwrap()
            .contains("env-secret-one")
    );
}
#[test]
fn unsupported_auth_headers_and_unsafe_endpoints_are_explicit() {
    let f = fixture();
    for source in [
        r#"{"env":{"ANTHROPIC_BASE_URL":"https://user:secret@example.invalid","ANTHROPIC_API_KEY":"key","ANTHROPIC_MODEL":"model"}}"#,
        r#"{"env":{"ANTHROPIC_API_KEY":"key","ANTHROPIC_MODEL":"model","ANTHROPIC_CUSTOM_HEADERS":"X-Org: needed"}}"#,
        r#"{"apiKeyHelper":"touch /tmp/must-not-run-local-agent-test","model":"model"}"#,
        r#"{"env":{"CLAUDE_CODE_OAUTH_TOKEN":"oauth-key","ANTHROPIC_MODEL":"model"}}"#,
        r#"{"env":{"CLAUDE_CODE_USE_BEDROCK":"1","ANTHROPIC_API_KEY":"key","ANTHROPIC_MODEL":"model"}}"#,
    ] {
        write(&f, "claude", source);
        let c = claude(&f);
        assert!(!c.can_import);
        assert!(import(&f, &c, None).is_err());
    }
    write(
        &f,
        "codex",
        "model='model'\nmodel_provider='proxy'\n[model_providers.proxy]\nbase_url='https://example.invalid'\nexperimental_bearer_token='key'\nhttp_headers={Org='required'}\n",
    );
    let c = f
        .manager
        .discover_imports(&[])
        .into_iter()
        .find(|c| c.source_label == "proxy")
        .unwrap();
    assert!(!c.can_import);
    assert!(f.vault.0.lock().unwrap().is_empty());
}
#[test]
fn malformed_or_symlink_sources_are_isolated_from_the_other_agent() {
    let f = fixture();
    write(&f, "claude", "invalid secret-bearing JSON");
    write(
        &f,
        "codex",
        "model='m'\nmodel_provider='local'\n[model_providers.local]\nbase_url='http://localhost:11434/v1'\n",
    );
    assert!(!claude(&f).can_import);
    let list = f.manager.discover_imports(&[]);
    assert!(
        list.iter()
            .any(|c| c.source_label == "local" && c.can_import)
    );
    let path = config::path(&f.home, "claude").unwrap();
    fs::remove_file(&path).unwrap();
    std::os::unix::fs::symlink(config::path(&f.home, "codex").unwrap(), &path).unwrap();
    assert!(!claude(&f).can_import);
}
#[test]
fn managed_helpers_reuse_vault_and_invalid_metadata_precedes_new_credential_writes() {
    let f = fixture();
    let credential = f
        .manager
        .credential("test", Zeroizing::new("managed-key".into()))
        .unwrap();
    let helper = f.data.join("provider-configs/bin/credential-helper-abc");
    write(
        &f,
        "codex",
        &format!(
            "model='m'\nmodel_provider='managed'\n[model_providers.managed]\nbase_url='https://example.invalid'\n[model_providers.managed.auth]\ncommand='{}'\nargs=['--credential','{}']\n",
            helper.display(),
            credential.credential_ref
        ),
    );
    let c = f
        .manager
        .discover_imports(&[])
        .into_iter()
        .find(|c| c.source_label == "managed")
        .unwrap();
    assert!(c.can_import);
    let p = import(&f, &c, None).unwrap().provider;
    assert_eq!(
        p.credential_ref.as_deref(),
        Some(credential.credential_ref.as_str())
    );
    assert_eq!(f.vault.0.lock().unwrap().len(), 1);
    write(&f,"claude",&serde_json::json!({"env":{"ANTHROPIC_API_KEY":"new-key","ANTHROPIC_MODEL":"m","ANTHROPIC_DEFAULT_SONNET_MODEL":"x".repeat(210)}}).to_string());
    assert!(import(&f, &claude(&f), None).is_err());
    assert_eq!(
        f.vault.0.lock().unwrap().len(),
        1,
        "failed metadata save must discard only newly created secret"
    );
    assert!(f.vault.get(&credential.credential_ref).is_ok());
}
#[test]
fn managed_gateway_import_unwraps_upstream_and_detects_vault_rotation() {
    let f = fixture();
    let credential = f
        .manager
        .credential("source", Zeroizing::new("upstream-key".into()))
        .unwrap();
    let local = f
        .manager
        .credential("local", Zeroizing::new("local-key".into()))
        .unwrap();
    let source: Provider = serde_json::from_value(serde_json::json!({"id":"deepseek","name":"source","vendor":"deepseek","endpoint":"https://api.deepseek.com","protocol":"chat-completions","model":"real-model","configured":true,"credentialRef":credential.credential_ref,"revision":1})).unwrap();
    let mut rendered = source.clone();
    rendered.endpoint = "http://127.0.0.1:12345/gateway/test".into();
    rendered.protocol = "messages".into();
    rendered.credential_ref = Some(local.credential_ref);
    let t = f
        .manager
        .prepare_routed(
            super::super::BindingDraft {
                operation: None,
                agent_id: "claude".into(),
                expected_revision: "absent".into(),
                provider_id: Some(source.id.clone()),
                expected_provider_revision: Some(1),
                model: None,
            },
            Some(&source),
            Some((
                &rendered,
                super::super::Routing {
                    id: uuid::Uuid::new_v4().to_string(),
                    client_protocol: "messages".into(),
                    upstream_protocol: "chat-completions".into(),
                    local_endpoint: rendered.endpoint.clone(),
                    models: vec![],
                },
            )),
        )
        .unwrap();
    f.manager.apply(&t.id, Some(&source), true).unwrap();
    let c = claude(&f);
    assert!(c.can_import);
    assert_eq!(c.endpoint, Some(source.endpoint));
    assert_eq!(c.protocol, "chat-completions");
    f.vault
        .set(&credential.credential_ref, b"rotated-upstream-key")
        .unwrap();
    assert_eq!(import(&f, &c, None).err().unwrap().code, "conflict");
    let result = import(&f, &claude(&f), None).unwrap();
    assert_eq!(
        result.provider.credential_ref,
        Some(credential.credential_ref)
    );
    // An unknown or damaged gateway snapshot must not create another gateway hop.
    fs::write(f.data.join("provider-configs/journal.json"), b"invalid").unwrap();
    assert!(!claude(&f).can_import);
}
