use super::*;
use crate::{
    domain::provider_access::Target,
    providers::{operations::Kind, vault::MemoryVault},
};
use std::{os::unix::fs::PermissionsExt, path::Path, sync::Arc};

fn setup(root: &Path) -> (Store, ProviderManager) {
    let data = root.join("data");
    let home = root.join("home");
    std::fs::create_dir_all(&home).unwrap();
    (
        Store::open(&data).unwrap(),
        ProviderManager::new(
            home,
            &data,
            root.join("unused-helper"),
            Arc::new(MemoryVault::default()),
        )
        .unwrap(),
    )
}
fn provider() -> Provider {
    serde_json::from_value(serde_json::json!({
        "id":format!("custom-{}",uuid::Uuid::new_v4()),"name":"Owned source","vendor":"custom",
        "endpoint":"http://127.0.0.1:11434/v1/","protocol":"responses","model":"fixture",
        "models":["fixture"],"configured":false,"category":"local","revision":0,
        "credentialRef":null,"credentialLabel":null
    }))
    .unwrap()
}
fn request(store: &Store, provider: Provider) -> Request {
    let protocol = store.provider_write_protocol().unwrap();
    Request {
        store_id: protocol.store_id,
        operation_id: uuid::Uuid::new_v4().to_string(),
        expected_sequence: protocol.sequence,
        kind: Kind::Connect,
        provider,
    }
}
fn choice(p: &Provider) -> Selection {
    Selection {
        plugin_id: "org.example.catalog".into(),
        digest: "a".repeat(64),
        binding_id: uuid::Uuid::new_v4().to_string(),
        target: Target::Draft {
            label: p.name.clone(),
            endpoint: p.endpoint.clone(),
            protocol: p.protocol.clone(),
            vendor: p.vendor.clone(),
            credential_ref: None,
        },
    }
}
fn stage(store: &Store, manager: &ProviderManager, r: &Request, selected: &Selection) {
    assert!(store.start_provider_operation(r).unwrap().started);
    manager
        .record_catalog_operation(store, r, selected)
        .unwrap();
    manager.stage_catalog_source(store, r, &r.provider).unwrap();
}
fn finish(store: &Store, r: &Request) -> Provider {
    store
        .finish_provider_connection(r, r.provider.clone(), "2026-11-01T00:00:00Z".into(), |_| {
            Ok(())
        })
        .unwrap()
        .provider
        .unwrap()
}
fn path(manager: &ProviderManager, store: &Store, p: &Provider) -> PathBuf {
    manager
        .catalog_source_path(&store.database_id().unwrap(), &p.id, false)
        .unwrap()
}
fn code<T>(r: Result<T, ConfigError>) -> String {
    r.err().expect("must refuse").code
}

#[test]
fn catalog_source_absence_is_native_without_migrating_or_writing_a_choice() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let (store, manager) = setup(&root);
    let p = store.providers().unwrap().remove(0);
    let before = store.provider_write_protocol().unwrap();
    let snapshot = manager.catalog_source_snapshot(&store, &p).unwrap();
    assert!(
        snapshot.catalog.is_none()
            && snapshot.revision.is_none()
            && snapshot.scope_matches
            && !snapshot.pending
    );
    assert!(!path(&manager, &store, &p).exists());
    assert_eq!(
        store.provider_write_protocol().unwrap().sequence,
        before.sequence
    );
    assert_eq!(
        store
            .connection
            .pragma_query_value(None, "user_version", |r| r.get::<_, u32>(0))
            .unwrap(),
        15
    );
}

