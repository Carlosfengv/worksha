use super::super::vault::{MemoryVault, Vault};
use super::*;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
mod cancellation_tests;

#[derive(Default)]
struct FaultVault {
    inner: MemoryVault,
    mode: AtomicUsize,
    writes: std::sync::Mutex<Vec<(String, Vec<u8>)>>,
}
impl Vault for FaultVault {
    fn get(&self, id: &str) -> Result<Zeroizing<Vec<u8>>, ConfigError> {
        self.inner.get(id)
    }
    fn set(&self, id: &str, value: &[u8]) -> Result<(), ConfigError> {
        self.writes
            .lock()
            .unwrap()
            .push((id.into(), value.to_vec()));
        let target = value.len() == 32 && value.iter().all(u8::is_ascii_hexdigit);
        let mode = if target {
            self.mode.swap(0, Ordering::SeqCst)
        } else {
            0
        };
        if mode == 2 {
            return fail("fixture", "before write");
        }
        self.inner.set(id, value)?;
        if mode == 1 {
            return fail("fixture", "lost write reply");
        }
        Ok(())
    }
    fn delete(&self, id: &str) -> Result<(), ConfigError> {
        self.inner.delete(id)
    }
}
struct Fixture {
    root: tempfile::TempDir,
    store: Store,
    manager: ProviderManager,
    vault: Arc<FaultVault>,
}
impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
        let store = Store::open(&root.path().join("data")).unwrap();
        std::fs::create_dir(root.path().join("home")).unwrap();
        let mut provider = store.providers().unwrap().remove(0);
        provider.configured = true;
        provider.protocol = "messages".into();
        provider.endpoint = "http://127.0.0.1:31234".into();
        provider.model = "fixture-model".into();
        provider.models = vec![provider.model.clone()];
        provider.credential_ref = None;
        store.save_provider(provider).unwrap();
        let vault = Arc::new(FaultVault::default());
        let manager = ProviderManager::new(
            root.path().join("home"),
            &root.path().join("data"),
            root.path().join("helper"),
            vault.clone(),
        )
        .unwrap();
        Self {
            root,
            store,
            manager,
            vault,
        }
    }
    fn request(&self, purpose: Purpose) -> Request {
        let protocol = self.manager.local_token_protocol(&self.store).unwrap();
        let provider = self
            .store
            .providers()
            .unwrap()
            .into_iter()
            .find(|p| p.configured)
            .unwrap();
        let path = self.root.path().join("home/.claude/settings.json");
        let service = if purpose == Purpose::ModelService {
            let model_id = uuid::Uuid::new_v4().to_string();
            let saved = self
                .store
                .save_model_service(crate::model_services::ServiceDraft {
                    id: uuid::Uuid::new_v4().to_string(),
                    revision: 0,
                    name: "Fixture service".into(),
                    default_model_id: model_id.clone(),
                    models: vec![crate::model_services::ModelDraft {
                        id: model_id,
                        name: "Default".into(),
                        provider_id: provider.id.clone(),
                        model: provider.model.clone(),
                        capabilities: Default::default(),
                    }],
                    routing_policy: Default::default(),
                })
                .unwrap();
            Some(ServiceIdentity {
                id: saved.id,
                version_id: saved.version_id,
                revision: saved.revision,
            })
        } else {
            None
        };
        Request {
            identity: IdentityV3 {
                store_id: protocol.store_id,
                namespace_id: protocol.namespace_id,
                operation_id: uuid::Uuid::new_v4().to_string(),
            },
            purpose,
            agent_id: "claude".into(),
            configuration_path: path.to_string_lossy().into(),
            configuration_path_hash: files::hash(path.as_os_str().as_encoded_bytes()),
            expected_revision: "absent".into(),
            provider,
            service,
        }
    }
    fn count(&self, table: &str) -> i64 {
        self.store
            .connection
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
            .unwrap()
    }
}

