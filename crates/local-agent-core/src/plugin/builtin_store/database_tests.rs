use super::*;
use rusqlite::Connection;

fn change(store: &Store) -> CompositionChange {
    CompositionChange {
        operation_id: uuid::Uuid::new_v4().to_string(),
        store_id: store.database_id().unwrap(),
        expected_revision: 0u64.into(),
        expected_generation: uuid::Uuid::new_v4().to_string(),
        expected_impact: None,
        composition: Composition {
            profile: Profile::Desktop,
            disabled_plugin_ids: vec!["builtin.projects".into()],
            removed_plugin_ids: vec!["builtin.projects".into()],
        },
    }
}

#[test]
fn initial_state_is_durable_and_only_untouched_stores_follow_the_entry_profile() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().canonicalize().unwrap();
    let store = Store::open(&directory).unwrap();
    assert_eq!(
        store
            .builtin_composition(Profile::Background)
            .unwrap()
            .composition
            .profile,
        Profile::Background
    );
    store
        .connection
        .execute("DELETE FROM builtin_plugin_composition", [])
        .unwrap();
    assert_eq!(
        store
            .builtin_composition(Profile::Desktop)
            .unwrap_err()
            .code,
        "invalid_record"
    );
    drop(store);
    assert!(Store::open(&directory).is_err());
}

#[test]
fn evicted_receipts_remain_unknown_and_old_retries_cannot_reapply_or_revoke_new_grants() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(&root.path().canonicalize().unwrap()).unwrap();
    let first = change(&store);
    store
        .save_builtin_composition(&first, Profile::Desktop, &[])
        .unwrap();
    for revision in 1..=256u64 {
        let mut input = change(&store);
        input.expected_revision = revision.into();
        store
            .save_builtin_composition(&input, Profile::Desktop, &[])
            .unwrap();
    }
    assert_eq!(
        store
            .connection
            .query_row("SELECT count(*) FROM builtin_plugin_operations", [], |r| {
                r.get::<_, i64>(0)
            })
            .unwrap(),
        256
    );
    assert!(
        store
            .builtin_composition_operation(&CompositionOperation {
                store_id: first.store_id.clone(),
                operation_id: first.operation_id.clone()
            })
            .unwrap()
            .is_none()
    );
    store.save_native_source("codex", true).unwrap();
    assert_eq!(
        store
            .save_builtin_composition(&first, Profile::Desktop, &["codex".into()])
            .unwrap_err()
            .code,
        "stale_snapshot"
    );
    assert_eq!(store.native_sources().unwrap(), ["codex"]);
    assert_eq!(
        store
            .builtin_composition(Profile::Desktop)
            .unwrap()
            .revision
            .value(),
        257
    );
}

fn rows(db: &Connection, table: &str) -> Vec<Vec<rusqlite::types::Value>> {
    let mut query = db
        .prepare(&format!("SELECT * FROM {table} ORDER BY rowid"))
        .unwrap();
    let columns = query.column_count();
    query
        .query_map([], |r| (0..columns).map(|i| r.get(i)).collect())
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}

