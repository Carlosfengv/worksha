use super::*;
use ed25519_dalek::SigningKey;
use std::{
    io::{Cursor, Write},
    path::Path,
};

pub(super) fn manifest() -> Manifest {
    let mut value: Manifest = serde_json::from_str(include_str!(
        "../../../../../sdk/plugins/examples/aurora-agent/package/manifest.json"
    ))
    .unwrap();
    value.runtime = Runtime::Declarative;
    value.capabilities = vec!["ui.contribution@1".into()];
    value.permissions = vec![Permission::UiContribute];
    value.requires.clear();
    value.agent = None;
    value.views.retain(|v| v.kind == ViewKind::Settings);
    value
}
#[test]
fn quota_broker_rejects_decoded_json_credential_echoes() {
    for body in [
        r#"{"balance":"fixture/key"}"#,
        r#"{"balance":"fixture\/key"}"#,
        r#"{"nested":["\u0066ixture/key"]}"#,
        r#"{"\u0066ixture/key":0}"#,
    ] {
        assert!(broker::body_contains_secret(
            body.as_bytes(),
            b"fixture/key"
        ));
    }
    assert!(!broker::body_contains_secret(
        br#"{"balance":"12.50"}"#,
        b"fixture/key"
    ));
}

#[tokio::test]
async fn dependency_graph_detects_disabled_cycles_and_missing_active_dependencies() {
    let root = tempfile::tempdir().unwrap();
    let host = new_host(root.path());
    let mut a = manifest();
    a.id = "org.example.dependency-a".into();
    a.requires = vec!["plugin:org.example.dependency-b".into()];
    let sa = install(&host, root.path(), &a);
    assert_eq!(
        host.change(StateRequest {
            plugin_id: sa.manifest.id.clone(),
            digest: sa.digest.clone(),
            action: StateAction::Enable
        })
        .await
        .unwrap_err()
        .code,
        "dependency_missing"
    );
    let mut b = manifest();
    b.id = "org.example.dependency-b".into();
    b.requires = vec!["plugin:org.example.dependency-a".into()];
    install(&host, root.path(), &b);
    assert_eq!(
        host.change(StateRequest {
            plugin_id: sa.manifest.id,
            digest: sa.digest,
            action: StateAction::Enable
        })
        .await
        .unwrap_err()
        .code,
        "dependency_cycle"
    );
}

#[tokio::test]
async fn settings_invalidate_selected_snapshots_and_recovery_removes_uncommitted_bytes() {
    let root = tempfile::tempdir().unwrap();
    let host = new_host(root.path());
    let s = install(&host, root.path(), &manifest());
    let s = host
        .change(StateRequest {
            plugin_id: s.manifest.id.clone(),
            digest: s.digest.clone(),
            action: StateAction::Enable,
        })
        .await
        .unwrap();
    let mut values = s.settings.clone();
    values.insert("title-style".into(), "with-model".into());
    let changed = host
        .save_private_settings(&s.manifest.id, &s.digest, &values)
        .unwrap();
    assert_ne!(changed.grant_revision, s.grant_revision);
    assert_eq!(
        host.save_private_settings(&s.manifest.id, &s.digest, &values)
            .unwrap()
            .grant_revision,
        changed.grant_revision
    );
    let review_path = root.path().join("review.laplug");
    std::fs::write(
        &review_path,
        create_package(&root.path().join("author-2"), &manifest()),
    )
    .unwrap();
    let review = host.inspect(&review_path).unwrap();
    let staging = host.root.join("staging").join(&review.package_ref);
    let orphan = host
        .root
        .join("packages/org.example.orphan/1.0.0")
        .join("f".repeat(64));
    std::fs::create_dir_all(&orphan).unwrap();
    std::fs::write(orphan.join("package.laplug"), b"uncommitted").unwrap();
    drop(host);
    let host = new_host(root.path());
    assert!(!staging.exists());
    assert!(!orphan.exists());
    assert_eq!(
        host.install(&InstallRequest {
            package_ref: review.package_ref,
            digest: review.digest,
            approve_publisher: true
        })
        .unwrap_err()
        .code,
        "stale_snapshot"
    );
    assert!(host.status(&s.manifest.id, &s.digest).unwrap().enabled);
}
fn create_package(root: &Path, manifest: &Manifest) -> Vec<u8> {
    std::fs::create_dir_all(root).unwrap();
    std::fs::write(
        root.join("manifest.json"),
        serde_json::to_vec(manifest).unwrap(),
    )
    .unwrap();
    sdk::pack(root, Some(&SigningKey::from_bytes(&[7; 32]))).unwrap()
}
#[test]
fn unified_upload_dispatches_signed_capabilities_without_installing_or_running() {
    let root = tempfile::tempdir().unwrap();
    let host = new_host(root.path());
    let source: Manifest = serde_json::from_str(include_str!(
        "../../../../../sdk/plugins/examples/full-history/package/manifest.json"
    ))
    .unwrap();
    let policy: Manifest = serde_json::from_str(include_str!(
        "../../../../../sdk/plugins/examples/pure-policy/package/manifest.json"
    ))
    .unwrap();
    for (kind, manifest) in [
        ("plugin", manifest()),
        ("source", source),
        ("policy", policy),
    ] {
        let author = root.path().join(kind);
        std::fs::create_dir(&author).unwrap();
        if matches!(manifest.runtime, Runtime::WasmComponent { .. }) {
            std::fs::write(author.join("plugin.wasm"), b"\0asm\x0d\0\x01\0").unwrap();
        }
        let archive = create_package(&author, &manifest);
        let path = root.path().join(format!("{kind}.laplug"));
        std::fs::write(&path, &archive).unwrap();
        let result = host.inspect_upload(&path).unwrap();
        assert_eq!(result["kind"], kind);
        assert_eq!(result["review"]["digest"], package::hash(&archive));
        assert_eq!(result["review"]["manifest"]["id"], manifest.id);
        match kind {
            "plugin" => {
                serde_json::from_value::<PackageReview>(result["review"].clone()).unwrap();
            }
            "source" => {
                serde_json::from_value::<local_agent_core::plugin::full::Review>(
                    result["review"].clone(),
                )
                .unwrap();
            }
            _ => {
                serde_json::from_value::<local_agent_core::plugin::policy::Review>(
                    result["review"].clone(),
                )
                .unwrap();
            }
        }
    }
    assert!(host.list().unwrap().is_empty());
    assert!(host.full_list().unwrap().is_empty());
    assert!(host.policy_inventory().unwrap().is_empty());
}