#[test]
fn every_purpose_recovers_the_same_reference_after_an_ambiguous_write() {
    for purpose in [
        Purpose::FixedRoute,
        Purpose::DynamicBinding,
        Purpose::ModelService,
    ] {
        let f = Fixture::new();
        let request = f.request(purpose);
        f.vault.mode.store(1, Ordering::SeqCst);
        let mut session = f
            .manager
            .begin_local_token(&f.store, request.clone())
            .unwrap();
        let reference = session.operation().credential_ref.clone();
        assert_eq!(
            session.ensure_credential().unwrap_err().code,
            "local_token_result_unknown"
        );
        let original = f.vault.inner.get(&reference).unwrap();
        drop(session);
        let pending = f
            .manager
            .local_token_operation(&f.store, &request.identity)
            .unwrap();
        assert_eq!(pending.status, Status::Pending);
        assert_eq!(
            f.count("credential_recovery_values"),
            0,
            "local tokens must not enter API-key discovery"
        );
        let mut resumed = f
            .manager
            .resume_local_token(&f.store, &request.identity)
            .unwrap();
        assert_eq!(
            resumed.ensure_credential().unwrap().credential_ref,
            reference
        );
        assert_eq!(*f.vault.inner.get(&reference).unwrap(), *original);
        resumed.mark_owner_saved().unwrap();
        assert!(
            resumed
                .finish_preview(&uuid::Uuid::new_v4().to_string())
                .is_err()
        );
        let preview = request.identity.operation_id.clone();
        assert_eq!(
            resumed.finish_preview(&preview).unwrap().preview_id,
            Some(preview)
        );
        drop(resumed);
        assert!(
            f.manager
                .local_token_protocol(&f.store)
                .unwrap()
                .pending
                .is_empty()
        );
        assert_eq!(
            f.vault
                .writes
                .lock()
                .unwrap()
                .iter()
                .filter(|(id, _)| id == &reference)
                .count(),
            1
        );
    }
}

#[test]
fn failed_before_target_write_reuses_the_encrypted_original_after_store_reopen() {
    let f = Fixture::new();
    let request = f.request(Purpose::DynamicBinding);
    f.vault.mode.store(2, Ordering::SeqCst);
    let mut session = f
        .manager
        .begin_local_token(&f.store, request.clone())
        .unwrap();
    let reference = session.operation().credential_ref.clone();
    assert!(session.ensure_credential().is_err());
    drop(session);
    let first = f
        .vault
        .writes
        .lock()
        .unwrap()
        .iter()
        .find(|(id, _)| id == &reference)
        .unwrap()
        .1
        .clone();
    let reopened = Store::open(&f.root.path().join("data")).unwrap();
    let mut resumed = f
        .manager
        .resume_local_token(&reopened, &request.identity)
        .unwrap();
    resumed.ensure_credential().unwrap();
    assert_eq!(*f.vault.inner.get(&reference).unwrap(), first);
    assert!(
        f.vault
            .writes
            .lock()
            .unwrap()
            .iter()
            .filter(|(id, _)| id == &reference)
            .all(|(_, bytes)| bytes == &first)
    );
    let raw: String = f
        .store
        .connection
        .query_row("SELECT value FROM local_token_owners", [], |r| r.get(0))
        .unwrap();
    assert!(
        !raw.contains(std::str::from_utf8(&first).unwrap()),
        "token plaintext must not enter SQLite"
    );
}

#[test]
fn cancelled_pending_owner_keeps_staged_credentials_and_fences_continuation() {
    let f = Fixture::new();
    let request = f.request(Purpose::FixedRoute);
    let mut session = f
        .manager
        .begin_local_token(&f.store, request.clone())
        .unwrap();
    let saved = session.ensure_credential().unwrap();
    drop(session);
    let before = f.vault.inner.0.lock().unwrap().clone();
    assert_eq!(
        f.manager
            .cancel_local_token(&f.store, &request.identity)
            .unwrap()
            .status,
        Status::Cancelled
    );
    let mut resumed = f
        .manager
        .resume_local_token(&f.store, &request.identity)
        .unwrap();
    assert_eq!(
        resumed.ensure_credential().unwrap_err().code,
        "local_token_cancelled"
    );
    drop(resumed);
    assert_eq!(*f.vault.inner.0.lock().unwrap(), before);
    assert_eq!(
        f.manager
            .check_local_token_credential(&f.store, &request.identity)
            .unwrap()
            .unwrap()
            .receipt
            .unwrap()
            .credential_ref,
        saved.credential_ref
    );
}

