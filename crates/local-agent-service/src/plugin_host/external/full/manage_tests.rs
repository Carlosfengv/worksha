use super::super::Manager;
use super::*;
use ed25519_dalek::SigningKey;
use local_agent_core::{plugin::full as f, providers::vault::MemoryVault};
use std::sync::Arc;

#[test]
fn dsh_authorization_ticket_is_invalid_after_publisher_revocation_and_reapproval() {
    let root = tempfile::tempdir().unwrap();
    let host = Manager::open(root.path(), Arc::new(MemoryVault::default())).unwrap();
    let installed = host
        .full_install(&full_review(
            &host,
            root.path(),
            super::dsh::PLUGIN,
            "net.worksha",
            7,
        ))
        .unwrap();
    let query = f::PackageQuery {
        plugin_id: installed.manifest.id,
        digest: installed.digest,
    };
    let package = host.full.lock().unwrap().installed(&query).unwrap();
    let before = host.dsh_publisher_ticket(&package, true).unwrap().unwrap();
    host.revoke_shared_publisher("net.worksha").unwrap();
    host.db
        .lock()
        .unwrap()
        .execute(
            "INSERT INTO publishers VALUES(?1,?2)",
            rusqlite::params!["net.worksha", package.signing_key],
        )
        .unwrap();
    assert_eq!(host.dsh_publisher_ticket(&package, false).unwrap(), None);
    assert_ne!(
        host.dsh_publisher_ticket(&package, true).unwrap().unwrap(),
        before
    );
}