#[test]
fn unified_upload_keeps_signature_and_package_admission_fail_closed() {
    let root = tempfile::tempdir().unwrap();
    let host = new_host(root.path());
    let author = root.path().join("author");
    let archive = create_package(&author, &manifest());
    let mut corrupted = archive;
    corrupted[0] ^= 1;
    let path = root.path().join("bad.laplug");
    std::fs::write(&path, corrupted).unwrap();
    assert!(host.inspect_upload(&path).is_err());
    let unsigned = sdk::pack(&author, None).unwrap();
    std::fs::write(&path, unsigned).unwrap();
    assert_eq!(
        host.inspect_upload(&path).unwrap_err().code,
        "publisher_untrusted"
    );
    assert!(host.list().unwrap().is_empty());
}
#[test]
fn diagnostics_are_read_only_exact_version_scoped_and_reset_on_restart() {
    use super::diagnostics::Operation;
    let root = tempfile::tempdir().unwrap();
    let host = new_host(root.path());
    let installed = install(&host, root.path(), &manifest());
    let items = host.list().unwrap();
    let initial = host.diagnostics(&items);
    assert!(initial["items"][0]["activity"].is_null());
    assert_eq!(initial, host.diagnostics(&items));
    let mut success = host.observe(&installed.manifest.id, &installed.digest, Operation::Enable);
    assert_eq!(
        host.diagnostics(&items)["items"][0]["activity"]["inFlight"],
        "1"
    );
    success.finish(&Ok(()));
    for code in [
        "deadline_exceeded",
        "resource_limit",
        "worker_crashed",
        "permission_denied",
    ] {
        host.observe(
            &installed.manifest.id,
            &installed.digest,
            Operation::Refresh,
        )
        .finish(&Err::<(), _>(fail(code)));
    }
    drop(host.observe(
        &installed.manifest.id,
        &installed.digest,
        Operation::ReadSession,
    ));
    let final_state = host.diagnostics(&items);
    let a = &final_state["items"][0]["activity"];
    assert_eq!(a["attempts"], "6");
    assert_eq!(a["succeeded"], "1");
    assert_eq!(a["failed"], "4");
    assert_eq!(a["interrupted"], "1");
    assert_eq!(a["inFlight"], "0");
    assert_eq!(a["timeouts"], "1");
    assert_eq!(a["resourceLimits"], "1");
    assert_eq!(a["workerCrashes"], "1");
    assert_eq!(a["lastErrorCode"], "interrupted");
    assert!(a["lastSuccessAt"].is_string());
    assert_eq!(
        serde_json::to_value(host.list().unwrap()).unwrap(),
        serde_json::to_value(&items).unwrap()
    );
    drop(host.observe(&installed.manifest.id, &"0".repeat(64), Operation::Refresh));
    assert_eq!(final_state, host.diagnostics(&items));
    let serialized = serde_json::to_string(&final_state).unwrap();
    for secret in [
        "sourceLabel",
        "connectionLabel",
        "signingKey",
        "settings",
        "sessions",
        "credential",
    ] {
        assert!(!serialized.contains(secret));
    }
    drop(host);
    let reopened = new_host(root.path());
    assert_eq!(
        serde_json::to_value(reopened.list().unwrap()).unwrap(),
        serde_json::to_value(&items).unwrap()
    );
    assert!(reopened.diagnostics(&items)["items"][0]["activity"].is_null());
}