#[test]
fn duplicate_unknown_calls_and_changed_payloads_cannot_allocate_another_token() {
    let f = Fixture::new();
    let request = f.request(Purpose::FixedRoute);
    drop(
        f.manager
            .begin_local_token(&f.store, request.clone())
            .unwrap(),
    );
    let changed = f.request(Purpose::FixedRoute);
    assert_eq!(
        f.manager
            .begin_local_token(&f.store, changed)
            .err()
            .unwrap()
            .code,
        "local_token_recovery_required"
    );
    let mut changed = request.clone();
    changed.provider.name = "changed".into();
    assert_eq!(
        f.manager
            .begin_local_token(&f.store, changed)
            .err()
            .unwrap()
            .code,
        "operation_conflict"
    );
    assert!(f.vault.inner.0.lock().unwrap().is_empty());
    assert_eq!(f.count("local_token_owners"), 1);
}

#[test]
fn ignored_intent_scope_or_cipher_writes_fail_before_target_vault_writes() {
    for trigger in [
        "CREATE TRIGGER fault BEFORE INSERT ON local_token_owners BEGIN SELECT RAISE(IGNORE);END;",
        "CREATE TRIGGER fault BEFORE UPDATE ON local_token_scope BEGIN SELECT RAISE(IGNORE);END;",
        "CREATE TRIGGER fault BEFORE UPDATE ON local_token_owners BEGIN SELECT RAISE(IGNORE);END;",
    ] {
        let f = Fixture::new();
        let request = f.request(Purpose::FixedRoute);
        f.store.connection.execute_batch(trigger).unwrap();
        if let Ok(mut session) = f.manager.begin_local_token(&f.store, request) {
            assert!(session.ensure_credential().is_err());
        }
        assert!(
            !f.vault
                .writes
                .lock()
                .unwrap()
                .iter()
                .any(|(_, bytes)| bytes.len() == 32 && bytes.iter().all(u8::is_ascii_hexdigit)),
            "no target write before durable exact material"
        );
    }
}

#[test]
fn original_store_namespace_and_operation_lock_are_required() {
    let f = Fixture::new();
    let request = f.request(Purpose::FixedRoute);
    let session = f
        .manager
        .begin_local_token(&f.store, request.clone())
        .unwrap();
    assert_eq!(
        f.manager
            .resume_local_token(&f.store, &request.identity)
            .err()
            .unwrap()
            .code,
        "busy"
    );
    drop(session);
    let mut foreign = request.identity.clone();
    foreign.store_id = uuid::Uuid::new_v4().to_string();
    assert_eq!(
        f.manager
            .resume_local_token(&f.store, &foreign)
            .err()
            .unwrap()
            .code,
        "credential_store_changed"
    );
    foreign = request.identity;
    foreign.namespace_id = uuid::Uuid::new_v4().to_string();
    assert_eq!(
        f.manager
            .resume_local_token(&f.store, &foreign)
            .err()
            .unwrap()
            .code,
        "credential_namespace_changed"
    );
    assert!(f.vault.inner.0.lock().unwrap().is_empty());
}

#[test]
fn changed_native_source_or_provider_stops_continuation_but_keeps_original_material() {
    for change_file in [true, false] {
        let f = Fixture::new();
        let request = f.request(Purpose::FixedRoute);
        let mut session = f
            .manager
            .begin_local_token(&f.store, request.clone())
            .unwrap();
        let reference = session.ensure_credential().unwrap().credential_ref;
        drop(session);
        let before = f.vault.inner.0.lock().unwrap().clone();
        if change_file {
            let path = std::path::Path::new(&request.configuration_path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, b"{}").unwrap();
        } else {
            let mut provider = request.provider.clone();
            provider.name = "Later Provider edit".into();
            f.store.save_provider(provider).unwrap();
        }
        let mut resumed = f
            .manager
            .resume_local_token(&f.store, &request.identity)
            .unwrap();
        assert_eq!(resumed.ensure_credential().unwrap_err().code, "conflict");
        drop(resumed);
        assert_eq!(*f.vault.inner.0.lock().unwrap(), before);
        assert_eq!(
            f.manager
                .check_local_token_credential(&f.store, &request.identity)
                .unwrap()
                .unwrap()
                .receipt
                .unwrap()
                .credential_ref,
            reference
        );
    }
}

