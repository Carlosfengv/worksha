use super::*;
use local_agent_core::providers::vault::MemoryVault;

#[test]
fn m1_plugin_writes_use_the_control_plane_connection() {
    let root = tempfile::tempdir().unwrap();
    let writer =
        local_agent_core::StoreWriter::new(local_agent_core::Store::open(root.path()).unwrap());
    let manager = Manager::with_control_writer(
        root.path(),
        Arc::new(MemoryVault::default()),
        writer.clone(),
    );
    writer
        .connection()
        .unwrap()
        .execute_batch(
            "CREATE TEMP TRIGGER fixture_shared_writer BEFORE INSERT ON main.ext_host_publishers
         BEGIN SELECT RAISE(ABORT,'control writer fixture'); END;",
        )
        .unwrap();
    assert!(
        manager
            .db
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO ext_host_publishers VALUES('fixture.local','key')",
                [],
            )
            .is_err()
    );
    writer
        .connection()
        .unwrap()
        .execute_batch("DROP TRIGGER fixture_shared_writer")
        .unwrap();
    assert_eq!(
        manager
            .db
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO ext_host_publishers VALUES('fixture.local','key')",
                [],
            )
            .unwrap(),
        1
    );
    assert_eq!(
        writer
            .connection()
            .unwrap()
            .query_row(
                "SELECT public_key FROM ext_host_publishers WHERE id='fixture.local'",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
        "key"
    );
}

#[test]
fn wal_readers_stay_available_during_a_control_plane_write() {
    let root = tempfile::tempdir().unwrap();
    let manager = Manager::open(root.path(), Arc::new(MemoryVault::default())).unwrap();
    let held = Connection::open(root.path().join(store::CONTROL_DATABASE)).unwrap();
    held.execute_batch("BEGIN IMMEDIATE;").unwrap();
    assert!(manager.extension_history().is_ok());
    assert!(manager.list().unwrap().is_empty());
    held.execute_batch("ROLLBACK;").unwrap();
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
    let root = tempfile::tempdir().unwrap();
    let names = [
        "plugin-host.sqlite3",
        "plugin-extensions.sqlite3",
        "plugin-source-host.sqlite3",
        "routing-policy.sqlite3",
        "domain-host.sqlite3",
        "object-contributions.sqlite3",
    ];
    for name in names {
        std::fs::write(root.path().join(name), name).unwrap();
    }
    let manager = Manager::for_service(root.path(), Arc::new(MemoryVault::default()));
    assert!(manager.list().unwrap().is_empty());
    assert!(manager.extension_history().is_ok());
    assert!(manager.full_list().unwrap().is_empty());
    assert!(manager.policy_inventory().unwrap().is_empty());
    assert_eq!(manager.diagnostics(&[])["archivedDatabases"], true);
    drop(manager);
    for name in names {
        assert_eq!(
            std::fs::read(root.path().join(name)).unwrap(),
            name.as_bytes()
        );
    }
}