#[test]
fn diagnostics_keep_snapshot_unknown_empty_stale_and_revoked_distinct() {
    let root = tempfile::tempdir().unwrap();
    let host = new_host(root.path());
    let mut status = install(&host, root.path(), &manifest());
    status.manifest.capabilities = vec!["session.source@1".into()];
    status.enabled = true;
    let read = |status: &Status| {
        host.diagnostics(std::slice::from_ref(status))["items"][0]["snapshot"].clone()
    };
    assert_eq!(read(&status)["state"], "awaiting-authorization");
    status.source_authorized = true;
    assert_eq!(read(&status)["state"], "not-read");
    host.cache.lock().unwrap().data.insert(
        status.manifest.id.clone(),
        Cached {
            observed_at: chrono::Utc::now().to_rfc3339(),
            report: None,
            collected_at: std::time::Instant::now(),
            digest: status.digest.clone(),
            epoch: status.activation_epoch.clone(),
            revision: status.grant_revision.clone(),
            snapshot: "complete".into(),
            sessions: vec![],
            source_revision: Some("fixture-source".into()),
            quota: None,
        }
        .into(),
    );
    assert_eq!(read(&status)["state"], "confirmed-empty");
    assert_eq!(read(&status)["records"], "0");
    status.error_code = Some("invalid_input".into());
    assert_eq!(read(&status)["state"], "stale");
    assert!(read(&status)["observedAt"].is_string());
    status.error_code = None;
    std::sync::Arc::make_mut(
        host.cache
            .lock()
            .unwrap()
            .data
            .get_mut(&status.manifest.id)
            .unwrap(),
    )
    .collected_at -= std::time::Duration::from_secs(301);
    assert_eq!(read(&status)["state"], "stale");
    status.grant_revision = "replaced-revision".into();
    assert_eq!(read(&status)["state"], "not-read");
    assert!(read(&status)["observedAt"].is_null());
    assert!(read(&status)["records"].is_null());
    status.source_authorized = false;
    assert_eq!(read(&status)["state"], "awaiting-authorization");
    status.enabled = false;
    assert_eq!(read(&status)["state"], "disabled");
}

pub(super) fn new_host(root: &Path) -> Arc<Manager> {
    Arc::new(
        Manager::open(
            root,
            Arc::new(local_agent_core::providers::vault::MemoryVault::default()),
        )
        .unwrap(),
    )
}

#[test]
fn plugin_gates_reclaim_idle_identities() {
    let root = tempfile::tempdir().unwrap();
    let host = new_host(root.path());
    for index in 0..256 {
        drop(host.gate(&format!("missing-plugin-{index}")).unwrap());
    }
    assert_eq!(host.gates.lock().unwrap().len(), 1);
}

#[test]
fn plugin_gates_keep_the_same_lock_while_an_owned_guard_is_alive() {
    let root = tempfile::tempdir().unwrap();
    let host = new_host(root.path());
    let gate = host.gate("active-plugin").unwrap();
    let weak = Arc::downgrade(&gate);
    let guard = gate.try_lock_owned().unwrap();
    for index in 0..16 {
        drop(host.gate(&format!("other-plugin-{index}")).unwrap());
    }
    let gate = host.gate("active-plugin").unwrap();
    assert!(Arc::ptr_eq(&gate, &weak.upgrade().unwrap()));
    assert!(gate.clone().try_lock_owned().is_err());
    drop(guard);
    let guard = gate.try_lock_owned().unwrap();
    drop(guard);
    assert!(weak.upgrade().is_none());
    assert!(host.gate("active-plugin").unwrap().try_lock_owned().is_ok());
}

pub(super) fn install(host: &Manager, root: &Path, manifest: &Manifest) -> Status {
    let bytes = create_package(&root.join(uuid::Uuid::new_v4().to_string()), manifest);
    let path = root.join(format!("{}.laplug", uuid::Uuid::new_v4()));
    std::fs::write(&path, bytes).unwrap();
    let review = host.inspect(&path).unwrap();
    host.install(&InstallRequest {
        package_ref: review.package_ref,
        digest: review.digest,
        approve_publisher: true,
    })
    .unwrap()
}

