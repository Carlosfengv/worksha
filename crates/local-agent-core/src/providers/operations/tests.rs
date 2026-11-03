use super::*;
use std::sync::{
    Arc, Barrier,
    atomic::{AtomicBool, Ordering},
};

fn provider() -> Provider {
    Provider {
        id: format!("custom-{}", uuid::Uuid::new_v4()),
        name: "Fixture".into(),
        vendor: "custom".into(),
        endpoint: "http://127.0.0.1:11434/v1".into(),
        protocol: "responses".into(),
        model: "fixture".into(),
        configured: false,
        credential_ref: None,
        credential_label: None,
        models: vec!["fixture".into()],
        disabled_models: vec![],
        model_catalog: None,
        revision: 0,
        category: "local".into(),
    }
}
fn request(store: &Store, kind: Kind, provider: Provider) -> Request {
    let protocol = store.provider_write_protocol().unwrap();
    Request {
        store_id: protocol.store_id,
        operation_id: uuid::Uuid::new_v4().to_string(),
        expected_sequence: protocol.sequence,
        kind,
        provider,
    }
}
fn code<T>(value: Result<T, ConfigError>) -> String {
    value.err().expect("expected failure").code
}
fn rows(store: &Store) -> u32 {
    store
        .connection
        .query_row("SELECT count(*) FROM providers", [], |r| r.get::<_, u32>(0))
        .unwrap()
}
fn finish(store: &Store, r: &Request) -> Result<Operation, ConfigError> {
    store.finish_provider_connection(r, r.provider.clone(), "2026-10-28T00:00:00Z".into(), |_| {
        Ok(())
    })
}

