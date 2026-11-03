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
const SECRET: &str = "disposable-legacy-migration-fixture-key";
#[derive(Default)]
struct FaultVault {
    inner: MemoryVault,
    gets: AtomicUsize,
    sets: AtomicUsize,
    target_sets: AtomicUsize,
    fail_target: AtomicBool,
    fail_aux_reply: AtomicBool,
}
impl Vault for FaultVault {
    fn get(&self, id: &str) -> Result<Zeroizing<Vec<u8>>, ConfigError> {
        self.gets.fetch_add(1, Ordering::SeqCst);
        self.inner.get(id)
    }
    fn set(&self, id: &str, bytes: &[u8]) -> Result<(), ConfigError> {
        self.sets.fetch_add(1, Ordering::SeqCst);
        if bytes == SECRET.as_bytes() {
            self.target_sets.fetch_add(1, Ordering::SeqCst);
            if self.fail_target.load(Ordering::SeqCst) {
                return Err(invalid());
            }
        }
        self.inner.set(id, bytes)?;
        if bytes != SECRET.as_bytes() && self.fail_aux_reply.swap(false, Ordering::SeqCst) {
            return Err(invalid());
        }
        Ok(())
    }
    fn delete(&self, id: &str) -> Result<(), ConfigError> {
        self.inner.delete(id)
    }
}
struct Fixture {
    _root: tempfile::TempDir,
    store: Store,
    manager: ProviderManager,
    vault: Arc<FaultVault>,
}
impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
        let data = root.path().join("data");
        let store = Store::open(&data).unwrap();
        let vault = Arc::new(FaultVault::default());
        let manager = ProviderManager::new(
            root.path().join("home"),
            &data,
            root.path().join("helper"),
            vault.clone(),
        )
        .unwrap();
        Self {
            _root: root,
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
    fn old(&self, id: &str) -> Result<super::super::Receipt, ConfigError> {
        self.manager
            .create_credential_operation(id, "Fixture", Zeroizing::new(SECRET.into()))
    }
    fn preview(&self, id: &str) -> Result<Preview, ConfigError> {
        self.manager
            .preview_legacy_credential(&self.scope(), &self.namespace(), id)
    }
    fn confirm(&self, p: &Preview) -> Result<Preview, ConfigError> {
        self.manager.confirm_legacy_credential(
            &self.scope(),
            &Confirmation {
                store_id: p.store_id.clone(),
                namespace_id: p.namespace_id.clone(),
                operation_id: p.operation_id.clone(),
                source_digest: p.source_digest.clone(),
                confirmed: true,
            },
        )
    }
    fn query(
        &self,
        id: &str,
    ) -> Result<crate::providers::credential_scope::ScopedOperationV3, ConfigError> {
        self.manager
            .credential_operation_v3(&self.scope(), &self.namespace(), id, false)
    }
    fn bytes(&self, id: &str) -> Vec<u8> {
        fs::read(self.manager.credential_operation_path(id).unwrap()).unwrap()
    }
    fn journal(&self, id: &str) -> Option<serde_json::Value> {
        self.store
            .connection
            .query_row(
                "SELECT value FROM credential_recovery_values WHERE domain=3 AND name=?",
                [format!("operations/{id}.json")],
                |r| r.get::<_, Vec<u8>>(0),
            )
            .ok()
            .map(|bytes| serde_json::from_slice(&bytes).unwrap())
    }
    fn counts(&self) -> (usize, usize) {
        (
            self.vault.gets.load(Ordering::SeqCst),
            self.vault.sets.load(Ordering::SeqCst),
        )
    }
}
#[test]
fn preview_is_bound_to_actual_source_and_does_not_touch_vault_or_sql() {
    let f = Fixture::new();
    let id = uuid::Uuid::new_v4().to_string();
    f.old(&id).unwrap();
    let raw = f.bytes(&id);
    let before = f.counts();
    let p = f.preview(&id).unwrap();
    assert!(p.stage == Stage::Unclaimed);
    assert!(p.phase == Status::Ready);
    assert_eq!(p.label.as_deref(), Some("Fixture"));
    assert_eq!(p.source_digest.len(), 64);
    assert_eq!(f.counts(), before);
    assert_eq!(f.bytes(&id), raw);
    assert!(f.journal(&id).is_none());
    let text = serde_json::to_string(&p).unwrap();
    assert!(!text.contains(SECRET));
    assert!(!text.contains("credentialRef"));
}
#[test]
fn ready_migration_preserves_original_reference_raw_material_and_idempotence() {
    let f = Fixture::new();
    let id = uuid::Uuid::new_v4().to_string();
    let receipt = f.old(&id).unwrap();
    let raw = f.bytes(&id);
    let key = fs::read(f.manager.directory.join("credential-operation-key-v1.json")).unwrap();
    let p = f.preview(&id).unwrap();
    let target_writes = f.vault.target_sets.load(Ordering::SeqCst);
    assert!(f.confirm(&p).unwrap().stage == Stage::Committed);
    let state = f.journal(&id).unwrap();
    assert_eq!(
        state["legacyMigration"]["evidence"]["operation"]
            .as_str()
            .unwrap()
            .as_bytes(),
        raw
    );
    assert_eq!(
        state["legacyMigration"]["evidence"]["protectionKey"]
            .as_str()
            .unwrap()
            .as_bytes(),
        key
    );
    assert!(
        f.query(&id)
            .unwrap()
            .operation
            .receipt
            .unwrap()
            .credential_ref
            == receipt.credential_ref
    );
    let before = f.counts();
    assert!(f.confirm(&p).unwrap().stage == Stage::Committed);
    assert_eq!(f.counts(), before);
    assert_eq!(f.vault.target_sets.load(Ordering::SeqCst), target_writes);
    assert_ne!(f.bytes(&id), raw);
    assert!(f.old(&id).is_err());
    assert!(f.manager.cancel_credential_operation(&id).is_err());
}
#[test]
fn pending_migration_never_writes_target_and_explicit_same_key_retry_uses_original_ref() {
    let f = Fixture::new();
    let id = uuid::Uuid::new_v4().to_string();
    f.vault.fail_target.store(true, Ordering::SeqCst);
    assert!(f.old(&id).is_err());
    let original: serde_json::Value = serde_json::from_slice(&f.bytes(&id)).unwrap();
    let reference = original["reference"].as_str().unwrap();
    let p = f.preview(&id).unwrap();
    let writes = f.vault.target_sets.load(Ordering::SeqCst);
    assert!(p.phase == Status::Pending);
    assert!(f.confirm(&p).unwrap().stage == Stage::Committed);
    assert!(f.query(&id).unwrap().operation.status == Status::Pending);
    assert_eq!(f.vault.target_sets.load(Ordering::SeqCst), writes);
    f.vault.fail_target.store(false, Ordering::SeqCst);
    let saved = f
        .manager
        .create_credential_v3(
            &f.scope(),
            &f.namespace(),
            &id,
            "Fixture",
            Zeroizing::new(SECRET.into()),
        )
        .unwrap();
    assert_eq!(saved.operation.receipt.unwrap().credential_ref, reference);
    assert!(f.journal(&id).unwrap()["legacyMigration"]["stage"] == "committed");
}
#[test]
fn cancelled_pending_stays_cancelled_and_can_observe_a_late_original_target() {
    let f = Fixture::new();
    let id = uuid::Uuid::new_v4().to_string();
    f.vault.fail_target.store(true, Ordering::SeqCst);
    assert!(f.old(&id).is_err());
    let state = f.manager.cancel_credential_operation(&id).unwrap();
    assert!(state.status == Status::CancelledPending);
    let original: serde_json::Value = serde_json::from_slice(&f.bytes(&id)).unwrap();
    let reference = original["reference"].as_str().unwrap();
    let p = f.preview(&id).unwrap();
    let writes = f.vault.target_sets.load(Ordering::SeqCst);
    f.confirm(&p).unwrap();
    assert!(f.query(&id).unwrap().operation.status == Status::CancelledPending);
    assert!(
        f.manager
            .create_credential_v3(
                &f.scope(),
                &f.namespace(),
                &id,
                "Fixture",
                Zeroizing::new(SECRET.into())
            )
            .is_err()
    );
    assert_eq!(f.vault.target_sets.load(Ordering::SeqCst), writes);
    f.vault.inner.set(reference, SECRET.as_bytes()).unwrap();
    assert_eq!(
        f.query(&id)
            .unwrap()
            .operation
            .receipt
            .unwrap()
            .credential_ref,
        reference
    );
}
#[test]
fn a_cancelled_tombstone_migrates_without_any_vault_access() {
    let f = Fixture::new();
    let id = uuid::Uuid::new_v4().to_string();
    f.manager.cancel_credential_operation(&id).unwrap();
    let before = f.counts();
    let p = f.preview(&id).unwrap();
    f.confirm(&p).unwrap();
    assert!(f.query(&id).unwrap().operation.status == Status::Cancelled);
    assert_eq!(f.counts(), before);
    assert!(f.old(&id).is_err());
}
#[test]
fn source_change_after_preview_is_rejected_before_sql_or_vault() {
    let f = Fixture::new();
    let id = uuid::Uuid::new_v4().to_string();
    f.old(&id).unwrap();
    let p = f.preview(&id).unwrap();
    let mut raw = f.bytes(&id);
    raw.push(b' ');
    files::atomic_write(
        &f.manager.credential_operation_path(&id).unwrap(),
        &raw,
        0o600,
    )
    .unwrap();
    let before = f.counts();
    assert_eq!(
        f.confirm(&p).err().unwrap().code,
        "credential_legacy_source_changed"
    );
    assert!(f.journal(&id).is_none());
    assert_eq!(f.counts(), before);
    assert_eq!(f.bytes(&id), raw);
}
#[test]
fn explicit_confirmation_and_original_namespace_are_required() {
    let f = Fixture::new();
    let id = uuid::Uuid::new_v4().to_string();
    f.old(&id).unwrap();
    let p = f.preview(&id).unwrap();
    let before = f.counts();
    for foreign in ["confirmation", "namespace", "store"] {
        let mut input = Confirmation {
            store_id: p.store_id.clone(),
            namespace_id: p.namespace_id.clone(),
            operation_id: id.clone(),
            source_digest: p.source_digest.clone(),
            confirmed: true,
        };
        match foreign {
            "confirmation" => input.confirmed = false,
            "namespace" => input.namespace_id = uuid::Uuid::new_v4().to_string(),
            _ => input.store_id = uuid::Uuid::new_v4().to_string(),
        };
        assert!(
            f.manager
                .confirm_legacy_credential(&f.scope(), &input)
                .is_err()
        );
    }
    assert_eq!(f.counts(), before);
    assert!(f.journal(&id).is_none());
}
#[test]
fn an_existing_v3_operation_is_never_overwritten_or_adopted_as_migration() {
    let f = Fixture::new();
    let id = uuid::Uuid::new_v4().to_string();
    f.old(&id).unwrap();
    let p = f.preview(&id).unwrap();
    let saved = f
        .manager
        .create_credential_v3(
            &f.scope(),
            &f.namespace(),
            &id,
            "Fixture",
            Zeroizing::new(SECRET.into()),
        )
        .unwrap();
    let before = f.counts();
    let raw = f.bytes(&id);
    assert_eq!(f.confirm(&p).err().unwrap().code, "operation_conflict");
    assert_eq!(f.counts(), before);
    assert_eq!(f.bytes(&id), raw);
    assert_eq!(
        f.query(&id).unwrap().operation.receipt.unwrap(),
        saved.operation.receipt.unwrap()
    );
}
#[test]
fn ignored_intent_insert_cannot_fence_the_source_or_touch_vault() {
    let f = Fixture::new();
    let id = uuid::Uuid::new_v4().to_string();
    f.old(&id).unwrap();
    let p = f.preview(&id).unwrap();
    let raw = f.bytes(&id);
    f.store.connection.execute_batch("CREATE TRIGGER ignore_legacy_intent BEFORE INSERT ON credential_recovery_values WHEN NEW.domain=3 AND NEW.name GLOB 'operations/*' BEGIN SELECT RAISE(IGNORE);END;").unwrap();
    let before = f.counts();
    assert!(f.confirm(&p).is_err());
    assert_eq!(f.counts(), before);
    assert_eq!(f.bytes(&id), raw);
    assert!(f.journal(&id).is_none());
}
#[test]
fn a_fence_with_ignored_sql_phase_is_resumed_without_recreating_target() {
    let f = Fixture::new();
    let id = uuid::Uuid::new_v4().to_string();
    let receipt = f.old(&id).unwrap();
    let p = f.preview(&id).unwrap();
    f.store.connection.execute_batch("CREATE TRIGGER ignore_fence BEFORE UPDATE ON credential_recovery_values WHEN NEW.domain=3 AND CAST(NEW.value AS TEXT) LIKE '%\"stage\":\"fenced\"%' BEGIN SELECT RAISE(IGNORE);END;").unwrap();
    let before = f.counts();
    assert!(f.confirm(&p).is_err());
    assert_eq!(f.counts(), before);
    assert_eq!(
        f.journal(&id).unwrap()["legacyMigration"]["stage"],
        "prepared"
    );
    assert!(f.old(&id).is_err());
    f.store
        .connection
        .execute_batch("DROP TRIGGER ignore_fence")
        .unwrap();
    f.confirm(&p).unwrap();
    assert_eq!(
        f.query(&id)
            .unwrap()
            .operation
            .receipt
            .unwrap()
            .credential_ref,
        receipt.credential_ref
    );
}
#[test]
fn ignored_finalization_preserves_fenced_intent_and_reuses_tracked_auxiliary_key() {
    let f = Fixture::new();
    let id = uuid::Uuid::new_v4().to_string();
    let receipt = f.old(&id).unwrap();
    let p = f.preview(&id).unwrap();
    f.store.connection.execute_batch("CREATE TRIGGER ignore_claim_commit BEFORE UPDATE ON credential_recovery_values WHEN NEW.domain=3 AND CAST(NEW.value AS TEXT) LIKE '%\"stage\":\"committed\"%' BEGIN SELECT RAISE(IGNORE);END;").unwrap();
    assert!(f.confirm(&p).is_err());
    let sets = f.vault.sets.load(Ordering::SeqCst);
    assert_eq!(
        f.query(&id).err().unwrap().code,
        "credential_migration_pending"
    );
    f.store
        .connection
        .execute_batch("DROP TRIGGER ignore_claim_commit")
        .unwrap();
    f.confirm(&p).unwrap();
    assert_eq!(f.vault.sets.load(Ordering::SeqCst), sets);
    assert_eq!(
        f.query(&id)
            .unwrap()
            .operation
            .receipt
            .unwrap()
            .credential_ref,
        receipt.credential_ref
    );
}
#[test]
fn missing_old_auxiliary_key_retains_resumable_intent_and_fence() {
    let f = Fixture::new();
    let id = uuid::Uuid::new_v4().to_string();
    let receipt = f.old(&id).unwrap();
    let p = f.preview(&id).unwrap();
    let key: ProtectionKey = serde_json::from_slice(
        &fs::read(f.manager.directory.join("credential-operation-key-v1.json")).unwrap(),
    )
    .unwrap();
    let raw = f.vault.inner.get(&key.reference).unwrap();
    f.vault.inner.delete(&key.reference).unwrap();
    let before = f.vault.sets.load(Ordering::SeqCst);
    assert_eq!(
        f.confirm(&p).err().unwrap().code,
        "credential_migration_pending"
    );
    assert_eq!(f.vault.sets.load(Ordering::SeqCst), before);
    let resumed = f.preview(&id).unwrap();
    assert!(resumed.stage == Stage::Fenced);
    assert_eq!(resumed.source_digest, p.source_digest);
    f.vault.inner.set(&key.reference, &raw).unwrap();
    f.confirm(&resumed).unwrap();
    assert_eq!(
        f.query(&id)
            .unwrap()
            .operation
            .receipt
            .unwrap()
            .credential_ref,
        receipt.credential_ref
    );
}
#[test]
fn whole_provider_directory_loss_after_fencing_keeps_sql_evidence_and_original_reference() {
    let f = Fixture::new();
    let id = uuid::Uuid::new_v4().to_string();
    let receipt = f.old(&id).unwrap();
    let p = f.preview(&id).unwrap();
    f.store.connection.execute_batch("CREATE TRIGGER hold_claim BEFORE UPDATE ON credential_recovery_values WHEN CAST(NEW.value AS TEXT) LIKE '%\"stage\":\"fenced\"%' BEGIN SELECT RAISE(IGNORE);END;").unwrap();
    assert!(f.confirm(&p).is_err());
    let moved = f.manager.directory.with_extension("moved-fixture");
    fs::rename(&f.manager.directory, &moved).unwrap();
    f.store
        .connection
        .execute_batch("DROP TRIGGER hold_claim")
        .unwrap();
    f.confirm(&p).unwrap();
    assert_eq!(
        f.query(&id)
            .unwrap()
            .operation
            .receipt
            .unwrap()
            .credential_ref,
        receipt.credential_ref
    );
    assert!(f.old(&id).is_err());
    assert!(moved.exists());
}
#[test]
fn evidence_or_identity_tampering_is_rejected_even_when_the_value_hash_is_recomputed() {
    let f = Fixture::new();
    let id = uuid::Uuid::new_v4().to_string();
    f.old(&id).unwrap();
    let p = f.preview(&id).unwrap();
    f.confirm(&p).unwrap();
    let mut changed = f.journal(&id).unwrap();
    changed["legacyMigration"]["evidence"]["operation"] =
        serde_json::json!("{\"state\":\"cancelled\"}");
    let bytes = serde_json::to_vec(&changed).unwrap();
    f.store.connection.execute("UPDATE credential_recovery_values SET value=?1,value_hash=?2 WHERE domain=3 AND name=?3",params![bytes,files::hash(&bytes),format!("operations/{id}.json")]).unwrap();
    let before = f.counts();
    assert!(f.query(&id).is_err());
    assert!(f.preview(&id).is_err());
    assert_eq!(f.counts(), before);
}
#[test]
fn malformed_or_missing_material_does_not_become_a_new_operation() {
    let f = Fixture::new();
    let id = uuid::Uuid::new_v4().to_string();
    let before = f.counts();
    assert!(f.preview(&id).is_err());
    assert_eq!(f.counts(), before);
    assert!(f.journal(&id).is_none());
    f.manager.cancel_credential_operation(&id).unwrap();
    files::atomic_write(
        &f.manager.credential_operation_path(&id).unwrap(),
        b"{broken",
        0o600,
    )
    .unwrap();
    assert!(f.preview(&id).is_err());
    assert_eq!(f.counts(), before);
}
#[test]
fn exact_material_and_cancelled_result_survive_store_reopen() {
    let root = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
    let data = root.path().join("data");
    let store = Store::open(&data).unwrap();
    let vault = Arc::new(FaultVault::default());
    let manager = ProviderManager::new(
        root.path().join("home"),
        &data,
        root.path().join("helper"),
        vault.clone(),
    )
    .unwrap();
    let id = uuid::Uuid::new_v4().to_string();
    manager.cancel_credential_operation(&id).unwrap();
    let store_id = store.database_id().unwrap();
    let scope = store.credential_scope(&store_id).unwrap();
    let namespace = manager
        .scoped_credential_protocol_v3(&scope)
        .unwrap()
        .namespace_id;
    let p = manager
        .preview_legacy_credential(&scope, &namespace, &id)
        .unwrap();
    manager
        .confirm_legacy_credential(
            &scope,
            &Confirmation {
                store_id: store_id.clone(),
                namespace_id: namespace.clone(),
                operation_id: id.clone(),
                source_digest: p.source_digest.clone(),
                confirmed: true,
            },
        )
        .unwrap();
    drop(scope);
    drop(store);
    let reopened = Store::open(&data).unwrap();
    let scope = reopened.credential_scope(&store_id).unwrap();
    assert!(
        manager
            .preview_legacy_credential(&scope, &namespace, &id)
            .unwrap()
            .stage
            == Stage::Committed
    );
    assert!(
        manager
            .credential_operation_v3(&scope, &namespace, &id, false)
            .unwrap()
            .operation
            .status
            == Status::Cancelled
    );
    assert_eq!(vault.sets.load(Ordering::SeqCst), 0);
}