#[tokio::test(flavor = "current_thread")]
async fn cancelled_activation_keeps_graph_and_plugin_guards_until_the_commit_worker_finishes() {
    use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
    use std::time::Duration;
    let temp = tempfile::tempdir().unwrap();
    let host = new_host(temp.path());
    let installed = install(&host, temp.path(), &manifest());
    let (entered, ready) = tokio::sync::oneshot::channel();
    let mut entered = Some(entered);
    let (release, gate) = std::sync::mpsc::channel();
    host.db
        .lock()
        .unwrap()
        .authorizer(Some(move |context: AuthContext<'_>| {
            if matches!(
                context.action,
                AuthAction::Update {
                    table_name: "selections",
                    ..
                }
            ) && let Some(entered) = entered.take()
            {
                let _ = entered.send(());
                let _ = gate.recv_timeout(Duration::from_secs(4));
            }
            Authorization::Allow
        }))
        .unwrap();
    let change = tokio::spawn({
        let host = host.clone();
        let installed = installed.clone();
        async move {
            host.change(StateRequest {
                plugin_id: installed.manifest.id,
                digest: installed.digest,
                action: StateAction::Enable,
            })
            .await
        }
    });
    tokio::time::timeout(Duration::from_secs(2), ready)
        .await
        .unwrap()
        .unwrap();
    change.abort();
    assert!(change.await.unwrap_err().is_cancelled());
    assert!(host.activation.try_lock().is_err());
    assert!(
        host.gate(&installed.manifest.id)
            .unwrap()
            .try_lock()
            .is_err()
    );
    release.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if host.activation.try_lock().is_ok() {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(
        host.status(&installed.manifest.id, &installed.digest)
            .unwrap()
            .enabled
    );
}

