use super::*;
use crate::providers::vault::{MemoryVault, Vault};
use std::{
    os::unix::fs::PermissionsExt,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU8, Ordering},
    },
};

const SECRET: &str = "credential-operation-private-fixture-718280";
#[derive(Default)]
struct FaultVault {
    values: MemoryVault,
    fault: AtomicU8,
    receipt_failure: Mutex<Option<std::path::PathBuf>>,
}
impl Vault for FaultVault {
    fn get(&self, account: &str) -> Result<Zeroizing<Vec<u8>>, ConfigError> {
        self.values.get(account)
    }
    fn set(&self, account: &str, value: &[u8]) -> Result<(), ConfigError> {
        let mode = self.fault.swap(0, Ordering::SeqCst);
        if mode != 1 {
            self.values.set(account, value)?;
            if let Some(path) = self.receipt_failure.lock().unwrap().take() {
                std::fs::rename(&path, path.with_extension("before-receipt")).unwrap();
                std::fs::create_dir(&path).unwrap();
            }
        }
        if mode == 0 {
            Ok(())
        } else {
            fail("credential_unavailable", "fixture ambiguous Vault reply")
        }
    }
    fn delete(&self, account: &str) -> Result<(), ConfigError> {
        self.values.delete(account)
    }
}
struct Fixture {
    root: tempfile::TempDir,
    vault: Arc<FaultVault>,
    manager: ProviderManager,
}
impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
        std::fs::create_dir(root.path().join("home")).unwrap();
        let vault = Arc::new(FaultVault::default());
        let manager = ProviderManager::new(
            root.path().join("home"),
            &root.path().join("data"),
            root.path().join("unused-fixture-helper"),
            vault.clone(),
        )
        .unwrap();
        manager.credential_operation_key(true).unwrap();
        Self {
            root,
            vault,
            manager,
        }
    }
    fn create(&self, id: &str) -> Result<Receipt, ConfigError> {
        self.manager.create_credential_operation(
            id,
            "Fixture API Key",
            Zeroizing::new(SECRET.into()),
        )
    }
    fn reopen(&self) -> ProviderManager {
        ProviderManager::new(
            self.root.path().join("home"),
            &self.root.path().join("data"),
            self.root.path().join("unused-fixture-helper"),
            self.vault.clone(),
        )
        .unwrap()
    }
    fn target_count(&self) -> usize {
        self.vault
            .values
            .0
            .lock()
            .unwrap()
            .values()
            .filter(|v| v.as_slice() == SECRET.as_bytes())
            .count()
    }
}