fn full_review(
    host: &Manager,
    root: &std::path::Path,
    id: &str,
    publisher: &str,
    key: u8,
) -> f::Install {
    let author = root.join(uuid::Uuid::new_v4().to_string());
    std::fs::create_dir(&author).unwrap();
    let mut manifest: f::Manifest = serde_json::from_str(include_str!(
        "../../../../../../sdk/plugins/examples/full-history/package/manifest.json"
    ))
    .unwrap();
    manifest.id = id.into();
    manifest.publisher = publisher.into();
    std::fs::write(
        author.join("manifest.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
    std::fs::write(author.join("plugin.wasm"), b"\0asm\x0d\0\x01\0").unwrap();
    let file = author.join("input.laplug");
    let archive = package::pack(&author, &SigningKey::from_bytes(&[key; 32])).unwrap();
    std::fs::write(&file, archive).unwrap();
    let review = host.full_inspect(&file).unwrap();
    f::Install {
        package_ref: review.package_ref,
        digest: review.digest,
        approve_publisher: true,
        expected_signing_key: review.signing_key,
    }
}
fn legacy_review(
    host: &Manager,
    root: &std::path::Path,
    manifest: &local_agent_core::plugin::external::Manifest,
) -> local_agent_core::plugin::external::InstallRequest {
    let author = root.join(uuid::Uuid::new_v4().to_string());
    std::fs::create_dir(&author).unwrap();
    std::fs::write(
        author.join("manifest.json"),
        serde_json::to_vec(manifest).unwrap(),
    )
    .unwrap();
    let archive =
        super::super::sdk::pack(&author, Some(&SigningKey::from_bytes(&[7; 32]))).unwrap();
    let file = author.join("input.laplug");
    std::fs::write(&file, archive).unwrap();
    let review = host.inspect(&file).unwrap();
    local_agent_core::plugin::external::InstallRequest {
        package_ref: review.package_ref,
        digest: review.digest,
        approve_publisher: true,
    }
}

#[test]
fn live_archive_residue_is_charged_across_both_installers_and_cache_formats() {
    for install_full in [true, false] {
        for residue_full in [true, false] {
            let root = tempfile::tempdir().unwrap();
            let host = Manager::open(root.path(), Arc::new(MemoryVault::default())).unwrap();
            let full = full_review(&host, root.path(), "org.example.capacity", "org.example", 7);
            let mut manifest = super::super::tests::manifest();
            manifest.id = "org.example.legacy-capacity".into();
            let legacy = legacy_review(&host, root.path(), &manifest);
            // Stores/recovery are already open. Simulate a live failed cleanup,
            // rather than an orphan that startup reconciliation can remove.
            let cache = if residue_full {
                host.full.lock().unwrap().root.clone()
            } else {
                host.root.clone()
            };
            let orphan = cache
                .join("packages/org.example.residue/1.0.0")
                .join("f".repeat(64))
                .join("package.laplug");
            std::fs::create_dir_all(orphan.parent().unwrap()).unwrap();
            std::fs::File::create(&orphan)
                .unwrap()
                .set_len(256 * 1_048_576)
                .unwrap();
            let error = if install_full {
                host.full_install(&full).unwrap_err()
            } else {
                host.install(&legacy).unwrap_err()
            };
            assert_eq!(error.code, "resource_limit");
            assert!(host.full_list().unwrap().is_empty() && host.list().unwrap().is_empty());
            assert_eq!(std::fs::metadata(&orphan).unwrap().len(), 256 * 1_048_576);
            // Removing only our synthetic residue must make the same review retryable.
            std::fs::remove_file(orphan).unwrap();
            if install_full {
                host.full_install(&full).unwrap();
            } else {
                host.install(&legacy).unwrap();
            }
        }
    }
}

#[tokio::test]
async fn a_failed_archive_removal_cannot_disappear_from_new_install_admission() {
    let root = tempfile::tempdir().unwrap();
    let host = Arc::new(Manager::open(root.path(), Arc::new(MemoryVault::default())).unwrap());
    let first = full_review(&host, root.path(), "org.example.removal", "org.example", 7);
    let installed = host.full_install(&first).unwrap();
    let next = full_review(
        &host,
        root.path(),
        "org.example.after-removal",
        "org.example",
        7,
    );
    let archive = host
        .full
        .lock()
        .unwrap()
        .root
        .join("packages")
        .join(&installed.manifest.id)
        .join(&installed.manifest.version)
        .join(&installed.digest)
        .join("package.laplug");
    let original = std::fs::read(&archive).unwrap();
    let alias = root.path().join("owned-archive-alias.laplug");
    std::fs::hard_link(&archive, &alias).unwrap();
    assert_eq!(
        host.full_change(f::StateChange {
            package: f::PackageQuery {
                plugin_id: installed.manifest.id,
                digest: installed.digest
            },
            action: f::StateAction::Remove
        })
        .await
        .unwrap_err()
        .code,
        "permission_denied"
    );
    assert!(host.full_list().unwrap().is_empty());
    assert_eq!(
        host.full_install(&next).unwrap_err().code,
        "permission_denied"
    );
    assert_eq!(std::fs::read(&alias).unwrap(), original);
    assert_eq!(std::fs::read(&archive).unwrap(), original);
    std::fs::remove_file(alias).unwrap();
    host.full_install(&next).unwrap();
}

#[test]
fn live_staging_residue_rejects_reviews_without_changing_healthy_plugins_or_sources() {
    for full in [true, false] {
        let root = tempfile::tempdir().unwrap();
        let host = Manager::open(root.path(), Arc::new(MemoryVault::default())).unwrap();
        let healthy = full_review(&host, root.path(), "org.example.healthy", "org.example", 7);
        host.full_install(&healthy).unwrap();
        let reviewed = full_review(
            &host,
            root.path(),
            "org.example.review-capacity",
            "org.example",
            7,
        );
        let legacy = legacy_review(&host, root.path(), &super::super::tests::manifest());
        let (stage, limit) = if full {
            (
                host.full
                    .lock()
                    .unwrap()
                    .root
                    .join("staging")
                    .join(format!("{}.laplug", reviewed.package_ref)),
                128 * 1_048_576,
            )
        } else {
            (
                host.root
                    .join("staging")
                    .join(&legacy.package_ref)
                    .join("package.laplug"),
                256 * 1_048_576,
            )
        };
        let selected = root.path().join("selected-original.laplug");
        std::fs::copy(&stage, &selected).unwrap();
        let original = std::fs::read(&selected).unwrap();
        let before = serde_json::to_value(host.full_list().unwrap()).unwrap();
        let id = uuid::Uuid::new_v4();
        let orphan = if full {
            stage.parent().unwrap().join(format!("{id}.laplug"))
        } else {
            let folder = host.root.join("staging").join(id.to_string());
            std::fs::create_dir(&folder).unwrap();
            folder.join("package.laplug")
        };
        std::fs::File::create(&orphan)
            .unwrap()
            .set_len(limit)
            .unwrap();
        let code = if full {
            host.full_inspect(&selected).unwrap_err().code
        } else {
            host.inspect(&selected).unwrap_err().code
        };
        assert_eq!(code, "resource_limit");
        assert_eq!(
            serde_json::to_value(host.full_list().unwrap()).unwrap(),
            before
        );
        assert!(host.list().unwrap().is_empty());
        assert_eq!(std::fs::read(&selected).unwrap(), original);
        assert_eq!(std::fs::metadata(&orphan).unwrap().len(), limit);
        std::fs::remove_file(&orphan).unwrap();
        if !full {
            std::fs::remove_dir(orphan.parent().unwrap()).unwrap();
        }
        if full {
            host.full_inspect(&selected).unwrap();
        } else {
            host.inspect(&selected).unwrap();
        }
    }
}

#[test]
fn older_installer_cannot_take_over_a_source_identity_after_trust_revocation() {
    let root = tempfile::tempdir().unwrap();
    let host = Manager::open(root.path(), Arc::new(MemoryVault::default())).unwrap();
    let reviewed = full_review(
        &host,
        root.path(),
        "org.example.owner.full-history",
        "org.example.owner",
        31,
    );
    let installed = host.full_install(&reviewed).unwrap();
    // Key revocation is allowed, but does not transfer ownership of installed IDs.
    host.db
        .lock()
        .unwrap()
        .execute("DELETE FROM publishers", [])
        .unwrap();
    for publisher in ["org.example", "org.example.owner"] {
        let mut manifest = super::super::tests::manifest();
        manifest.id = installed.manifest.id.clone();
        manifest.publisher = publisher.into();
        manifest.version = "2.0.0".into();
        let request = legacy_review(&host, root.path(), &manifest);
        assert_eq!(
            host.install(&request).unwrap_err().code,
            "publisher_changed"
        );
        assert!(host.list().unwrap().is_empty());
        assert_eq!(host.full_list().unwrap().len(), 1);
        assert_eq!(host.full_list().unwrap()[0].digest, installed.digest);
        assert!(!host.full_list().unwrap()[0].publisher_trusted);
    }
}

#[test]
fn source_status_cannot_claim_an_active_runtime_when_its_selection_marker_is_missing_or_changed() {
    let root = tempfile::tempdir().unwrap();
    let host = Manager::open(root.path(), Arc::new(MemoryVault::default())).unwrap();
    let request = full_review(
        &host,
        root.path(),
        "org.example.full-history",
        "org.example",
        31,
    );
    let installed = host.full_install(&request).unwrap();
    let query = f::PackageQuery {
        plugin_id: installed.manifest.id.clone(),
        digest: installed.digest.clone(),
    };
    let marker;
    {
        let mut full = host.full.lock().unwrap();
        let mut selected = full.selection(&query.plugin_id).unwrap();
        selected.enabled = true;
        full.save_selection(&query.plugin_id, &selected).unwrap();
        full.active.insert(
            query.plugin_id.clone(),
            super::store::Lease {
                id: query.plugin_id.clone(),
                digest: query.digest.clone(),
                publisher: installed.manifest.publisher,
                epoch: selected.epoch,
                revision: selected.revision,
                publisher_revision: 0,
            },
        );
        full.selected_marker(&query.plugin_id, Some(&query.digest))
            .unwrap();
        marker = full.root.join("selected").join(&query.plugin_id);
    }
    assert!(host.full_status(&query).unwrap().runtime_active);
    for contents in [None, Some("f".repeat(64))] {
        if let Some(contents) = contents {
            std::fs::write(&marker, contents).unwrap();
        } else {
            std::fs::remove_file(&marker).unwrap();
        }
        let status = host.full_status(&query).unwrap();
        assert!(status.enabled);
        assert!(!status.runtime_active && !status.source_authorized && !status.unified_authorized);
        assert!(status.fact_summary.is_none());
        assert_eq!(status.problem.as_deref(), Some("grant_revoked"));
    }
}

#[test]
fn both_installers_enforce_one_budget_and_a_busy_install_does_not_block_catalog_reads() {
    let root = tempfile::tempdir().unwrap();
    let host = Manager::open(root.path(), Arc::new(MemoryVault::default())).unwrap();
    let mut first_legacy = None;
    for n in 0..62 {
        let mut manifest = super::super::tests::manifest();
        manifest.id = format!("org.example.legacy{n}");
        let request = legacy_review(&host, root.path(), &manifest);
        host.install(&request).unwrap();
        first_legacy.get_or_insert(request);
    }
    // Installing old-format packages must not eagerly create the source store.
    assert!(!root.path().join("plugin-source-host.sqlite3").exists());
    let first_full = full_review(&host, root.path(), "org.example.full0", "org.example", 7);
    host.full_install(&first_full).unwrap();
    host.full_install(&full_review(
        &host,
        root.path(),
        "org.example.full1",
        "org.example",
        7,
    ))
    .unwrap();
    let overflow_full = full_review(&host, root.path(), "org.example.full2", "org.example", 7);
    let overflow_legacy = legacy_review(&host, root.path(), &super::super::tests::manifest());
    for code in [
        host.full_install(&overflow_full).unwrap_err().code,
        host.install(&overflow_legacy).unwrap_err().code,
    ] {
        assert_eq!(code, "resource_limit");
    }
    // Exact reviewed replays have no extra capacity cost.
    host.full_install(&first_full).unwrap();
    host.install(first_legacy.as_ref().unwrap()).unwrap();
    assert_eq!(host.list().unwrap().len(), 62);
    assert_eq!(host.full_list().unwrap().len(), 2);
    let held = host.installation.lock().unwrap();
    assert_eq!(host.install(&overflow_legacy).unwrap_err().code, "busy");
    assert_eq!(host.full_install(&overflow_full).unwrap_err().code, "busy");
    assert_eq!(host.list().unwrap().len(), 62);
    assert_eq!(host.full_list().unwrap().len(), 2);
    drop(held);
}

#[test]
fn source_resource_transition_removes_only_derived_metadata_and_rolls_back_on_storage_failure() {
    use super::store::{Host, Selection};
    let data = tempfile::tempdir().unwrap();
    let host = Host::new(data.path());
    let before = Selection::installed(&"a".repeat(64));
    host.save_selection("org.example.reader", &before).unwrap();
    host.db()
        .unwrap()
        .execute(
            "INSERT INTO full_projections VALUES('org.example.reader','previous metadata')",
            [],
        )
        .unwrap();
    host.db().unwrap().execute_batch("CREATE TRIGGER reject_projection_delete BEFORE DELETE ON full_projections BEGIN SELECT RAISE(ABORT,'storage failure fixture'); END;").unwrap();
    let mut after = before.clone();
    after.advance().unwrap();
    assert_eq!(
        host.save_selection_and_clear_projection("org.example.reader", &after)
            .unwrap_err()
            .code,
        "unavailable"
    );
    assert_eq!(host.selection("org.example.reader").unwrap(), before);
    let saved: String = host
        .db()
        .unwrap()
        .query_row("SELECT record FROM full_projections", [], |r| r.get(0))
        .unwrap();
    assert_eq!(saved, "previous metadata");
    host.db()
        .unwrap()
        .execute_batch("DROP TRIGGER reject_projection_delete")
        .unwrap();
    host.save_selection_and_clear_projection("org.example.reader", &after)
        .unwrap();
    assert_eq!(host.selection("org.example.reader").unwrap(), after);
    let count: i64 = host
        .db()
        .unwrap()
        .query_row("SELECT count(*) FROM full_projections", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 0);
}

#[test]
fn source_install_is_separate_disabled_and_requires_the_exact_reviewed_key() {
    let data = tempfile::tempdir().unwrap();
    let host = Manager::open(data.path(), Arc::new(MemoryVault::default())).unwrap();
    assert!(!data.path().join("plugin-sources").exists());
    assert!(!data.path().join("plugin-source-host.sqlite3").exists());
    let source = tempfile::tempdir().unwrap();
    let manifest: f::Manifest = serde_json::from_str(include_str!(
        "../../../../../../sdk/plugins/examples/full-history/package/manifest.json"
    ))
    .unwrap();
    std::fs::write(
        source.path().join("manifest.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
    std::fs::write(source.path().join("plugin.wasm"), b"\0asm\x0d\0\x01\0").unwrap();
    let key = SigningKey::from_bytes(&[31; 32]);
    let bytes = package::pack(source.path(), &key).unwrap();
    let file = data.path().join("input.laplug");
    std::fs::write(&file, &bytes).unwrap();
    let review = host.full_inspect(&file).unwrap();
    assert!(!review.trusted_publisher);
    let mut install = f::Install {
        package_ref: review.package_ref.clone(),
        digest: review.digest.clone(),
        approve_publisher: false,
        expected_signing_key: review.signing_key.clone(),
    };
    assert_eq!(
        host.full_install(&install).unwrap_err().code,
        "publisher_untrusted"
    );
    install.approve_publisher = true;
    install.expected_signing_key = "wrong-key".into();
    assert_eq!(
        host.full_install(&install).unwrap_err().code,
        "stale_snapshot"
    );
    install.expected_signing_key = review.signing_key.clone();
    let status = host.full_install(&install).unwrap();
    assert!(
        !status.enabled
            && !status.runtime_active
            && !status.source_authorized
            && !status.unified_authorized
    );
    assert_eq!(
        status.freshness,
        local_agent_core::domain::Freshness::Unread
    );
    assert!(status.publisher_trusted);
    assert_eq!(host.full_list().unwrap().len(), 1);
    assert!(host.list().unwrap().is_empty());
    assert_eq!(
        host.full_publisher_key(&manifest.publisher)
            .unwrap()
            .as_ref(),
        Some(&review.signing_key)
    );
    assert_eq!(host.full_install(&install).unwrap().digest, status.digest);
    drop(host);
    let reopened = Manager::open(data.path(), Arc::new(MemoryVault::default())).unwrap();
    let status = reopened.full_list().unwrap().pop().unwrap();
    assert!(!status.runtime_active && !status.source_authorized);
    assert_eq!(std::fs::read(file).unwrap(), bytes);
}
#[test]
fn publisher_revocation_does_not_wait_for_optional_store_io_or_retrust_old_leases() {
    let data = tempfile::tempdir().unwrap();
    let host = Arc::new(Manager::open(data.path(), Arc::new(MemoryVault::default())).unwrap());
    assert_eq!(host.full_publisher_revision("org.example").unwrap(), 0);
    let held = host.full.lock().unwrap();
    let (sent, received) = std::sync::mpsc::channel();
    let worker = std::thread::spawn({
        let host = host.clone();
        move || {
            host.full_revoke_publisher("org.example");
            sent.send(()).unwrap();
        }
    });
    received
        .recv_timeout(std::time::Duration::from_secs(1))
        .unwrap();
    drop(held);
    worker.join().unwrap();
    assert_eq!(host.full_publisher_revision("org.example").unwrap(), 1);
    assert_eq!(
        host.full_publisher_revision("org.example.child").unwrap(),
        0
    );
    host.full_publisher_revisions
        .lock()
        .unwrap()
        .insert("org.example".into(), Some(u64::MAX));
    host.full_revoke_publisher("org.example");
    assert_eq!(
        host.full_publisher_revision("org.example")
            .unwrap_err()
            .code,
        "grant_revoked"
    );
    assert_eq!(
        host.full_publisher_revision("org.example.child").unwrap(),
        0
    );
}

#[test]
fn source_install_cannot_claim_an_existing_legacy_plugin_identity_with_another_publisher_or_key() {
    use rusqlite::params;
    let data = tempfile::tempdir().unwrap();
    let host = Manager::open(data.path(), Arc::new(MemoryVault::default())).unwrap();
    let source = tempfile::tempdir().unwrap();
    let manifest: f::Manifest = serde_json::from_str(include_str!(
        "../../../../../../sdk/plugins/examples/full-history/package/manifest.json"
    ))
    .unwrap();
    std::fs::write(
        source.path().join("manifest.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
    std::fs::write(source.path().join("plugin.wasm"), b"\0asm\x0d\0\x01\0").unwrap();
    let bytes = package::pack(source.path(), &SigningKey::from_bytes(&[31; 32])).unwrap();
    let file = data.path().join("input.laplug");
    std::fs::write(&file, bytes).unwrap();
    let review = host.full_inspect(&file).unwrap();
    let install = f::Install {
        package_ref: review.package_ref,
        digest: review.digest,
        approve_publisher: true,
        expected_signing_key: review.signing_key.clone(),
    };
    for (publisher, key) in [
        ("org.example.other", review.signing_key.as_str()),
        (manifest.publisher.as_str(), "different-key"),
    ] {
        let mut old = super::super::tests::manifest();
        old.id = manifest.id.clone();
        old.publisher = publisher.into();
        let db = host.db.lock().unwrap();
        db.execute("DELETE FROM packages", []).unwrap();
        db.execute(
            "INSERT INTO packages VALUES(?1,?2,?3,?4,?5,1,0)",
            params![
                "a".repeat(64),
                old.id,
                old.version,
                serde_json::to_string(&old).unwrap(),
                key
            ],
        )
        .unwrap();
        drop(db);
        assert_eq!(
            host.full_install(&install).unwrap_err().code,
            "publisher_changed"
        );
        assert!(host.full_list().unwrap().is_empty());
    }
}
#[test]
fn future_source_database_is_preserved_and_does_not_gate_unrelated_legacy_readers() {
    let data = tempfile::tempdir().unwrap();
    let file = data.path().join("plugin-source-host.sqlite3");
    let db = rusqlite::Connection::open(&file).unwrap();
    db.execute_batch("CREATE TABLE full_version(version INTEGER);INSERT INTO full_version VALUES(2);CREATE TABLE future_data(value TEXT);INSERT INTO future_data VALUES('preserve');").unwrap();
    drop(db);
    let original = std::fs::read(&file).unwrap();
    let host = Manager::open(data.path(), Arc::new(MemoryVault::default())).unwrap();
    assert_eq!(host.full_list().unwrap_err().code, "incompatible_api");
    assert_eq!(std::fs::read(&file).unwrap(), original);
    assert!(!legacy_conflict(&data.path().join("plugins"), "org.example.unrelated").unwrap());
    assert!(host.list().unwrap().is_empty());
}

#[test]
fn unversioned_nonempty_source_database_is_preserved_and_empty_file_can_initialize() {
    let data = tempfile::tempdir().unwrap();
    let file = data.path().join("plugin-source-host.sqlite3");
    let db = rusqlite::Connection::open(&file).unwrap();
    db.execute_batch(
        "CREATE TABLE foreign_data(value TEXT);INSERT INTO foreign_data VALUES('preserve');",
    )
    .unwrap();
    drop(db);
    let original = std::fs::read(&file).unwrap();
    let host = Manager::open(data.path(), Arc::new(MemoryVault::default())).unwrap();
    for _ in 0..2 {
        assert_eq!(host.full_list().unwrap_err().code, "invalid_record");
        assert_eq!(std::fs::read(&file).unwrap(), original);
        assert!(host.list().unwrap().is_empty());
    }
    drop(host);

    // Explicitly replacing this owned fixture is separate from product startup.
    std::fs::write(&file, []).unwrap();
    let host = Manager::open(data.path(), Arc::new(MemoryVault::default())).unwrap();
    assert!(host.full_list().unwrap().is_empty());
    drop(host);
    let reopened = Manager::open(data.path(), Arc::new(MemoryVault::default())).unwrap();
    assert!(reopened.full_list().unwrap().is_empty());
}

#[test]
fn trust_write_failure_keeps_the_committed_package_disabled_and_never_retrusts_on_reopen() {
    let data = tempfile::tempdir().unwrap();
    let host = Manager::open(data.path(), Arc::new(MemoryVault::default())).unwrap();
    let install = full_review(
        &host,
        data.path(),
        "org.example.full-history",
        "org.example",
        31,
    );
    host.db.lock().unwrap().execute_batch("CREATE TRIGGER reject_trust BEFORE INSERT ON publishers BEGIN SELECT RAISE(ABORT,'trust write failed'); END;").unwrap();
    assert_eq!(host.full_install(&install).unwrap_err().code, "unavailable");
    let status = host.full_list().unwrap().pop().unwrap();
    assert!(
        !status.publisher_trusted
            && !status.enabled
            && !status.runtime_active
            && !status.source_authorized
    );
    let query = f::PackageQuery {
        plugin_id: status.manifest.id,
        digest: status.digest,
    };
    let archive = {
        let full = host.full.lock().unwrap();
        full.package_path(&full.installed(&query).unwrap())
    };
    assert!(archive.exists());
    drop(host);
    let reopened = Manager::open(data.path(), Arc::new(MemoryVault::default())).unwrap();
    assert!(!reopened.full_list().unwrap()[0].publisher_trusted);
    // A new explicit review/approval can finish the interrupted trust step.
    reopened
        .db
        .lock()
        .unwrap()
        .execute_batch("DROP TRIGGER reject_trust")
        .unwrap();
    let review = reopened.full_inspect(&archive).unwrap();
    let status = reopened
        .full_install(&f::Install {
            package_ref: review.package_ref,
            digest: review.digest,
            approve_publisher: true,
            expected_signing_key: review.signing_key,
        })
        .unwrap();
    assert!(status.publisher_trusted && !status.enabled && !status.source_authorized);
}

#[test]
fn in_flight_install_cannot_restore_publisher_trust_revoked_during_package_publication() {
    use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        mpsc,
    };
    for approved in [false, true] {
        let data = tempfile::tempdir().unwrap();
        let host = Arc::new(Manager::open(data.path(), Arc::new(MemoryVault::default())).unwrap());
        let mut install = full_review(
            &host,
            data.path(),
            "org.example.full-history",
            "org.example",
            31,
        );
        host.db
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO publishers VALUES('org.example',?1)",
                [&install.expected_signing_key],
            )
            .unwrap();
        install.approve_publisher = approved;
        let (arrived, reached) = mpsc::channel();
        let (release, released) = mpsc::channel();
        let once = AtomicBool::new(false);
        host.full
            .lock()
            .unwrap()
            .db()
            .unwrap()
            .authorizer(Some({
                move |action: AuthContext<'_>| {
                    if matches!(
                        action.action,
                        AuthAction::Insert {
                            table_name: "full_packages"
                        }
                    ) && !once.swap(true, Ordering::SeqCst)
                        && (arrived.send(()).is_err()
                            || released
                                .recv_timeout(std::time::Duration::from_secs(5))
                                .is_err())
                    {
                        return Authorization::Deny;
                    }
                    Authorization::Allow
                }
            }))
            .unwrap();
        let worker = std::thread::spawn({
            let host = host.clone();
            move || host.full_install(&install)
        });
        reached
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap();
        host.revoke_shared_publisher("org.example").unwrap();
        release.send(()).unwrap();
        assert_eq!(worker.join().unwrap().unwrap_err().code, "grant_revoked");
        assert_eq!(host.full_publisher_key("org.example").unwrap(), None);
        let status = host.full_list().unwrap().pop().unwrap();
        assert!(!status.publisher_trusted && !status.enabled && !status.source_authorized);
    }
}