#[tokio::test]
async fn authorization_commit_rejects_a_revoked_or_replaced_validation_snapshot() {
    use super::store::{Authorization, ConnectionGrant, SourceGrant};
    for operation in ["source", "connection", "report"] {
        for transition in ["revoke", "reenable", "upgrade"] {
            let root = tempfile::tempdir().unwrap();
            let host = new_host(root.path());
            let initial = install(&host, root.path(), &manifest());
            host.change(StateRequest {
                plugin_id: initial.manifest.id.clone(),
                digest: initial.digest.clone(),
                action: StateAction::Enable,
            })
            .await
            .unwrap();
            let before = host.selection(&initial.manifest.id).unwrap();
            match transition {
                "revoke" => host
                    .revoke(&initial.manifest.id, &initial.digest, false)
                    .unwrap(),
                "reenable" => {
                    host.revoke(&initial.manifest.id, &initial.digest, true)
                        .unwrap();
                    host.change(StateRequest {
                        plugin_id: initial.manifest.id.clone(),
                        digest: initial.digest.clone(),
                        action: StateAction::Enable,
                    })
                    .await
                    .unwrap();
                }
                _ => {
                    let mut next = manifest();
                    next.version = "1.1.0".into();
                    let next = install(&host, root.path(), &next);
                    host.change(StateRequest {
                        plugin_id: next.manifest.id,
                        digest: next.digest,
                        action: StateAction::Enable,
                    })
                    .await
                    .unwrap();
                }
            }
            let grant = match operation {
                "source" => Authorization::Source(SourceGrant {
                    path: root.path().join("selected.jsonl"),
                    device: 1,
                    inode: 1,
                }),
                "connection" => Authorization::Connection(ConnectionGrant {
                    provider_id: "fixture".into(),
                    endpoint: "https://example.invalid/common".into(),
                    credential_ref: None,
                    revision: 1,
                    allow_private_network: false,
                }),
                _ => Authorization::Report,
            };
            assert_eq!(
                host.commit_authorization(&initial.manifest.id, &before, grant)
                    .unwrap_err()
                    .code,
                "grant_revoked",
                "{operation}/{transition}"
            );
            let after = host.selection(&initial.manifest.id).unwrap();
            assert!(after.source.is_none() && after.connection.is_none() && !after.report);
        }
    }
}
fn archive(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut archive = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, bytes) in entries {
        archive
            .start_file(*name, zip::write::SimpleFileOptions::default())
            .unwrap();
        archive.write_all(bytes).unwrap();
    }
    archive.finish().unwrap().into_inner()
}
#[test]
fn signature_checks_cover_payload_changes_and_untrusted_development() {
    let temp = tempfile::tempdir().unwrap();
    let bytes = create_package(temp.path(), &manifest());
    let package = package::inspect(&bytes, false).unwrap();
    assert!(package.signing_key.is_some());
    let mut entries = package.files;
    entries.insert(
        "manifest.json".into(),
        serde_json::to_vec(&serde_json::json!({"malicious":true})).unwrap(),
    );
    let list: Vec<_> = entries
        .iter()
        .map(|(n, b)| (n.as_str(), b.as_slice()))
        .collect();
    assert!(package::inspect(&archive(&list), false).is_err());
    let raw = sdk::pack(temp.path(), None).unwrap();
    assert_eq!(
        package::inspect(&raw, false).err().unwrap().code,
        "publisher_untrusted"
    );
    assert!(package::inspect(&raw, true).is_ok());
}
#[test]
fn archive_paths_duplicates_symlinks_and_expansion_are_bounded() {
    for name in [
        "../escape",
        "/absolute",
        "a/../../escape",
        "C:drive",
        "a\\b",
        "a//b",
        "a/./b",
        "é.json",
    ] {
        assert_eq!(
            package::inspect(&archive(&[(name, b"x")]), true)
                .err()
                .unwrap()
                .code,
            "invalid_input"
        );
    }
    assert!(package::inspect(&archive(&[("one.json", b"1"), ("ONE.json", b"2")]), true).is_err());
    let mut bytes = archive(&[("link", b"x")]);
    let central = bytes.windows(4).position(|b| b == b"PK\x01\x02").unwrap();
    bytes[central + 38..central + 42].copy_from_slice(&((0o120777u32) << 16).to_le_bytes());
    assert!(package::inspect(&bytes, true).is_err());
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    writer
        .start_file(
            "large",
            zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated),
        )
        .unwrap();
    writer.write_all(&vec![b'x'; 33 * 1_048_576]).unwrap();
    let bytes = writer.finish().unwrap().into_inner();
    assert!(bytes.len() < 1_048_576);
    assert_eq!(
        package::inspect(&bytes, true).err().unwrap().code,
        "resource_limit"
    );
}
#[test]
fn permissions_namespaces_versions_and_unknown_fields_fail_closed() {
    let base = manifest();
    let mut m = base.clone();
    m.id = "builtin.claude".into();
    assert!(package::validate(&m).is_err());
    let mut m = base.clone();
    m.host_api = "^1.1".into();
    assert_eq!(
        package::validate(&m).err().unwrap().code,
        "incompatible_api"
    );
    let mut m = base.clone();
    m.permissions.push(Permission::SourceContentRead {
        resource: "history".into(),
    });
    m.permissions.push(Permission::ConnectionRequest {
        path: "/quota".into(),
    });
    assert_eq!(
        package::validate(&m).err().unwrap().code,
        "permission_denied"
    );
    for path in ["/../token", "/quota?url=evil", "//host/path", "/a%2fb"] {
        let mut m = base.clone();
        m.permissions
            .push(Permission::ConnectionRequest { path: path.into() });
        assert!(package::validate(&m).is_err());
    }
    let mut value = serde_json::to_value(base).unwrap();
    value["installScript"] = "arbitrary.sh".into();
    assert!(serde_json::from_value::<Manifest>(value).is_err());
    assert!(serde_json::from_value::<ActionRequest>(serde_json::json!({"pluginId":"org.example.a","digest":"x","action":{"kind":"execute-shell","command":"id"}})).is_err());
    assert!(serde_json::from_value::<Quota>(serde_json::json!({"remaining":9007199254740993u64,"limit":null,"unit":"tokens","observedAt":"2026-10-23T00:00:00Z","expiresAt":null})).is_err());
}
#[tokio::test]
async fn declarative_lifecycle_preserves_settings_and_recovers_uncommitted_install() {
    let root = tempfile::tempdir().unwrap();
    let host = new_host(root.path());
    let initial = install(&host, root.path(), &manifest());
    assert!(!initial.enabled);
    let active = host
        .change(StateRequest {
            plugin_id: initial.manifest.id.clone(),
            digest: initial.digest.clone(),
            action: StateAction::Enable,
        })
        .await
        .unwrap();
    assert!(active.enabled);
    let epoch = active.activation_epoch.clone();
    assert_eq!(
        host.change(StateRequest {
            plugin_id: active.manifest.id.clone(),
            digest: active.digest.clone(),
            action: StateAction::Enable
        })
        .await
        .unwrap()
        .activation_epoch,
        epoch
    );
    let values = BTreeMap::from([("title-style".into(), "with-model".into())]);
    host.save_private_settings(&active.manifest.id, &active.digest, &values)
        .unwrap();
    let mut next = manifest();
    next.version = "1.1.0".into();
    let next = install(&host, root.path(), &next);
    assert!(!next.enabled);
    assert!(
        host.status(&active.manifest.id, &active.digest)
            .unwrap()
            .enabled
    );
    let changed = host
        .change(StateRequest {
            plugin_id: next.manifest.id.clone(),
            digest: next.digest.clone(),
            action: StateAction::Enable,
        })
        .await
        .unwrap();
    assert!(changed.enabled);
    let old = host
        .change(StateRequest {
            plugin_id: changed.manifest.id.clone(),
            digest: changed.digest.clone(),
            action: StateAction::Rollback,
        })
        .await
        .unwrap();
    assert_eq!(old.digest, active.digest);
    assert_eq!(old.settings, values);
    host.db
        .lock()
        .unwrap()
        .execute(
            "INSERT INTO operations VALUES('interrupted-test','x','publishing','{}')",
            [],
        )
        .unwrap();
    drop(host);
    let reopened = new_host(root.path());
    assert!(
        reopened
            .status(&old.manifest.id, &old.digest)
            .unwrap()
            .enabled
    );
    let phase: String = reopened
        .db
        .lock()
        .unwrap()
        .query_row(
            "SELECT phase FROM operations WHERE id='interrupted-test'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(phase, "interrupted");
}
#[test]
fn checked_package_is_revalidated_and_publisher_key_is_pinned() {
    let root = tempfile::tempdir().unwrap();
    let host = new_host(root.path());
    let folder = root.path().join("author");
    let bytes = create_package(&folder, &manifest());
    let path = root.path().join("package.laplug");
    std::fs::write(&path, &bytes).unwrap();
    let review = host.inspect(&path).unwrap();
    let staging = host
        .root
        .join("staging")
        .join(&review.package_ref)
        .join("package.laplug");
    std::fs::write(&staging, b"changed after inspect").unwrap();
    assert!(
        host.install(&InstallRequest {
            package_ref: review.package_ref,
            digest: review.digest,
            approve_publisher: true
        })
        .is_err()
    );
    let installed = install(&host, root.path(), &manifest());
    assert!(
        host.verified(&installed.manifest.id, &installed.digest)
            .is_ok()
    );
    let other = sdk::pack(&folder, Some(&SigningKey::from_bytes(&[8; 32]))).unwrap();
    std::fs::write(&path, other).unwrap();
    assert_eq!(host.inspect(&path).err().unwrap().code, "publisher_changed");
    let p = host
        .installed(&installed.manifest.id, &installed.digest)
        .unwrap();
    std::fs::write(&p.path, b"package replaced").unwrap();
    assert!(
        host.verified(&installed.manifest.id, &installed.digest)
            .is_err()
    );
}
#[test]
fn schemas_are_the_runtime_authority_and_sdk_wit_matches() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../contracts/plugins");
    for (name, schema) in schemas().as_object().unwrap() {
        let committed: serde_json::Value = serde_json::from_slice(
            &std::fs::read(root.join(format!("{name}.schema.json"))).unwrap(),
        )
        .unwrap();
        assert_eq!(&committed, schema, "{name}");
    }
    assert_eq!(
        std::fs::read(root.join("plugin.wit")).unwrap(),
        std::fs::read(root.join("../../sdk/plugins/wit/plugin.wit")).unwrap()
    );
}
#[test]
fn protocol_frames_are_bounded_before_allocation() {
    assert!(worker::read_line(&mut Cursor::new(vec![b'x'; MAX_FRAME + 1])).is_err());
    assert!(worker::read_line(&mut Cursor::new(b"partial")).is_err());
    assert_eq!(
        worker::read_line(&mut Cursor::new(b"{}\nnext\n")).unwrap(),
        b"{}\n"
    );
}