#[test]
fn oversized_exact_material_is_rejected_before_intent_or_fence() {
    let f = Fixture::new();
    let id = uuid::Uuid::new_v4().to_string();
    f.old(&id).unwrap();
    let mut raw = f.bytes(&id);
    raw.extend(std::iter::repeat_n(
        b' ',
        3500usize.saturating_sub(raw.len()),
    ));
    files::atomic_write(
        &f.manager.credential_operation_path(&id).unwrap(),
        &raw,
        0o600,
    )
    .unwrap();
    let before = f.counts();
    assert_eq!(
        f.preview(&id).err().unwrap().code,
        "credential_migration_capacity"
    );
    assert!(f.journal(&id).is_none());
    assert_eq!(f.bytes(&id), raw);
    assert_eq!(f.counts(), before);
}
#[test]
fn migrated_ready_with_missing_target_does_not_recreate_the_removed_credential() {
    let f = Fixture::new();
    let id = uuid::Uuid::new_v4().to_string();
    let receipt = f.old(&id).unwrap();
    let p = f.preview(&id).unwrap();
    f.vault.inner.delete(&receipt.credential_ref).unwrap();
    let target_writes = f.vault.target_sets.load(Ordering::SeqCst);
    f.confirm(&p).unwrap();
    assert!(f.query(&id).unwrap().operation.status == Status::Removed);
    assert_eq!(
        f.manager
            .create_credential_v3(
                &f.scope(),
                &f.namespace(),
                &id,
                "Fixture",
                Zeroizing::new(SECRET.into())
            )
            .err()
            .unwrap()
            .code,
        "credential_removed"
    );
    assert_eq!(f.vault.target_sets.load(Ordering::SeqCst), target_writes);
}
#[test]
fn committed_migration_cannot_be_relabelled_as_another_store_or_namespace() {
    let a = Fixture::new();
    let b = Fixture::new();
    let id = uuid::Uuid::new_v4().to_string();
    a.old(&id).unwrap();
    a.confirm(&a.preview(&id).unwrap()).unwrap();
    let mut value = a.journal(&id).unwrap();
    value["storeId"] = serde_json::json!(b.store.database_id().unwrap());
    value["namespaceId"] = serde_json::json!(b.namespace());
    let bytes = serde_json::to_vec(&value).unwrap();
    b.store
        .connection
        .execute(
            "INSERT INTO credential_recovery_values VALUES(3,?1,?2,?3,2)",
            params![format!("operations/{id}.json"), bytes, files::hash(&bytes)],
        )
        .unwrap();
    b.store
        .connection
        .execute(
            "UPDATE credential_recovery_scope SET value_count=1,revision=2",
            [],
        )
        .unwrap();
    let before = b.counts();
    assert!(b.query(&id).is_err());
    assert!(b.preview(&id).is_err());
    assert_eq!(b.counts(), before);
}

