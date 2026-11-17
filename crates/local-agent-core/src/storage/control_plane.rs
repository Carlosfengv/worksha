use rusqlite::Connection;

const SCHEMA: &str = include_str!("control_plane.sql");
const TABLES: &[&str] = &[
    "ext_host_host_version",
    "ext_host_publishers",
    "ext_host_publisher_source_tickets",
    "ext_host_packages",
    "ext_host_selections",
    "ext_host_private_settings",
    "ext_host_provider_presets",
    "ext_host_operations",
    "ext_host_audit",
    "ext_adv_extension_version",
    "ext_adv_resources",
    "ext_adv_protocols",
    "ext_adv_plans",
    "ext_adv_connector_calls",
    "ext_source_full_version",
    "ext_source_full_packages",
    "ext_source_full_selections",
    "ext_source_full_reviews",
    "ext_source_full_projections",
    "ext_source_full_hidden_projects",
    "ext_source_full_saved_sources",
    "ext_domain_domain_version",
    "ext_domain_source_consents",
    "ext_domain_source_projections",
    "ext_policy_policy_meta",
    "ext_policy_policy_packages",
    "ext_policy_policy_selections",
    "ext_policy_policy_reviews",
    "ext_policy_policy_controls",
    "ext_policy_policy_bindings",
    "ext_policy_policy_intents",
    "ext_policy_policy_scope_recoveries",
    "ext_obj_object_grants",
];

pub(super) fn initialize(db: &Connection) -> rusqlite::Result<()> {
    // These namespaces are new at the v18/v19 publication boundary.
    for table in TABLES {
        let occupied: bool = db.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name=?1)",
            [table],
            |row| row.get(0),
        )?;
        if occupied {
            return Err(rusqlite::Error::InvalidQuery);
        }
    }
    db.execute_batch(SCHEMA)?;
    validate(db)
}

pub(super) fn validate(db: &Connection) -> rusqlite::Result<()> {
    for table in TABLES {
        let exists: bool = db.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table' AND name=?1)",
            [table],
            |row| row.get(0),
        )?;
        if !exists {
            return Err(rusqlite::Error::InvalidQuery);
        }
    }
    for (table, column) in [
        ("ext_host_host_version", "version"),
        ("ext_adv_extension_version", "version"),
        ("ext_source_full_version", "version"),
        ("ext_domain_domain_version", "version"),
        ("ext_policy_policy_meta", "format"),
    ] {
        let valid: bool = db.query_row(
            &format!("SELECT count(*)=1 AND min({column})=1 FROM {table}"),
            [],
            |row| row.get(0),
        )?;
        if !valid {
            return Err(rusqlite::Error::InvalidQuery);
        }
    }
    Ok(())
}

#[cfg(test)]
pub(crate) fn remove_from_fixture(db: &Connection) {
    for table in TABLES.iter().rev() {
        db.execute(&format!("DROP TABLE {table}"), []).unwrap();
    }
    db.execute_batch(
        "DROP TABLE gateway_route_change_archive;
         DROP TABLE gateway_dispatch_sequences;
         DROP TABLE gateway_session_watermarks;
         DROP TABLE gateway_route_sequences;
         DROP INDEX service_attempt_turn;",
    )
    .unwrap();
}

#[cfg(test)]
mod tests {
    use crate::Store;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn m1_upgrade_v16_v17_preserves_scope_and_rolls_back_on_schema_conflict() {
        for protected in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let store = Store::open(dir.path()).unwrap();
            let old_version = if protected { 17 } else { 16 };
            let new_version = if protected { 19 } else { 18 };
            let db = &store.connection;
            if protected {
                db.execute_batch(
                    "INSERT INTO iam_accounts VALUES('owner','owner','Owner','fixture','active',1,1);
                     INSERT INTO iam_memberships VALUES('local','owner','owner');
                     UPDATE iam_mode SET enabled=1,recovery_hash='fixture',recovery_owner='owner';",
                ).unwrap();
            }
            let identity = |db: &rusqlite::Connection| {
                db.query_row(
                    "SELECT store_id,namespace_id FROM credential_recovery_scope",
                    [],
                    |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
                )
                .unwrap()
            };
            let before = identity(db);
            super::remove_from_fixture(db);
            db.pragma_update(None, "user_version", old_version).unwrap();
            db.execute_batch("CREATE VIEW gateway_session_watermarks AS SELECT 1")
                .unwrap();
            let old_sidecar = dir.path().join("plugin-extensions.sqlite3");
            std::fs::write(&old_sidecar, b"legacy fixture preserved").unwrap();
            drop(store);

            assert!(Store::open(dir.path()).is_err());
            let db = rusqlite::Connection::open(dir.path().join("local-agent.sqlite3")).unwrap();
            assert_eq!(
                db.pragma_query_value(None, "user_version", |r| r.get::<_, u32>(0))
                    .unwrap(),
                old_version
            );
            assert_eq!(identity(&db), before);
            assert_eq!(
                db.query_row(
                    "SELECT count(*) FROM sqlite_schema WHERE name='ext_host_packages'",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
                0
            );
            db.execute_batch("DROP VIEW gateway_session_watermarks")
                .unwrap();
            drop(db);

            let upgraded = Store::open(dir.path()).unwrap();
            assert_eq!(
                upgraded
                    .connection
                    .pragma_query_value(None, "user_version", |r| r.get::<_, u32>(0))
                    .unwrap(),
                new_version
            );
            assert_eq!(identity(&upgraded.connection), before);
            assert_eq!(upgraded.iam_enabled().unwrap(), protected);
            assert_eq!(
                upgraded.iam_accounts().unwrap().len(),
                usize::from(protected)
            );
            assert_eq!(
                std::fs::read(old_sidecar).unwrap(),
                b"legacy fixture preserved"
            );
            let backup = dir.path().join(format!(
                "local-agent.pre-single-control-v{old_version}.sqlite3"
            ));
            assert_eq!(
                std::fs::metadata(&backup).unwrap().permissions().mode() & 0o777,
                0o600
            );
            let snapshot = rusqlite::Connection::open(backup).unwrap();
            assert_eq!(
                snapshot
                    .pragma_query_value(None, "user_version", |r| r.get::<_, u32>(0))
                    .unwrap(),
                old_version
            );
            assert_eq!(identity(&snapshot), before);
        }
    }
}