#[test]
fn catalog_source_crash_windows_resolve_only_from_the_original_receipt_after_reopen() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let (store, manager) = setup(&root);
    let r = request(&store, provider());
    let selected = choice(&r.provider);
    stage(&store, &manager, &r, &selected);
    let file = path(&manager, &store, &r.provider);
    let bytes = std::fs::read(&file).unwrap();
    assert_eq!(
        std::fs::metadata(&file).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert!(
        manager
            .catalog_source_snapshot(&store, &r.provider)
            .unwrap()
            .pending
    );
    assert_eq!(
        code(manager.select_catalog_source(&store, &r.provider, Some(&digest(&bytes)), None)),
        "catalog_source_pending"
    );
    drop(manager);
    drop(store);
    let (store, manager) = setup(&root);
    assert!(
        manager
            .catalog_source_snapshot(&store, &r.provider)
            .unwrap()
            .pending
    );
    assert_eq!(std::fs::read(&file).unwrap(), bytes);
    let saved = finish(&store, &r);
    assert!(!saved.endpoint.ends_with('/'));
    drop(manager);
    drop(store);
    let (store, manager) = setup(&root);
    let snapshot = manager.catalog_source_snapshot(&store, &saved).unwrap();
    assert!(!snapshot.pending && snapshot.scope_matches);
    assert!(snapshot.catalog.as_ref() == Some(&selected));
    // A read is recovery evidence, never a bookkeeping write or a replay.
    assert_eq!(std::fs::read(&file).unwrap(), bytes);
    assert!(!store.start_provider_operation(&r).unwrap().started);
    manager.finish_catalog_source(&store, &r).unwrap();
    let snapshot = manager.catalog_source_snapshot(&store, &saved).unwrap();
    assert!(snapshot.catalog.as_ref() == Some(&selected) && snapshot.scope_matches);
    assert_ne!(std::fs::read(&file).unwrap(), bytes);
}

#[test]
fn catalog_source_failed_or_cancelled_sql_rolls_back_to_the_previous_choice() {
    for cancel in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let (store, manager) = setup(&root);
        let p = provider();
        store.save_provider(p.clone()).unwrap();
        let p = store.provider_by_id(&p.id).unwrap().unwrap();
        let previous = choice(&p);
        manager
            .select_catalog_source(&store, &p, None, Some(previous.clone()))
            .unwrap();
        let r = request(&store, p.clone());
        stage(&store, &manager, &r, &choice(&p));
        if cancel {
            store.cancel_provider_connection(&r).unwrap();
        } else {
            store
                .fail_provider_connection(&r, &ConfigError::new("fixture", "offline failure"))
                .unwrap();
        }
        drop(manager);
        drop(store);
        let (store, manager) = setup(&root);
        let snapshot = manager.catalog_source_snapshot(&store, &p).unwrap();
        assert!(!snapshot.pending && snapshot.scope_matches);
        assert!(snapshot.catalog.as_ref() == Some(&previous));
        manager.finish_catalog_source(&store, &r).unwrap();
        let snapshot = manager.catalog_source_snapshot(&store, &p).unwrap();
        assert!(snapshot.catalog.as_ref() == Some(&previous));
    }
}

#[test]
fn catalog_source_cas_fences_in_flight_connection_and_requires_explicit_native_selection() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let (store, manager) = setup(&root);
    let p = provider();
    store.save_provider(p.clone()).unwrap();
    let p = store.provider_by_id(&p.id).unwrap().unwrap();
    let r = request(&store, p.clone());
    store.start_provider_operation(&r).unwrap();
    manager
        .record_catalog_operation(&store, &r, &choice(&p))
        .unwrap();
    manager.verify_catalog_source_intent(&store, &r).unwrap();
    let selected = choice(&p);
    manager
        .select_catalog_source(&store, &p, None, Some(selected))
        .unwrap();
    let token = manager
        .catalog_source_snapshot(&store, &p)
        .unwrap()
        .revision
        .unwrap();
    assert_eq!(
        code(manager.stage_catalog_source(&store, &r, &p)),
        "catalog_source_changed"
    );
    assert_eq!(
        code(manager.select_catalog_source(&store, &p, None, None)),
        "catalog_source_changed"
    );
    manager
        .select_catalog_source(&store, &p, Some(&token), None)
        .unwrap();
    let native_token = manager
        .catalog_source_snapshot(&store, &p)
        .unwrap()
        .revision
        .unwrap();
    manager
        .select_catalog_source(&store, &p, Some(&native_token), Some(choice(&p)))
        .unwrap();
    let second_token = manager
        .catalog_source_snapshot(&store, &p)
        .unwrap()
        .revision
        .unwrap();
    manager
        .select_catalog_source(&store, &p, Some(&second_token), None)
        .unwrap();
    let snapshot = manager.catalog_source_snapshot(&store, &p).unwrap();
    assert!(snapshot.catalog.is_none() && snapshot.revision.is_some());
    assert_ne!(
        snapshot.revision.as_deref(),
        Some(native_token.as_str()),
        "switching away and back must retire old reads"
    );
    assert_eq!(
        store
            .provider_operation(&r.store_id, &r.operation_id)
            .unwrap()
            .status,
        Status::Pending
    );
}

