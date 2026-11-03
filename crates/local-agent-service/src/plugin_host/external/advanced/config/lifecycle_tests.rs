use super::*;
use local_agent_core::{
    plugin::external::{StateAction, StateRequest},
    providers::vault::MemoryVault,
};
use std::path::Path;

async fn host(root: &Path) -> (Arc<Manager>, Query, Value) {
    let (manager, query) = super::super::record_tests::host(root);
    let manager = Arc::new(manager);
    manager
        .change(StateRequest {
            plugin_id: query.plugin_id.clone(),
            digest: query.digest.clone(),
            action: StateAction::Enable,
        })
        .await
        .unwrap();
    let mut plan = super::super::record_tests::valid_plan(root, &query);
    let lease = Lease::from(&manager.status(&query.plugin_id, &query.digest).unwrap());
    plan["lease"] = serde_json::to_value(&lease).unwrap();
    plan["grant"]["lease"] = plan["lease"].clone();
    plan["created_at"] = json!(chrono::Utc::now().to_rfc3339());
    (manager, query, plan)
}

fn count(manager: &Manager) -> u32 {
    manager
        .advanced
        .lock()
        .unwrap()
        .db()
        .unwrap()
        .query_row("SELECT count(*) FROM plans", [], |r| r.get(0))
        .unwrap()
}
fn insert(manager: &Manager, value: &Value) {
    manager
        .advanced
        .lock()
        .unwrap()
        .db()
        .unwrap()
        .execute(
            "INSERT OR REPLACE INTO plans VALUES(?1,?2,?3)",
            params![
                value["id"].as_str().unwrap(),
                value["plugin_id"].as_str().unwrap(),
                value.to_string()
            ],
        )
        .unwrap();
}

#[tokio::test]
async fn full_catalog_reuses_the_original_preview_and_discard_releases_exactly_one_slot() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let (manager, query, original) = host(&root).await;
    let target = root.join("selected.json");
    let before = files::read(&target).unwrap();
    for index in 0..MAX_PLANS {
        let mut record = original.clone();
        record["id"] = json!(format!("preview-{index}"));
        record["provider"]["revision"] = json!(index + 1);
        manager
            .commit_config_preview(serde_json::from_value(record).unwrap())
            .unwrap();
    }
    let mut same = original.clone();
    same["provider"]["revision"] = json!(1);
    let reused = manager
        .commit_config_preview(serde_json::from_value(same.clone()).unwrap())
        .unwrap();
    assert_eq!(reused["planId"], "preview-0");
    assert_eq!(reused["createdAt"], original["created_at"]);
    assert_eq!(count(&manager), 64);
    let mut next = original.clone();
    next["provider"]["revision"] = json!(65);
    assert_eq!(
        manager
            .commit_config_preview(serde_json::from_value(next.clone()).unwrap())
            .unwrap_err()
            .code,
        "plan_capacity_full"
    );
    assert_eq!(count(&manager), 64);
    assert_eq!(
        manager.extension_history().unwrap()["planCapacity"],
        json!({"used":64,"limit":64,"remaining":0})
    );
    manager.discard_config(&query, "preview-0").unwrap();
    assert_eq!(count(&manager), 63);
    manager
        .commit_config_preview(serde_json::from_value(next).unwrap())
        .unwrap();
    assert_eq!(count(&manager), 64);
    assert_eq!(
        manager.config_plan(&query, "preview-0").err().unwrap().code,
        "not_found"
    );
    assert_eq!(files::read(&target).unwrap().bytes, before.bytes);
    assert_eq!(files::read(&target).unwrap().stamp, before.stamp);
}

#[tokio::test]
async fn reuse_requires_the_same_file_authorization_provider_and_unexpired_unwritten_plan() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let (_, _, original) = host(&root).await;
    let next: Plan = serde_json::from_value(original.clone()).unwrap();
    for change in [
        "provider",
        "lease",
        "grant",
        "stamp",
        "edits",
        "pointers",
        "expired",
        "backup",
        "backup-ref",
        "after-hash",
        "after-stamp",
        "state",
        "plugin",
        "digest",
    ] {
        let mut changed = original.clone();
        match change {
            "provider" => changed["provider"]["revision"] = json!(999),
            "lease" => changed["lease"]["epoch"] = json!("different"),
            "grant" => changed["grant"]["inode"] = json!(0),
            "stamp" => changed["stamp"]["mode"] = json!(0),
            "edits" => changed["edits"] = json!([{"pointer":"/model","value":"different"}]),
            "pointers" => changed["credential_pointers"] = json!(["/key"]),
            "expired" => {
                changed["created_at"] =
                    json!((chrono::Utc::now() - chrono::Duration::hours(25)).to_rfc3339())
            }
            "backup" => changed["backup"] = json!([1]),
            "backup-ref" => changed["backup_ref"] = json!("protected-key"),
            "after-hash" => changed["after_hash"] = json!("protected-hash"),
            "after-stamp" => changed["after_stamp"] = changed["stamp"].clone(),
            "state" => changed["state"] = json!("applying"),
            "plugin" => changed["plugin_id"] = json!("org.other.config"),
            "digest" => changed["digest"] = json!("b".repeat(64)),
            _ => unreachable!(),
        }
        let old: Plan = serde_json::from_value(changed).unwrap();
        assert!(!old.reusable_for(&next), "unexpected reuse after {change}");
    }
}