#[test]
fn same_configuration_bytes_under_another_home_do_not_retarget_the_operation() {
    let f = Fixture::new();
    let request = f.request(Purpose::FixedRoute);
    drop(
        f.manager
            .begin_local_token(&f.store, request.clone())
            .unwrap(),
    );
    let home = f.root.path().join("other-home");
    std::fs::create_dir(&home).unwrap();
    let other = ProviderManager::new(
        home,
        &f.root.path().join("data"),
        f.root.path().join("helper"),
        f.vault.clone(),
    )
    .unwrap();
    let mut resumed = other
        .resume_local_token(&f.store, &request.identity)
        .unwrap();
    assert_eq!(resumed.ensure_credential().unwrap_err().code, "conflict");
    assert!(f.vault.inner.0.lock().unwrap().is_empty());
}

#[test]
fn owned_dynamic_binding_and_service_attachment_reuse_the_original_owner() {
    for purpose in [Purpose::DynamicBinding, Purpose::ModelService] {
        let f = Fixture::new();
        let request = f.request(purpose);
        let mut session = f
            .manager
            .begin_local_token(&f.store, request.clone())
            .unwrap();
        let reference = session.ensure_credential().unwrap().credential_ref;
        let first = f
            .store
            .create_owned_local_binding(session.operation())
            .unwrap();
        assert_eq!(first.id, request.identity.operation_id);
        assert_eq!(first.credential_ref, reference);
        drop(session);
        let mut reopened = f
            .manager
            .resume_local_token(&f.store, &request.identity)
            .unwrap();
        assert!(reopened.operation().owner_saved);
        let second = f
            .store
            .create_owned_local_binding(reopened.operation())
            .unwrap();
        assert_eq!(first.target.id, second.target.id);
        reopened.mark_owner_saved().unwrap();
        assert_eq!(f.count("gateway_bindings"), 1);
        assert_eq!(f.count("gateway_route_versions"), 1);
        assert_eq!(
            f.count("model_service_bindings"),
            i64::from(purpose == Purpose::ModelService)
        );
    }
}

#[test]
fn failed_owner_marker_rolls_back_binding_without_deleting_the_staged_token() {
    for trigger in [
        "CREATE TRIGGER fault BEFORE UPDATE ON local_token_owners WHEN json_extract(NEW.value,'$.operation.ownerSaved')=1 BEGIN SELECT RAISE(IGNORE);END;",
        "CREATE TRIGGER fault AFTER UPDATE ON local_token_owners WHEN json_extract(NEW.value,'$.operation.ownerSaved')=1 BEGIN UPDATE gateway_bindings SET credential='00000000-0000-4000-8000-000000000000';END;",
    ] {
        let f = Fixture::new();
        let request = f.request(Purpose::DynamicBinding);
        let mut session = f
            .manager
            .begin_local_token(&f.store, request.clone())
            .unwrap();
        let reference = session.ensure_credential().unwrap().credential_ref;
        let before = f.vault.inner.0.lock().unwrap().clone();
        f.store.connection.execute_batch(trigger).unwrap();
        assert!(
            f.store
                .create_owned_local_binding(session.operation())
                .is_err()
        );
        assert_eq!(f.count("gateway_bindings"), 0);
        assert_eq!(f.count("gateway_route_versions"), 0);
        assert_eq!(*f.vault.inner.0.lock().unwrap(), before);
        assert!(f.vault.inner.get(&reference).is_ok());
    }
}

#[test]
fn unicode_default_metadata_must_roundtrip_before_it_can_be_an_owned_snapshot() {
    let f = Fixture::new();
    let mut p = f.request(Purpose::FixedRoute).provider;
    p.model = "模".repeat(100);
    p.models.clear();
    f.store.save_provider(p.clone()).unwrap();
    let saved = f
        .store
        .providers()
        .unwrap()
        .into_iter()
        .find(|v| v.id == p.id)
        .unwrap();
    assert!(saved.models.contains(&saved.model));
    assert!(
        f.store.save_provider(saved).is_ok(),
        "normalized Unicode model metadata must be writable without losing its catalog"
    );
}