#[test]
fn catalog_source_remembers_metadata_edits_but_marks_endpoint_protocol_vendor_or_key_changed() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let (store, manager) = setup(&root);
    let r = request(&store, provider());
    stage(&store, &manager, &r, &choice(&r.provider));
    let mut saved = finish(&store, &r);
    manager.finish_catalog_source(&store, &r).unwrap();
    let token = manager
        .catalog_source_snapshot(&store, &saved)
        .unwrap()
        .revision;
    saved.name = "Renamed".into();
    saved.model = "another-model".into();
    store.save_provider(saved.clone()).unwrap();
    let saved = store.provider_by_id(&saved.id).unwrap().unwrap();
    let snapshot = manager.catalog_source_snapshot(&store, &saved).unwrap();
    assert!(snapshot.catalog.is_some() && snapshot.scope_matches && snapshot.revision == token);
    for index in 0..4 {
        let mut changed = saved.clone();
        match index {
            0 => changed.endpoint = "http://127.0.0.1:7777/v1".into(),
            1 => changed.protocol = "messages".into(),
            2 => changed.vendor = "openai".into(),
            _ => changed.credential_ref = Some(uuid::Uuid::new_v4().to_string()),
        }
        let snapshot = manager.catalog_source_snapshot(&store, &changed).unwrap();
        assert!(
            snapshot.catalog.is_some() && !snapshot.scope_matches && snapshot.revision == token
        );
    }
}

#[test]
fn catalog_source_missing_corrupt_or_mismatched_receipt_preserves_unknown_bytes_and_refuses_reset()
{
    for case in 0..5 {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let (store, manager) = setup(&root);
        let r = request(&store, provider());
        stage(&store, &manager, &r, &choice(&r.provider));
        let saved = finish(&store, &r);
        let file = path(&manager, &store, &saved);
        let mut record: Source = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
        match case {
            0 => {
                store
                    .connection
                    .execute(
                        "DELETE FROM provider_operations WHERE operation_id=?1",
                        [&r.operation_id],
                    )
                    .unwrap();
            }
            1 => record.pending.as_mut().unwrap().sequence += 1,
            2 => record.pending.as_mut().unwrap().request_hash = "0".repeat(64),
            3 => record.pending.as_mut().unwrap().provider_revision += 1,
            _ => {
                record
                    .pending
                    .as_mut()
                    .unwrap()
                    .next
                    .as_mut()
                    .unwrap()
                    .scope = "0".repeat(64)
            }
        }
        std::fs::write(&file, serde_json::to_vec(&record).unwrap()).unwrap();
        let bytes = std::fs::read(&file).unwrap();
        assert_eq!(
            code(manager.catalog_source_snapshot(&store, &saved)),
            "catalog_source_unavailable"
        );
        assert_eq!(
            code(manager.select_catalog_source(&store, &saved, Some(&digest(&bytes)), None)),
            "catalog_source_unavailable"
        );
        assert_eq!(std::fs::read(&file).unwrap(), bytes);
    }
}

