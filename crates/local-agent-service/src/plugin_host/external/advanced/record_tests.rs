use super::*;
use local_agent_core::{Store as CoreStore, plugin::external::InstallRequest, providers::vault};
use std::sync::Arc;

pub(super) fn host(root: &Path) -> (Manager, Query) {
    let folder = root.join("author");
    std::fs::create_dir(&folder).unwrap();
    let mut manifest: Manifest = serde_json::from_str(include_str!(
        "../../../../../../sdk/plugins/examples/json-config/package/manifest.json"
    ))
    .unwrap();
    manifest.runtime = Runtime::Declarative;
    manifest.capabilities = vec!["integration.connector@1".into(), "ui.contribution@1".into()];
    std::fs::write(
        folder.join("manifest.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
    std::fs::write(
        folder.join("extensions.json"),
        serde_json::to_vec(&Policy {
            schema_version: 1,
            protocol: None,
            config: None,
            connector: Some(Connector::McpStdio),
        })
        .unwrap(),
    )
    .unwrap();
    let archive = super::super::sdk::pack(
        &folder,
        Some(&ed25519_dalek::SigningKey::from_bytes(&[7; 32])),
    )
    .unwrap();
    let package = root.join("record-test.laplug");
    std::fs::write(&package, archive).unwrap();
    let manager = Manager::open(root, Arc::new(vault::MemoryVault::default())).unwrap();
    let review = manager.inspect(&package).unwrap();
    let installed = manager
        .install(&InstallRequest {
            package_ref: review.package_ref,
            digest: review.digest,
            approve_publisher: true,
        })
        .unwrap();
    let query = Query {
        plugin_id: installed.manifest.id,
        digest: installed.digest,
    };
    (manager, query)
}

pub(super) fn valid_plan(root: &Path, query: &Query) -> Value {
    let target = root.join("selected.json");
    std::fs::write(&target, b"{\"keep\":42}").unwrap();
    let stamp = local_agent_core::providers::files::read(&target)
        .unwrap()
        .stamp;
    let provider = CoreStore::open(root)
        .unwrap()
        .providers()
        .unwrap()
        .remove(0);
    let lease = json!({"id":query.plugin_id,"digest":query.digest,"epoch":"1","revision":"1"});
    json!({"id":"valid-plan","plugin_id":query.plugin_id,"digest":query.digest,
        "lease":lease,"grant":{"lease":lease,"path":target,"device":stamp.device,
        "inode":stamp.inode,"hash":package::hash(b"{\"keep\":42}"),"executable":null},
        "provider":provider,"edits":[],"credential_pointers":[],"stamp":stamp,
        "state":"prepared","created_at":"2026-10-27T00:00:00Z","after_hash":null,
        "backup":null,"backup_ref":null,"after_stamp":null})
}

#[test]
fn damaged_plan_never_becomes_an_empty_or_partially_successful_history() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let (manager, query) = host(&root);
    assert_eq!(manager.extension_history().unwrap()["plans"], json!([]));
    assert_eq!(
        manager.extension_status(&query).unwrap()["plans"],
        json!([])
    );
    let valid = valid_plan(&root, &query);
    for damaged in ["{credential=must-not-echo", "{}", "null"] {
        {
            let store = manager.advanced.lock().unwrap();
            store
                .db()
                .unwrap()
                .execute(
                    "INSERT OR REPLACE INTO plans VALUES('broken',?1,?2)",
                    params![query.plugin_id, damaged],
                )
                .unwrap();
        }
        assert_eq!(
            manager.extension_history().unwrap_err().code,
            "invalid_record"
        );
        assert_eq!(
            manager.extension_status(&query).unwrap_err().code,
            "invalid_record"
        );
        {
            let store = manager.advanced.lock().unwrap();
            let db = store.db().unwrap();
            db.execute(
                "INSERT OR REPLACE INTO plans VALUES('valid-plan',?1,?2)",
                params![query.plugin_id, valid.to_string()],
            )
            .unwrap();
            let kept: String = db
                .query_row("SELECT record FROM plans WHERE id='broken'", [], |r| {
                    r.get(0)
                })
                .unwrap();
            assert_eq!(kept, damaged);
        }
        assert_eq!(
            manager.extension_history().unwrap_err().code,
            "invalid_record"
        );
        assert_eq!(
            manager.extension_status(&query).unwrap_err().code,
            "invalid_record"
        );
        assert_eq!(
            manager.restore_config(&query, "broken").unwrap_err().code,
            "invalid_record"
        );
        assert_eq!(
            std::fs::read(root.join("selected.json")).unwrap(),
            b"{\"keep\":42}"
        );
        assert_eq!(manager.list().unwrap().len(), 1);
        manager
            .advanced
            .lock()
            .unwrap()
            .db()
            .unwrap()
            .execute("DELETE FROM plans WHERE id='broken'", [])
            .unwrap();
        assert_eq!(
            manager.extension_history().unwrap()["plans"][0]["planId"],
            "valid-plan"
        );
    }
}

#[test]
fn malformed_optional_resource_and_call_records_report_unreadable_metadata() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let (manager, query) = host(&root);
    assert!(manager.extension_status(&query).is_ok());
    {
        let store = manager.advanced.lock().unwrap();
        store
            .db()
            .unwrap()
            .execute(
                "INSERT INTO resources VALUES(?1,'{broken')",
                [&query.plugin_id],
            )
            .unwrap();
    }
    assert_eq!(
        manager.extension_status(&query).unwrap_err().code,
        "invalid_record"
    );
    {
        let store = manager.advanced.lock().unwrap();
        let db = store.db().unwrap();
        db.execute("DELETE FROM resources", []).unwrap();
        db.execute(
            "INSERT INTO connector_calls(plugin_id,identity,record) VALUES(?1,'broken','{broken')",
            [&query.plugin_id],
        )
        .unwrap();
    }
    assert_eq!(
        manager.extension_status(&query).unwrap_err().code,
        "invalid_record"
    );
    assert_eq!(manager.extension_history().unwrap()["plans"], json!([]));
    assert_eq!(manager.list().unwrap().len(), 1);
}

#[test]
fn unreadable_history_survives_restart_without_rewriting_or_hiding_records() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let (manager, query) = host(&root);
    manager
        .advanced
        .lock()
        .unwrap()
        .db()
        .unwrap()
        .execute(
            "INSERT INTO plans VALUES('broken',?1,'{}')",
            [&query.plugin_id],
        )
        .unwrap();
    assert_eq!(
        manager.extension_history().unwrap_err().code,
        "invalid_record"
    );
    drop(manager);
    let reopened = Manager::open(&root, Arc::new(vault::MemoryVault::default())).unwrap();
    assert_eq!(
        reopened.extension_history().unwrap_err().code,
        "invalid_record"
    );
    let store = reopened.advanced.lock().unwrap();
    let kept: String = store
        .db()
        .unwrap()
        .query_row("SELECT record FROM plans WHERE id='broken'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(kept, "{}");
}

#[test]
fn sqlite_query_failure_is_not_a_confirmed_empty_history() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let (manager, _) = host(&root);
    assert!(manager.extension_history().is_ok());
    manager
        .advanced
        .lock()
        .unwrap()
        .db()
        .unwrap()
        .execute("ALTER TABLE plans RENAME TO preserved_plans", [])
        .unwrap();
    assert_eq!(manager.extension_history().unwrap_err().code, "unavailable");
    assert_eq!(manager.list().unwrap().len(), 1);
}
