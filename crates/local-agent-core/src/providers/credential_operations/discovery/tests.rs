use super::*;
use crate::{
    Store,
    providers::{
        credential_operations::legacy::Confirmation,
        vault::{MemoryVault, Vault},
    },
};
use rusqlite::params;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use zeroize::Zeroizing;
const SECRET: &str = "discovery-disposable-private-fixture";
#[derive(Default)]
struct CountingVault {
    inner: MemoryVault,
    calls: AtomicUsize,
}
impl Vault for CountingVault {
    fn get(&self, id: &str) -> Result<Zeroizing<Vec<u8>>, ConfigError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.inner.get(id)
    }
    fn set(&self, id: &str, value: &[u8]) -> Result<(), ConfigError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.inner.set(id, value)
    }
    fn delete(&self, id: &str) -> Result<(), ConfigError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.inner.delete(id)
    }
}
struct Fixture {
    root: tempfile::TempDir,
    store: Store,
    manager: ProviderManager,
    vault: Arc<CountingVault>,
    namespace: String,
}
impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
        let store = Store::open(&root.path().join("data")).unwrap();
        let vault = Arc::new(CountingVault::default());
        let manager = ProviderManager::new(
            root.path().join("home"),
            &root.path().join("data"),
            root.path().join("helper"),
            vault.clone(),
        )
        .unwrap();
        let scope = store
            .credential_scope(&store.database_id().unwrap())
            .unwrap();
        let namespace = manager
            .scoped_credential_protocol_v3(&scope)
            .unwrap()
            .namespace_id;
        Self {
            root,
            store,
            manager,
            vault,
            namespace,
        }
    }
    fn scope(&self) -> CredentialScope {
        self.store
            .credential_scope(&self.store.database_id().unwrap())
            .unwrap()
    }
    fn request(&self, cursor: Option<String>) -> Request {
        Request {
            store_id: self.store.database_id().unwrap(),
            namespace_id: self.namespace.clone(),
            cursor,
        }
    }
    fn page(&self, cursor: Option<String>) -> Page {
        let before = self.vault.calls.load(Ordering::SeqCst);
        let page = self
            .manager
            .discover_credentials(&self.scope(), &self.request(cursor))
            .unwrap();
        assert_eq!(before, self.vault.calls.load(Ordering::SeqCst));
        page
    }
    fn old(&self, id: &str) {
        self.manager
            .create_credential_operation(id, "Old fixture", Zeroizing::new(SECRET.into()))
            .unwrap();
    }
    fn v3(&self, id: &str) {
        self.manager
            .create_credential_v3(
                &self.scope(),
                &self.namespace,
                id,
                "Current fixture",
                Zeroizing::new(SECRET.into()),
            )
            .unwrap();
    }
    fn migrate(&self, id: &str) {
        let p = self
            .manager
            .preview_legacy_credential(&self.scope(), &self.namespace, id)
            .unwrap();
        self.manager
            .confirm_legacy_credential(
                &self.scope(),
                &Confirmation {
                    store_id: p.store_id,
                    namespace_id: p.namespace_id,
                    operation_id: p.operation_id,
                    source_digest: p.source_digest,
                    confirmed: true,
                },
            )
            .unwrap();
    }
    fn rows(&self) -> Vec<(u32, String, Vec<u8>, String, i64)> {
        let mut q=self.store.connection.prepare("SELECT domain,name,value,value_hash,revision FROM credential_recovery_values ORDER BY domain,name").unwrap();
        q.query_map([], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
        })
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
    }
}
#[test]
fn empty_discovery_is_exact_and_does_not_activate_write_or_call_vault() {
    let f = Fixture::new();
    let before = f.rows();
    let p = f.page(None);
    assert_eq!(p.total_candidates, 0);
    assert_eq!(p.scanned, 0);
    assert!(p.items.is_empty());
    assert!(p.next_cursor.is_none());
    assert_eq!(p.snapshot, f.page(None).snapshot);
    assert_eq!(f.rows(), before);
    let value = serde_json::to_value(p).unwrap();
    assert_eq!(value.as_object().unwrap().len(), 11);
    assert_eq!(value["version"], 1);
    assert_eq!(value["pageSize"], 32);
}
#[test]
fn three_protocol_domains_keep_the_same_uuid_separate_without_returning_references() {
    let f = Fixture::new();
    let id = uuid::Uuid::new_v4().to_string();
    f.old(&id);
    f.manager
        .create_scoped_credential(&f.scope(), &id, "V2 fixture", Zeroizing::new(SECRET.into()))
        .unwrap();
    f.v3(&id);
    let before = f.rows();
    let p = f.page(None);
    assert_eq!(p.items.len(), 3);
    assert_eq!(p.total_candidates, 3);
    let value = serde_json::to_value(p).unwrap();
    assert_eq!(value["items"][0]["kind"], "legacy");
    assert_eq!(value["items"][1]["kind"], "scoped_v2");
    assert_eq!(value["items"][2]["kind"], "scoped_v3");
    assert_eq!(value["items"][0]["issue"], "conflict");
    assert_eq!(value["items"][2]["issue"], serde_json::Value::Null);
    assert!(!value.to_string().contains(SECRET));
    assert!(!value.to_string().contains("credentialRef"));
    assert!(!value.to_string().contains("commitment"));
    assert_eq!(f.rows(), before);
}
#[test]
fn committed_migrations_merge_old_fences_and_survive_whole_source_directory_loss() {
    let f = Fixture::new();
    let id = uuid::Uuid::new_v4().to_string();
    f.old(&id);
    f.migrate(&id);
    let rows = f.rows();
    let p = f.page(None);
    assert_eq!(p.total_candidates, 2);
    assert_eq!(p.merged, 1);
    assert_eq!(p.items.len(), 1);
    assert!(matches!(
        p.items[0].legacy_stage,
        Some(legacy::Stage::Committed)
    ));
    fs::rename(
        &f.manager.directory,
        f.manager.directory.with_extension("held-discovery"),
    )
    .unwrap();
    let p = f.page(None);
    // The durable negative-admission ID remains a candidate even without its
    // old file; the committed migration still merges it into one visible row.
    assert_eq!(p.total_candidates, 2);
    assert_eq!(p.merged, 1);
    assert_eq!(p.items.len(), 1);
    assert!(!f.manager.directory.exists());
    assert_eq!(f.rows(), rows);
}
#[test]
fn prepared_migrations_are_discoverable_as_pending_recovery_without_accessing_vault() {
    let f = Fixture::new();
    let id = uuid::Uuid::new_v4().to_string();
    f.old(&id);
    f.store.connection.execute_batch("CREATE TRIGGER hold_discovery_fence BEFORE UPDATE ON credential_recovery_values WHEN CAST(NEW.value AS TEXT) LIKE '%\"stage\":\"fenced\"%' BEGIN SELECT RAISE(IGNORE);END;").unwrap();
    let p = f
        .manager
        .preview_legacy_credential(&f.scope(), &f.namespace, &id)
        .unwrap();
    assert!(
        f.manager
            .confirm_legacy_credential(
                &f.scope(),
                &Confirmation {
                    store_id: p.store_id,
                    namespace_id: p.namespace_id,
                    operation_id: id,
                    source_digest: p.source_digest,
                    confirmed: true
                }
            )
            .is_err()
    );
    let rows = f.rows();
    let page = f.page(None);
    assert_eq!(page.merged, 1);
    assert!(matches!(
        page.items[0].legacy_stage,
        Some(legacy::Stage::Prepared)
    ));
    assert_eq!(f.rows(), rows);
}
#[test]
fn bad_files_remain_visible_and_unrecognized_names_are_reported_without_poisoning_valid_rows() {
    let f = Fixture::new();
    let id = uuid::Uuid::new_v4().to_string();
    f.old(&id);
    let folder = f.manager.directory.join("credential-operations-v1");
    for bytes in [b"{broken".as_slice(), &vec![b'X'; 4097]] {
        files::atomic_write(
            &folder.join(format!("{}.json", uuid::Uuid::new_v4())),
            bytes,
            0o600,
        )
        .unwrap();
    }
    let link = folder.join(format!("{}.json", uuid::Uuid::new_v4()));
    std::os::unix::fs::symlink(f.root.path().join("nonexistent"), &link).unwrap();
    files::atomic_write(
        &folder.join("unrecognized-name"),
        b"private-no-discovery-output",
        0o600,
    )
    .unwrap();
    let p = f.page(None);
    assert_eq!(p.items.len(), 4);
    assert_eq!(p.unrecognized, 1);
    assert_eq!(p.items.iter().filter(|i| i.issue.is_some()).count(), 3);
    assert_eq!(p.items.iter().filter(|i| i.digest.is_some()).count(), 1);
    assert!(
        !serde_json::to_string(&p)
            .unwrap()
            .contains("private-no-discovery-output")
    );
    assert!(link.symlink_metadata().unwrap().file_type().is_symlink());
}
#[test]
fn corrupt_sql_values_are_individual_unknown_rows_without_hiding_other_records() {
    let f = Fixture::new();
    let a = uuid::Uuid::new_v4().to_string();
    let b = uuid::Uuid::new_v4().to_string();
    f.v3(&a);
    f.v3(&b);
    let broken = b"{broken";
    f.store.connection.execute("UPDATE credential_recovery_values SET value=?1,value_hash=?2 WHERE domain=3 AND name=?3",params![broken.as_slice(),files::hash(broken),format!("operations/{a}.json")]).unwrap();
    let rows = f.rows();
    let p = f.page(None);
    assert_eq!(p.items.len(), 2);
    assert_eq!(p.items.iter().filter(|i| i.issue.is_some()).count(), 1);
    assert_eq!(f.rows(), rows);
}
#[test]
fn keyset_pages_cover_every_id_once_and_reject_sql_changes() {
    let f = Fixture::new();
    for _ in 0..70 {
        f.manager
            .credential_operation_v3(
                &f.scope(),
                &f.namespace,
                &uuid::Uuid::new_v4().to_string(),
                true,
            )
            .unwrap();
    }
    let first = f.page(None);
    let cursor = first.next_cursor.clone();
    let second = f.page(first.next_cursor.clone());
    let third = f.page(second.next_cursor.clone());
    assert_eq!((first.scanned, second.scanned, third.scanned), (32, 32, 6));
    let ids = first
        .items
        .into_iter()
        .chain(second.items)
        .chain(third.items)
        .map(|i| i.operation_id)
        .collect::<BTreeSet<_>>();
    assert_eq!(ids.len(), 70);
    assert!(third.next_cursor.is_none());
    f.manager
        .credential_operation_v3(
            &f.scope(),
            &f.namespace,
            &uuid::Uuid::new_v4().to_string(),
            true,
        )
        .unwrap();
    assert_eq!(
        f.manager
            .discover_credentials(&f.scope(), &f.request(cursor))
            .err()
            .unwrap()
            .code,
        "credential_discovery_changed"
    );
}
#[test]
fn cursor_detects_legacy_content_changes_even_if_mtime_is_restored() {
    let f = Fixture::new();
    let id = uuid::Uuid::new_v4().to_string();
    f.old(&id);
    let p = f.page(None);
    let source = f.manager.credential_operation_path(&id).unwrap();
    let modified = fs::metadata(&source).unwrap().modified().unwrap();
    let mut bytes = fs::read(&source).unwrap();
    bytes.push(b' ');
    fs::write(&source, bytes).unwrap();
    fs::File::open(&source)
        .unwrap()
        .set_modified(modified)
        .unwrap();
    let cursor = Some(format!("{}:1:{id}", p.snapshot));
    assert_eq!(
        f.manager
            .discover_credentials(&f.scope(), &f.request(cursor))
            .err()
            .unwrap()
            .code,
        "credential_discovery_changed"
    );
}
#[test]
fn malformed_and_foreign_cursors_or_namespaces_never_adopt_another_source() {
    let f = Fixture::new();
    for cursor in [
        "".into(),
        "x".repeat(129),
        format!("{}:1:{}", "a".repeat(64), uuid::Uuid::new_v4()),
    ] {
        assert!(
            f.manager
                .discover_credentials(&f.scope(), &f.request(Some(cursor)))
                .is_err()
        );
    }
    let mut request = f.request(None);
    request.namespace_id = uuid::Uuid::new_v4().to_string();
    assert!(
        f.manager
            .discover_credentials(&f.scope(), &request)
            .is_err()
    );
    request = f.request(None);
    request.store_id = uuid::Uuid::new_v4().to_string();
    assert!(
        f.manager
            .discover_credentials(&f.scope(), &request)
            .is_err()
    );
    assert_eq!(f.vault.calls.load(Ordering::SeqCst), 0);
}
#[test]
fn root_corruption_main_file_loss_and_unverified_namespace_do_not_become_empty_success() {
    for damage in [
        "UPDATE credential_recovery_scope SET format=2",
        "UPDATE credential_recovery_scope SET state='unverified'",
    ] {
        let f = Fixture::new();
        let scope = f.scope();
        f.store.connection.execute_batch(damage).unwrap();
        assert!(
            f.manager
                .discover_credentials(&scope, &f.request(None))
                .is_err()
        );
        assert_eq!(f.vault.calls.load(Ordering::SeqCst), 0);
    }
    let f = Fixture::new();
    let scope = f.scope();
    fs::rename(
        f.root.path().join("data/local-agent.sqlite3"),
        f.root.path().join("data/main-held"),
    )
    .unwrap();
    assert!(
        f.manager
            .discover_credentials(&scope, &f.request(None))
            .is_err()
    );
    assert_eq!(f.vault.calls.load(Ordering::SeqCst), 0);
}
#[test]
fn over_capacity_legacy_directories_fail_explicitly_without_returning_partial_empty_data() {
    let f = Fixture::new();
    let directory = f.manager.directory.join("credential-operations-v1");
    files::private_dir(&directory).unwrap();
    for index in 0..4097 {
        fs::write(directory.join(format!("unknown-{index}")), []).unwrap();
    }
    assert_eq!(
        f.manager
            .discover_credentials(&f.scope(), &f.request(None))
            .err()
            .unwrap()
            .code,
        "credential_discovery_capacity"
    );
    assert_eq!(f.vault.calls.load(Ordering::SeqCst), 0);
}
#[test]
fn unverified_v2_material_is_a_visible_non_actionable_row() {
    let f = Fixture::new();
    let id = uuid::Uuid::new_v4().to_string();
    f.manager
        .create_scoped_credential(&f.scope(), &id, "V2 fixture", Zeroizing::new(SECRET.into()))
        .unwrap();
    f.store
        .connection
        .execute(
            "UPDATE credential_recovery_scope SET legacy_state='unverified'",
            [],
        )
        .unwrap();
    let p = f.page(None);
    assert_eq!(p.items.len(), 1);
    assert!(matches!(p.items[0].issue, Some(Issue::SourceUnverified)));
    assert!(p.items[0].digest.is_none());
}
#[test]
fn root_and_legacy_lock_contention_release_partial_guards_before_any_vault_access() {
    let f = Fixture::new();
    let scope = f.scope();
    let held = scope.journal.lock().unwrap();
    assert_eq!(
        f.manager
            .discover_credentials(&scope, &f.request(None))
            .err()
            .unwrap()
            .code,
        "busy"
    );
    drop(held);
    let held = f.manager.lock().unwrap();
    assert_eq!(
        f.manager
            .discover_credentials(&scope, &f.request(None))
            .err()
            .unwrap()
            .code,
        "busy"
    );
    let root_again = scope.journal.lock().unwrap();
    drop(root_again);
    drop(held);
    assert_eq!(f.vault.calls.load(Ordering::SeqCst), 0);
    assert!(f.page(None).items.is_empty());
}
#[test]
fn old_protection_metadata_changes_invalidate_the_entire_source_cursor() {
    let f = Fixture::new();
    let id = uuid::Uuid::new_v4().to_string();
    f.old(&id);
    let p = f.page(None);
    let key = f.manager.directory.join("credential-operation-key-v1.json");
    let mut bytes = fs::read(&key).unwrap();
    bytes.push(b' ');
    files::atomic_write(&key, &bytes, 0o600).unwrap();
    assert_eq!(
        f.manager
            .discover_credentials(
                &f.scope(),
                &f.request(Some(format!("{}:1:{id}", p.snapshot)))
            )
            .err()
            .unwrap()
            .code,
        "credential_discovery_changed"
    );
}
