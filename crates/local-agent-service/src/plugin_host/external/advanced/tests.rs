use super::*;

#[test]
fn retention_inventory_does_not_initialize_the_lazy_extension_database_or_namespace() {
    use local_agent_core::{observation::retention::impact, providers::vault::MemoryVault};
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().canonicalize().unwrap();
    let manager = Manager::open(&dir, std::sync::Arc::new(MemoryVault::default())).unwrap();
    assert!(manager.advanced.lock().unwrap().db.get().is_none());
    assert_eq!(
        manager
            .retention_tool_impact(30, chrono::Utc::now())
            .unwrap_err(),
        impact::Error::SourceAbsent
    );
    assert!(manager.advanced.lock().unwrap().db.get().is_none());
    assert!(!dir.join("plugin-extensions.sqlite3").exists());
}

fn sample(name: &str) -> (Manifest, BTreeMap<String, Vec<u8>>) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../sdk/plugins/examples")
        .join(name)
        .join("package");
    let manifest: Manifest =
        serde_json::from_slice(&std::fs::read(root.join("manifest.json")).unwrap()).unwrap();
    let files = BTreeMap::from([(
        "extensions.json".into(),
        std::fs::read(root.join("extensions.json")).unwrap(),
    )]);
    (manifest, files)
}
#[test]
fn advanced_permissions_are_separate_and_cannot_expand_into_basic_or_native_execution() {
    for name in ["envelope-protocol", "json-config", "mcp-stdio", "mcp-http"] {
        let (mut m, files) = sample(name);
        validate_manifest(&m).unwrap();
        validate_policy(&m, &files).unwrap();
        m.manifest_version = 99;
        assert!(package::validate(&m).is_err());
    }
    let (m, mut files) = sample("json-config");
    let mut policy: Policy = serde_json::from_slice(&files["extensions.json"]).unwrap();
    policy.protocol = Some(Protocol {
        base_protocol: "responses".into(),
        path: "/x".into(),
    });
    files.insert(
        "extensions.json".into(),
        serde_json::to_vec(&policy).unwrap(),
    );
    assert!(validate_policy(&m, &files).is_err());
    for pointer in [
        "/../private",
        "//key",
        "/env/~1escape",
        "/constructor/value",
        "/env/__proto__",
    ] {
        assert!(!safe_pointer(pointer), "{pointer}");
    }
    for path in ["/../file", "//host?query", "/a#token", "/foo%2fbar"] {
        assert!(!safe_route(path), "{path}");
    }
}
#[test]
fn extension_schemas_have_a_separate_authority() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../contracts/plugins/extensions");
    for (name, schema) in schemas().as_object().unwrap() {
        let stored: Value = serde_json::from_slice(
            &std::fs::read(path.join(format!("{name}.schema.json"))).unwrap(),
        )
        .unwrap();
        assert_eq!(*schema, stored, "{name}");
    }
}
#[test]
fn dsh_without_a_verified_target_never_admits_a_connection() {
    assert_eq!(
        connector::validate_connection(
            &Connector::DshSessions,
            &Connection::Dsh {
                url: "http://127.0.0.1:1".into(),
                launch_token: "not-a-real-token".into(),
                version: "unverified".into()
            }
        )
        .unwrap_err()
        .code,
        "unverified_connector"
    );
}

#[test]
fn credential_echo_is_blocked_before_guest_delivery_across_raw_and_json_chunks() {
    for wire in [
        b"prefix fake-secret suffix".to_vec(),
        br"prefix fa\u006be-\u0073ecret suffix".to_vec(),
    ] {
        for width in 1..wire.len() {
            let mut filter = protocol::SecretFilter {
                secret: b"fake-secret".to_vec(),
                pending: vec![],
            };
            let mut rejected = false;
            for chunk in wire.chunks(width) {
                if filter.push(chunk, false).is_err() {
                    rejected = true;
                    break;
                }
            }
            assert!(rejected, "width {width}");
        }
    }
    let mut filter = protocol::SecretFilter {
        secret: b"fake-secret".to_vec(),
        pending: vec![],
    };
    assert_eq!(
        filter.push(b"data: ordinary-event\n\n", false).unwrap(),
        b"data: ordinary-event\n\n"
    );
    assert_eq!(filter.push(b"fa", false).unwrap(), b"");
    assert_eq!(filter.push(b"lse", false).unwrap(), b"false");
}

