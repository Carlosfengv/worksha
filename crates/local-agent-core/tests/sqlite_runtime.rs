#[test]
fn bundled_sqlite_contains_the_unix_wal_deadlock_and_reset_fixes() {
    // 3.51.1 could deadlock while closing and opening WAL connections. The
    // standalone deterministic scheduler in scripts/fixtures reproduces it.
    assert_eq!(rusqlite::version(), "3.51.3");
    let db = rusqlite::Connection::open_in_memory().unwrap();
    let source: String = db
        .query_row("SELECT sqlite_source_id()", [], |row| row.get(0))
        .unwrap();
    assert!(source.contains("737ae4a34738ffa0c3ff7f9bb18df914dd1cad163f28fd6b6e114a344fe6d618"));
}