fn contribution_manifest(id: &str, dependency: &str) -> (Manifest, serde_json::Value) {
    let mut m = manifest();
    m.id = id.into();
    m.requires = vec![
        local_agent_core::plugin::contributions::OPTIONAL.into(),
        local_agent_core::plugin::contributions::DETAILS.into(),
    ];
    let mut extra = m.views[0].clone();
    extra.id = "enhanced".into();
    m.views.push(extra);
    let c = serde_json::json!({"schemaVersion":1,"optionalDependencies":[{"pluginId":dependency,"version":"^1.0","views":["enhanced"]}],"detailSlots":[{"id":"extra","target":"agent","objectIds":["claude-code-cli"],"placement":"tab","viewId":"enhanced","title":"附加信息"}]});
    (m, c)
}
fn install_contributions(
    host: &Manager,
    root: &Path,
    m: &Manifest,
    c: &serde_json::Value,
) -> Status {
    let folder = root.join(uuid::Uuid::new_v4().to_string());
    std::fs::create_dir(&folder).unwrap();
    std::fs::write(folder.join("manifest.json"), serde_json::to_vec(m).unwrap()).unwrap();
    std::fs::write(
        folder.join("contributions.json"),
        serde_json::to_vec(c).unwrap(),
    )
    .unwrap();
    let bytes = sdk::pack(&folder, Some(&SigningKey::from_bytes(&[7; 32]))).unwrap();
    let file = folder.with_extension("laplug");
    std::fs::write(&file, bytes).unwrap();
    let p = host.inspect(&file).unwrap();
    host.install(&InstallRequest {
        package_ref: p.package_ref,
        digest: p.digest,
        approve_publisher: true,
    })
    .unwrap()
}
async fn contribution_state(
    host: &Arc<Manager>,
    s: &Status,
    action: StateAction,
) -> Result<Status> {
    host.change(StateRequest {
        plugin_id: s.manifest.id.clone(),
        digest: s.digest.clone(),
        action,
    })
    .await
}
#[tokio::test]
async fn contributions_optional_missing_failed_disabled_upgraded_and_reopened_only_degrade_named_views()
 {
    let root = tempfile::tempdir().unwrap();
    let host = new_host(root.path());
    let (m, c) = contribution_manifest("org.example.consumer", "org.example.helper");
    let consumer = install_contributions(&host, root.path(), &m, &c);
    let consumer = contribution_state(&host, &consumer, StateAction::Enable)
        .await
        .unwrap();
    let query = |view: &str| ViewQuery {
        plugin_id: m.id.clone(),
        view_id: view.into(),
        cursor: None,
    };
    assert!(host.query_view(&query("preferences")).is_ok());
    assert_eq!(
        host.query_view(&query("enhanced")).unwrap_err().code,
        "dependency_missing"
    );
    let mut helper = manifest();
    helper.id = "org.example.helper".into();
    let old = install(&host, root.path(), &helper);
    let old = contribution_state(&host, &old, StateAction::Enable)
        .await
        .unwrap();
    assert!(host.query_view(&query("enhanced")).is_ok());
    let helper_file = host.installed(&helper.id, &old.digest).unwrap().path;
    let original = std::fs::read(&helper_file).unwrap();
    std::fs::write(&helper_file, b"tampered").unwrap();
    assert_eq!(
        host.query_view(&query("enhanced")).unwrap_err().code,
        "dependency_missing"
    );
    assert!(host.query_view(&query("preferences")).is_ok());
    std::fs::write(&helper_file, original).unwrap();
    host.mark_failed(&helper.id, "worker_crashed").unwrap();
    assert_eq!(
        host.query_view(&query("enhanced")).unwrap_err().code,
        "dependency_missing"
    );
    contribution_state(&host, &old, StateAction::Enable)
        .await
        .unwrap();
    contribution_state(&host, &old, StateAction::Disable)
        .await
        .unwrap();
    let kept = host.status(&m.id, &consumer.digest).unwrap();
    assert!(kept.enabled);
    assert_eq!(kept.grant_revision, consumer.grant_revision);
    assert!(host.query_view(&query("preferences")).is_ok());
    assert!(host.query_view(&query("enhanced")).is_err());
    helper.version = "2.0.0".into();
    let new = install(&host, root.path(), &helper);
    contribution_state(&host, &new, StateAction::Enable)
        .await
        .unwrap();
    let cat = host.contribution_catalog(&host.list().unwrap());
    assert_eq!(
        cat.items[0].dependencies[0].reason.as_deref(),
        Some("version-mismatch")
    );
    assert_eq!(
        host.query_view(&query("enhanced")).unwrap_err().code,
        "dependency_missing"
    );
    contribution_state(&host, &new, StateAction::Rollback)
        .await
        .unwrap();
    assert!(host.query_view(&query("enhanced")).is_ok());
    drop(host);
    let host = new_host(root.path());
    assert!(host.query_view(&query("enhanced")).is_ok());
    contribution_state(&host, &old, StateAction::Remove)
        .await
        .unwrap();
    assert!(host.status(&m.id, &consumer.digest).unwrap().enabled);
    assert!(host.query_view(&query("enhanced")).is_err());
    assert!(host.query_view(&query("preferences")).is_ok());
}
#[tokio::test]
async fn contributions_optional_cycle_cannot_be_published_and_bad_package_is_isolated() {
    let root = tempfile::tempdir().unwrap();
    let host = new_host(root.path());
    let (a, ca) = contribution_manifest("org.example.optional-a", "org.example.optional-b");
    let (b, cb) = contribution_manifest("org.example.optional-b", "org.example.optional-a");
    let sa = install_contributions(&host, root.path(), &a, &ca);
    let sb = install_contributions(&host, root.path(), &b, &cb);
    contribution_state(&host, &sa, StateAction::Enable)
        .await
        .unwrap();
    assert_eq!(
        contribution_state(&host, &sb, StateAction::Enable)
            .await
            .unwrap_err()
            .code,
        "dependency_cycle"
    );
    assert!(!host.status(&b.id, &sb.digest).unwrap().enabled);
    std::fs::write(host.installed(&a.id, &sa.digest).unwrap().path, b"tampered").unwrap();
    let items = host.list().unwrap();
    let cat = host.contribution_catalog(&items);
    assert_eq!(
        cat.items
            .iter()
            .find(|p| p.plugin_id == a.id)
            .unwrap()
            .error_code
            .as_deref(),
        Some("signature_invalid")
    );
    let mut independent = manifest();
    independent.id = "org.example.independent".into();
    let si = install(&host, root.path(), &independent);
    assert!(
        contribution_state(&host, &si, StateAction::Enable)
            .await
            .unwrap()
            .enabled
    );
    assert_eq!(items.len(), 2);
    assert!(
        host.query_view(&ViewQuery {
            plugin_id: a.id,
            view_id: "enhanced".into(),
            cursor: None
        })
        .is_err()
    );
}
#[test]
fn contributions_reject_unknown_unsafe_or_unscoped_declarations_and_match_schemas() {
    let (m, c) = contribution_manifest("org.example.consumer", "org.example.helper");
    let parse = |m: &Manifest, c: &serde_json::Value| {
        super::contributions::parse(
            m,
            &std::collections::BTreeMap::from([(
                "contributions.json".into(),
                serde_json::to_vec(c).unwrap(),
            )]),
        )
    };
    assert!(parse(&m, &c).is_ok());
    for bad in [
        serde_json::json!({"schemaVersion":2}),
        {
            let mut b = c.clone();
            b["detailSlots"][0]["html"] = "<script>".into();
            b
        },
        {
            let mut b = c.clone();
            b["detailSlots"][0]["objectIds"] = serde_json::json!(["*", "claude"]);
            b
        },
        {
            let mut b = c.clone();
            b["optionalDependencies"][0]["views"] = serde_json::json!(["missing"]);
            b
        },
        {
            let mut b = c.clone();
            b["optionalDependencies"][0]["pluginId"] = m.id.clone().into();
            b
        },
    ] {
        assert!(parse(&m, &bad).is_err(), "{bad}");
    }
    let mut no_permission = m.clone();
    no_permission.permissions.clear();
    assert_eq!(
        parse(&no_permission, &c).unwrap_err().code,
        "permission_denied"
    );
    let legacy = manifest();
    assert!(parse(&legacy, &c).unwrap().is_none());
    let mut extension: Manifest = serde_json::from_str(include_str!(
        "../../../../../sdk/plugins/examples/mcp-stdio/package/manifest.json"
    ))
    .unwrap();
    extension.requires = m.requires.clone();
    let mut c2 = c.clone();
    c2["optionalDependencies"][0]["views"] = serde_json::json!([extension.views[0].id]);
    c2["detailSlots"][0]["viewId"] = extension.views[0].id.clone().into();
    package::validate(&extension).unwrap();
    assert!(parse(&extension, &c2).is_ok());
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../contracts/plugins/contributions");
    for (name, schema) in local_agent_core::plugin::contributions::schemas()
        .as_object()
        .unwrap()
    {
        let committed: serde_json::Value = serde_json::from_slice(
            &std::fs::read(root.join(format!("{name}.schema.json"))).unwrap(),
        )
        .unwrap();
        assert_eq!(&committed, schema);
    }
}