fn assert_old_entrypoints_fenced(f: &Fixture, id: &str) {
    let before = f.counts();
    for action in 0..3 {
        let code = match action {
            0 => f.old(id).err().unwrap().code,
            1 => f.manager.credential_operation(id).err().unwrap().code,
            _ => {
                f.manager
                    .cancel_credential_operation(id)
                    .err()
                    .unwrap()
                    .code
            }
        };
        assert_eq!(code, "credential_legacy_migrated");
    }
    assert_eq!(f.counts(), before);
}
#[test]
fn durable_admission_blocks_all_old_entrypoints_after_whole_directory_recreation() {
    let f = Fixture::new();
    let id = uuid::Uuid::new_v4().to_string();
    let original = f.old(&id).unwrap();
    f.confirm(&f.preview(&id).unwrap()).unwrap();
    let value = f.journal(&id).unwrap();
    fs::rename(
        &f.manager.directory,
        f.manager.directory.with_extension("held-original"),
    )
    .unwrap();
    files::private_dir(&f.manager.directory).unwrap();
    assert_old_entrypoints_fenced(&f, &id);
    assert!(!f.manager.credential_operation_path(&id).unwrap().exists());
    assert_eq!(f.journal(&id).unwrap(), value);
    assert_eq!(
        f.query(&id)
            .unwrap()
            .operation
            .receipt
            .unwrap()
            .credential_ref,
        original.credential_ref
    );
}
#[test]
fn durable_admission_blocks_all_old_entrypoints_after_only_fence_file_loss() {
    let f = Fixture::new();
    let id = uuid::Uuid::new_v4().to_string();
    f.old(&id).unwrap();
    f.confirm(&f.preview(&id).unwrap()).unwrap();
    fs::remove_file(f.manager.credential_operation_path(&id).unwrap()).unwrap();
    assert_old_entrypoints_fenced(&f, &id);
    assert!(!f.manager.credential_operation_path(&id).unwrap().exists());
}
#[test]
fn a_fresh_manager_uses_main_store_evidence_without_a_source_directory() {
    let f = Fixture::new();
    let id = uuid::Uuid::new_v4().to_string();
    f.old(&id).unwrap();
    f.confirm(&f.preview(&id).unwrap()).unwrap();
    fs::rename(
        &f.manager.directory,
        f.manager.directory.with_extension("held-fresh"),
    )
    .unwrap();
    let manager = ProviderManager::new(
        f._root.path().join("home"),
        &f._root.path().join("data"),
        f._root.path().join("helper"),
        f.vault.clone(),
    )
    .unwrap();
    let before = f.counts();
    assert_eq!(
        manager
            .create_credential_operation(&id, "Fixture", Zeroizing::new(SECRET.into()))
            .err()
            .unwrap()
            .code,
        "credential_legacy_migrated"
    );
    assert_eq!(
        manager.credential_operation(&id).err().unwrap().code,
        "credential_legacy_migrated"
    );
    assert_eq!(
        manager.cancel_credential_operation(&id).err().unwrap().code,
        "credential_legacy_migrated"
    );
    assert_eq!(f.counts(), before);
}
#[test]
fn managed_binding_precedes_the_first_legacy_request_and_survives_main_loss() {
    let f = Fixture::new();
    let id = uuid::Uuid::new_v4().to_string();
    f.old(&id).unwrap();
    f.confirm(&f.preview(&id).unwrap()).unwrap();
    fs::rename(
        &f.manager.directory,
        f.manager.directory.with_extension("held-startup"),
    )
    .unwrap();
    let manager = ProviderManager::new(
        f._root.path().join("home"),
        &f._root.path().join("data"),
        f._root.path().join("helper"),
        f.vault.clone(),
    )
    .unwrap();
    let before = f.counts();
    manager.bind_legacy_credential_source(&f.scope()).unwrap();
    assert_eq!(f.counts(), before);
    let db = f._root.path().join("data/local-agent.sqlite3");
    fs::rename(&db, db.with_extension("held-startup")).unwrap();
    assert!(
        manager
            .create_credential_operation(&id, "Fixture", Zeroizing::new(SECRET.into()))
            .is_err()
    );
    assert!(manager.credential_operation(&id).is_err());
    assert!(manager.cancel_credential_operation(&id).is_err());
    assert_eq!(f.counts(), before);
    assert!(!manager.credential_operation_path(&id).unwrap().exists());
}
#[test]
fn prepared_intent_fences_legacy_even_if_the_old_file_is_restored() {
    let f = Fixture::new();
    let id = uuid::Uuid::new_v4().to_string();
    f.old(&id).unwrap();
    let raw = f.bytes(&id);
    let p = f.preview(&id).unwrap();
    f.store.connection.execute_batch("CREATE TRIGGER hold_prepared BEFORE UPDATE ON credential_recovery_values WHEN CAST(NEW.value AS TEXT) LIKE '%\"stage\":\"fenced\"%' BEGIN SELECT RAISE(IGNORE);END;").unwrap();
    assert!(f.confirm(&p).is_err());
    assert_eq!(
        f.journal(&id).unwrap()["legacyMigration"]["stage"],
        "prepared"
    );
    files::atomic_write(
        &f.manager.credential_operation_path(&id).unwrap(),
        &raw,
        0o600,
    )
    .unwrap();
    assert_old_entrypoints_fenced(&f, &id);
    assert_eq!(f.bytes(&id), raw);
}
#[test]
fn bound_main_file_loss_cannot_downgrade_to_unfenced_files() {
    let f = Fixture::new();
    let id = uuid::Uuid::new_v4().to_string();
    f.old(&id).unwrap();
    let raw = f.bytes(&id);
    fs::remove_file(f._root.path().join("data/local-agent.sqlite3")).unwrap();
    let before = f.counts();
    assert!(f.old(&id).is_err());
    assert!(f.manager.credential_operation(&id).is_err());
    assert!(f.manager.cancel_credential_operation(&id).is_err());
    assert_eq!(f.counts(), before);
    assert_eq!(f.bytes(&id), raw);
}
#[test]
fn same_identity_database_file_replacement_is_not_a_new_binding() {
    let f = Fixture::new();
    let id = uuid::Uuid::new_v4().to_string();
    f.old(&id).unwrap();
    let replacement = f._root.path().join("data/replacement.sqlite3");
    f.store
        .connection
        .execute("VACUUM INTO ?1", [replacement.to_string_lossy().as_ref()])
        .unwrap();
    fs::rename(
        &replacement,
        f._root.path().join("data/local-agent.sqlite3"),
    )
    .unwrap();
    let before = f.counts();
    assert!(f.old(&id).is_err());
    assert!(f.manager.credential_operation(&id).is_err());
    assert_eq!(f.counts(), before);
}
#[test]
fn corrupt_or_missing_main_evidence_never_falls_back_to_a_legacy_file() {
    for damage in [
        "UPDATE credential_recovery_scope SET format=2",
        "DROP TABLE credential_recovery_values",
        "DELETE FROM credential_recovery_scope",
    ] {
        let f = Fixture::new();
        let id = uuid::Uuid::new_v4().to_string();
        f.old(&id).unwrap();
        let raw = f.bytes(&id);
        f.store.connection.execute_batch(damage).unwrap();
        let before = f.counts();
        assert!(f.old(&id).is_err(), "{damage}");
        assert!(f.manager.credential_operation(&id).is_err(), "{damage}");
        assert!(
            f.manager.cancel_credential_operation(&id).is_err(),
            "{damage}"
        );
        assert_eq!(f.counts(), before);
        assert_eq!(f.bytes(&id), raw);
    }
}
#[test]
fn corrupt_same_id_scoped_entry_cannot_be_inferred_to_be_unclaimed() {
    let f = Fixture::new();
    let id = uuid::Uuid::new_v4().to_string();
    f.old(&id).unwrap();
    f.confirm(&f.preview(&id).unwrap()).unwrap();
    let broken = b"{broken scoped claim";
    f.store.connection.execute("UPDATE credential_recovery_values SET value=?1,value_hash=?2 WHERE domain=3 AND name=?3",params![broken.as_slice(),files::hash(broken),format!("operations/{id}.json")]).unwrap();
    let before = f.counts();
    assert!(f.old(&id).is_err());
    assert!(f.manager.credential_operation(&id).is_err());
    assert_eq!(f.counts(), before);
}
#[test]
fn known_unverified_namespace_allows_unclaimed_legacy_without_auto_activation() {
    let f = Fixture::new();
    f.store
        .connection
        .execute(
            "UPDATE credential_recovery_scope SET state='unverified',legacy_state='unverified'",
            [],
        )
        .unwrap();
    let policy = f.store.legacy_credential_policy().unwrap();
    assert_eq!(policy.schema_version, Store::SCHEMA_VERSION);
    let id = uuid::Uuid::new_v4().to_string();
    let receipt = f.old(&id).unwrap();
    assert_eq!(
        f.manager.credential_operation(&id).unwrap().receipt,
        Some(receipt)
    );
    assert!(matches!(
        f.manager
            .scoped_credential_protocol_v3(&f.scope())
            .unwrap()
            .namespace_state,
        crate::providers::credential_scope::NamespaceState::Unverified
    ));
    assert!(f.journal(&id).is_none());
}
#[test]
fn lock_contention_cannot_enter_vault_and_releases_a_partially_acquired_guard() {
    let f = Fixture::new();
    let id = uuid::Uuid::new_v4().to_string();
    f.old(&id).unwrap();
    let scope = f.scope();
    let held = scope.journal.lock().unwrap();
    let before = f.counts();
    assert_eq!(f.old(&id).err().unwrap().code, "busy");
    assert_eq!(f.counts(), before);
    drop(held);
    let old_lock = f.manager.provider_edit_guard().unwrap();
    assert_eq!(f.old(&id).err().unwrap().code, "busy");
    assert_eq!(f.counts(), before);
    let root_again = scope.journal.lock().unwrap();
    drop(root_again);
    drop(old_lock);
    assert!(f.manager.credential_operation(&id).is_ok());
}
#[test]
fn legacy_policy_is_exact_read_only_and_does_not_claim_old_material() {
    let f = Fixture::new();
    let before = f.counts();
    let value = serde_json::to_value(f.store.legacy_credential_policy().unwrap()).unwrap();
    assert_eq!(
        value,
        serde_json::json!({"version":2,"storeId":f.store.database_id().unwrap(),"schemaVersion":Store::SCHEMA_VERSION})
    );
    assert_eq!(f.counts(), before);
    assert_eq!(
        f.store
            .connection
            .query_row("SELECT count(*) FROM credential_recovery_values", [], |r| r
                .get::<_, u32>(0))
            .unwrap(),
        0
    );
}

