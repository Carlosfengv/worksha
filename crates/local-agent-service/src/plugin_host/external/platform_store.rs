use std::{
    collections::BTreeMap,
    fs::OpenOptions,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::Path,
    time::Duration,
};

use local_agent_core::plugin::platform::*;
use rusqlite::{Connection, OpenFlags, OptionalExtension, TransactionBehavior, params};
use serde_json::Value;

use super::{Checked, Result, fail, package};

const PAGE_SIZE: usize = 32;
const MAX_RECORD_BYTES: usize = 16 * 1024;
const SCHEMA: &str = "
    CREATE TABLE platform_meta (
        id INTEGER PRIMARY KEY CHECK (id = 1),
        schema_version INTEGER NOT NULL CHECK (schema_version = 1),
        main_store_id TEXT NOT NULL
    );
    CREATE TABLE ext_platform_revisions (
        plugin_id TEXT PRIMARY KEY,
        revision INTEGER NOT NULL
    ) WITHOUT ROWID;
    CREATE TABLE ext_entity_grants (
        plugin_id TEXT NOT NULL,
        digest TEXT NOT NULL,
        target_kind TEXT NOT NULL,
        granted_at TEXT NOT NULL,
        host_revision TEXT NOT NULL,
        PRIMARY KEY (plugin_id, target_kind)
    ) WITHOUT ROWID;
    CREATE TABLE ext_data_kv (
        plugin_id TEXT NOT NULL,
        key TEXT NOT NULL,
        value_json TEXT NOT NULL,
        byte_len INTEGER NOT NULL,
        updated_at TEXT NOT NULL,
        PRIMARY KEY (plugin_id, key)
    ) WITHOUT ROWID;
    CREATE TABLE ext_data_records (
        plugin_id TEXT NOT NULL,
        collection TEXT NOT NULL,
        record_id TEXT NOT NULL,
        group_key TEXT,
        sort_key INTEGER NOT NULL DEFAULT 0,
        record_json TEXT NOT NULL,
        byte_len INTEGER NOT NULL,
        created_at TEXT NOT NULL,
        updated_at TEXT NOT NULL,
        PRIMARY KEY (plugin_id, collection, record_id)
    ) WITHOUT ROWID;
    CREATE INDEX idx_ext_records_group
        ON ext_data_records (plugin_id, collection, group_key, sort_key, updated_at DESC);
    CREATE TABLE ext_data_refs (
        plugin_id TEXT NOT NULL,
        collection TEXT NOT NULL,
        record_id TEXT NOT NULL,
        field_name TEXT NOT NULL,
        target_kind TEXT NOT NULL,
        target_id TEXT NOT NULL,
        PRIMARY KEY (plugin_id, collection, record_id, field_name),
        FOREIGN KEY (plugin_id, collection, record_id)
            REFERENCES ext_data_records (plugin_id, collection, record_id) ON DELETE CASCADE
    ) WITHOUT ROWID;
    CREATE INDEX idx_ext_refs_lookup ON ext_data_refs (plugin_id, target_kind, target_id);
    CREATE TABLE ext_source_directory_grants (
        plugin_id TEXT PRIMARY KEY,
        digest TEXT NOT NULL,
        host_revision TEXT NOT NULL,
        dir_path TEXT NOT NULL,
        device INTEGER NOT NULL,
        inode INTEGER NOT NULL
    ) WITHOUT ROWID;
    CREATE TABLE ext_sink_grants (
        plugin_id TEXT PRIMARY KEY,
        digest TEXT NOT NULL,
        host_revision TEXT NOT NULL,
        dir_path TEXT NOT NULL,
        device INTEGER NOT NULL,
        inode INTEGER NOT NULL,
        format TEXT NOT NULL,
        rotate_policy TEXT NOT NULL,
        filter_json TEXT NOT NULL,
        quota_bytes INTEGER NOT NULL DEFAULT 536870912,
        written_bytes INTEGER NOT NULL DEFAULT 0,
        enabled INTEGER NOT NULL CHECK (enabled IN (0, 1)),
        updated_at TEXT NOT NULL
    ) WITHOUT ROWID;
    CREATE TABLE ext_sink_cursors (
        plugin_id TEXT PRIMARY KEY,
        last_timestamp TEXT,
        last_request_id TEXT,
        exported_rows INTEGER NOT NULL DEFAULT 0,
        last_flushed_at TEXT,
        last_error TEXT
    ) WITHOUT ROWID;
    PRAGMA user_version = 1;
";

pub(super) struct PlatformStore(pub(super) Connection);

