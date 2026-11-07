use super::*;
use local_agent_core::providers::vault::MemoryVault;

#[test]
fn optional_extension_initialization_retries_after_a_transient_write_lock() {
    let root = tempfile::tempdir().unwrap();
    let first = Manager::for_service(root.path(), Arc::new(MemoryVault::default()));
    first.extension_history().unwrap();
    drop(first);
    let held = Connection::open(root.path().join("plugin-extensions.sqlite3")).unwrap();
    held.execute_batch("BEGIN IMMEDIATE;").unwrap();
    let manager = Manager::for_service(root.path(), Arc::new(MemoryVault::default()));
    assert!(manager.extension_history().is_err());
    held.execute_batch("ROLLBACK;").unwrap();
    assert!(manager.extension_history().is_ok());
    assert!(manager.list().unwrap().is_empty());
}

#[test]
fn plugin_schema_initialization_rolls_back_actual_sqlite_full_and_can_retry() {
    let root = tempfile::tempdir().unwrap();
    let db = Connection::open(root.path().join("owned.sqlite3")).unwrap();
    db.pragma_update(None, "max_page_count", 8).unwrap();
    let failed = store::initialize_database(
        &db,
        "test_version",
        "version",
        &["owned"],
        "CREATE TABLE test_version(version INTEGER); INSERT INTO test_version VALUES(1); CREATE TABLE owned(value BLOB); INSERT INTO owned VALUES(zeroblob(1048576));",
    );
    assert!(failed.is_err());
    assert!(db.is_autocommit());
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM sqlite_master WHERE type='table'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    db.pragma_update(None, "max_page_count", 1024).unwrap();
    store::initialize_database(&db, "test_version", "version", &["owned"],
        "CREATE TABLE test_version(version INTEGER); INSERT INTO test_version VALUES(1); CREATE TABLE owned(value BLOB);").unwrap();
    assert_eq!(
        db.query_row("SELECT version FROM test_version", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
}

#[test]
fn plugin_schema_rejects_incomplete_empty_duplicate_and_future_markers_without_repair() {
    for fixture in [
        "CREATE TABLE test_version(version INTEGER); INSERT INTO test_version VALUES(1);",
        "CREATE TABLE test_version(version INTEGER); CREATE TABLE owned(value BLOB);",
        "CREATE TABLE test_version(version INTEGER); INSERT INTO test_version VALUES(1),(1); CREATE TABLE owned(value BLOB);",
        "CREATE TABLE test_version(version INTEGER); INSERT INTO test_version VALUES(2); CREATE TABLE owned(value BLOB);",
        "CREATE TABLE foreign_data(value TEXT); INSERT INTO foreign_data VALUES('preserve');",
    ] {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("owned.sqlite3");
        let db = Connection::open(&path).unwrap();
        db.execute_batch(fixture).unwrap();
        drop(db);
        let before = std::fs::read(&path).unwrap();
        let db = Connection::open(&path).unwrap();
        assert!(
            store::initialize_database(
                &db,
                "test_version",
                "version",
                &["owned"],
                "CREATE TABLE IF NOT EXISTS owned(value BLOB);"
            )
            .is_err()
        );
        drop(db);
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }
}

#[test]
fn optional_hosts_preserve_unrecognized_databases_and_leave_other_plugins_available() {
    for name in [
        "plugin-host.sqlite3",
        "plugin-extensions.sqlite3",
        "plugin-source-host.sqlite3",
        "routing-policy.sqlite3",
        "domain-host.sqlite3",
    ] {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join(name);
        let db = Connection::open(&path).unwrap();
        db.execute_batch(
            "CREATE TABLE foreign_data(value TEXT); INSERT INTO foreign_data VALUES('preserve');",
        )
        .unwrap();
        drop(db);
        let before = std::fs::read(&path).unwrap();
        let manager = Manager::for_service(root.path(), Arc::new(MemoryVault::default()));
        let error = match name {
            "plugin-host.sqlite3" => manager.list().err(),
            "plugin-extensions.sqlite3" => manager.extension_history().err(),
            "plugin-source-host.sqlite3" => manager.full_list().err(),
            "routing-policy.sqlite3" => manager.policy_inventory().err(),
            "domain-host.sqlite3" => domain_store::Host::new(path.clone())
                .consent("org.example.reader")
                .err(),
            _ => unreachable!(),
        }
        .unwrap();
        if name == "plugin-host.sqlite3" {
            assert!(
                matches!(&manager.db, Database::Unavailable(reason) if reason.code == "invalid_record")
            );
            assert_eq!(error.code, "unavailable");
        } else {
            assert_eq!(error.code, "invalid_record", "{name}");
            assert!(manager.list().unwrap().is_empty());
        }
        drop(manager);
        assert_eq!(std::fs::read(path).unwrap(), before, "{name}");
    }
}
