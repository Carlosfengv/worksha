use super::store::{Host, Installed, Selection};
use local_agent_core::plugin::full as f;
use rusqlite::params;

fn package(index: u8) -> Installed {
    Installed {
        manifest: serde_json::from_str(include_str!(
            "../../../../../../sdk/plugins/examples/full-history/package/manifest.json"
        ))
        .unwrap(),
        digest: format!("{index:064x}"),
        signing_key: "a".repeat(64),
        compressed_bytes: 1,
        expanded_bytes: 1,
    }
}
fn seed(host: &Host, p: &Installed, enabled: bool) -> Selection {
    host.db()
        .unwrap()
        .execute(
            "INSERT INTO full_packages VALUES(?1,?2,?3,0)",
            params![p.digest, p.manifest.id, super::store::encode(p).unwrap()],
        )
        .unwrap();
    let mut s = Selection::installed(&p.digest);
    s.enabled = enabled;
    host.save_selection(&p.manifest.id, &s).unwrap();
    host.selected_marker(&p.manifest.id, enabled.then_some(p.digest.as_str()))
        .unwrap();
    s
}
fn commit_failure(host: &Host) {
    host.db().unwrap().execute_batch("PRAGMA foreign_keys=ON;CREATE TABLE commit_parent(id INTEGER PRIMARY KEY);CREATE TABLE commit_child(parent INTEGER REFERENCES commit_parent(id) DEFERRABLE INITIALLY DEFERRED);CREATE TRIGGER reject_commit AFTER INSERT ON full_selections BEGIN INSERT INTO commit_child VALUES(42); END;").unwrap();
}

#[test]
fn failed_commit_restores_marker_selection_projection_and_selected_package() {
    for (enabled, remove) in [(false, false), (true, false), (true, true)] {
        let data = tempfile::tempdir().unwrap();
        let host = Host::new(data.path());
        let p = package(1);
        let before = seed(&host, &p, enabled);
        host.db()
            .unwrap()
            .execute(
                "INSERT INTO full_projections VALUES(?1,'old metadata')",
                [&p.manifest.id],
            )
            .unwrap();
        commit_failure(&host);
        let mut after = before.clone();
        after.advance().unwrap();
        after.enabled = !remove;
        if !remove {
            after.digest = package(2).digest;
        }
        let result = if remove {
            host.remove_selected_package(&p.manifest.id, &p.digest, &after)
        } else {
            host.save_selection_and_clear_projection(&p.manifest.id, &after)
        };
        assert_eq!(result.unwrap_err().code, "unavailable");
        assert_eq!(host.selection(&p.manifest.id).unwrap(), before);
        let saved: String = host
            .db()
            .unwrap()
            .query_row("SELECT record FROM full_projections", [], |r| r.get(0))
            .unwrap();
        assert_eq!(saved, "old metadata");
        assert!(
            host.installed(&f::PackageQuery {
                plugin_id: p.manifest.id.clone(),
                digest: p.digest.clone()
            })
            .is_ok()
        );
        let marker = host.root.join("selected").join(&p.manifest.id);
        if enabled {
            assert_eq!(std::fs::read_to_string(marker).unwrap(), p.digest);
        } else {
            assert!(!marker.exists());
        }
    }
}

#[test]
fn failed_unselected_removal_preserves_previous_version_and_current_projection() {
    let data = tempfile::tempdir().unwrap();
    let host = Host::new(data.path());
    let current = package(1);
    let old = package(2);
    let mut before = seed(&host, &current, true);
    before.previous = Some(old.digest.clone());
    host.save_selection(&current.manifest.id, &before).unwrap();
    host.db()
        .unwrap()
        .execute(
            "INSERT INTO full_projections VALUES(?1,'current metadata')",
            [&current.manifest.id],
        )
        .unwrap();
    host.db()
        .unwrap()
        .execute(
            "INSERT INTO full_packages VALUES(?1,?2,?3,0)",
            params![
                old.digest,
                old.manifest.id,
                super::store::encode(&old).unwrap()
            ],
        )
        .unwrap();
    commit_failure(&host);
    let mut after = before.clone();
    after.previous = None;
    assert_eq!(
        host.remove_unselected_package(&old.manifest.id, &old.digest, &after)
            .unwrap_err()
            .code,
        "unavailable"
    );
    assert_eq!(host.selection(&current.manifest.id).unwrap(), before);
    let saved: String = host
        .db()
        .unwrap()
        .query_row("SELECT record FROM full_projections", [], |r| r.get(0))
        .unwrap();
    assert_eq!(saved, "current metadata");
    assert!(
        host.installed(&f::PackageQuery {
            plugin_id: old.manifest.id,
            digest: old.digest
        })
        .is_ok()
    );
}

#[test]
fn failed_package_insert_cleans_new_archive_without_trusting_or_selecting_it() {
    let data = tempfile::tempdir().unwrap();
    let host = Host::new(data.path());
    host.db().unwrap().execute_batch("CREATE TRIGGER reject_package BEFORE INSERT ON full_packages BEGIN SELECT RAISE(ABORT,'storage failure'); END;").unwrap();
    let mut p = package(1);
    p.digest = super::super::package::hash(b"owned archive");
    p.compressed_bytes = b"owned archive".len() as u64;
    assert_eq!(
        host.publish_package(&p, b"owned archive").unwrap_err().code,
        "unavailable"
    );
    assert!(!host.package_path(&p).exists());
    assert!(!host.root.join("packages").join(&p.manifest.id).exists());
    assert!(host.packages().unwrap().is_empty());
    assert!(host.selection(&p.manifest.id).is_err());
}