#[test]
fn schema_15_upgrade_preserves_all_existing_rows_and_verified_private_backup() {
    for has_plugin_state in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(&root.path().canonicalize().unwrap()).unwrap();
        store.save_native_source("codex", true).unwrap();
        let input = change(&store);
        if has_plugin_state {
            store
                .save_builtin_composition(&input, Profile::Desktop, &[])
                .unwrap();
        } else {
            store
                .connection
                .execute_batch(
                    "DROP TABLE builtin_plugin_operations; DROP TABLE builtin_plugin_composition;",
                )
                .unwrap();
        }
        let identity = store.database_id().unwrap();
        let mut query = store.connection.prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name").unwrap();
        let names: Vec<String> = query
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        drop(query);
        let before: Vec<_> = names
            .iter()
            .map(|name| rows(&store.connection, name))
            .collect();
        store
            .connection
            .pragma_update(None, "user_version", 15)
            .unwrap();
        drop(store);
        let upgraded = Store::open(&root.path().canonicalize().unwrap()).unwrap();
        assert_eq!(upgraded.database_id().unwrap(), identity);
        for (name, before) in names.iter().zip(&before) {
            assert_eq!(&rows(&upgraded.connection, name), before, "{name}");
        }
        assert_eq!(
            upgraded
                .connection
                .pragma_query_value(None, "user_version", |r| r.get::<_, u32>(0))
                .unwrap(),
            16
        );
        let state = upgraded.builtin_composition(Profile::Desktop).unwrap();
        assert_eq!(state.revision.value(), u64::from(has_plugin_state));
        if has_plugin_state {
            assert_eq!(state.composition, input.composition);
        }
        let backup_path = root
            .path()
            .join("local-agent.pre-plugin-lifecycle-v15.sqlite3");
        let backup =
            Connection::open_with_flags(&backup_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
                .unwrap();
        assert_eq!(
            backup
                .pragma_query_value(None, "user_version", |r| r.get::<_, u32>(0))
                .unwrap(),
            15
        );
        assert_eq!(
            backup
                .pragma_query_value(None, "quick_check", |r| r.get::<_, String>(0))
                .unwrap(),
            "ok"
        );
        for (name, before) in names.iter().zip(&before) {
            assert_eq!(&rows(&backup, name), before);
        }
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(backup_path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}

#[test]
fn current_schema_never_recreates_missing_plugin_tables() {
    for missing in [
        "builtin_plugin_composition",
        "builtin_plugin_operations",
        "builtin_plugin_composition; DROP TABLE builtin_plugin_operations",
    ] {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(&root.path().canonicalize().unwrap()).unwrap();
        store
            .save_builtin_composition(&change(&store), Profile::Desktop, &[])
            .unwrap();
        store
            .connection
            .execute_batch(&format!("DROP TABLE {missing};"))
            .unwrap();
        drop(store);
        assert!(Store::open(&root.path().canonicalize().unwrap()).is_err());
        let db = Connection::open(root.path().join("local-agent.sqlite3")).unwrap();
        assert_eq!(
            db.query_row(
                "SELECT count(*) FROM sqlite_master WHERE name='builtin_plugin_composition'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            i64::from(missing == "builtin_plugin_operations")
        );
    }
}

#[test]
fn lost_composition_is_unknown_instead_of_reenabling_plugins() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(&root.path().canonicalize().unwrap()).unwrap();
    let input = change(&store);
    let receipt = store
        .save_builtin_composition(&input, Profile::Desktop, &[])
        .unwrap();
    store
        .connection
        .execute("DELETE FROM builtin_plugin_composition", [])
        .unwrap();
    assert_eq!(
        store
            .builtin_composition(Profile::Desktop)
            .unwrap_err()
            .code,
        "invalid_record"
    );
    drop(store);
    assert!(Store::open(&root.path().canonicalize().unwrap()).is_err());
    let db = Connection::open(root.path().join("local-agent.sqlite3")).unwrap();
    let preserved: String = db
        .query_row(
            "SELECT receipt FROM builtin_plugin_operations WHERE operation_id=?1",
            [&input.operation_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        serde_json::from_str::<CompositionReceipt>(&preserved).unwrap(),
        receipt
    );
}

#[test]
fn locked_and_full_transactions_preserve_state_consent_and_receipts() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(&root.path().canonicalize().unwrap()).unwrap();
    store.save_native_source("codex", true).unwrap();
    let input = change(&store);
    let other = Connection::open(root.path().join("local-agent.sqlite3")).unwrap();
    store
        .connection
        .busy_timeout(std::time::Duration::from_millis(10))
        .unwrap();
    other.execute_batch("BEGIN IMMEDIATE;").unwrap();
    assert!(
        store
            .save_builtin_composition(&input, Profile::Desktop, &["codex".into()])
            .is_err()
    );
    other.execute_batch("ROLLBACK;").unwrap();
    // Set an actual SQLite page budget, without filling the user's physical disk.
    store.connection.execute_batch("PRAGMA wal_checkpoint(TRUNCATE); PRAGMA page_size; CREATE TABLE owned_fill(value BLOB);").unwrap();
    let pages: i64 = store
        .connection
        .pragma_query_value(None, "page_count", |r| r.get(0))
        .unwrap();
    store
        .connection
        .pragma_update(None, "max_page_count", pages)
        .unwrap();
    // A trigger runs after config and consent updates, forcing SQLITE_FULL at the receipt boundary.
    store.connection.execute_batch("CREATE TEMP TRIGGER fail_receipt BEFORE INSERT ON main.builtin_plugin_operations BEGIN INSERT INTO owned_fill VALUES(zeroblob(1048576)); END;").unwrap();
    assert!(
        store
            .save_builtin_composition(&input, Profile::Desktop, &["codex".into()])
            .is_err()
    );
    assert!(store.connection.is_autocommit());
    assert_eq!(store.native_sources().unwrap(), ["codex"]);
    assert_eq!(
        store
            .builtin_composition(Profile::Desktop)
            .unwrap()
            .revision
            .value(),
        0
    );
    assert!(
        store
            .builtin_composition_operation(&CompositionOperation {
                store_id: input.store_id.clone(),
                operation_id: input.operation_id.clone()
            })
            .unwrap()
            .is_none()
    );
    store
        .connection
        .execute_batch("DROP TRIGGER fail_receipt;")
        .unwrap();
    store
        .connection
        .pragma_update(None, "max_page_count", pages + 1024)
        .unwrap();
    assert_eq!(
        store
            .save_builtin_composition(&input, Profile::Desktop, &["codex".into()])
            .unwrap()
            .revision
            .value(),
        1
    );
}

#[test]
fn failed_migration_cannot_publish_the_new_reader_barrier_or_partial_plugin_tables() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(&root.path().canonicalize().unwrap()).unwrap();
    store.connection.execute_batch("DROP TABLE builtin_plugin_operations; DROP TABLE builtin_plugin_composition; PRAGMA user_version=15; CREATE VIEW builtin_plugin_operations AS SELECT 1;").unwrap();
    drop(store);
    assert!(Store::open(&root.path().canonicalize().unwrap()).is_err());
    let db = Connection::open(root.path().join("local-agent.sqlite3")).unwrap();
    assert_eq!(
        db.pragma_query_value(None, "user_version", |r| r.get::<_, u32>(0))
            .unwrap(),
        15
    );
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM sqlite_master WHERE name='builtin_plugin_composition'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    db.execute_batch("DROP VIEW builtin_plugin_operations;")
        .unwrap();
    drop(db);
    Store::open(&root.path().canonicalize().unwrap()).unwrap();
}
