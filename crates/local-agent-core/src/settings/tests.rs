use super::*;

fn update(store: &Store, revision: u64, days: u16) -> SettingsUpdate {
    SettingsUpdate {
        store_id: Some(store.database_id().unwrap()),
        value: Settings {
            capture_content: false,
            retention_days: days,
        },
        expected_revision: revision,
        operation_id: uuid::Uuid::new_v4().to_string(),
    }
}

#[test]
fn stale_writes_are_rejected_and_retries_return_the_original_committed_result() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    let mut first = update(&store, 0, 7);
    first.value.capture_content = true;
    let saved = store.save_settings(&first).unwrap();
    assert_eq!(saved.revision, 1);
    assert_eq!(
        store
            .save_settings(&update(&store, 0, 90))
            .unwrap_err()
            .code,
        "conflict"
    );
    assert_eq!(store.settings().unwrap(), first.value);
    let second = store.save_settings(&update(&store, 1, 30)).unwrap();
    assert_eq!(store.save_settings(&first).unwrap(), saved);
    assert_eq!(store.settings_state().unwrap().revision, second.revision);
    assert_eq!(store.settings().unwrap(), second.value);
    drop(store);
    let store = Store::open(directory.path()).unwrap();
    assert_eq!(
        store.settings_operation(&first.operation_id).unwrap(),
        saved
    );
    assert_eq!(store.save_settings(&first).unwrap(), saved);
    first.value.retention_days = 90;
    assert_eq!(
        store.save_settings(&first).unwrap_err().code,
        "operation_conflict"
    );
}

#[test]
fn independent_connections_cannot_both_commit_the_same_base_revision() {
    let directory = tempfile::tempdir().unwrap();
    let first = Store::open(directory.path()).unwrap();
    let second = Store::open(directory.path()).unwrap();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let threads = [first, second]
        .into_iter()
        .map(|store| {
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                store.save_settings(&update(&store, 0, 7))
            })
        })
        .collect::<Vec<_>>();
    let results = threads
        .into_iter()
        .map(|t| t.join().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter_map(|r| r.as_ref().err())
            .next()
            .unwrap()
            .code,
        "conflict"
    );
    assert_eq!(
        Store::open(directory.path())
            .unwrap()
            .settings_state()
            .unwrap()
            .revision,
        1
    );
}

#[test]
fn settings_and_operation_result_roll_back_together_on_storage_failure() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    store.connection.execute_batch("CREATE TRIGGER fail_settings_receipt BEFORE INSERT ON settings_operations BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
    let request = update(&store, 0, 7);
    assert_eq!(
        store.save_settings(&request).unwrap_err().code,
        "unavailable"
    );
    assert_eq!(
        store.settings_state().unwrap(),
        SettingsState {
            value: Settings::default(),
            revision: 0
        }
    );
    assert_eq!(
        store
            .settings_operation(&request.operation_id)
            .unwrap_err()
            .code,
        "operation_not_found"
    );
    store
        .connection
        .execute_batch("DROP TRIGGER fail_settings_receipt;")
        .unwrap();
    assert_eq!(store.save_settings(&request).unwrap().revision, 1);
}

#[test]
fn migration_preserves_v6_settings_and_keeps_a_consistent_private_backup() {
    let directory = tempfile::tempdir().unwrap();
    let canonical = directory.path().canonicalize().unwrap();
    let database = canonical.join("local-agent.sqlite3");
    let connection = rusqlite::Connection::open(&database).unwrap();
    connection.execute_batch("PRAGMA user_version=6;CREATE TABLE app_settings(id INTEGER PRIMARY KEY,value TEXT NOT NULL);INSERT INTO app_settings VALUES(1,'{\"captureContent\":true,\"retentionDays\":90}');").unwrap();
    drop(connection);
    let store = Store::open(&canonical).unwrap();
    assert_eq!(
        store.settings_state().unwrap(),
        SettingsState {
            value: Settings {
                capture_content: true,
                retention_days: 90
            },
            revision: 0
        }
    );
    let backup =
        rusqlite::Connection::open(directory.path().join("local-agent.pre-settings-v6.sqlite3"))
            .unwrap();
    assert_eq!(
        backup
            .query_row("PRAGMA user_version", [], |r| r.get::<_, u32>(0))
            .unwrap(),
        6
    );
    assert_eq!(
        backup
            .query_row("SELECT value FROM app_settings", [], |r| r
                .get::<_, String>(0))
            .unwrap(),
        "{\"captureContent\":true,\"retentionDays\":90}"
    );
    assert_eq!(
        store
            .connection
            .query_row("PRAGMA user_version", [], |r| r.get::<_, u32>(0))
            .unwrap(),
        Store::SCHEMA_VERSION
    );
    let scope = store.model_service_operation_store_id().unwrap();
    assert_eq!(
        store
            .model_service_operation(&uuid::Uuid::new_v4().to_string(), &scope)
            .unwrap_err()
            .code,
        "operation_not_found"
    );
    let request = SettingsUpdate {
        store_id: Some(scope.clone()),
        value: store.settings_state().unwrap().value,
        expected_revision: 0,
        operation_id: uuid::Uuid::new_v4().to_string(),
    };
    let saved = store.save_settings(&request).unwrap();
    drop(store);
    let reopened = Store::open(&canonical).unwrap();
    assert_eq!(reopened.model_service_operation_store_id().unwrap(), scope);
    assert_eq!(
        reopened.settings_operation(&request.operation_id).unwrap(),
        saved
    );
    assert_eq!(reopened.settings_state().unwrap().value, request.value);
    assert!(
        !canonical
            .join("local-agent.pre-model-service-operations-v7.sqlite3")
            .exists()
    );
}