#[test]
fn save_replay_after_reopen_preserves_the_original_receipt_and_does_not_overwrite_newer_metadata() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let first = request(&store, Kind::Save, provider());
    let committed = store.start_provider_operation(&first).unwrap().operation;
    assert_eq!(committed.status, Status::Committed);
    assert_eq!(committed.provider.as_ref().unwrap().revision, 1);
    let mut newer = committed.provider.clone().unwrap();
    newer.name = "Newer name".into();
    store.save_provider(newer).unwrap();
    drop(store);
    let store = Store::open(dir.path()).unwrap();
    let replay = store.start_provider_operation(&first).unwrap();
    assert!(!replay.started);
    assert_eq!(
        serde_json::to_value(replay.operation).unwrap(),
        serde_json::to_value(committed).unwrap()
    );
    let current = store
        .providers()
        .unwrap()
        .into_iter()
        .find(|p| p.id == first.provider.id)
        .unwrap();
    assert_eq!((current.revision, current.name.as_str()), (2, "Newer name"));
}
#[test]
fn same_id_with_changed_payload_is_rejected_for_pending_and_committed_operations() {
    for kind in [Kind::Save, Kind::Connect] {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let r = request(&store, kind, provider());
        store.start_provider_operation(&r).unwrap();
        let mut other = r.clone();
        other.provider.name = "Changed".into();
        assert_eq!(
            code(store.start_provider_operation(&other)),
            "operation_conflict"
        );
        assert_eq!(store.provider_write_protocol().unwrap().sequence, 1);
        assert_eq!(
            code(store.cancel_provider_connection(&other)),
            "operation_conflict"
        );
    }
}
#[test]
fn request_query_and_cancel_cannot_cross_database_identity() {
    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    let a = Store::open(a.path()).unwrap();
    let b = Store::open(b.path()).unwrap();
    let r = request(&a, Kind::Connect, provider());
    a.start_provider_operation(&r).unwrap();
    assert_eq!(
        code(b.start_provider_operation(&r)),
        "operation_store_changed"
    );
    assert_eq!(
        code(b.provider_operation(&r.store_id, &r.operation_id)),
        "operation_store_changed"
    );
    assert_eq!(
        code(b.cancel_provider_connection(&r)),
        "operation_store_changed"
    );
    assert_eq!(b.provider_write_protocol().unwrap().sequence, 0);
    assert_eq!(rows(&b), 0);
}
#[test]
fn only_the_creator_may_probe_and_pending_capacity_has_a_cancel_recovery_path() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let first = request(&store, Kind::Connect, provider());
    assert!(store.start_provider_operation(&first).unwrap().started);
    assert!(!store.start_provider_operation(&first).unwrap().started);
    let competing = request(&store, Kind::Save, first.provider.clone());
    assert_eq!(
        code(store.start_provider_operation(&competing)),
        "operation_in_progress"
    );
    for _ in 1..PENDING_LIMIT {
        let r = request(&store, Kind::Connect, provider());
        assert!(store.start_provider_operation(&r).unwrap().started);
    }
    let blocked = request(&store, Kind::Connect, provider());
    assert_eq!(code(store.start_provider_operation(&blocked)), "capacity");
    store.cancel_provider_connection(&first).unwrap();
    assert!(store.start_provider_operation(&blocked).unwrap().started);
    assert_eq!(rows(&store), 0);
}
#[test]
fn cancel_before_delivery_blocks_late_manual_save_and_late_probe() {
    for kind in [Kind::Save, Kind::Connect] {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let r = request(&store, kind, provider());
        let cancelled = store.cancel_provider_connection(&r).unwrap();
        assert_eq!(cancelled.status, Status::Cancelled);
        let late = store.start_provider_operation(&r).unwrap();
        assert!(!late.started);
        assert_eq!(late.operation.status, Status::Cancelled);
        assert_eq!(rows(&store), 0);
        assert_eq!(store.provider_write_protocol().unwrap().sequence, 1);
    }
}
#[test]
fn cancellation_wins_over_late_detection_and_never_publishes_verification() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let r = request(&store, Kind::Connect, provider());
    store.start_provider_operation(&r).unwrap();
    store.cancel_provider_connection(&r).unwrap();
    let called = AtomicBool::new(false);
    let late = store
        .finish_provider_connection(
            &r,
            r.provider.clone(),
            "2026-10-28T00:00:00Z".into(),
            |_| {
                called.store(true, Ordering::SeqCst);
                Ok(())
            },
        )
        .unwrap();
    assert_eq!(late.status, Status::Cancelled);
    assert!(!called.load(Ordering::SeqCst));
    assert_eq!(rows(&store), 0);
}
#[test]
fn success_persists_exact_verification_and_failure_cannot_replace_committed_result() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let r = request(&store, Kind::Connect, provider());
    store.start_provider_operation(&r).unwrap();
    let committed = finish(&store, &r).unwrap();
    assert_eq!(committed.status, Status::Committed);
    assert_eq!(
        committed.verification.as_ref().unwrap().fingerprint,
        super::super::fingerprint(committed.provider.as_ref().unwrap())
    );
    let after = store
        .fail_provider_connection(&r, &ConfigError::new("probe_network", "Interrupted"))
        .unwrap();
    assert_eq!(
        serde_json::to_value(after).unwrap(),
        serde_json::to_value(&committed).unwrap()
    );
    drop(store);
    let store = Store::open(dir.path()).unwrap();
    assert_eq!(
        serde_json::to_value(
            store
                .provider_operation(&r.store_id, &r.operation_id)
                .unwrap()
        )
        .unwrap(),
        serde_json::to_value(committed).unwrap()
    );
}
#[test]
fn changed_provider_and_verification_file_failure_do_not_commit_metadata() {
    for change_metadata in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let r = request(&store, Kind::Connect, provider());
        store.start_provider_operation(&r).unwrap();
        if change_metadata {
            let mut changed = r.provider.clone();
            changed.name = "Other writer".into();
            store.save_provider(changed).unwrap();
        }
        let result = store.finish_provider_connection(
            &r,
            r.provider.clone(),
            "2026-10-28T00:00:00Z".into(),
            |_| Err(ConfigError::new("write_failed", "Synthetic file failure")),
        );
        assert_eq!(
            code(result),
            if change_metadata {
                "conflict"
            } else {
                "write_failed"
            }
        );
        let pending = store
            .provider_operation(&r.store_id, &r.operation_id)
            .unwrap();
        assert_eq!(pending.status, Status::Pending);
        assert_eq!(rows(&store), u32::from(change_metadata));
        let failed = store
            .fail_provider_connection(&r, &ConfigError::new("write_failed", "Synthetic rollback"))
            .unwrap();
        assert_eq!(failed.status, Status::Failed);
        assert!(!store.start_provider_operation(&r).unwrap().started);
    }
}
#[test]
fn ignored_metadata_receipt_or_clock_writes_roll_back_the_entire_save() {
    for table in [
        "providers",
        "provider_operations",
        "provider_operation_scope",
    ] {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let r = request(&store, Kind::Save, provider());
        let action = if table == "provider_operation_scope" {
            "UPDATE"
        } else {
            "INSERT"
        };
        store.connection.execute_batch(&format!("CREATE TRIGGER reject_write BEFORE {action} ON {table} BEGIN SELECT RAISE(IGNORE); END;")).unwrap();
        assert_eq!(
            code(store.start_provider_operation(&r)),
            "service_unavailable"
        );
        assert_eq!(rows(&store), 0);
        assert_eq!(store.provider_write_protocol().unwrap().sequence, 0);
        assert_eq!(
            store
                .connection
                .query_row("SELECT count(*) FROM provider_operations", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }
}
#[test]
fn ignored_finish_receipt_update_leaves_pending_and_rolls_back_provider() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let r = request(&store, Kind::Connect, provider());
    store.start_provider_operation(&r).unwrap();
    store.connection.execute_batch("CREATE TRIGGER ignore_result BEFORE UPDATE ON provider_operations BEGIN SELECT RAISE(IGNORE); END;").unwrap();
    assert_eq!(code(finish(&store, &r)), "service_unavailable");
    assert_eq!(rows(&store), 0);
    assert_eq!(
        store
            .provider_operation(&r.store_id, &r.operation_id)
            .unwrap()
            .status,
        Status::Pending
    );
}
#[test]
fn reclaimed_cancelled_receipt_cannot_be_replayed_or_be_falsely_cancelled_again() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let old = request(&store, Kind::Connect, provider());
    store.cancel_provider_connection(&old).unwrap();
    for _ in 0..RECEIPT_LIMIT {
        let r = request(&store, Kind::Connect, provider());
        store.cancel_provider_connection(&r).unwrap();
    }
    assert_eq!(
        code(store.provider_operation(&old.store_id, &old.operation_id)),
        "operation_not_found"
    );
    assert_eq!(
        code(store.start_provider_operation(&old)),
        "operation_expired"
    );
    assert_eq!(
        code(store.cancel_provider_connection(&old)),
        "operation_not_found"
    );
    assert_eq!(
        store
            .connection
            .query_row("SELECT count(*) FROM provider_operations", [], |r| r
                .get::<_, u32>(0))
            .unwrap(),
        RECEIPT_LIMIT as u32
    );
    assert_eq!(rows(&store), 0);
}
#[test]
fn terminal_byte_budget_reclaims_large_results_without_reopening_old_write_identity() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let mut p = provider();
    p.models = (0..1800)
        .map(|n| format!("{n:04}-{}", "m".repeat(145)))
        .collect();
    let first = request(&store, Kind::Save, p.clone());
    for n in 0..40 {
        let r = if n == 0 {
            first.clone()
        } else {
            request(&store, Kind::Save, p)
        };
        p = store
            .start_provider_operation(&r)
            .unwrap()
            .operation
            .provider
            .unwrap();
    }
    let bytes: i64 = store
        .connection
        .query_row(
            "SELECT sum(length(CAST(value AS BLOB))) FROM provider_operations",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(bytes <= RECEIPT_BYTES as i64);
    assert_eq!(
        code(store.start_provider_operation(&first)),
        "operation_expired"
    );
    assert_eq!(
        store
            .providers()
            .unwrap()
            .into_iter()
            .find(|p| p.id == first.provider.id)
            .unwrap()
            .revision,
        40
    );
}
#[test]
fn future_missing_or_rolled_back_protocol_state_is_preserved_and_rejected() {
    for mutation in [
        "UPDATE provider_operation_scope SET version=2",
        "DELETE FROM provider_operation_scope",
        "UPDATE provider_operation_scope SET sequence=0",
    ] {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let r = request(&store, Kind::Save, provider());
        store.start_provider_operation(&r).unwrap();
        store.connection.execute_batch(mutation).unwrap();
        assert_eq!(
            code(store.provider_operation(&r.store_id, &r.operation_id)),
            "invalid_record"
        );
        if mutation.contains("DELETE") {
            drop(store);
            assert!(Store::open(dir.path()).is_err());
            let db = Connection::open(dir.path().join("local-agent.sqlite3")).unwrap();
            assert_eq!(
                db.query_row("SELECT count(*) FROM provider_operation_scope", [], |r| {
                    r.get::<_, u32>(0)
                })
                .unwrap(),
                0
            );
        }
    }
}
#[test]
fn corrupt_receipt_is_rejected_without_repair_or_false_success() {
    for field in [
        "$.apiVersion",
        "$.sequence",
        "$.request.provider.revision",
        "$.provider.revision",
    ] {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let r = request(&store, Kind::Save, provider());
        store.start_provider_operation(&r).unwrap();
        store
            .connection
            .execute(
                "UPDATE provider_operations SET value=json_set(value,?1,999)",
                [field],
            )
            .unwrap();
        let before: String = store
            .connection
            .query_row("SELECT value FROM provider_operations", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            code(store.provider_operation(&r.store_id, &r.operation_id)),
            "invalid_record"
        );
        assert_eq!(
            store
                .connection
                .query_row("SELECT value FROM provider_operations", [], |r| r
                    .get::<_, String>(0))
                .unwrap(),
            before
        );
    }
}
#[test]
fn invalid_ids_unsafe_revisions_and_oversized_inputs_never_advance_clock() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    for case in 0..6 {
        let mut r = request(&store, Kind::Connect, provider());
        match case {
            0 => r.operation_id = "not-an-id".into(),
            1 => r.expected_sequence = MAX_REVISION,
            2 => r.provider.revision = MAX_REVISION,
            3 => r.provider.name = "x".repeat(MAX_REQUEST_BYTES),
            4 => r.operation_id = "00000000-0000-4000-0000-000000000000".into(),
            _ => r.store_id = "00000000-0000-4000-0000-000000000000".into(),
        }
        assert_eq!(code(store.start_provider_operation(&r)), "invalid_input");
    }
    let mut wire = serde_json::to_value(request(&store, Kind::Save, provider())).unwrap();
    wire["future"] = true.into();
    assert!(serde_json::from_value::<Request>(wire).is_err());
    assert_eq!(store.provider_write_protocol().unwrap().sequence, 0);
}
#[test]
fn legacy_writers_compare_revisions_inside_one_database_transaction() {
    let dir = tempfile::tempdir().unwrap();
    let initial = Store::open(dir.path()).unwrap();
    let p = provider();
    drop(initial);
    let barrier = Arc::new(Barrier::new(2));
    let mut workers = vec![];
    for n in 0..2 {
        let path = dir.path().to_path_buf();
        let b = barrier.clone();
        let mut draft = p.clone();
        draft.name = format!("Writer {n}");
        workers.push(std::thread::spawn(move || {
            let store = Store::open(&path).unwrap();
            b.wait();
            store.save_provider(draft)
        }));
    }
    let results: Vec<_> = workers.into_iter().map(|w| w.join().unwrap()).collect();
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    let store = Store::open(dir.path()).unwrap();
    assert_eq!(
        store
            .providers()
            .unwrap()
            .into_iter()
            .find(|x| x.id == p.id)
            .unwrap()
            .revision,
        1
    );
}
#[test]
fn v8_migration_preserves_identity_provider_and_private_backup_version() {
    let dir = tempfile::tempdir().unwrap();
    let directory = dir.path().canonicalize().unwrap();
    let store = Store::open(&directory).unwrap();
    store.save_provider(provider()).unwrap();
    let identity = store.database_id().unwrap();
    let before = serde_json::to_value(store.providers().unwrap()).unwrap();
    store.connection.execute_batch("DROP TABLE local_token_cancellations;DROP TABLE local_token_owners;DROP TABLE local_token_values;DROP TABLE local_token_scope;DROP TABLE native_import_operations;DROP TABLE native_import_scope;DROP TABLE credential_legacy_operations;DROP TABLE credential_legacy_scope;DROP TABLE credential_recovery_values;DROP TABLE credential_recovery_scope;DROP TABLE provider_operations;DROP TABLE provider_operation_scope;PRAGMA user_version=8;").unwrap();
    drop(store);
    let upgraded = Store::open(&directory).unwrap();
    assert_eq!(upgraded.database_id().unwrap(), identity);
    assert_eq!(
        serde_json::to_value(upgraded.providers().unwrap()).unwrap(),
        before
    );
    assert_eq!(
        upgraded
            .connection
            .pragma_query_value(None, "user_version", |r| r.get::<_, u32>(0))
            .unwrap(),
        Store::SCHEMA_VERSION
    );
    let path = dir
        .path()
        .join("local-agent.pre-provider-operations-v8.sqlite3");
    let backup =
        Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    assert_eq!(
        backup
            .pragma_query_value(None, "user_version", |r| r.get::<_, u32>(0))
            .unwrap(),
        8
    );
    assert_eq!(
        backup
            .query_row(
                "SELECT store_id FROM model_service_operation_scope",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
        identity
    );
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let bytes = std::fs::read(&path).unwrap();
    drop(backup);
    drop(upgraded);
    drop(Store::open(&directory).unwrap());
    assert_eq!(std::fs::read(path).unwrap(), bytes);
}
#[test]
fn backend_pending_identities_survive_reopen_and_allow_recovery_without_client_records() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let mut original = vec![];
    for _ in 0..PENDING_LIMIT {
        let r = request(&store, Kind::Connect, provider());
        store.start_provider_operation(&r).unwrap();
        original.push(r);
    }
    let ids: Vec<_> = original.iter().map(|r| r.operation_id.clone()).collect();
    drop(store);
    let store = Store::open(dir.path()).unwrap();
    assert_eq!(
        store
            .provider_write_protocol()
            .unwrap()
            .pending_operation_ids,
        ids
    );
    for r in original {
        let recovered = store
            .provider_operation(&r.store_id, &r.operation_id)
            .unwrap();
        assert_eq!(
            serde_json::to_value(&recovered.request).unwrap(),
            serde_json::to_value(&r).unwrap()
        );
        assert_eq!(
            store
                .cancel_provider_connection(&recovered.request)
                .unwrap()
                .status,
            Status::Cancelled
        );
    }
    assert!(
        store
            .provider_write_protocol()
            .unwrap()
            .pending_operation_ids
            .is_empty()
    );
    let next = request(&store, Kind::Connect, provider());
    assert!(store.start_provider_operation(&next).unwrap().started);
}

