use super::*;
use crate::{
    Store,
    providers::vault::{MemoryVault, Vault},
};
use rusqlite::params;
use std::{
    fs,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};
const SECRET: &str = "scoped-credential-disposable-secret";
#[derive(Default)]
struct CountVault {
    inner: MemoryVault,
    gets: AtomicUsize,
    sets: AtomicUsize,
    ambiguous: AtomicBool,
}
impl Vault for CountVault {
    fn get(&self, id: &str) -> Result<Zeroizing<Vec<u8>>, ConfigError> {
        self.gets.fetch_add(1, Ordering::SeqCst);
        self.inner.get(id)
    }
    fn set(&self, id: &str, value: &[u8]) -> Result<(), ConfigError> {
        self.sets.fetch_add(1, Ordering::SeqCst);
        self.inner.set(id, value)?;
        if value == SECRET.as_bytes() && self.ambiguous.swap(false, Ordering::SeqCst) {
            return fail("fixture_ambiguous", "fixture");
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
    vault: Arc<CountVault>,
}
impl Fixture {
    fn new(vault: Arc<CountVault>) -> Self {
        let root = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
        fs::create_dir(root.path().join("home")).unwrap();
        let store = Store::open(&root.path().join("data")).unwrap();
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
    fn scope(&self) -> CredentialScope {
        self.store
            .credential_scope(&self.store.database_id().unwrap())
            .unwrap()
    }
    fn namespace(&self) -> String {
        self.manager
            .scoped_credential_protocol_v3(&self.scope())
            .unwrap()
            .namespace_id
    }
    fn folder(&self) -> PathBuf {
        self.root
            .path()
            .join("data/provider-configs/credential-operations-v2")
    }
    fn create2(&self, id: &str) -> Result<ScopedOperation, ConfigError> {
        self.manager.create_scoped_credential(
            &self.scope(),
            id,
            "Fixture",
            Zeroizing::new(SECRET.into()),
        )
    }
    fn create3(&self, id: &str) -> Result<ScopedOperationV3, ConfigError> {
        self.manager.create_credential_v3(
            &self.scope(),
            &self.namespace(),
            id,
            "Fixture",
            Zeroizing::new(SECRET.into()),
        )
    }
    fn old_v2(&self) -> ProviderManager {
        let directory = self.folder();
        files::private_dir(&directory).unwrap();
        files::atomic_write(
            &directory.join("scope.json"),
            &serde_json::to_vec(&Marker {
                version: 2,
                store_id: self.store.database_id().unwrap(),
            })
            .unwrap(),
            0o600,
        )
        .unwrap();
        ProviderManager {
            adapters: self.manager.adapters.clone(),
            home: self.manager.home.clone(),
            directory,
            helper_source: self.manager.helper_source.clone(),
            vault: self.vault.clone(),
            credential_store_id: Some(self.store.database_id().unwrap()),
            credential_namespace_id: None,
            credential_journal: None,
            legacy_credential_fence: Arc::new(std::sync::OnceLock::new()),
        }
    }
    fn version9(&self) {
        self.store.connection.execute_batch("DROP TABLE local_token_cancellations;DROP TABLE local_token_owners;DROP TABLE local_token_values;DROP TABLE local_token_scope;DROP TABLE native_import_operations;DROP TABLE native_import_scope;DROP TABLE credential_legacy_operations;DROP TABLE credential_legacy_scope;DROP TABLE credential_recovery_values;DROP TABLE credential_recovery_scope;PRAGMA user_version=9;").unwrap();
    }
    fn reopen(&self) -> Store {
        Store::open(&self.root.path().join("data")).unwrap()
    }
}
fn id() -> String {
    uuid::Uuid::new_v4().to_string()
}
#[test]
fn protocol_does_not_create_a_file_namespace_or_touch_vault_and_wrong_store_rejects() {
    let vault = Arc::new(CountVault::default());
    let a = Fixture::new(vault.clone());
    let b = Fixture::new(vault.clone());
    let p = a.manager.scoped_credential_protocol_v3(&a.scope()).unwrap();
    assert_eq!(p.version, 3);
    assert!(matches!(p.namespace_state, NamespaceState::Active));
    assert!(!a.folder().exists());
    assert_eq!(
        a.store
            .credential_scope(&b.scope().store_id)
            .err()
            .unwrap()
            .code,
        "credential_store_changed"
    );
    assert_eq!(
        a.manager
            .scoped_credential_protocol_v3(&b.scope())
            .unwrap_err()
            .code,
        "credential_store_changed"
    );
    assert_eq!(vault.gets.load(Ordering::SeqCst), 0);
    assert_eq!(vault.sets.load(Ordering::SeqCst), 0);
}
#[test]
fn same_id_isolated_between_stores_and_protocol_domains_and_replay_keeps_reference() {
    let vault = Arc::new(CountVault::default());
    let a = Fixture::new(vault.clone());
    let b = Fixture::new(vault);
    let id = id();
    let old = a.create2(&id).unwrap();
    let one = a.create3(&id).unwrap();
    let two = b.create3(&id).unwrap();
    assert_ne!(old.operation.receipt, one.operation.receipt);
    assert_ne!(one.operation.receipt, two.operation.receipt);
    let sets = a.vault.sets.load(Ordering::SeqCst);
    assert_eq!(a.create3(&id).unwrap().operation, one.operation);
    assert_eq!(a.vault.sets.load(Ordering::SeqCst), sets);
    assert_eq!(
        a.manager
            .create_credential_v3(
                &a.scope(),
                &a.namespace(),
                &id,
                "Fixture",
                Zeroizing::new("other-key".into())
            )
            .unwrap_err()
            .code,
        "operation_conflict"
    );
    assert_eq!(
        a.store
            .connection
            .pragma_query_value(None, "user_version", |r| r.get::<_, u32>(0))
            .unwrap(),
        Store::SCHEMA_VERSION
    );
}
#[test]
fn old_v1_records_are_not_automatically_assigned_to_the_sql_journal() {
    let a = Fixture::new(Arc::new(CountVault::default()));
    let id = id();
    let old = a
        .manager
        .create_credential_operation(&id, "Fixture", Zeroizing::new(SECRET.into()))
        .unwrap();
    assert_eq!(
        a.manager
            .scoped_credential_operation(&a.scope(), &id, false)
            .unwrap_err()
            .code,
        "operation_not_found"
    );
    let new = a.create3(&id).unwrap();
    assert_ne!(
        new.operation.receipt.as_ref().unwrap().credential_ref,
        old.credential_ref
    );
    assert_eq!(
        a.manager.credential_operation(&id).unwrap().receipt,
        Some(old)
    );
}
#[test]
fn cancellation_before_delivery_survives_reopen_without_vault_calls() {
    let a = Fixture::new(Arc::new(CountVault::default()));
    let scope = a.scope();
    let ns = a.namespace();
    let id = id();
    let cancel = a
        .manager
        .credential_operation_v3(&scope, &ns, &id, true)
        .unwrap();
    assert_eq!(cancel.operation.status, Status::Cancelled);
    assert_eq!(
        a.create3(&id).unwrap_err().code,
        "credential_operation_cancelled"
    );
    let reopened = a.reopen();
    let scope = reopened
        .credential_scope(&reopened.database_id().unwrap())
        .unwrap();
    assert_eq!(
        a.manager
            .credential_operation_v3(&scope, &ns, &id, false)
            .unwrap()
            .operation,
        cancel.operation
    );
    assert_eq!(a.vault.gets.load(Ordering::SeqCst), 0);
    assert_eq!(a.vault.sets.load(Ordering::SeqCst), 0);
}
#[test]
fn whole_provider_directory_loss_does_not_allocate_another_credential() {
    let a = Fixture::new(Arc::new(CountVault::default()));
    let id = id();
    let first = a.create3(&id).unwrap();
    let scope = a.scope();
    let ns = a.namespace();
    let sets = a.vault.sets.load(Ordering::SeqCst);
    fs::rename(
        a.root.path().join("data/provider-configs"),
        a.root.path().join("moved-original-provider-files"),
    )
    .unwrap();
    assert_eq!(
        a.manager
            .credential_operation_v3(&scope, &ns, &id, false)
            .unwrap()
            .operation,
        first.operation
    );
    assert_eq!(a.create3(&id).unwrap().operation, first.operation);
    assert_eq!(a.vault.sets.load(Ordering::SeqCst), sets);
    let store = a.reopen();
    let manager = ProviderManager::new(
        a.root.path().join("home"),
        &a.root.path().join("data"),
        a.root.path().join("helper"),
        a.vault.clone(),
    )
    .unwrap();
    let scope = store
        .credential_scope(&store.database_id().unwrap())
        .unwrap();
    assert_eq!(
        manager
            .credential_operation_v3(&scope, &ns, &id, false)
            .unwrap()
            .operation,
        first.operation
    );
}
#[test]
fn ambiguous_vault_completion_is_recovered_with_the_original_reference() {
    let a = Fixture::new(Arc::new(CountVault::default()));
    let id = id();
    a.vault.ambiguous.store(true, Ordering::SeqCst);
    assert_eq!(
        a.create3(&id).unwrap_err().code,
        "credential_result_unknown"
    );
    let sets = a.vault.sets.load(Ordering::SeqCst);
    let result = a
        .manager
        .credential_operation_v3(&a.scope(), &a.namespace(), &id, false)
        .unwrap();
    assert_eq!(result.operation.status, Status::Ready);
    assert_eq!(a.create3(&id).unwrap().operation, result.operation);
    assert_eq!(a.vault.sets.load(Ordering::SeqCst), sets);
}
#[test]
fn missing_current_sql_metadata_is_not_reinitialized_on_reopen() {
    for sql in [
        "DELETE FROM credential_recovery_scope",
        "UPDATE credential_recovery_scope SET format=2",
        "DROP TABLE credential_recovery_values",
    ] {
        let a = Fixture::new(Arc::new(CountVault::default()));
        a.create3(&id()).unwrap();
        let gets = a.vault.gets.load(Ordering::SeqCst);
        a.store.connection.execute_batch(sql).unwrap();
        let store = a.reopen();
        assert!(
            store
                .credential_scope(&store.database_id().unwrap())
                .is_err()
        );
        assert_eq!(a.vault.gets.load(Ordering::SeqCst), gets);
    }
}
#[test]
fn deleting_a_single_authoritative_record_is_detected_before_vault_and_exact_restore_recovers() {
    let a = Fixture::new(Arc::new(CountVault::default()));
    let id = id();
    let first = a.create3(&id).unwrap();
    let scope = a.scope();
    let ns = a.namespace();
    let name = format!("operations/{id}.json");
    let row:(Vec<u8>,String,i64)=a.store.connection.query_row("SELECT value,value_hash,revision FROM credential_recovery_values WHERE domain=3 AND name=?1",[&name],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).unwrap();
    a.store
        .connection
        .execute(
            "DELETE FROM credential_recovery_values WHERE domain=3 AND name=?1",
            [&name],
        )
        .unwrap();
    let gets = a.vault.gets.load(Ordering::SeqCst);
    assert_eq!(
        a.manager
            .create_credential_v3(&scope, &ns, &id, "Fixture", Zeroizing::new(SECRET.into()))
            .unwrap_err()
            .code,
        "credential_recovery_corrupt"
    );
    assert_eq!(a.vault.gets.load(Ordering::SeqCst), gets);
    a.store
        .connection
        .execute(
            "INSERT INTO credential_recovery_values VALUES(3,?1,?2,?3,?4)",
            params![name, row.0, row.1, row.2],
        )
        .unwrap();
    assert_eq!(
        a.manager
            .credential_operation_v3(&scope, &ns, &id, false)
            .unwrap()
            .operation,
        first.operation
    );
}
#[test]
fn ignored_intent_insert_rolls_back_metadata_and_does_not_write_a_target_secret() {
    let a = Fixture::new(Arc::new(CountVault::default()));
    a.create3(&id()).unwrap();
    let sets = a.vault.sets.load(Ordering::SeqCst);
    let revision: i64 = a
        .store
        .connection
        .query_row("SELECT revision FROM credential_recovery_scope", [], |r| {
            r.get(0)
        })
        .unwrap();
    a.store.connection.execute_batch("CREATE TRIGGER ignore_credential_intent BEFORE INSERT ON credential_recovery_values WHEN NEW.name GLOB 'operations/*' BEGIN SELECT RAISE(IGNORE);END;").unwrap();
    assert!(a.create3(&id()).is_err());
    assert_eq!(a.vault.sets.load(Ordering::SeqCst), sets);
    assert_eq!(
        a.store
            .connection
            .query_row("SELECT revision FROM credential_recovery_scope", [], |r| {
                r.get::<_, i64>(0)
            })
            .unwrap(),
        revision
    );
}
#[test]
fn failed_ready_write_preserves_intent_and_later_query_does_not_repeat_vault_set() {
    let a = Fixture::new(Arc::new(CountVault::default()));
    let id = id();
    a.store.connection.execute_batch("CREATE TRIGGER ignore_credential_ready BEFORE UPDATE ON credential_recovery_values WHEN NEW.domain=3 AND NEW.name GLOB 'operations/*' AND json_extract(CAST(NEW.value AS TEXT),'$.operation.state')='ready' BEGIN SELECT RAISE(IGNORE);END;").unwrap();
    assert_eq!(
        a.create3(&id).unwrap_err().code,
        "credential_result_unknown"
    );
    let sets = a.vault.sets.load(Ordering::SeqCst);
    a.store
        .connection
        .execute_batch("DROP TRIGGER ignore_credential_ready;")
        .unwrap();
    let result = a
        .manager
        .credential_operation_v3(&a.scope(), &a.namespace(), &id, false)
        .unwrap();
    assert_eq!(result.operation.status, Status::Ready);
    assert_eq!(a.vault.sets.load(Ordering::SeqCst), sets);
}
#[test]
fn actual_v2_archive_migrates_without_rewriting_files_or_changing_references() {
    let a = Fixture::new(Arc::new(CountVault::default()));
    a.version9();
    let old = a.old_v2();
    let id = id();
    let original = old
        .create_credential_operation(&id, "Fixture", Zeroizing::new(SECRET.into()))
        .unwrap();
    let path = a.folder().join("operations").join(format!("{id}.json"));
    let bytes = fs::read(&path).unwrap();
    let sets = a.vault.sets.load(Ordering::SeqCst);
    let store = a.reopen();
    let scope = store
        .credential_scope(&store.database_id().unwrap())
        .unwrap();
    assert_eq!(
        a.manager
            .scoped_credential_operation(&scope, &id, false)
            .unwrap()
            .operation
            .receipt,
        Some(original.clone())
    );
    assert_eq!(fs::read(&path).unwrap(), bytes);
    fs::rename(a.folder(), a.root.path().join("moved-v2-archive")).unwrap();
    assert_eq!(
        a.manager
            .create_scoped_credential(&scope, &id, "Fixture", Zeroizing::new(SECRET.into()))
            .unwrap()
            .operation
            .receipt,
        Some(original)
    );
    assert_eq!(a.vault.sets.load(Ordering::SeqCst), sets);
    assert_eq!(
        a.manager
            .create_scoped_credential(
                &scope,
                &self::id(),
                "Fixture",
                Zeroizing::new(SECRET.into())
            )
            .unwrap_err()
            .code,
        "credential_protocol_unavailable"
    );
    let backup = rusqlite::Connection::open(
        a.root
            .path()
            .join("data/local-agent.pre-credential-recovery-v9.sqlite3"),
    )
    .unwrap();
    assert_eq!(
        backup
            .pragma_query_value(None, "user_version", |r| r.get::<_, u32>(0))
            .unwrap(),
        9
    );
}
#[test]
fn old_database_without_material_is_unverified_and_new_space_never_enables_old_id_creation() {
    let a = Fixture::new(Arc::new(CountVault::default()));
    a.version9();
    let store = a.reopen();
    let scope = store
        .credential_scope(&store.database_id().unwrap())
        .unwrap();
    let p = a.manager.scoped_credential_protocol_v3(&scope).unwrap();
    assert!(matches!(p.namespace_state, NamespaceState::Unverified));
    assert_eq!(
        a.manager
            .create_credential_v3(
                &scope,
                &p.namespace_id,
                &id(),
                "Fixture",
                Zeroizing::new(SECRET.into())
            )
            .unwrap_err()
            .code,
        "credential_namespace_unverified"
    );
    let ready = a
        .manager
        .initialize_credential_namespace(&scope, &p.namespace_id, false)
        .unwrap();
    assert_eq!(ready.namespace_id, p.namespace_id);
    assert!(matches!(ready.namespace_state, NamespaceState::Active));
    assert!(matches!(ready.legacy_state, LegacyState::Unverified));
    assert_eq!(a.vault.sets.load(Ordering::SeqCst), 0);
    assert_eq!(
        a.manager
            .create_scoped_credential(&scope, &id(), "Fixture", Zeroizing::new(SECRET.into()))
            .unwrap_err()
            .code,
        "credential_namespace_unverified"
    );
}
#[test]
fn explicitly_restoring_lost_old_material_recovers_original_reference_without_new_vault_write() {
    let a = Fixture::new(Arc::new(CountVault::default()));
    a.version9();
    let old = a.old_v2();
    let id = id();
    let saved = old
        .create_credential_operation(&id, "Fixture", Zeroizing::new(SECRET.into()))
        .unwrap();
    let backup = a.root.path().join("old-material-backup");
    fs::rename(a.folder(), &backup).unwrap();
    let store = a.reopen();
    let scope = store
        .credential_scope(&store.database_id().unwrap())
        .unwrap();
    let p = a.manager.scoped_credential_protocol_v3(&scope).unwrap();
    assert!(matches!(p.legacy_state, LegacyState::Unverified));
    assert!(
        a.manager
            .initialize_credential_namespace(&scope, &p.namespace_id, true)
            .is_err()
    );
    let sets = a.vault.sets.load(Ordering::SeqCst);
    fs::rename(backup, a.folder()).unwrap();
    let ready = a
        .manager
        .initialize_credential_namespace(&scope, &p.namespace_id, true)
        .unwrap();
    assert!(matches!(ready.legacy_state, LegacyState::Imported));
    assert_eq!(
        a.manager
            .scoped_credential_operation(&scope, &id, false)
            .unwrap()
            .operation
            .receipt,
        Some(saved)
    );
    assert_eq!(a.vault.sets.load(Ordering::SeqCst), sets);
}
#[test]
fn adding_a_new_key_preserves_old_material_and_allows_later_explicit_recovery() {
    let a = Fixture::new(Arc::new(CountVault::default()));
    a.version9();
    let old_id = id();
    let old_receipt = a
        .old_v2()
        .create_credential_operation(&old_id, "Old provider", Zeroizing::new(SECRET.into()))
        .unwrap();
    let old_record = format!("operations/{old_id}.json");
    let original_bytes = fs::read(a.folder().join(&old_record)).unwrap();
    let backup = a.root.path().join("old-material-backup");
    fs::rename(a.folder(), &backup).unwrap();
    let store = a.reopen();
    let scope = store
        .credential_scope(&store.database_id().unwrap())
        .unwrap();
    let before = a.manager.scoped_credential_protocol_v3(&scope).unwrap();
    assert!(matches!(before.namespace_state, NamespaceState::Unverified));
    let sets = a.vault.sets.load(Ordering::SeqCst);
    let gets = a.vault.gets.load(Ordering::SeqCst);
    let prepared = a
        .manager
        .initialize_credential_namespace(&scope, &before.namespace_id, false)
        .unwrap();
    assert!(matches!(prepared.namespace_state, NamespaceState::Active));
    assert!(matches!(prepared.legacy_state, LegacyState::Unverified));
    assert_eq!(prepared.namespace_id, before.namespace_id);
    assert_eq!(a.vault.sets.load(Ordering::SeqCst), sets);
    assert_eq!(a.vault.gets.load(Ordering::SeqCst), gets);
    let new_id = id();
    let new_receipt = a
        .manager
        .create_credential_v3(
            &scope,
            &prepared.namespace_id,
            &new_id,
            "New provider",
            Zeroizing::new("new-provider-disposable-key".into()),
        )
        .unwrap()
        .operation
        .receipt
        .unwrap();
    assert_ne!(new_receipt.credential_ref, old_receipt.credential_ref);
    assert_eq!(
        a.vault
            .inner
            .get(&old_receipt.credential_ref)
            .unwrap()
            .as_slice(),
        SECRET.as_bytes()
    );
    assert_eq!(fs::read(backup.join(&old_record)).unwrap(), original_bytes);
    let sets = a.vault.sets.load(Ordering::SeqCst);
    fs::rename(backup, a.folder()).unwrap();
    let recovered = a
        .manager
        .initialize_credential_namespace(&scope, &prepared.namespace_id, true)
        .unwrap();
    assert!(matches!(recovered.legacy_state, LegacyState::Imported));
    assert_eq!(
        a.manager
            .scoped_credential_operation(&scope, &old_id, false)
            .unwrap()
            .operation
            .receipt,
        Some(old_receipt)
    );
    assert_eq!(
        a.manager
            .credential_operation_v3(&scope, &prepared.namespace_id, &new_id, false)
            .unwrap()
            .operation
            .receipt,
        Some(new_receipt)
    );
    assert_eq!(
        fs::read(a.folder().join(&old_record)).unwrap(),
        original_bytes
    );
    assert_eq!(a.vault.sets.load(Ordering::SeqCst), sets);
}

#[test]
fn namespace_changes_and_future_payloads_never_get_adopted() {
    let a = Fixture::new(Arc::new(CountVault::default()));
    let id = id();
    a.create3(&id).unwrap();
    let scope = a.scope();
    let ns = a.namespace();
    let gets = a.vault.gets.load(Ordering::SeqCst);
    assert_eq!(
        a.manager
            .credential_operation_v3(&scope, &self::id(), &id, false)
            .unwrap_err()
            .code,
        "credential_namespace_changed"
    );
    assert_eq!(a.vault.gets.load(Ordering::SeqCst), gets);
    let name = format!("operations/{id}.json");
    let mut value: serde_json::Value =
        serde_json::from_slice(&scope.journal.domain(3).read(&name).unwrap().unwrap()).unwrap();
    value["version"] = serde_json::json!(4);
    let bytes = serde_json::to_vec(&value).unwrap();
    a.store.connection.execute("UPDATE credential_recovery_values SET value=?1,value_hash=?2 WHERE domain=3 AND name=?3",params![bytes,files::hash(&bytes),name]).unwrap();
    assert!(
        a.manager
            .credential_operation_v3(&scope, &ns, &id, false)
            .is_err()
    );
    assert_eq!(a.vault.gets.load(Ordering::SeqCst), gets);
}
#[test]
fn invalid_ids_do_not_write_recovery_values_or_vault() {
    let a = Fixture::new(Arc::new(CountVault::default()));
    for id in ["../bad", "00000000-0000-4000-0000-000000000000"] {
        assert_eq!(
            a.create3(id).unwrap_err().code,
            "invalid_credential_operation"
        );
    }
    assert_eq!(
        a.store
            .connection
            .query_row("SELECT count(*) FROM credential_recovery_values", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(a.vault.sets.load(Ordering::SeqCst), 0);
}

#[test]
fn copied_and_relabelled_sql_values_cannot_adopt_another_stores_credential() {
    let vault = Arc::new(CountVault::default());
    let a = Fixture::new(vault.clone());
    let b = Fixture::new(vault);
    let id = id();
    let original = a.create3(&id).unwrap();
    let mut statement = a
        .store
        .connection
        .prepare("SELECT name,value,revision FROM credential_recovery_values WHERE domain=3")
        .unwrap();
    let rows: Vec<(String, Vec<u8>, i64)> = statement
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    let revision: i64 = a
        .store
        .connection
        .query_row("SELECT revision FROM credential_recovery_scope", [], |r| {
            r.get(0)
        })
        .unwrap();
    for (name, bytes, stamp) in &rows {
        let mut value: serde_json::Value = serde_json::from_slice(bytes).unwrap();
        if name.starts_with("operations/") {
            value["storeId"] = serde_json::json!(b.store.database_id().unwrap());
            value["namespaceId"] = serde_json::json!(b.namespace());
        }
        let value = serde_json::to_vec(&value).unwrap();
        b.store
            .connection
            .execute(
                "INSERT INTO credential_recovery_values VALUES(3,?1,?2,?3,?4)",
                params![name, value, files::hash(&value), stamp],
            )
            .unwrap();
    }
    b.store
        .connection
        .execute(
            "UPDATE credential_recovery_scope SET value_count=?1,revision=?2",
            params![rows.len() as i64, revision],
        )
        .unwrap();
    assert_eq!(
        b.manager
            .credential_operation_v3(&b.scope(), &b.namespace(), &id, false)
            .unwrap_err()
            .code,
        "credential_result_unknown"
    );
    assert_eq!(
        a.manager
            .credential_operation_v3(&a.scope(), &a.namespace(), &id, false)
            .unwrap()
            .operation,
        original.operation
    );
}
#[test]
fn ignored_namespace_activation_cannot_claim_success_or_touch_vault() {
    let a = Fixture::new(Arc::new(CountVault::default()));
    a.version9();
    let store = a.reopen();
    let scope = store
        .credential_scope(&store.database_id().unwrap())
        .unwrap();
    let p = a.manager.scoped_credential_protocol_v3(&scope).unwrap();
    store.connection.execute_batch("CREATE TRIGGER ignore_namespace_activation BEFORE UPDATE ON credential_recovery_scope BEGIN SELECT RAISE(IGNORE);END;").unwrap();
    assert!(
        a.manager
            .initialize_credential_namespace(&scope, &p.namespace_id, false)
            .is_err()
    );
    assert!(matches!(
        a.manager
            .scoped_credential_protocol_v3(&scope)
            .unwrap()
            .namespace_state,
        NamespaceState::Unverified
    ));
    assert_eq!(a.vault.sets.load(Ordering::SeqCst), 0);
}
#[test]
fn a_missing_protection_metadata_row_never_allocates_a_replacement_key() {
    let a = Fixture::new(Arc::new(CountVault::default()));
    let id = id();
    a.create3(&id).unwrap();
    let scope = a.scope();
    let ns = a.namespace();
    let sets = a.vault.sets.load(Ordering::SeqCst);
    let gets = a.vault.gets.load(Ordering::SeqCst);
    a.store
        .connection
        .execute(
            "DELETE FROM credential_recovery_values WHERE domain=3 AND name='protection-key.json'",
            [],
        )
        .unwrap();
    assert!(
        a.manager
            .create_credential_v3(&scope, &ns, &id, "Fixture", Zeroizing::new(SECRET.into()))
            .is_err()
    );
    assert_eq!(a.vault.sets.load(Ordering::SeqCst), sets);
    assert_eq!(a.vault.gets.load(Ordering::SeqCst), gets);
}

fn compatibility_page(a: &Fixture) -> super::super::credential_operations::discovery::Page {
    let gets = a.vault.gets.load(Ordering::SeqCst);
    let sets = a.vault.sets.load(Ordering::SeqCst);
    let page = a
        .manager
        .discover_credentials(
            &a.scope(),
            &super::super::credential_operations::discovery::Request {
                store_id: a.scope().store_id.clone(),
                namespace_id: a.namespace(),
                cursor: None,
            },
        )
        .unwrap();
    assert_eq!(a.vault.gets.load(Ordering::SeqCst), gets);
    assert_eq!(a.vault.sets.load(Ordering::SeqCst), sets);
    page
}

#[test]
fn unnumbered_compatibility_writes_keep_old_shape_and_each_has_a_discoverable_intent() {
    let a = Fixture::new(Arc::new(CountVault::default()));
    let first = a
        .manager
        .compatibility_credential(&a.scope(), "Compatibility", Zeroizing::new(SECRET.into()))
        .unwrap();
    let second = a
        .manager
        .compatibility_credential(&a.scope(), "Compatibility", Zeroizing::new(SECRET.into()))
        .unwrap();
    assert_ne!(first.credential_ref, second.credential_ref);
    let old = serde_json::to_value(&first).unwrap();
    assert_eq!(old.as_object().unwrap().len(), 2);
    assert_eq!(old["credentialRef"], first.credential_ref);
    assert_eq!(old["credentialLabel"], "Compatibility");
    let page = compatibility_page(&a);
    assert_eq!(page.items.len(), 2);
    let mut references = vec![];
    for item in page.items {
        assert!(matches!(
            item.kind,
            super::super::credential_operations::discovery::Kind::ScopedV3
        ));
        assert_eq!(item.phase, Some(Status::Ready));
        let saved = a
            .manager
            .credential_operation_v3(&a.scope(), &a.namespace(), &item.operation_id, false)
            .unwrap();
        references.push(saved.operation.receipt.unwrap().credential_ref);
    }
    assert!(
        references.contains(&first.credential_ref) && references.contains(&second.credential_ref)
    );
}

#[test]
fn unnumbered_ambiguous_vault_write_is_discoverable_and_recovers_without_a_second_write() {
    let a = Fixture::new(Arc::new(CountVault::default()));
    a.vault.ambiguous.store(true, Ordering::SeqCst);
    assert!(
        a.manager
            .compatibility_credential(&a.scope(), "Compatibility", Zeroizing::new(SECRET.into()))
            .is_err()
    );
    let page = compatibility_page(&a);
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].phase, Some(Status::Pending));
    let sets = a.vault.sets.load(Ordering::SeqCst);
    let saved = a
        .manager
        .credential_operation_v3(
            &a.scope(),
            &a.namespace(),
            &page.items[0].operation_id,
            false,
        )
        .unwrap();
    assert_eq!(saved.operation.status, Status::Ready);
    assert_eq!(a.vault.sets.load(Ordering::SeqCst), sets);
}

#[test]
fn unnumbered_write_cannot_touch_vault_when_the_source_or_intent_is_unverifiable() {
    for sql in [
        "UPDATE credential_recovery_scope SET state='unverified'",
        "DROP TABLE credential_recovery_values",
        "CREATE TRIGGER ignore_compat_intent BEFORE INSERT ON credential_recovery_values WHEN NEW.name GLOB 'operations/*' BEGIN SELECT RAISE(IGNORE); END;",
    ] {
        let a = Fixture::new(Arc::new(CountVault::default()));
        // Prewarm only the auxiliary protection key. The target for this
        // unnumbered request must never be written when its intent is rejected.
        a.create3(&id()).unwrap();
        let scope = a.scope();
        let gets = a.vault.gets.load(Ordering::SeqCst);
        let sets = a.vault.sets.load(Ordering::SeqCst);
        a.store.connection.execute_batch(sql).unwrap();
        assert!(
            a.manager
                .compatibility_credential(&scope, "Compatibility", Zeroizing::new(SECRET.into()))
                .is_err()
        );
        assert_eq!(a.vault.sets.load(Ordering::SeqCst), sets);
        if !sql.starts_with("CREATE") {
            assert_eq!(a.vault.gets.load(Ordering::SeqCst), gets);
        }
    }
}

#[test]
fn unnumbered_compatibility_capacity_preserves_existing_records_and_writes_no_new_key() {
    let a = Fixture::new(Arc::new(CountVault::default()));
    let first = a.create3(&id()).unwrap();
    let store_id = a.scope().store_id;
    let namespace_id = a.namespace();
    let cancelled = serde_json::to_vec(&serde_json::json!({"version":3,"storeId":store_id,"namespaceId":namespace_id,"operation":{"state":"cancelled"}})).unwrap();
    let hash = files::hash(&cancelled);
    let tx = a.store.connection.unchecked_transaction().unwrap();
    let revision: i64 = tx
        .query_row("SELECT revision FROM credential_recovery_scope", [], |r| {
            r.get(0)
        })
        .unwrap();
    for _ in 1..4096 {
        tx.execute(
            "INSERT INTO credential_recovery_values VALUES(3,?1,?2,?3,?4)",
            params![
                format!("operations/{}.json", id()),
                cancelled,
                hash,
                revision
            ],
        )
        .unwrap();
    }
    tx.execute("UPDATE credential_recovery_scope SET value_count=(SELECT count(*) FROM credential_recovery_values)", []).unwrap();
    tx.commit().unwrap();
    let sets = a.vault.sets.load(Ordering::SeqCst);
    assert_eq!(
        a.manager
            .compatibility_credential(&a.scope(), "Compatibility", Zeroizing::new(SECRET.into()))
            .err()
            .unwrap()
            .code,
        "credential_operation_capacity"
    );
    assert_eq!(a.vault.sets.load(Ordering::SeqCst), sets);
    assert_eq!(
        a.manager
            .credential_operation_v3(
                &a.scope(),
                &namespace_id,
                &first.operation.operation_id,
                false
            )
            .unwrap()
            .operation,
        first.operation
    );
    assert_eq!(a.store.connection.query_row("SELECT count(*) FROM credential_recovery_values WHERE domain=3 AND name GLOB 'operations/*'",[],|r|r.get::<_,i64>(0)).unwrap(),4096);
}