#[test]
fn pending_capacity_stops_before_external_writes_and_cancellation_releases_only_a_slot() {
    let f = Fixture::new();
    let mut ids = Vec::new();
    for (agent, purpose) in [
        ("claude", Purpose::FixedRoute),
        ("codex", Purpose::FixedRoute),
        ("opencode", Purpose::FixedRoute),
        ("claude", Purpose::ModelService),
    ] {
        let mut request = f.request(purpose);
        request.agent_id = agent.into();
        let path = f
            .manager
            .adapters
            .resolve(agent)
            .unwrap()
            .path(&f.manager.home)
            .unwrap();
        request.configuration_path = path.to_string_lossy().into();
        request.configuration_path_hash = files::hash(path.as_os_str().as_encoded_bytes());
        ids.push(request.identity.clone());
        drop(f.manager.begin_local_token(&f.store, request).unwrap());
    }
    let request = f.request(Purpose::DynamicBinding);
    assert_eq!(
        f.manager
            .local_token_protocol(&f.store)
            .unwrap()
            .pending
            .len(),
        4
    );
    assert_eq!(
        f.manager
            .begin_local_token(&f.store, request.clone())
            .err()
            .unwrap()
            .code,
        "capacity"
    );
    assert!(f.vault.writes.lock().unwrap().is_empty());
    f.manager
        .resume_local_token(&f.store, &ids[0])
        .unwrap()
        .cancel()
        .unwrap();
    drop(f.manager.begin_local_token(&f.store, request).unwrap());
    assert_eq!(
        f.manager
            .local_token_protocol(&f.store)
            .unwrap()
            .pending
            .len(),
        4
    );
    assert_eq!(
        f.store.local_token_record(&ids[0]).unwrap().unwrap().status,
        Status::Cancelled
    );
    assert!(f.vault.writes.lock().unwrap().is_empty());
}

#[test]
fn corrupt_owner_bytes_are_preserved_and_never_reconstructed_from_a_new_token() {
    let f = Fixture::new();
    let request = f.request(Purpose::FixedRoute);
    drop(
        f.manager
            .begin_local_token(&f.store, request.clone())
            .unwrap(),
    );
    f.store
        .connection
        .execute(
            "UPDATE local_token_owners SET value_hash=?1 WHERE operation_id=?2",
            rusqlite::params!["0".repeat(64), request.identity.operation_id],
        )
        .unwrap();
    assert!(
        f.manager
            .resume_local_token(&f.store, &request.identity)
            .is_err()
    );
    assert!(f.manager.local_token_protocol(&f.store).is_err());
    assert!(f.vault.writes.lock().unwrap().is_empty());
    let saved: String = f
        .store
        .connection
        .query_row(
            "SELECT value_hash FROM local_token_owners WHERE operation_id=?1",
            [&request.identity.operation_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(saved, "0".repeat(64));
}

#[test]
fn unicode_catalog_limits_use_scalar_count_for_default_and_catalog_entries() {
    let f = Fixture::new();
    for model in ["模".repeat(200), "😀".repeat(200)] {
        let mut provider = f
            .store
            .providers()
            .unwrap()
            .into_iter()
            .find(|p| p.configured)
            .unwrap();
        provider.model = model.clone();
        provider.models = vec![model];
        f.store.save_provider(provider).unwrap();
        let saved = f
            .store
            .providers()
            .unwrap()
            .into_iter()
            .find(|p| p.configured)
            .unwrap();
        f.store.save_provider(saved).unwrap();
    }
    for default in [false, true] {
        let mut provider = f
            .store
            .providers()
            .unwrap()
            .into_iter()
            .find(|p| p.configured)
            .unwrap();
        if default {
            provider.model = "😀".repeat(201);
        } else {
            provider.models.push("模".repeat(201));
        }
        assert!(f.store.save_provider(provider).is_err());
    }
}