#[test]
fn catalog_source_unknown_identity_version_or_unsafe_file_never_becomes_native() {
    for case in 0..7 {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let (store, manager) = setup(&root);
        let p = provider();
        manager
            .select_catalog_source(&store, &p, None, Some(choice(&p)))
            .unwrap();
        let file = path(&manager, &store, &p);
        let mut value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
        match case {
            0 => value["version"] = 2.into(),
            1 => value["store_id"] = uuid::Uuid::new_v4().to_string().into(),
            2 => value["provider_id"] = "openai".into(),
            3 => value["future"] = true.into(),
            4 => {
                std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
            }
            5 => {
                let actual = file.with_extension("kept");
                std::fs::rename(&file, &actual).unwrap();
                std::os::unix::fs::symlink(actual, &file).unwrap();
            }
            _ => value["padding"] = "x".repeat(16385).into(),
        }
        if ![4, 5].contains(&case) {
            std::fs::write(&file, serde_json::to_vec(&value).unwrap()).unwrap();
        }
        let bytes = std::fs::read(&file).unwrap();
        assert!(manager.catalog_source_snapshot(&store, &p).is_err());
        assert!(
            manager
                .select_catalog_source(&store, &p, Some(&digest(&bytes)), None)
                .is_err()
        );
        assert_eq!(std::fs::read(&file).unwrap(), bytes);
    }
}

#[test]
fn catalog_manual_save_keeps_the_original_receipt_without_inventing_verification_or_replaying() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let (store, manager) = setup(&root);
    let mut r = request(&store, provider());
    r.kind = Kind::Save;
    let selected = choice(&r.provider);
    let op = manager
        .save_catalog_provider(&store, &r, &selected, || Ok(()))
        .unwrap();
    assert_eq!(op.status, Status::Committed);
    assert!(op.verification.is_none());
    let saved = op.provider.as_ref().unwrap();
    assert_eq!(saved.revision, 1);
    let source = manager.catalog_source_snapshot(&store, saved).unwrap();
    assert!(source.scope_matches && !source.pending && source.catalog.as_ref() == Some(&selected));
    assert!(matches!(
        source.catalog.unwrap().target,
        Target::Draft { .. }
    ));
    let replay = manager
        .save_catalog_provider(&store, &r, &selected, || {
            panic!("replay must not authorize or execute work")
        })
        .unwrap();
    assert_eq!(
        serde_json::to_value(replay).unwrap(),
        serde_json::to_value(&op).unwrap()
    );
    assert_eq!(store.provider_write_protocol().unwrap().sequence, 1);
    assert_eq!(
        serde_json::to_value(&r).unwrap().as_object().unwrap().len(),
        5
    );
    let mut changed = selected;
    changed.digest = "b".repeat(64);
    assert!(
        manager
            .save_catalog_provider(&store, &r, &changed, || Ok(()))
            .is_err()
    );
    assert_eq!(
        store.provider_by_id(&saved.id).unwrap().unwrap().revision,
        1
    );
}

#[test]
fn catalog_manual_save_revocation_before_commit_restores_absence_and_allows_same_metadata_retry() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let (store, manager) = setup(&root);
    let mut r = request(&store, provider());
    r.kind = Kind::Save;
    let selected = choice(&r.provider);
    assert_eq!(
        code(
            manager.save_catalog_provider(&store, &r, &selected, || Err(ConfigError::new(
                "grant_revoked",
                "owned revoke"
            )))
        ),
        "grant_revoked"
    );
    assert!(store.provider_by_id(&r.provider.id).unwrap().is_none());
    assert_eq!(store.provider_write_protocol().unwrap().sequence, 0);
    assert!(store.existing_provider_operation(&r).unwrap().is_none());
    assert!(!path(&manager, &store, &r.provider).exists());
    let op = manager
        .save_catalog_provider(&store, &r, &selected, || Ok(()))
        .unwrap();
    assert_eq!(op.status, Status::Committed);
    assert!(op.verification.is_none());
}

#[test]
fn catalog_manual_save_rollback_preserves_previous_source_bytes_and_provider_metadata() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let (store, manager) = setup(&root);
    let p = provider();
    store.save_provider(p.clone()).unwrap();
    let p = store.provider_by_id(&p.id).unwrap().unwrap();
    let selected = choice(&p);
    manager
        .select_catalog_source(&store, &p, None, Some(selected))
        .unwrap();
    let source_file = path(&manager, &store, &p);
    let before = std::fs::read(&source_file).unwrap();
    let mut r = request(&store, p.clone());
    r.kind = Kind::Save;
    r.provider.name = "new name".into();
    assert!(
        manager
            .save_catalog_provider(&store, &r, &choice(&r.provider), || Err(ConfigError::new(
                "grant_revoked",
                "owned revoke"
            )))
            .is_err()
    );
    assert_eq!(std::fs::read(&source_file).unwrap(), before);
    assert_eq!(
        serde_json::to_value(store.provider_by_id(&p.id).unwrap().unwrap()).unwrap(),
        serde_json::to_value(p).unwrap()
    );
    assert_eq!(store.provider_write_protocol().unwrap().sequence, 0);
}