fn assert_unclaimed_material_unknown(f: &Fixture, id: &str) {
    let before = f.counts();
    assert_eq!(f.old(id).err().unwrap().code, "credential_result_unknown");
    assert_eq!(
        f.manager.credential_operation(id).err().unwrap().code,
        "credential_result_unknown"
    );
    assert_eq!(
        f.manager
            .cancel_credential_operation(id)
            .err()
            .unwrap()
            .code,
        "credential_result_unknown"
    );
    assert_eq!(f.counts(), before);
    assert!(!f.manager.credential_operation_path(id).unwrap().exists());
}

#[test]
fn write_admission_file_loss_cannot_create_or_cancel_an_unclaimed_target_and_exact_restore_works() {
    let f = Fixture::new();
    let id = uuid::Uuid::new_v4().to_string();
    let receipt = f.old(&id).unwrap();
    let original = f.bytes(&id);
    let before = f.vault.target_sets.load(Ordering::SeqCst);
    fs::remove_file(f.manager.credential_operation_path(&id).unwrap()).unwrap();
    assert_unclaimed_material_unknown(&f, &id);
    files::atomic_write(
        &f.manager.credential_operation_path(&id).unwrap(),
        &original,
        0o600,
    )
    .unwrap();
    assert_eq!(f.old(&id).unwrap(), receipt);
    assert_eq!(
        f.manager.credential_operation(&id).unwrap().receipt,
        Some(receipt)
    );
    assert_eq!(f.vault.target_sets.load(Ordering::SeqCst), before);
}