#[tokio::test]
async fn contributions_concurrent_cycles_and_idempotent_enables_keep_graph_publication_safe() {
    let root = tempfile::tempdir().unwrap();
    let host = new_host(root.path());
    let (a, ca) = contribution_manifest("org.example.concurrent-a", "org.example.concurrent-b");
    let (b, cb) = contribution_manifest("org.example.concurrent-b", "org.example.concurrent-a");
    let sa = install_contributions(&host, root.path(), &a, &ca);
    let sb = install_contributions(&host, root.path(), &b, &cb);
    let (ra, rb) = tokio::join!(
        contribution_state(&host, &sa, StateAction::Enable),
        contribution_state(&host, &sb, StateAction::Enable)
    );
    assert_eq!(usize::from(ra.is_ok()) + usize::from(rb.is_ok()), 1);
    let (enabled, other) = if let Ok(s) = ra {
        (s, sb)
    } else {
        (rb.unwrap(), sa)
    };
    let guard = host.activation.lock().await;
    assert_eq!(
        contribution_state(&host, &enabled, StateAction::Enable)
            .await
            .unwrap()
            .activation_epoch,
        enabled.activation_epoch
    );
    assert_eq!(
        contribution_state(&host, &other, StateAction::Enable)
            .await
            .unwrap_err()
            .code,
        "resource_limit"
    );
    drop(guard);
    assert_eq!(
        contribution_state(&host, &other, StateAction::Enable)
            .await
            .unwrap_err()
            .code,
        "dependency_cycle"
    );
}