#[tokio::test]
async fn damaged_optional_metadata_does_not_break_original_or_fallback_bound_traffic() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().canonicalize().unwrap();
    std::fs::write(
        path.join("plugin-extensions.sqlite3"),
        b"damaged optional metadata",
    )
    .unwrap();
    let manager = std::sync::Arc::new(
        Manager::open(
            &path,
            std::sync::Arc::new(local_agent_core::providers::vault::MemoryVault::default()),
        )
        .unwrap(),
    );
    assert!(manager.list().unwrap().is_empty());
    assert!(manager.extension_history().is_err());
    let provider = local_agent_core::Store::open(&path)
        .unwrap()
        .providers()
        .unwrap()
        .remove(0);
    assert!(
        protocol::prepare(manager.clone(), &provider, b"{}")
            .await
            .unwrap()
            .is_none()
    );
    let binding = protocol::Binding {
        lease: Lease {
            id: "org.example.missing".into(),
            digest: "a".repeat(64),
            epoch: "1".into(),
            revision: "1".into(),
        },
        provider_id: provider.id.clone(),
        endpoint: provider.endpoint.clone(),
        revision: provider.revision,
        credential_ref: provider.credential_ref.clone(),
        protocol: provider.protocol.clone(),
        allow_private: false,
    };
    manager
        .advanced
        .lock()
        .unwrap()
        .save_bindings(&BTreeMap::from([(provider.id.clone(), binding)]))
        .unwrap();
    assert!(
        protocol::prepare(manager.clone(), &provider, b"{}")
            .await
            .is_err()
    );
    assert_eq!(
        std::fs::read(path.join("plugin-extensions.sqlite3")).unwrap(),
        b"damaged optional metadata"
    );
}

#[test]
fn encrypted_host_restore_survives_package_removal_and_refuses_changed_files() {
    use local_agent_core::providers::{
        files,
        vault::{self, Vault},
    };
    let root = tempfile::tempdir().unwrap();
    let vault = std::sync::Arc::new(vault::MemoryVault::default());
    let canonical = root.path().canonicalize().unwrap();
    let manager = Manager::open(&canonical, vault.clone()).unwrap();
    let core = local_agent_core::Store::open(&canonical).unwrap();
    let provider = core.providers().unwrap().remove(0);
    let path = canonical.join("selected.json");
    let before = b"{\"key\":\"old-fixture-secret\"}";
    let after = b"{\"key\":\"new-fixture-secret\"}";
    std::fs::write(&path, after).unwrap();
    let stamp = files::read(&path).unwrap().stamp;
    let key = vault::random_key();
    vault.set("backup-reference", &key).unwrap();
    let q = Query {
        plugin_id: "org.example.removed".into(),
        digest: "a".repeat(64),
    };
    let lease = json!({"id":q.plugin_id,"digest":q.digest,"epoch":"1","revision":"1"});
    let plan = json!({"id":"restore-test","plugin_id":q.plugin_id,"digest":q.digest,"lease":lease,"grant":{"lease":lease,"path":path,"device":stamp.device,"inode":stamp.inode,"hash":package::hash(before),"executable":null},"provider":provider,"edits":[],"credential_pointers":[],"stamp":stamp,"state":"applied","created_at":chrono::Utc::now().to_rfc3339(),"after_hash":package::hash(after),"backup":vault::seal(&key,before,"restore-test").unwrap(),"backup_ref":"backup-reference","after_stamp":stamp});
    manager
        .advanced
        .lock()
        .unwrap()
        .db()
        .unwrap()
        .execute(
            "INSERT INTO plans VALUES(?1,?2,?3)",
            params!["restore-test", q.plugin_id, plan.to_string()],
        )
        .unwrap();
    let result = manager.restore_config(&q, "restore-test").unwrap();
    assert_eq!(result["state"], "restored");
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert!(!result.to_string().contains("fixture-secret"));
    assert_eq!(
        manager.restore_config(&q, "restore-test").unwrap()["state"],
        "restored"
    );
    std::fs::write(&path, after).unwrap();
    let mut changed = plan;
    changed["id"] = json!("conflict-test");
    changed["state"] = json!("applied");
    manager
        .advanced
        .lock()
        .unwrap()
        .db()
        .unwrap()
        .execute(
            "INSERT INTO plans VALUES(?1,?2,?3)",
            params!["conflict-test", q.plugin_id, changed.to_string()],
        )
        .unwrap();
    std::fs::write(&path, b"{\"user\":\"new-content\"}").unwrap();
    assert_eq!(
        manager
            .restore_config(&q, "conflict-test")
            .unwrap_err()
            .code,
        "stale_snapshot"
    );
    assert_eq!(std::fs::read(&path).unwrap(), b"{\"user\":\"new-content\"}");
}