#[test]
fn catalog_manual_save_sql_commit_failure_undoes_its_own_source_and_never_commits_a_receipt() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let (store, manager) = setup(&root);
    let mut r = request(&store, provider());
    r.kind = Kind::Save;
    let selected = choice(&r.provider);
    store.connection.execute_batch("PRAGMA foreign_keys=ON;
        CREATE TABLE owned_deferred_failure(id TEXT REFERENCES provider_operations(operation_id) DEFERRABLE INITIALLY DEFERRED);
        CREATE TRIGGER owned_commit_failure AFTER INSERT ON provider_operations BEGIN INSERT INTO owned_deferred_failure VALUES('missing-owned-operation'); END;").unwrap();
    assert!(
        manager
            .save_catalog_provider(&store, &r, &selected, || Ok(()))
            .is_err()
    );
    assert!(store.existing_provider_operation(&r).unwrap().is_none());
    assert_eq!(store.provider_write_protocol().unwrap().sequence, 0);
    assert!(store.provider_by_id(&r.provider.id).unwrap().is_none());
    assert!(!path(&manager, &store, &r.provider).exists());
    store
        .connection
        .execute_batch("DROP TRIGGER owned_commit_failure;")
        .unwrap();
    assert_eq!(
        manager
            .save_catalog_provider(&store, &r, &selected, || Ok(()))
            .unwrap()
            .status,
        Status::Committed
    );
}

#[test]
fn catalog_manual_save_crash_before_sql_commit_stays_unknown_until_original_cancel_receipt() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let (store, manager) = setup(&root);
    let mut r = request(&store, provider());
    r.kind = Kind::Save;
    let selected = choice(&r.provider);
    // Bypass the in-process undo to reproduce a terminated process after its
    // durable source stage but before the SQL commit acknowledgement.
    assert!(
        store
            .start_provider_operation_before_commit(&r, |op| {
                manager.record_catalog_operation(&store, &r, &selected)?;
                manager.stage_catalog_source(&store, &r, op.provider.as_ref().unwrap())?;
                Err(ConfigError::new("owned_crash", "owned abrupt interruption"))
            })
            .is_err()
    );
    let file = path(&manager, &store, &r.provider);
    let bytes = std::fs::read(&file).unwrap();
    drop(manager);
    drop(store);
    let (store, manager) = setup(&root);
    assert_eq!(
        code(manager.catalog_source_snapshot(&store, &r.provider)),
        "catalog_source_unavailable"
    );
    assert_eq!(std::fs::read(&file).unwrap(), bytes);
    assert_eq!(
        store.cancel_provider_connection(&r).unwrap().status,
        Status::Cancelled
    );
    let source = manager
        .catalog_source_snapshot(&store, &r.provider)
        .unwrap();
    assert!(!source.pending && source.catalog.is_none());
    assert!(store.provider_by_id(&r.provider.id).unwrap().is_none());
    manager.finish_catalog_source(&store, &r).unwrap();
}