#[test]
fn write_admission_whole_directory_loss_stays_unknown_and_discoverable_after_reopen() {
    let f = Fixture::new();
    let id = uuid::Uuid::new_v4().to_string();
    f.old(&id).unwrap();
    fs::rename(
        &f.manager.directory,
        f.manager.directory.with_extension("held-unclaimed"),
    )
    .unwrap();
    files::private_dir(&f.manager.directory).unwrap();
    assert_unclaimed_material_unknown(&f, &id);
    let store = Store::open(&f._root.path().join("data")).unwrap();
    let manager = ProviderManager::new(
        f._root.path().join("home"),
        &f._root.path().join("data"),
        f._root.path().join("helper"),
        f.vault.clone(),
    )
    .unwrap();
    let scope = store
        .credential_scope(&store.database_id().unwrap())
        .unwrap();
    manager.bind_legacy_credential_source(&scope).unwrap();
    assert_eq!(
        manager.credential_operation(&id).err().unwrap().code,
        "credential_result_unknown"
    );
    let p = manager.scoped_credential_protocol_v3(&scope).unwrap();
    let before = f.counts();
    let page = manager
        .discover_credentials(
            &scope,
            &super::super::discovery::Request {
                store_id: p.store_id,
                namespace_id: p.namespace_id,
                cursor: None,
            },
        )
        .unwrap();
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].operation_id, id);
    assert!(matches!(
        page.items[0].issue,
        Some(super::super::discovery::Issue::Unavailable)
    ));
    assert_eq!(f.counts(), before);
}