#[test]
fn invalid_v8_backup_prevents_migration_without_changing_provider_or_source_schema() {
    let dir = tempfile::tempdir().unwrap();
    let directory = dir.path().canonicalize().unwrap();
    let store = Store::open(&directory).unwrap();
    let p = provider();
    store.save_provider(p.clone()).unwrap();
    let bytes: String = store
        .connection
        .query_row("SELECT value FROM providers WHERE id=?1", [&p.id], |r| {
            r.get(0)
        })
        .unwrap();
    store.connection.execute_batch("DROP TABLE local_token_cancellations;DROP TABLE local_token_owners;DROP TABLE local_token_values;DROP TABLE local_token_scope;DROP TABLE native_import_operations;DROP TABLE native_import_scope;DROP TABLE credential_legacy_operations;DROP TABLE credential_legacy_scope;DROP TABLE credential_recovery_values;DROP TABLE credential_recovery_scope;DROP TABLE provider_operations;DROP TABLE provider_operation_scope;PRAGMA user_version=8;").unwrap();
    drop(store);
    let backup = directory.join("local-agent.pre-provider-operations-v8.sqlite3");
    std::fs::write(&backup, b"invalid synthetic backup").unwrap();
    assert!(Store::open(&directory).err().unwrap().contains("升级备份"));
    let db = Connection::open(directory.join("local-agent.sqlite3")).unwrap();
    assert_eq!(
        db.pragma_query_value(None, "user_version", |r| r.get::<_, u32>(0))
            .unwrap(),
        8
    );
    assert_eq!(
        db.query_row("SELECT value FROM providers WHERE id=?1", [&p.id], |r| {
            r.get::<_, String>(0)
        })
        .unwrap(),
        bytes
    );
    assert_eq!(std::fs::read(backup).unwrap(), b"invalid synthetic backup");
}