#[test]
fn catalog_manual_save_crash_after_sql_commit_recovers_choice_and_original_save_without_permissions()
 {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let (store, manager) = setup(&root);
    let mut r = request(&store, provider());
    r.kind = Kind::Save;
    let selected = choice(&r.provider);
    let op = store
        .start_provider_operation_before_commit(&r, |op| {
            manager.record_catalog_operation(&store, &r, &selected)?;
            manager.stage_catalog_source(&store, &r, op.provider.as_ref().unwrap())
        })
        .unwrap()
        .operation;
    drop(manager);
    drop(store);
    let (store, manager) = setup(&root);
    let source = manager
        .catalog_source_snapshot(&store, op.provider.as_ref().unwrap())
        .unwrap();
    assert!(!source.pending && source.scope_matches && source.catalog.as_ref() == Some(&selected));
    let recovered = manager
        .save_catalog_provider(&store, &r, &selected, || {
            panic!("result recovery needs no grant or work")
        })
        .unwrap();
    assert_eq!(
        serde_json::to_value(recovered).unwrap(),
        serde_json::to_value(op).unwrap()
    );
}
#[test]
fn catalog_bound_marker_refuses_legacy_takeover_even_when_the_sql_result_is_missing() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let (store, manager) = setup(&root);
    let r = request(&store, provider());
    manager
        .require_native_provider_operation(&store, &r)
        .unwrap();
    stage(&store, &manager, &r, &choice(&r.provider));
    store
        .connection
        .execute(
            "DELETE FROM provider_operations WHERE operation_id=?1",
            [&r.operation_id],
        )
        .unwrap();
    assert_eq!(
        code(manager.require_native_provider_operation(&store, &r)),
        "catalog_operation_unavailable"
    );
    std::fs::remove_file(manager.catalog_intent_path(&r, false).unwrap()).unwrap();
    assert_eq!(
        code(manager.require_native_provider_operation(&store, &r)),
        "catalog_operation_unavailable"
    );
    assert!(store.provider_by_id(&r.provider.id).unwrap().is_none());
}
#[test]
fn catalog_missing_registered_source_file_never_becomes_builtin_or_reselected_silently() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let (store, manager) = setup(&root);
    let p = provider();
    manager
        .select_catalog_source(&store, &p, None, Some(choice(&p)))
        .unwrap();
    let file = path(&manager, &store, &p);
    let bytes = std::fs::read(&file).unwrap();
    let marker = manager
        .catalog_marker_path(&store.database_id().unwrap(), &p.id, false)
        .unwrap();
    assert_eq!(
        std::fs::metadata(&marker).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let proof = std::fs::read(&marker).unwrap();
    std::fs::remove_file(&file).unwrap();
    assert_eq!(
        code(manager.catalog_source_snapshot(&store, &p)),
        "catalog_source_unavailable"
    );
    assert!(
        manager
            .select_catalog_source(&store, &p, Some(&digest(&bytes)), None)
            .is_err()
    );
    assert!(!file.exists());
    assert_eq!(std::fs::read(&marker).unwrap(), proof);
    std::fs::write(&file, &bytes).unwrap();
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert!(
        manager
            .catalog_source_snapshot(&store, &p)
            .unwrap()
            .catalog
            .is_some()
    );
}

#[test]
fn catalog_healthy_legacy_source_gets_an_integrity_marker_without_rewriting_or_authorizing_it() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let (store, manager) = setup(&root);
    let p = provider();
    manager
        .select_catalog_source(&store, &p, None, Some(choice(&p)))
        .unwrap();
    let file = path(&manager, &store, &p);
    let bytes = std::fs::read(&file).unwrap();
    let marker = manager
        .catalog_marker_path(&store.database_id().unwrap(), &p.id, false)
        .unwrap();
    std::fs::remove_file(&marker).unwrap();
    let before = store.provider_write_protocol().unwrap().sequence;
    assert!(matches!(
        manager
            .catalog_source_snapshot(&store, &p)
            .unwrap()
            .catalog
            .unwrap()
            .target,
        Target::Draft { .. }
    ));
    assert!(marker.exists());
    assert_eq!(std::fs::read(&file).unwrap(), bytes);
    assert_eq!(store.provider_write_protocol().unwrap().sequence, before);
}

#[test]
fn catalog_future_or_mismatched_integrity_marker_is_preserved_with_its_source() {
    for case in 0..3 {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let (store, manager) = setup(&root);
        let p = provider();
        manager
            .select_catalog_source(&store, &p, None, Some(choice(&p)))
            .unwrap();
        let file = path(&manager, &store, &p);
        let source = std::fs::read(&file).unwrap();
        let marker = manager
            .catalog_marker_path(&store.database_id().unwrap(), &p.id, false)
            .unwrap();
        let mut value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&marker).unwrap()).unwrap();
        match case {
            0 => value["version"] = 2.into(),
            1 => value["provider_id"] = "other".into(),
            _ => value["store_id"] = uuid::Uuid::new_v4().to_string().into(),
        }
        std::fs::write(&marker, serde_json::to_vec(&value).unwrap()).unwrap();
        let bytes = std::fs::read(&marker).unwrap();
        assert!(manager.catalog_source_snapshot(&store, &p).is_err());
        assert!(
            manager
                .select_catalog_source(&store, &p, None, None)
                .is_err()
        );
        assert_eq!(std::fs::read(&marker).unwrap(), bytes);
        assert_eq!(std::fs::read(&file).unwrap(), source);
    }
}

