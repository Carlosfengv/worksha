use super::*;
use local_agent_core::providers::{
    files,
    vault::{self, Vault},
};
use std::{os::unix::fs::PermissionsExt, sync::Arc};

#[test]
fn host_restore_recovers_both_commit_sides_without_a_plugin_or_a_second_write() {
    for state in ["applying", "restoring", "applied"] {
        for replaced in [false, true] {
            for corrupt_catalog in [false, true] {
                if state == "applied" && !replaced {
                    continue;
                }
                let root = tempfile::tempdir().unwrap();
                let root = root.path().canonicalize().unwrap();
                let vault = Arc::new(vault::MemoryVault::default());
                let before = b"{\"keep\":42}";
                let after = b"{\"keep\":42,\"model\":\"fixture\"}";
                let target = root.join("selected.json");
                std::fs::write(&target, before).unwrap();
                std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o644)).unwrap();
                let original = files::read(&target).unwrap().stamp;
                if replaced {
                    files::replace(&target, &original, Some(after), 0o600).unwrap();
                }
                let current = files::read(&target).unwrap().stamp;
                let manager = Manager::open(&root, vault.clone()).unwrap();
                let provider = local_agent_core::Store::open(&root)
                    .unwrap()
                    .providers()
                    .unwrap()
                    .remove(0);
                let q = Query {
                    plugin_id: "org.example.removed".into(),
                    digest: "a".repeat(64),
                };
                let lease = json!({"id":q.plugin_id,"digest":q.digest,"epoch":"1","revision":"1"});
                let key = vault::random_key();
                vault.set("recovery-backup", &key).unwrap();
                let record = json!({"id":"recovery","plugin_id":q.plugin_id,"digest":q.digest,"lease":lease,"grant":{"lease":lease,"path":target,"device":original.device,"inode":original.inode,"hash":package::hash(before),"executable":null},"provider":provider,"edits":[],"credential_pointers":["/credential"],"stamp":original,"state":state,"created_at":chrono::Utc::now().to_rfc3339(),"after_hash":package::hash(after),"backup":vault::seal(&key,before,"recovery").unwrap(),"backup_ref":"recovery-backup","after_stamp":replaced.then_some(current.clone())});
                manager
                    .advanced
                    .lock()
                    .unwrap()
                    .db()
                    .unwrap()
                    .execute(
                        "INSERT INTO plans VALUES('recovery',?1,?2)",
                        params![q.plugin_id, record.to_string()],
                    )
                    .unwrap();
                drop(manager);
                if corrupt_catalog {
                    std::fs::write(root.join("plugin-host.sqlite3"), b"damaged catalog").unwrap();
                }
                let reopened = Manager::for_service(&root, vault);
                if corrupt_catalog {
                    assert!(reopened.list().is_err());
                } else {
                    assert!(reopened.list().unwrap().is_empty());
                }
                assert_eq!(
                    reopened.restore_config(&q, "recovery").unwrap()["state"],
                    "restored"
                );
                let restored = files::read(&target).unwrap();
                assert_eq!(restored.bytes.as_deref(), Some(before.as_slice()));
                assert_eq!(restored.stamp.mode, 0o644);
                if !replaced {
                    assert_eq!(restored.stamp, original);
                }
                assert_eq!(
                    reopened.restore_config(&q, "recovery").unwrap()["state"],
                    "restored"
                );
                assert_eq!(files::read(&target).unwrap().stamp, restored.stamp);
            }
        }
    }
}

#[test]
fn original_bytes_with_changed_permissions_do_not_confirm_an_interrupted_restore() {
    use std::os::unix::fs::MetadataExt;
    let root = tempfile::tempdir().unwrap();
    let root = root.path().canonicalize().unwrap();
    let vault = Arc::new(vault::MemoryVault::default());
    let manager = Manager::open(&root, vault).unwrap();
    let target = root.join("selected.json");
    std::fs::write(&target, b"{}").unwrap();
    std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o600)).unwrap();
    let original = files::read(&target).unwrap().stamp;
    let q = Query {
        plugin_id: "org.example.removed".into(),
        digest: "b".repeat(64),
    };
    let lease = json!({"id":q.plugin_id,"digest":q.digest,"epoch":"1","revision":"1"});
    let provider = local_agent_core::Store::open(&root)
        .unwrap()
        .providers()
        .unwrap()
        .remove(0);
    let record = json!({"id":"permissions","plugin_id":q.plugin_id,"digest":q.digest,"lease":lease,"grant":{"lease":lease,"path":target,"device":original.device,"inode":original.inode,"hash":package::hash(b"{}"),"executable":null},"provider":provider,"edits":[],"credential_pointers":[],"stamp":original,"state":"restoring","created_at":chrono::Utc::now().to_rfc3339(),"after_hash":package::hash(b"{\"after\":true}"),"backup":null,"backup_ref":null,"after_stamp":null});
    manager
        .advanced
        .lock()
        .unwrap()
        .db()
        .unwrap()
        .execute(
            "INSERT INTO plans VALUES('permissions',?1,?2)",
            params![q.plugin_id, record.to_string()],
        )
        .unwrap();
    std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(
        manager.restore_config(&q, "permissions").unwrap_err().code,
        "result_pending"
    );
    assert_eq!(std::fs::read(&target).unwrap(), b"{}");
    assert_eq!(std::fs::metadata(&target).unwrap().mode() & 0o777, 0o644);
    assert_eq!(
        manager.extension_history().unwrap()["plans"][0]["state"],
        "restoring"
    );
    std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(
        manager.restore_config(&q, "permissions").unwrap()["state"],
        "restored"
    );
}