impl PlatformStore {
    pub(super) fn open(root: &Path, main_store_id: &str) -> Result<Self> {
        let path = root.join("platform.sqlite3");
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(&path)
            .checked("unavailable")?;
        let metadata = file.metadata().checked("unavailable")?;
        if !metadata.is_file()
            || metadata.nlink() != 1
            || metadata.mode() & 0o777 != 0o600
            || metadata.uid() != unsafe { libc::geteuid() }
        {
            return Err(fail("permission_denied"));
        }
        let mut connection = Connection::open_with_flags(
            &path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )
        .checked("unavailable")?;
        connection
            .busy_timeout(Duration::from_millis(250))
            .checked("unavailable")?;
        if Self::validate(&connection, main_store_id)? < 2 {
            let tx = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .checked("unavailable")?;
            if Self::validate(&tx, main_store_id)? == 0 {
                tx.execute_batch(SCHEMA).checked("unavailable")?;
                tx.execute(
                    "INSERT INTO platform_meta VALUES (1, 1, ?1)",
                    [main_store_id],
                )
                .checked("unavailable")?;
            }
            if Self::validate(&tx, main_store_id)? == 1 {
                tx.execute_batch(include_str!("platform_v2.sql"))
                    .checked("unavailable")?;
                Self::validate(&tx, main_store_id)?;
            }
            tx.commit().checked("unavailable")?;
        }
        let journal: String = connection
            .pragma_query_value(None, "journal_mode", |row| row.get(0))
            .checked("unavailable")?;
        if journal != "wal" {
            connection
                .pragma_update(None, "journal_mode", "WAL")
                .checked("unavailable")?;
        }
        connection
            .execute_batch(
                "PRAGMA foreign_keys = ON; PRAGMA synchronous = FULL; PRAGMA trusted_schema = OFF;",
            )
            .checked("unavailable")?;
        Ok(Self(connection))
    }

    fn validate(connection: &Connection, main_store_id: &str) -> Result<u32> {
        let version: u32 = connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .checked("invalid_record")?;
        let valid = match version {
            0 => connection.query_row("SELECT NOT EXISTS (SELECT 1 FROM sqlite_schema)", [], |row| row.get(0)),
            1 | 2 => connection.query_row(
                "SELECT (SELECT COUNT(*) FROM platform_meta) = 1
                    AND EXISTS (SELECT 1 FROM platform_meta WHERE id = 1 AND schema_version = ?2 AND main_store_id = ?1)
                    AND (?2 = 1 OR (SELECT COUNT(*) FROM sqlite_schema WHERE type='table'
                        AND name IN ('ext_action_receipts','ext_sink_progress','ext_sink_pending')) = 3)
                    AND (SELECT COUNT(*) FROM sqlite_schema WHERE type = 'table' AND name IN (
                        'ext_platform_revisions', 'ext_entity_grants', 'ext_data_kv', 'ext_data_records',
                        'ext_data_refs', 'ext_source_directory_grants', 'ext_sink_grants', 'ext_sink_cursors'
                    )) = 8
                    AND (SELECT COUNT(*) FROM sqlite_schema WHERE type = 'index' AND name IN (
                        'idx_ext_records_group', 'idx_ext_refs_lookup'
                    )) = 2",
                params![main_store_id,version], |row| row.get(0),
            ),
            _ => return Err(fail("invalid_record")),
        }.checked("invalid_record")?;
        if valid {
            Ok(version)
        } else {
            Err(fail("invalid_record"))
        }
    }

    pub(super) fn identity(db: &Connection) -> Result<String> {
        db.query_row(
            "SELECT main_store_id FROM platform_meta WHERE id=1",
            [],
            |row| row.get(0),
        )
        .checked("invalid_record")
    }

    pub(super) fn receipt(
        db: &Connection,
        request: &PlatformActionRequest,
        intent: Option<&str>,
        revision: &str,
    ) -> Result<Option<Value>> {
        let Some(intent) = intent else {
            return Ok(None);
        };
        if request.store_id.as_deref() != Some(Self::identity(db)?.as_str()) {
            return Err(fail("stale_snapshot"));
        }
        let row: Option<(String,Option<String>)> = db.query_row(
            "SELECT intent_hash,record_id FROM ext_action_receipts WHERE plugin_id=?1 AND operation_id=?2",
            params![request.plugin_id,request.operation_id], |r|Ok((r.get(0)?,r.get(1)?))).optional().checked("unavailable")?;
        match row {
            Some((hash, id)) if hash == intent => Ok(Some(
                serde_json::json!({"recordId":id,"grantRevision":revision}),
            )),
            Some(_) => Err(fail("operation_conflict")),
            None => Ok(None),
        }
    }