#[test]
fn catalog_failed_first_source_write_does_not_leave_a_false_integrity_marker() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let (store, manager) = setup(&root);
    let p = provider();
    let source = manager
        .catalog_source_path(&store.database_id().unwrap(), &p.id, true)
        .unwrap();
    let folder = source.parent().unwrap();
    std::fs::set_permissions(folder, std::fs::Permissions::from_mode(0o500)).unwrap();
    assert!(
        manager
            .select_catalog_source(&store, &p, None, Some(choice(&p)))
            .is_err()
    );
    let marker = manager
        .catalog_marker_path(&store.database_id().unwrap(), &p.id, false)
        .unwrap();
    assert!(!marker.exists());
    assert!(!source.exists());
    std::fs::set_permissions(folder, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert!(
        manager
            .catalog_source_snapshot(&store, &p)
            .unwrap()
            .catalog
            .is_none()
    );
}

fn explicit(
    store: &Store,
    p: Provider,
    kind: Kind,
    source: crate::domain::provider_source_operation::SourceChoice,
    revision: Option<String>,
) -> crate::domain::provider_source_operation::SourceRequest {
    let mut operation = request(store, p);
    operation.kind = kind;
    crate::domain::provider_source_operation::SourceRequest {
        operation,
        source,
        expected_source_revision: revision,
    }
}
#[test]
fn explicit_builtin_save_atomically_replaces_external_and_replays_original_receipt() {
    use crate::domain::provider_source_operation::SourceChoice;
    let temp = tempfile::tempdir().unwrap();
    let (store, manager) = setup(&temp.path().canonicalize().unwrap());
    let p = provider();
    let selected = choice(&p);
    let mut old = request(&store, p);
    old.kind = Kind::Save;
    let saved = manager
        .save_catalog_provider(&store, &old, &selected, || Ok(()))
        .unwrap()
        .provider
        .unwrap();
    let token = manager
        .catalog_source_snapshot(&store, &saved)
        .unwrap()
        .revision;
    let input = explicit(&store, saved, Kind::Save, SourceChoice::Builtin {}, token);
    let op = manager
        .save_source_provider(&store, &input, || Ok(()))
        .unwrap();
    assert!(op.status == Status::Committed && op.verification.is_none());
    manager.verify_source_operation(&input).unwrap();
    let source = manager
        .catalog_source_snapshot(&store, op.provider.as_ref().unwrap())
        .unwrap();
    assert!(source.catalog.is_none() && !source.pending && source.scope_matches);
    assert_ne!(source.revision, input.expected_source_revision);
    assert_eq!(
        serde_json::to_value(
            manager
                .save_source_provider(&store, &input, || panic!("must not reauthorize"))
                .unwrap()
        )
        .unwrap(),
        serde_json::to_value(op).unwrap()
    );
    let mut wrong = input.clone();
    wrong.expected_source_revision = None;
    assert!(manager.verify_source_operation(&wrong).is_err());
    assert!(
        manager
            .require_native_provider_operation(&store, &input.operation)
            .is_err()
    );
}
#[test]
fn explicit_source_cas_refuses_save_without_advancing_sql_or_erasing_choice() {
    use crate::domain::provider_source_operation::SourceChoice;
    let temp = tempfile::tempdir().unwrap();
    let (store, manager) = setup(&temp.path().canonicalize().unwrap());
    let p = provider();
    let selected = choice(&p);
    manager
        .select_catalog_source(&store, &p, None, Some(selected.clone()))
        .unwrap();
    let before = std::fs::read(path(&manager, &store, &p)).unwrap();
    let input = explicit(&store, p, Kind::Save, SourceChoice::Builtin {}, None);
    let clock = store.provider_write_protocol().unwrap().sequence;
    assert!(
        manager
            .save_source_provider(&store, &input, || Ok(()))
            .is_err()
    );
    assert_eq!(store.provider_write_protocol().unwrap().sequence, clock);
    assert!(
        store
            .existing_provider_operation(&input.operation)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        std::fs::read(path(&manager, &store, &input.operation.provider)).unwrap(),
        before
    );
}
#[test]
fn explicit_builtin_pending_cannot_publish_authority_and_cancel_keeps_old_choice() {
    use crate::domain::provider_source_operation::SourceChoice;
    let temp = tempfile::tempdir().unwrap();
    let (store, manager) = setup(&temp.path().canonicalize().unwrap());
    let p = provider();
    let selected = choice(&p);
    manager
        .select_catalog_source(&store, &p, None, Some(selected.clone()))
        .unwrap();
    let token = manager
        .catalog_source_snapshot(&store, &p)
        .unwrap()
        .revision;
    let input = explicit(&store, p, Kind::Connect, SourceChoice::Builtin {}, token);
    store.start_provider_operation(&input.operation).unwrap();
    manager.record_source_operation(&store, &input).unwrap();
    manager
        .stage_catalog_source(&store, &input.operation, &input.operation.provider)
        .unwrap();
    let source = manager
        .catalog_source_snapshot(&store, &input.operation.provider)
        .unwrap();
    assert!(source.pending && source.catalog.is_none());
    store.cancel_provider_connection(&input.operation).unwrap();
    manager
        .finish_catalog_source(&store, &input.operation)
        .unwrap();
    let source = manager
        .catalog_source_snapshot(&store, &input.operation.provider)
        .unwrap();
    assert!(!source.pending && source.catalog.as_ref() == Some(&selected));
}
#[test]
fn explicit_builtin_failed_save_restores_external_bytes_and_same_request_can_retry() {
    use crate::domain::provider_source_operation::SourceChoice;
    let temp = tempfile::tempdir().unwrap();
    let (store, manager) = setup(&temp.path().canonicalize().unwrap());
    let p = provider();
    let selected = choice(&p);
    manager
        .select_catalog_source(&store, &p, None, Some(selected))
        .unwrap();
    let token = manager
        .catalog_source_snapshot(&store, &p)
        .unwrap()
        .revision;
    let before = std::fs::read(path(&manager, &store, &p)).unwrap();
    let input = explicit(&store, p, Kind::Save, SourceChoice::Builtin {}, token);
    assert!(
        manager
            .save_source_provider(&store, &input, || Err(ConfigError::new(
                "revoked", "fixture"
            )))
            .is_err()
    );
    assert!(
        store
            .existing_provider_operation(&input.operation)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        std::fs::read(path(&manager, &store, &input.operation.provider)).unwrap(),
        before
    );
    manager
        .save_source_provider(&store, &input, || Ok(()))
        .unwrap();
}
#[test]
fn explicit_builtin_crash_after_sql_commit_resolves_only_original_receipt() {
    use crate::domain::provider_source_operation::SourceChoice;
    let temp = tempfile::tempdir().unwrap();
    let (store, manager) = setup(&temp.path().canonicalize().unwrap());
    let p = provider();
    let token = manager
        .catalog_source_snapshot(&store, &p)
        .unwrap()
        .revision;
    let input = explicit(&store, p, Kind::Connect, SourceChoice::Builtin {}, token);
    store.start_provider_operation(&input.operation).unwrap();
    manager.record_source_operation(&store, &input).unwrap();
    manager
        .stage_catalog_source(&store, &input.operation, &input.operation.provider)
        .unwrap();
    let saved = finish(&store, &input.operation);
    drop(manager);
    drop(store);
    let (store, manager) = setup(&temp.path().canonicalize().unwrap());
    let source = manager.catalog_source_snapshot(&store, &saved).unwrap();
    assert!(!source.pending && source.catalog.is_none());
    manager.verify_source_operation(&input).unwrap();
}