#[test]
fn reclamation_does_not_silently_delete_a_corrupt_old_receipt_or_commit_the_new_write() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let old = request(&store, Kind::Connect, provider());
    store.cancel_provider_connection(&old).unwrap();
    for _ in 1..RECEIPT_LIMIT {
        let r = request(&store, Kind::Connect, provider());
        store.cancel_provider_connection(&r).unwrap();
    }
    store.connection.execute("UPDATE provider_operations SET value=json_set(value,'$.apiVersion',2) WHERE operation_id=?1",[&old.operation_id]).unwrap();
    let before: String = store
        .connection
        .query_row(
            "SELECT value FROM provider_operations WHERE operation_id=?1",
            [&old.operation_id],
            |r| r.get(0),
        )
        .unwrap();
    let next = request(&store, Kind::Save, provider());
    assert_eq!(
        code(store.start_provider_operation(&next)),
        "invalid_record"
    );
    assert_eq!(
        store.provider_write_protocol().unwrap().sequence,
        RECEIPT_LIMIT as u64
    );
    assert_eq!(rows(&store), 0);
    assert_eq!(
        store
            .connection
            .query_row(
                "SELECT value FROM provider_operations WHERE operation_id=?1",
                [&old.operation_id],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
        before
    );
    assert_eq!(
        code(store.provider_operation(&next.store_id, &next.operation_id)),
        "operation_not_found"
    );
}
#[test]
fn complete_legacy_catalog_and_disabled_models_are_preserved_by_the_operation_protocol() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let mut p = provider();
    p.models = (0..2000)
        .map(|n| format!("{n:04}-{}", "m".repeat(195)))
        .collect();
    p.model = p.models[0].clone();
    p.disabled_models = p.models.iter().skip(1).cloned().collect();
    store.save_provider(p.clone()).unwrap();
    let mut current = store
        .providers()
        .unwrap()
        .into_iter()
        .find(|item| item.id == p.id)
        .unwrap();
    current.name = "Updated complete catalog".into();
    let r = request(&store, Kind::Save, current);
    assert!(serde_json::to_vec(&r).unwrap().len() > 512 * 1024);
    let saved = store.start_provider_operation(&r).unwrap().operation;
    assert!(serde_json::to_vec(&saved).unwrap().len() > 1024 * 1024);
    let committed = saved.provider.as_ref().unwrap();
    assert_eq!(committed.models, p.models);
    assert_eq!(committed.disabled_models, p.disabled_models);
    assert_eq!(committed.revision, 2);
    assert_eq!(
        serde_json::to_value(
            store
                .provider_operation(&r.store_id, &r.operation_id)
                .unwrap()
        )
        .unwrap(),
        serde_json::to_value(saved).unwrap()
    );
}