#[test]
fn write_admission_fresh_query_does_not_reserve_but_cancellation_cannot_be_recreated_after_loss() {
    let f = Fixture::new();
    let id = uuid::Uuid::new_v4().to_string();
    assert_eq!(
        f.manager.credential_operation(&id).err().unwrap().code,
        "operation_not_found"
    );
    assert_eq!(
        f.store
            .connection
            .query_row(
                "SELECT operation_count FROM credential_legacy_scope",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
    assert_eq!(
        f.manager.cancel_credential_operation(&id).unwrap().status,
        Status::Cancelled
    );
    fs::remove_file(f.manager.credential_operation_path(&id).unwrap()).unwrap();
    assert_unclaimed_material_unknown(&f, &id);
    assert_eq!(f.counts(), (0, 0));
}

#[test]
fn write_admission_ignored_reservation_count_or_revision_rolls_back_before_any_vault_call() {
    for trigger in [
        "CREATE TRIGGER reject_admission BEFORE INSERT ON credential_legacy_operations BEGIN SELECT RAISE(IGNORE);END;",
        "CREATE TRIGGER reject_admission BEFORE UPDATE ON credential_legacy_scope BEGIN SELECT RAISE(IGNORE);END;",
        "CREATE TRIGGER reject_admission BEFORE UPDATE ON credential_recovery_scope BEGIN SELECT RAISE(IGNORE);END;",
    ] {
        let f = Fixture::new();
        let id = uuid::Uuid::new_v4().to_string();
        let revision: i64 = f
            .store
            .connection
            .query_row("SELECT revision FROM credential_recovery_scope", [], |r| {
                r.get(0)
            })
            .unwrap();
        f.store.connection.execute_batch(trigger).unwrap();
        assert!(f.old(&id).is_err());
        assert_eq!(f.counts(), (0, 0));
        assert_eq!(
            f.store
                .connection
                .query_row(
                    "SELECT count(*) FROM credential_legacy_operations",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
        assert_eq!(
            f.store
                .connection
                .query_row("SELECT revision FROM credential_recovery_scope", [], |r| {
                    r.get::<_, i64>(0)
                })
                .unwrap(),
            revision
        );
        f.store
            .connection
            .execute_batch("DROP TRIGGER reject_admission")
            .unwrap();
        assert!(f.old(&id).is_ok());
    }
}

#[test]
fn write_admission_corrupt_metadata_or_missing_id_is_not_reinitialized_or_sent_to_vault() {
    for damage in [
        "DELETE FROM credential_legacy_scope",
        "UPDATE credential_legacy_scope SET format=2",
        "DELETE FROM credential_legacy_operations",
        "DROP TABLE credential_legacy_operations",
    ] {
        let f = Fixture::new();
        let id = uuid::Uuid::new_v4().to_string();
        f.old(&id).unwrap();
        f.store.connection.execute_batch(damage).unwrap();
        let before = f.counts();
        assert!(f.old(&id).is_err());
        assert!(f.manager.credential_operation(&id).is_err());
        assert!(f.manager.cancel_credential_operation(&id).is_err());
        assert!(f.store.legacy_credential_policy().is_err());
        assert_eq!(f.counts(), before);
    }
}

#[test]
fn write_admission_upgrade_11_keeps_existing_material_and_blocks_unseen_historical_ids() {
    let f = Fixture::new();
    let id = uuid::Uuid::new_v4().to_string();
    let receipt = f.old(&id).unwrap();
    let bytes = f.bytes(&id);
    f.store.connection.execute_batch("DROP TABLE local_token_cancellations;DROP TABLE local_token_owners;DROP TABLE local_token_values;DROP TABLE local_token_scope;DROP TABLE native_import_operations;DROP TABLE native_import_scope;DROP TABLE credential_legacy_operations;DROP TABLE credential_legacy_scope;PRAGMA user_version=11").unwrap();
    let upgraded = Store::open(&f._root.path().join("data")).unwrap();
    let scope = upgraded
        .credential_scope(&upgraded.database_id().unwrap())
        .unwrap();
    f.manager.bind_legacy_credential_source(&scope).unwrap();
    assert_eq!(
        upgraded
            .connection
            .query_row("SELECT allow_new FROM credential_legacy_scope", [], |r| r
                .get::<_, i64>(
                0
            ))
            .unwrap(),
        0
    );
    assert_eq!(f.old(&id).unwrap(), receipt);
    assert_eq!(f.bytes(&id), bytes);
    assert_unclaimed_material_unknown(&f, &uuid::Uuid::new_v4().to_string());
    fs::remove_file(f.manager.credential_operation_path(&id).unwrap()).unwrap();
    assert_unclaimed_material_unknown(&f, &id);
    let backup = f
        ._root
        .path()
        .join("data/local-agent.pre-legacy-write-v11.sqlite3");
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(
        fs::metadata(&backup).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let db =
        rusqlite::Connection::open_with_flags(backup, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    assert_eq!(
        db.pragma_query_value(None, "user_version", |r| r.get::<_, u32>(0))
            .unwrap(),
        11
    );
}

#[test]
fn write_admission_capacity_keeps_existing_operations_usable_without_more_vault_writes() {
    let f = Fixture::new();
    let id = uuid::Uuid::new_v4().to_string();
    let receipt = f.old(&id).unwrap();
    let tx = f.store.connection.unchecked_transaction().unwrap();
    for _ in 1..4096 {
        tx.execute(
            "INSERT INTO credential_legacy_operations VALUES(?1)",
            [uuid::Uuid::new_v4().to_string()],
        )
        .unwrap();
    }
    tx.execute(
        "UPDATE credential_legacy_scope SET operation_count=4096",
        [],
    )
    .unwrap();
    tx.commit().unwrap();
    let before = f.counts();
    assert_eq!(
        f.old(&uuid::Uuid::new_v4().to_string()).err().unwrap().code,
        "credential_operation_capacity"
    );
    assert_eq!(f.counts(), before);
    assert_eq!(f.old(&id).unwrap(), receipt);
}

#[test]
fn write_admission_stable_reference_does_not_duplicate_a_target_after_an_older_same_source_snapshot()
 {
    let f = Fixture::new();
    let id = uuid::Uuid::new_v4().to_string();
    let original = f.old(&id).unwrap();
    let target_sets = f.vault.target_sets.load(Ordering::SeqCst);
    // A prior same-source snapshot lacks this admission row. Existing target
    // credentials still cannot be duplicated or overwritten under this ID.
    f.store.connection.execute_batch("DELETE FROM credential_legacy_operations;UPDATE credential_legacy_scope SET operation_count=0").unwrap();
    fs::rename(
        &f.manager.directory,
        f.manager.directory.with_extension("held-snapshot"),
    )
    .unwrap();
    files::private_dir(&f.manager.directory).unwrap();
    assert_eq!(f.old(&id).unwrap(), original);
    assert_eq!(f.vault.target_sets.load(Ordering::SeqCst), target_sets);
}

#[test]
fn write_admission_ambiguous_auxiliary_write_keeps_a_missing_intent_unknown_before_retry() {
    let f = Fixture::new();
    let id = uuid::Uuid::new_v4().to_string();
    f.vault.fail_aux_reply.store(true, Ordering::SeqCst);
    assert!(f.old(&id).is_err());
    assert_eq!(f.vault.target_sets.load(Ordering::SeqCst), 0);
    assert_eq!(
        f.store
            .connection
            .query_row(
                "SELECT operation_count FROM credential_legacy_scope",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        1
    );
    assert_unclaimed_material_unknown(&f, &id);
}
#[test]
fn schema10_reader_barrier_upgrade_preserves_claim_bytes_identity_and_private_backup() {
    let root = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
    let data = root.path().join("data");
    let store = Store::open(&data).unwrap();
    let vault = Arc::new(FaultVault::default());
    let manager = ProviderManager::new(
        root.path().join("home"),
        &data,
        root.path().join("helper"),
        vault.clone(),
    )
    .unwrap();
    let id = uuid::Uuid::new_v4().to_string();
    let receipt = manager
        .create_credential_operation(&id, "Fixture", Zeroizing::new(SECRET.into()))
        .unwrap();
    let store_id = store.database_id().unwrap();
    let scope = store.credential_scope(&store_id).unwrap();
    let ns = manager
        .scoped_credential_protocol_v3(&scope)
        .unwrap()
        .namespace_id;
    let p = manager.preview_legacy_credential(&scope, &ns, &id).unwrap();
    manager
        .confirm_legacy_credential(
            &scope,
            &Confirmation {
                store_id: store_id.clone(),
                namespace_id: ns.clone(),
                operation_id: id.clone(),
                source_digest: p.source_digest,
                confirmed: true,
            },
        )
        .unwrap();
    let bytes: Vec<u8> = store
        .connection
        .query_row(
            "SELECT value FROM credential_recovery_values WHERE domain=3 AND name=?",
            [format!("operations/{id}.json")],
            |r| r.get(0),
        )
        .unwrap();
    store
        .connection
        .execute_batch(
            "DROP TABLE local_token_cancellations;DROP TABLE local_token_owners;DROP TABLE local_token_values;DROP TABLE local_token_scope;DROP TABLE native_import_operations;DROP TABLE native_import_scope;DROP TABLE credential_legacy_operations;DROP TABLE credential_legacy_scope;",
        )
        .unwrap();
    store
        .connection
        .pragma_update(None, "user_version", 10)
        .unwrap();
    drop(scope);
    drop(store);
    let before = vault.sets.load(Ordering::SeqCst);
    let upgraded = Store::open(&data).unwrap();
    assert_eq!(upgraded.database_id().unwrap(), store_id);
    assert_eq!(
        upgraded
            .connection
            .query_row("PRAGMA user_version", [], |r| r.get::<_, u32>(0))
            .unwrap(),
        Store::SCHEMA_VERSION
    );
    assert_eq!(
        upgraded
            .connection
            .query_row(
                "SELECT value FROM credential_recovery_values WHERE domain=3 AND name=?",
                [format!("operations/{id}.json")],
                |r| r.get::<_, Vec<u8>>(0)
            )
            .unwrap(),
        bytes
    );
    let backup = data.join("local-agent.pre-legacy-admission-v10.sqlite3");
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(
        fs::metadata(&backup).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let copy =
        rusqlite::Connection::open_with_flags(&backup, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    assert_eq!(
        copy.query_row("PRAGMA user_version", [], |r| r.get::<_, u32>(0))
            .unwrap(),
        10
    );
    assert_eq!(
        copy.query_row(
            "SELECT value FROM credential_recovery_values WHERE domain=3 AND name=?",
            [format!("operations/{id}.json")],
            |r| r.get::<_, Vec<u8>>(0)
        )
        .unwrap(),
        bytes
    );
    let scope = upgraded.credential_scope(&store_id).unwrap();
    assert_eq!(
        manager
            .credential_operation_v3(&scope, &ns, &id, false)
            .unwrap()
            .operation
            .receipt
            .unwrap()
            .credential_ref,
        receipt.credential_ref
    );
    assert_eq!(vault.sets.load(Ordering::SeqCst), before);
    assert_eq!(
        manager.credential_operation(&id).err().unwrap().code,
        "credential_legacy_migrated"
    );
}
#[test]
fn broken_schema10_recovery_metadata_is_not_promoted_or_repaired() {
    for damage in [
        "UPDATE credential_recovery_scope SET format=2",
        "DELETE FROM credential_recovery_scope",
        "DROP TABLE credential_recovery_values",
    ] {
        let root = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
        let data = root.path().join("data");
        let store = Store::open(&data).unwrap();
        store
            .connection
            .execute_batch(
                "DROP TABLE local_token_cancellations;DROP TABLE local_token_owners;DROP TABLE local_token_values;DROP TABLE local_token_scope;DROP TABLE native_import_operations;DROP TABLE native_import_scope;DROP TABLE credential_legacy_operations;DROP TABLE credential_legacy_scope;",
            )
            .unwrap();
        store
            .connection
            .pragma_update(None, "user_version", 10)
            .unwrap();
        store.connection.execute_batch(damage).unwrap();
        drop(store);
        assert!(Store::open(&data).is_err());
        let db = rusqlite::Connection::open(data.join("local-agent.sqlite3")).unwrap();
        assert_eq!(
            db.query_row("PRAGMA user_version", [], |r| r.get::<_, u32>(0))
                .unwrap(),
            10
        );
    }
}