#[test]
fn receipt_history_is_bounded_and_expired_operations_cannot_be_replayed() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    let first = update(&store, 0, 7);
    store.save_settings(&first).unwrap();
    for revision in 1..260 {
        store.save_settings(&update(&store, revision, 30)).unwrap();
    }
    assert_eq!(
        store
            .connection
            .query_row("SELECT count(*) FROM settings_operations", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        256
    );
    assert_eq!(
        store
            .settings_operation(&first.operation_id)
            .unwrap_err()
            .code,
        "operation_not_found"
    );
    assert_eq!(store.save_settings(&first).unwrap_err().code, "conflict");
    assert_eq!(store.settings_state().unwrap().revision, 260);
}

#[test]
fn database_identity_is_checked_before_save_or_receipt_lookup() {
    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    let first = Store::open(a.path()).unwrap();
    let second = Store::open(b.path()).unwrap();
    let request = update(&first, 0, 7);
    let saved = first.save_settings(&request).unwrap();
    let scope = request.store_id.as_deref().unwrap();
    assert_eq!(second.settings_state().unwrap().revision, 0);
    assert_eq!(
        second.save_settings(&request).unwrap_err().code,
        "operation_store_changed"
    );
    assert_eq!(
        second
            .scoped_settings_operation(&request.operation_id, scope)
            .unwrap_err()
            .code,
        "operation_store_changed"
    );
    assert_eq!(
        second
            .connection
            .query_row("SELECT count(*) FROM settings_operations", [], |r| r
                .get::<_, u32>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        first
            .scoped_settings_operation(&request.operation_id, scope)
            .unwrap(),
        saved
    );
    first.save_settings(&update(&first, 1, 90)).unwrap();
    assert_eq!(
        first
            .scoped_settings_operation(&request.operation_id, scope)
            .unwrap(),
        saved
    );
    assert_eq!(
        first.settings_write_protocol().unwrap().store_id,
        first.model_service_operation_store_id().unwrap()
    );
}

#[test]
fn unscoped_writes_are_rejected_and_historical_receipts_are_not_relabelled() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path()).unwrap();
    let mut request = update(&store, 0, 7);
    request.store_id = None;
    assert_eq!(
        store.save_settings(&request).unwrap_err().code,
        "write_protocol_unavailable"
    );
    assert_eq!(store.settings_state().unwrap().revision, 0);
    let receipt = SettingsSaved {
        store_id: None,
        value: request.value.clone(),
        revision: 1,
        operation_id: request.operation_id.clone(),
    };
    let before = (
        serde_json::to_string(&request).unwrap(),
        serde_json::to_string(&receipt).unwrap(),
    );
    store
        .connection
        .execute(
            "INSERT INTO settings_operations(operation_id,request,result) VALUES(?1,?2,?3)",
            params![request.operation_id, before.0, before.1],
        )
        .unwrap();
    assert_eq!(
        store.settings_operation(&request.operation_id).unwrap(),
        receipt
    );
    let scope = store.database_id().unwrap();
    assert_eq!(
        store
            .scoped_settings_operation(&request.operation_id, &scope)
            .unwrap_err()
            .code,
        "operation_not_scoped"
    );
    request.store_id = Some(scope);
    assert_eq!(
        store.save_settings(&request).unwrap_err().code,
        "operation_conflict"
    );
    assert_eq!(
        store
            .connection
            .query_row(
                "SELECT request,result FROM settings_operations WHERE operation_id=?1",
                [&request.operation_id],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            )
            .unwrap(),
        before
    );
}

#[test]
fn malformed_or_mismatched_settings_receipts_are_preserved_without_replay() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path()).unwrap();
    let request = update(&store, 0, 7);
    let original = store.save_settings(&request).unwrap();
    for patch in [
        serde_json::json!({"revision":2}),
        serde_json::json!({"operationId":uuid::Uuid::new_v4().to_string()}),
        serde_json::json!({"storeId":uuid::Uuid::new_v4().to_string()}),
        serde_json::json!({"storeId":null}),
        serde_json::json!({"value":{"captureContent":false,"retentionDays":90}}),
    ] {
        let mut result = serde_json::to_value(&original).unwrap();
        result
            .as_object_mut()
            .unwrap()
            .extend(patch.as_object().unwrap().clone());
        let bytes = serde_json::to_string(&result).unwrap();
        store
            .connection
            .execute(
                "UPDATE settings_operations SET result=?1 WHERE operation_id=?2",
                params![bytes, request.operation_id],
            )
            .unwrap();
        assert_eq!(
            store
                .scoped_settings_operation(
                    &request.operation_id,
                    request.store_id.as_deref().unwrap()
                )
                .unwrap_err()
                .code,
            "invalid_record"
        );
        assert_eq!(
            store.save_settings(&request).unwrap_err().code,
            "invalid_record"
        );
        assert_eq!(
            store
                .connection
                .query_row(
                    "SELECT result FROM settings_operations WHERE operation_id=?1",
                    [&request.operation_id],
                    |r| r.get::<_, String>(0)
                )
                .unwrap(),
            bytes
        );
        assert_eq!(store.settings_state().unwrap().revision, 1);
    }
}