#[test]
fn reopening_reconciles_both_sides_of_marker_commit_without_restoring_runtime_or_grants() {
    for enabled in [false, true] {
        let data = tempfile::tempdir().unwrap();
        let host = Host::new(data.path());
        let p = package(1);
        let before = seed(&host, &p, enabled);
        // Simulate a durable marker change followed by death before SQLite commit.
        host.selected_marker(&p.manifest.id, Some(&package(2).digest))
            .unwrap();
        drop(host);
        let reopened = Host::new(data.path());
        assert_eq!(reopened.selection(&p.manifest.id).unwrap(), before);
        let marker = reopened.root.join("selected").join(&p.manifest.id);
        if enabled {
            assert_eq!(std::fs::read_to_string(marker).unwrap(), p.digest);
        } else {
            assert!(!marker.exists());
        }
        assert!(reopened.active.is_empty() && reopened.grants.is_empty());
    }
}

#[test]
fn reopening_reaps_only_uncommitted_owned_archives_and_expires_old_review_refs() {
    let data = tempfile::tempdir().unwrap();
    let host = Host::new(data.path());
    let p = package(1);
    seed(&host, &p, false);
    let live = host.package_path(&p);
    host.write_private(&live, b"committed archive").unwrap();
    let orphan = host.package_path(&package(2));
    host.write_private(&orphan, b"interrupted archive").unwrap();
    let unknown = orphan.parent().unwrap().join("keep-user-note.txt");
    std::fs::write(&unknown, "preserve").unwrap();
    let temporary = live
        .parent()
        .unwrap()
        .join(format!(".{}.tmp", uuid::Uuid::new_v4()));
    std::fs::write(&temporary, "interrupted temporary").unwrap();
    let review = uuid::Uuid::new_v4().to_string();
    let stage = host.root.join("staging").join(format!("{review}.laplug"));
    std::fs::write(&stage, "staged archive").unwrap();
    host.db()
        .unwrap()
        .execute(
            "INSERT INTO full_reviews VALUES(?1,'review',9999999999999)",
            [&review],
        )
        .unwrap();
    drop(host);
    let reopened = Host::new(data.path());
    assert_eq!(reopened.packages().unwrap().len(), 1);
    assert_eq!(std::fs::read(live).unwrap(), b"committed archive");
    assert!(!orphan.exists() && !temporary.exists() && !stage.exists());
    assert_eq!(std::fs::read_to_string(unknown).unwrap(), "preserve");
    let reviews: i64 = reopened
        .db()
        .unwrap()
        .query_row("SELECT count(*) FROM full_reviews", [], |r| r.get(0))
        .unwrap();
    assert_eq!(reviews, 0);
}

#[test]
fn transient_optional_store_lock_is_bounded_and_retries_without_restart() {
    use super::super::Manager;
    use local_agent_core::providers::vault::MemoryVault;
    use std::{
        sync::Arc,
        time::{Duration, Instant},
    };
    let data = tempfile::tempdir().unwrap();
    let host = Host::new(data.path());
    seed(&host, &package(1), false);
    drop(host);
    let lock = rusqlite::Connection::open(data.path().join("plugin-source-host.sqlite3")).unwrap();
    lock.execute_batch("BEGIN IMMEDIATE;").unwrap();
    let manager = Manager::open(data.path(), Arc::new(MemoryVault::default())).unwrap();
    let started = Instant::now();
    assert_eq!(manager.full_list().unwrap_err().code, "unavailable");
    assert!(started.elapsed() < Duration::from_secs(1));
    assert!(manager.list().unwrap().is_empty());
    lock.execute_batch("ROLLBACK;").unwrap();
    assert_eq!(manager.full_list().unwrap().len(), 1);
}

#[test]
fn recovery_does_not_follow_an_uncommitted_archive_link() {
    use std::os::unix::fs::symlink;
    let data = tempfile::tempdir().unwrap();
    let host = Host::new(data.path());
    host.db().unwrap();
    let original = data.path().join("outside-source.jsonl");
    std::fs::write(&original, "source must stay intact").unwrap();
    let archive = host.package_path(&package(1));
    std::fs::create_dir_all(archive.parent().unwrap()).unwrap();
    symlink(&original, &archive).unwrap();
    drop(host);
    let reopened = Host::new(data.path());
    assert_eq!(reopened.packages().err().unwrap().code, "permission_denied");
    assert_eq!(
        std::fs::read_to_string(original).unwrap(),
        "source must stay intact"
    );
    assert!(
        std::fs::symlink_metadata(archive)
            .unwrap()
            .file_type()
            .is_symlink()
    );
}

#[test]
fn activation_never_records_a_removed_version_or_itself_as_a_rollback_target() {
    let data = tempfile::tempdir().unwrap();
    let host = Host::new(data.path());
    let current = package(1);
    let selection = seed(&host, &current, true);
    let next = package(2);
    assert_eq!(
        host.previous_for_activation(&current.manifest.id, &selection, &next.digest)
            .unwrap(),
        Some(current.digest.clone())
    );
    host.db()
        .unwrap()
        .execute("UPDATE full_packages SET removed=1", [])
        .unwrap();
    assert_eq!(
        host.previous_for_activation(&current.manifest.id, &selection, &next.digest)
            .unwrap(),
        None
    );
    let mut with_previous = selection;
    with_previous.previous = Some(next.digest.clone());
    host.db()
        .unwrap()
        .execute(
            "INSERT INTO full_packages VALUES(?1,?2,?3,0)",
            params![
                next.digest,
                next.manifest.id,
                super::store::encode(&next).unwrap()
            ],
        )
        .unwrap();
    assert_eq!(
        host.previous_for_activation(&current.manifest.id, &with_previous, &next.digest)
            .unwrap(),
        None
    );
    assert_eq!(
        host.previous_for_activation(&current.manifest.id, &with_previous, &package(3).digest)
            .unwrap(),
        Some(next.digest)
    );
}