#[test]
fn retries_reopen_and_conflicts_keep_one_credential_without_persisting_plaintext() {
    let fixture = Fixture::new();
    let id = uuid::Uuid::new_v4().to_string();
    let saved = fixture.create(&id).unwrap();
    assert_eq!(fixture.create(&id).unwrap(), saved);
    assert_eq!(
        fixture.reopen().credential_operation(&id).unwrap().receipt,
        Some(saved.clone())
    );
    assert_eq!(fixture.target_count(), 1);
    assert_eq!(
        fixture
            .manager
            .create_credential_operation(&id, "Another label", Zeroizing::new(SECRET.into()))
            .unwrap_err()
            .code,
        "operation_conflict"
    );
    assert_eq!(
        fixture
            .manager
            .create_credential_operation(
                &id,
                "Fixture API Key",
                Zeroizing::new("other-fixture-value".into())
            )
            .unwrap_err()
            .code,
        "operation_conflict"
    );
    let path = fixture.manager.credential_operation_path(&id).unwrap();
    let bytes = std::fs::read(&path).unwrap();
    assert!(!bytes.windows(SECRET.len()).any(|v| v == SECRET.as_bytes()));
    assert_eq!(
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert!(
        !serde_json::to_string(&fixture.manager.credential_operation(&id).unwrap())
            .unwrap()
            .contains(SECRET)
    );
    fixture.vault.delete(&saved.credential_ref).unwrap();
    assert_eq!(fixture.create(&id).unwrap_err().code, "credential_removed");
    assert_eq!(
        fixture.target_count(),
        0,
        "a completed operation never resurrects a deleted credential"
    );
}

#[test]
fn a_vault_commit_with_a_lost_reply_is_recovered_without_a_second_write() {
    let fixture = Fixture::new();
    let id = uuid::Uuid::new_v4().to_string();
    fixture.vault.fault.store(2, Ordering::SeqCst);
    assert_eq!(
        fixture.create(&id).unwrap_err().code,
        "credential_result_unknown"
    );
    assert_eq!(fixture.target_count(), 1);
    let recovered = fixture.reopen().credential_operation(&id).unwrap();
    assert_eq!(recovered.status, Status::Ready);
    fixture.vault.fault.store(1, Ordering::SeqCst);
    assert_eq!(fixture.create(&id).unwrap(), recovered.receipt.unwrap());
    assert_eq!(
        fixture.vault.fault.load(Ordering::SeqCst),
        1,
        "retry must not write Vault again"
    );
    assert_eq!(fixture.target_count(), 1);
}

#[test]
fn auxiliary_key_creation_also_reuses_its_durable_reference_after_a_lost_reply() {
    let fixture = Fixture::new();
    fixture.vault.values.0.lock().unwrap().clear();
    let path = fixture
        .manager
        .directory
        .join("credential-operation-key-v1.json");
    std::fs::remove_file(&path).unwrap();
    let id = uuid::Uuid::new_v4().to_string();
    fixture.vault.fault.store(2, Ordering::SeqCst);
    assert_eq!(
        fixture.create(&id).unwrap_err().code,
        "credential_result_unknown"
    );
    let before: ProtectionKey = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert!(!before.ready);
    assert_eq!(fixture.vault.values.0.lock().unwrap().len(), 1);
    assert_eq!(fixture.target_count(), 0);
    fixture.create(&id).unwrap();
    let after: ProtectionKey = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert!(after.ready);
    assert_eq!(before.reference, after.reference);
    assert_eq!(fixture.vault.values.0.lock().unwrap().len(), 2);
    assert_eq!(fixture.target_count(), 1);
}

#[test]
fn an_unconfirmed_auxiliary_write_cannot_later_replace_the_active_protection_key() {
    let fixture = Fixture::new();
    fixture.vault.values.0.lock().unwrap().clear();
    let path = fixture
        .manager
        .directory
        .join("credential-operation-key-v1.json");
    std::fs::remove_file(&path).unwrap();
    let id = uuid::Uuid::new_v4().to_string();
    fixture.vault.fault.store(1, Ordering::SeqCst);
    assert_eq!(
        fixture.create(&id).unwrap_err().code,
        "credential_result_unknown"
    );
    let before: ProtectionKey = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let receipt = fixture.create(&id).unwrap();
    let after: ProtectionKey = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_ne!(before.reference, after.reference);
    assert_eq!(after.abandoned, vec![before.reference.clone()]);
    fixture.vault.set(&before.reference, &[42; 32]).unwrap();
    assert_eq!(
        fixture.manager.credential_operation(&id).unwrap().receipt,
        Some(receipt)
    );
    assert_eq!(fixture.target_count(), 1);
}

#[test]
fn pending_intent_can_retry_or_cancel_and_late_creates_cannot_resurrect_it() {
    let fixture = Fixture::new();
    let id = uuid::Uuid::new_v4().to_string();
    fixture.vault.fault.store(1, Ordering::SeqCst);
    assert_eq!(
        fixture.create(&id).unwrap_err().code,
        "credential_result_unknown"
    );
    assert_eq!(
        fixture.manager.credential_operation(&id).unwrap().status,
        Status::Pending
    );
    assert_eq!(fixture.target_count(), 0);
    assert_eq!(
        fixture
            .reopen()
            .create_credential_operation(&id, "Fixture API Key", Zeroizing::new(SECRET.into()))
            .unwrap()
            .operation_id,
        id
    );
    assert_eq!(fixture.target_count(), 1);
    let id = uuid::Uuid::new_v4().to_string();
    fixture.vault.fault.store(1, Ordering::SeqCst);
    fixture.create(&id).unwrap_err();
    assert_eq!(
        fixture
            .manager
            .cancel_credential_operation(&id)
            .unwrap()
            .status,
        Status::CancelledPending
    );
    assert_eq!(
        fixture.create(&id).unwrap_err().code,
        "credential_operation_cancelled"
    );
    let never_started = uuid::Uuid::new_v4().to_string();
    assert_eq!(
        fixture
            .manager
            .credential_operation(&never_started)
            .unwrap_err()
            .code,
        "operation_not_found"
    );
    assert_eq!(
        fixture
            .manager
            .cancel_credential_operation(&never_started)
            .unwrap()
            .status,
        Status::Cancelled
    );
    assert_eq!(
        fixture.create(&never_started).unwrap_err().code,
        "credential_operation_cancelled"
    );
    assert_eq!(fixture.target_count(), 1);
}

#[test]
fn a_late_vault_write_after_cancellation_remains_owned_and_recoverable() {
    let fixture = Fixture::new();
    let id = uuid::Uuid::new_v4().to_string();
    fixture.vault.fault.store(1, Ordering::SeqCst);
    fixture.create(&id).unwrap_err();
    assert_eq!(
        fixture
            .manager
            .cancel_credential_operation(&id)
            .unwrap()
            .status,
        Status::CancelledPending
    );
    assert_eq!(
        fixture.create(&id).unwrap_err().code,
        "credential_operation_cancelled"
    );
    let Operation::CancelledPending { reference, .. } = fixture
        .manager
        .read_credential_operation(&id)
        .unwrap()
        .unwrap()
    else {
        panic!("retained intent")
    };
    assert_eq!(fixture.target_count(), 0);
    fixture.vault.set(&reference, SECRET.as_bytes()).unwrap();
    let result = fixture.reopen().credential_operation(&id).unwrap();
    assert_eq!(result.status, Status::Ready);
    assert_eq!(result.receipt.unwrap().credential_ref, reference);
    assert_eq!(fixture.target_count(), 1);
}

#[test]
fn cancellation_never_deletes_an_already_materialized_credential() {
    let fixture = Fixture::new();
    let id = uuid::Uuid::new_v4().to_string();
    fixture.vault.fault.store(2, Ordering::SeqCst);
    fixture.create(&id).unwrap_err();
    let status = fixture.manager.cancel_credential_operation(&id).unwrap();
    assert_eq!(status.status, Status::Ready);
    assert_eq!(
        fixture
            .vault
            .get(&status.receipt.unwrap().credential_ref)
            .unwrap()
            .as_slice(),
        SECRET.as_bytes()
    );
    assert_eq!(fixture.target_count(), 1);
}

#[test]
fn an_actual_receipt_write_failure_keeps_the_materialized_key_and_recovers_after_reopen() {
    let fixture = Fixture::new();
    let id = uuid::Uuid::new_v4().to_string();
    let path = fixture.manager.credential_operation_path(&id).unwrap();
    *fixture.vault.receipt_failure.lock().unwrap() = Some(path.clone());
    assert_eq!(
        fixture.create(&id).unwrap_err().code,
        "credential_result_unknown"
    );
    assert_eq!(fixture.target_count(), 1);
    std::fs::remove_dir(&path).unwrap();
    std::fs::rename(path.with_extension("before-receipt"), &path).unwrap();
    assert_eq!(
        fixture.reopen().credential_operation(&id).unwrap().status,
        Status::Ready
    );
    let count = fixture.vault.values.0.lock().unwrap().len();
    std::fs::remove_file(
        fixture
            .manager
            .directory
            .join("credential-operation-key-v1.json"),
    )
    .unwrap();
    assert_eq!(
        fixture.manager.credential_operation(&id).unwrap_err().code,
        "credential_result_unknown"
    );
    assert_eq!(
        fixture.vault.values.0.lock().unwrap().len(),
        count,
        "a read must never regenerate a missing operation protection key"
    );
}

#[test]
fn tampered_receipt_fields_never_overwrite_or_resurrect_a_key() {
    let fixture = Fixture::new();
    let id = uuid::Uuid::new_v4().to_string();
    let saved = fixture.create(&id).unwrap();
    let path = fixture.manager.credential_operation_path(&id).unwrap();
    let original = std::fs::read(&path).unwrap();
    let mut operation: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    operation["label"] = serde_json::json!("tampered label");
    files::atomic_write(&path, &serde_json::to_vec(&operation).unwrap(), 0o600).unwrap();
    assert_eq!(
        fixture.create(&id).unwrap_err().code,
        "credential_result_unknown"
    );
    assert_eq!(
        fixture.vault.get(&saved.credential_ref).unwrap().as_slice(),
        SECRET.as_bytes()
    );
    assert_eq!(fixture.target_count(), 1);
    let mut operation: serde_json::Value = serde_json::from_slice(&original).unwrap();
    operation["state"] = serde_json::json!("pending");
    files::atomic_write(&path, &serde_json::to_vec(&operation).unwrap(), 0o600).unwrap();
    fixture.vault.delete(&saved.credential_ref).unwrap();
    assert_eq!(
        fixture.create(&id).unwrap_err().code,
        "credential_result_unknown"
    );
    assert_eq!(
        fixture.target_count(),
        0,
        "a forged pending state cannot resurrect a deleted credential"
    );
}

#[test]
fn intent_storage_failure_capacity_and_input_validation_precede_target_writes() {
    let fixture = Fixture::new();
    let folder = fixture.manager.directory.join("credential-operations-v1");
    std::fs::write(&folder, b"fixture blocks directory").unwrap();
    assert!(fixture.create(&uuid::Uuid::new_v4().to_string()).is_err());
    assert_eq!(fixture.target_count(), 0);
    std::fs::remove_file(&folder).unwrap();
    files::private_dir(&folder).unwrap();
    for _ in 0..MAX_OPERATIONS {
        std::fs::write(
            folder.join(format!("{}.json", uuid::Uuid::new_v4())),
            br#"{"state":"cancelled"}"#,
        )
        .unwrap();
    }
    assert_eq!(
        fixture
            .create(&uuid::Uuid::new_v4().to_string())
            .unwrap_err()
            .code,
        "credential_operation_capacity"
    );
    assert_eq!(fixture.target_count(), 0);
    assert_eq!(
        fixture.create("../bad").unwrap_err().code,
        "invalid_credential_operation"
    );
    assert_eq!(
        fixture
            .manager
            .create_credential_operation(
                &uuid::Uuid::new_v4().to_string(),
                SECRET,
                Zeroizing::new(SECRET.into())
            )
            .unwrap_err()
            .code,
        "invalid_credential"
    );
}