#[tokio::test]
async fn discard_preserves_backups_unknown_records_and_all_started_states() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let (manager, query, original) = host(&root).await;
    let before = files::read(&root.join("selected.json")).unwrap();
    for change in [
        "applying",
        "applied",
        "restoring",
        "restored",
        "unknown",
        "backup",
        "backup-ref",
        "after-hash",
        "after-stamp",
        "future-field",
        "wrong-id",
    ] {
        let mut record = original.clone();
        match change {
            "backup" => record["backup"] = json!([1, 2, 3]),
            "backup-ref" => record["backup_ref"] = json!("protected-key"),
            "after-hash" => record["after_hash"] = json!("protected-hash"),
            "after-stamp" => record["after_stamp"] = record["stamp"].clone(),
            "future-field" => record["futureBackup"] = json!("keep"),
            _ => record["state"] = json!(change),
        }
        if change == "wrong-id" {
            record["id"] = json!("other-plan");
        }
        manager
            .advanced
            .lock()
            .unwrap()
            .db()
            .unwrap()
            .execute(
                "INSERT OR REPLACE INTO plans VALUES('valid-plan',?1,?2)",
                params![query.plugin_id, record.to_string()],
            )
            .unwrap();
        let code = manager
            .discard_config(&query, "valid-plan")
            .unwrap_err()
            .code;
        assert_eq!(
            code,
            if ["future-field", "wrong-id"].contains(&change) {
                "invalid_record"
            } else {
                "result_pending"
            }
        );
        let kept: String = manager
            .advanced
            .lock()
            .unwrap()
            .db()
            .unwrap()
            .query_row("SELECT record FROM plans WHERE id='valid-plan'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(kept, record.to_string());
        assert_eq!(count(&manager), 1);
    }
    assert_eq!(
        files::read(&root.join("selected.json")).unwrap().stamp,
        before.stamp
    );
}

#[tokio::test]
async fn host_can_discard_expired_preview_after_uninstall_restart_and_target_removal() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let (manager, query, mut original) = host(&root).await;
    original["created_at"] = json!((chrono::Utc::now() - chrono::Duration::days(30)).to_rfc3339());
    insert(&manager, &original);
    manager
        .change(StateRequest {
            plugin_id: query.plugin_id.clone(),
            digest: query.digest.clone(),
            action: StateAction::Remove,
        })
        .await
        .unwrap();
    drop(manager);
    std::fs::remove_file(root.join("selected.json")).unwrap();
    let reopened = Manager::for_service(&root, Arc::new(MemoryVault::default()));
    let wrong = Query {
        digest: "b".repeat(64),
        ..query.clone()
    };
    assert_eq!(
        reopened
            .discard_config(&wrong, "valid-plan")
            .unwrap_err()
            .code,
        "permission_denied"
    );
    assert_eq!(count(&reopened), 1);
    assert_eq!(
        reopened.discard_config(&query, "valid-plan").unwrap()["state"],
        "discarded"
    );
    assert_eq!(reopened.extension_history().unwrap()["plans"], json!([]));
    assert_eq!(
        reopened.extension_history().unwrap()["planCapacity"]["remaining"],
        64
    );
    assert!(!root.join("selected.json").exists());
}

#[tokio::test]
async fn plan_mutations_share_the_plugin_gate_and_failed_discard_keeps_the_record() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let (manager, query, record) = host(&root).await;
    insert(&manager, &record);
    let gate = manager.gate(&query.plugin_id).unwrap();
    let guard = gate.lock().await;
    let plan: Plan = serde_json::from_value(record.clone()).unwrap();
    assert_eq!(
        manager.discard_config(&query, &plan.id).unwrap_err().code,
        "resource_limit"
    );
    assert_eq!(
        manager
            .apply_config(&query, &plan.id, &plan.grant.hash, &plan.provider)
            .unwrap_err()
            .code,
        "resource_limit"
    );
    assert_eq!(
        manager
            .commit_config_preview(serde_json::from_value(record).unwrap())
            .unwrap_err()
            .code,
        "resource_limit"
    );
    assert_eq!(count(&manager), 1);
    drop(guard);
    manager.discard_config(&query, &plan.id).unwrap();
    assert_eq!(count(&manager), 0);
}