    pub(super) fn usage(db: &Connection, plugin: &str) -> Result<(u64, u64)> {
        let (rows, bytes): (i64, i64) = db.query_row(
            "SELECT SUM(rows), SUM(bytes) FROM (
                SELECT COUNT(*) rows, COALESCE(SUM(byte_len), 0) bytes FROM ext_data_records WHERE plugin_id = ?1
                UNION ALL
                SELECT COUNT(*), COALESCE(SUM(byte_len), 0) FROM ext_data_kv WHERE plugin_id = ?1
            )",
            [plugin],
            |row| Ok((row.get(0)?, row.get(1)?))
        ).checked("invalid_record")?;
        Ok((
            rows.try_into().checked("invalid_record")?,
            bytes.try_into().checked("invalid_record")?,
        ))
    }

    pub(super) fn revision(db: &Connection, plugin: &str, host_revision: u64) -> Result<String> {
        let revision: Option<i64> = db
            .query_row(
                "SELECT revision FROM ext_platform_revisions WHERE plugin_id = ?1",
                [plugin],
                |row| row.get(0),
            )
            .optional()
            .checked("invalid_record")?;
        Ok(format!("{host_revision}.{}", revision.unwrap_or_default()))
    }

    pub(super) fn bump_revision(db: &Connection, plugin: &str) -> Result<()> {
        db.execute(
            "INSERT INTO ext_platform_revisions VALUES (?1, 1)
             ON CONFLICT (plugin_id) DO UPDATE SET revision = revision + 1",
            [plugin],
        )
        .checked("unavailable")?;
        Ok(())
    }

    pub(super) fn entity_grants(
        db: &Connection,
        plugin: &str,
        digest: &str,
        host_revision: u64,
    ) -> Result<Vec<EntityKind>> {
        let mut query = db.prepare(
            "SELECT target_kind FROM ext_entity_grants WHERE plugin_id = ?1 AND digest = ?2 AND host_revision = ?3 ORDER BY target_kind",
        ).checked("invalid_record")?;
        query
            .query_map(params![plugin, digest, host_revision.to_string()], |row| {
                row.get::<_, String>(0)
            })
            .checked("invalid_record")?
            .map(|kind| match kind.checked("invalid_record")?.as_str() {
                "session" => Ok(EntityKind::Session),
                "project" => Ok(EntityKind::Project),
                "agent" => Ok(EntityKind::Agent),
                "provider" => Ok(EntityKind::Provider),
                _ => Err(fail("invalid_record")),
            })
            .collect()
    }

    pub(super) fn records(
        db: &Connection,
        plugin: &str,
        collection: &str,
        cursor: Option<&str>,
        filter: &Fields,
        reference: Option<(&str, EntityKind, &str)>,
    ) -> Result<(Vec<PlatformRecord>, Option<String>)> {
        let order = "ORDER BY r.record_id LIMIT ?8";
        let predicates = "r.plugin_id = ?1 AND r.collection = ?2 AND r.record_id > ?3
                          AND NOT EXISTS (SELECT 1 FROM json_each(?4) f WHERE json_extract(r.record_json, '$.' || f.key) IS NOT f.value)";
        let sql = if reference.is_some() {
            format!(
                "SELECT r.record_id, r.record_json FROM ext_data_refs f
                JOIN ext_data_records r USING (plugin_id, collection, record_id)
                WHERE {predicates} AND f.plugin_id = ?1 AND f.field_name = ?5
                    AND f.target_kind = ?6 AND f.target_id = ?7 {order}"
            )
        } else {
            format!(
                "SELECT r.record_id, r.record_json FROM ext_data_records r WHERE {predicates} {order}"
            )
        };
        let reference = reference.map(|(name, kind, id)| (name, kind.as_str(), id));
        let mut stmt = db.prepare(&sql).checked("invalid_record")?;
        let json = serde_json::to_string(filter).checked("invalid_input")?;
        let mut records = stmt
            .query_map(
                params![
                    plugin,
                    collection,
                    cursor.unwrap_or_default(),
                    json,
                    reference.map(|(name, _, _)| name),
                    reference.map(|(_, kind, _)| kind),
                    reference.map(|(_, _, id)| id),
                    (PAGE_SIZE + 1) as i64,
                ],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .checked("invalid_record")?
            .map(|row| {
                let (record_id, data) = row.checked("invalid_record")?;
                Ok(PlatformRecord {
                    record_id,
                    values: serde_json::from_str(&data).checked("invalid_record")?,
                    hydrated: BTreeMap::new(),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let next = if records.len() > PAGE_SIZE {
            records.pop();
            records.last().map(|record| record.record_id.clone())
        } else {
            None
        };
        Ok((records, next))
    }

    pub(super) fn change_record(
        db: &Connection,
        request: &PlatformActionRequest,
        collection_id: &str,
        collection: &CollectionDef,
    ) -> Result<Option<String>> {
        let plugin = &request.plugin_id;
        let record_id = match (request.action_id, request.record_id.as_deref()) {
            (PlatformAction::Create, None) => uuid::Uuid::new_v4().to_string(),
            (PlatformAction::Create, _) | (_, None) => return Err(fail("invalid_input")),
            (_, Some(id)) if package::local_id(id) => id.to_owned(),
            _ => return Err(fail("invalid_input")),
        };
        if request.action_id == PlatformAction::Delete {
            if !request.values.is_empty() {
                return Err(fail("invalid_input"));
            }
            let changed = db.execute(
                "DELETE FROM ext_data_records WHERE plugin_id = ?1 AND collection = ?2 AND record_id = ?3",
                params![plugin, collection_id, record_id],
            ).checked("unavailable")?;
            return if changed == 1 {
                Ok(None)
            } else {
                Err(fail("not_found"))
            };
        }
        let mut values: Fields = if request.action_id == PlatformAction::Create {
            BTreeMap::new()
        } else {
            let data: String = db.query_row(
                "SELECT record_json FROM ext_data_records WHERE plugin_id = ?1 AND collection = ?2 AND record_id = ?3",
                params![plugin, collection_id, record_id], |row| row.get(0),
            ).checked("not_found")?;
            serde_json::from_str(&data).checked("invalid_record")?
        };
        let group = collection.group_by.as_deref();
        if let Some(expected) = &request.expected_values
            && expected
                .iter()
                .any(|(key, value)| values.get(key).unwrap_or(&Value::Null) != value)
        {
            return Err(fail("record_conflict"));
        }
        if request.action_id == PlatformAction::Move
            && (request.values.len() != 1
                || !group.is_some_and(|key| request.values.contains_key(key)))
        {
            return Err(fail("invalid_input"));
        }
        for (key, value) in &request.values {
            if value.is_null() {
                if !collection.fields.contains_key(key) {
                    return Err(fail("invalid_input"));
                }
                values.remove(key);
            } else {
                values.insert(key.clone(), value.clone());
            }
        }
        validate_values(collection, &values, true)?;
        let data = serde_json::to_string(&values).checked("invalid_input")?;
        if data.len() > MAX_RECORD_BYTES {
            return Err(fail("resource_limit"));
        }
        let group_value = group
            .and_then(|key| values.get(key))
            .and_then(Value::as_str);
        db.execute(
            "INSERT INTO ext_data_records VALUES (
                ?1, ?2, ?3, ?4,
                (SELECT COALESCE(MAX(sort_key), 0) + 1 FROM ext_data_records
                 WHERE plugin_id = ?1 AND collection = ?2 AND group_key IS ?4),
                ?5, ?6, strftime('%Y-%m-%dT%H:%M:%fZ','now'), strftime('%Y-%m-%dT%H:%M:%fZ','now')
             ) ON CONFLICT (plugin_id, collection, record_id) DO UPDATE SET
               group_key = excluded.group_key,
               sort_key = CASE WHEN group_key IS excluded.group_key THEN sort_key ELSE excluded.sort_key END,
               record_json = excluded.record_json, byte_len = excluded.byte_len, updated_at = excluded.updated_at",
            params![plugin, collection_id, record_id, group_value, data, data.len() as i64],
        ).checked("unavailable")?;
        db.execute(
            "DELETE FROM ext_data_refs WHERE plugin_id = ?1 AND collection = ?2 AND record_id = ?3",
            params![plugin, collection_id, record_id],
        )
        .checked("unavailable")?;
        for (field, value) in &values {
            if let Some(target) = collection.fields[field].kind.reference() {
                db.execute(
                    "INSERT INTO ext_data_refs VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![
                        plugin,
                        collection_id,
                        record_id,
                        field,
                        target.as_str(),
                        value.as_str()
                    ],
                )
                .checked("unavailable")?;
            }
        }
        Ok(Some(record_id))
    }

    pub(super) fn set_value(
        db: &Connection,
        plugin: &str,
        request: &PlatformActionRequest,
    ) -> Result<()> {
        let key = request
            .record_id
            .as_deref()
            .filter(|key| package::local_id(key))
            .ok_or(fail("invalid_input"))?;
        let value = request
            .values
            .get("value")
            .filter(|_| request.values.len() == 1 && request.collection.is_none())
            .ok_or(fail("invalid_input"))?;
        let json = serde_json::to_string(value).checked("invalid_input")?;
        if json.len() > MAX_RECORD_BYTES {
            return Err(fail("resource_limit"));
        }
        db.execute(
            "INSERT INTO ext_data_kv VALUES (?1, ?2, ?3, ?4, strftime('%Y-%m-%dT%H:%M:%fZ','now'))
             ON CONFLICT (plugin_id, key) DO UPDATE SET value_json = excluded.value_json,
                 byte_len = excluded.byte_len, updated_at = excluded.updated_at",
            params![plugin, key, json, json.len() as i64],
        )
        .checked("unavailable")?;
        Ok(())
    }
}

pub(super) fn validate_values(
    collection: &CollectionDef,
    values: &Fields,
    required: bool,
) -> Result<()> {
    if required
        && collection
            .fields
            .iter()
            .any(|(name, field)| field.required && !values.contains_key(name))
    {
        return Err(fail("invalid_input"));
    }
    for (key, value) in values {
        let field = collection.fields.get(key).ok_or(fail("invalid_input"))?;
        let text = value.as_str();
        let valid = match field.kind {
            FieldKind::Text => text.is_some_and(|text| {
                text.len() <= 512
                    && !text.chars().any(char::is_control)
                    && (!field.required || !text.trim().is_empty())
            }),
            FieldKind::Textarea => text.is_some_and(|text| {
                text.len() <= 8192 && (!field.required || !text.trim().is_empty())
            }),
            FieldKind::Number => value.is_number(),
            FieldKind::Boolean => value.is_boolean(),
            FieldKind::Enum => {
                text.is_some_and(|text| field.options.iter().any(|option| option == text))
            }
            _ => text.is_some_and(valid_object_id),
        };
        if !valid {
            return Err(fail("invalid_input"));
        }
    }
    Ok(())
}

pub(super) fn valid_object_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 512 && !id.chars().any(char::is_control)
}

#[cfg(test)]
mod repair_tests {
    use super::*;
    #[test]
    fn v1_upgrade_preserves_identity_watermark_and_operation_receipts_across_restart() {
        let root = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
        let path = root.path().join("platform.sqlite3");
        let _file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(&path)
            .unwrap();
        let db = Connection::open(&path).unwrap();
        db.execute_batch(SCHEMA).unwrap();
        db.execute_batch(
            "INSERT INTO platform_meta VALUES(1,1,'owned');
            INSERT INTO ext_sink_cursors VALUES('test','old-time','old-id',8,NULL,NULL);",
        )
        .unwrap();
        assert!(PlatformStore::open(root.path(), "other").is_err());
        assert_eq!(PlatformStore::validate(&db, "owned").unwrap(), 1);
        drop(db);
        let store = PlatformStore::open(root.path(), "owned").unwrap();
        assert_eq!(PlatformStore::validate(&store.0, "owned").unwrap(), 2);
        let cursor:(String,String,i64)=store.0.query_row("SELECT legacy_time,legacy_id,exported_rows FROM ext_sink_progress JOIN ext_sink_cursors USING(plugin_id)",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).unwrap();
        assert_eq!(cursor, ("old-time".into(), "old-id".into(), 8));
        let request:PlatformActionRequest=serde_json::from_value(serde_json::json!({"pluginId":"test","digest":"package","expectedEpoch":"1","expectedGrantRevision":"1.0","actionId":"records.create","operationId":uuid::Uuid::new_v4().to_string(),"storeId":"owned"})).unwrap();
        store
            .0
            .execute(
                "INSERT INTO ext_action_receipts VALUES(?1,?2,'intent','result')",
                params![request.plugin_id, request.operation_id],
            )
            .unwrap();
        drop(store);
        let store = PlatformStore::open(root.path(), "owned").unwrap();
        assert_eq!(
            PlatformStore::receipt(&store.0, &request, Some("intent"), "1.0")
                .unwrap()
                .unwrap()["recordId"],
            "result"
        );
        assert_eq!(
            PlatformStore::receipt(&store.0, &request, Some("other"), "1.0")
                .unwrap_err()
                .code,
            "operation_conflict"
        );
    }
}
