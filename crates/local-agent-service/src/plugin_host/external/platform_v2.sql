ALTER TABLE platform_meta RENAME TO platform_meta_v1;
CREATE TABLE platform_meta (
    id INTEGER PRIMARY KEY CHECK (id=1),
    schema_version INTEGER NOT NULL CHECK (schema_version=2),
    main_store_id TEXT NOT NULL
);
INSERT INTO platform_meta SELECT id,2,main_store_id FROM platform_meta_v1;
DROP TABLE platform_meta_v1;
CREATE TABLE ext_sink_progress (
    plugin_id TEXT PRIMARY KEY, epoch TEXT, last_seq INTEGER NOT NULL DEFAULT 0, retention_floor INTEGER NOT NULL DEFAULT 0,
    legacy_time TEXT, legacy_id TEXT
) WITHOUT ROWID;
INSERT INTO ext_sink_progress(plugin_id,legacy_time,legacy_id)
SELECT plugin_id,last_timestamp,last_request_id FROM ext_sink_cursors WHERE length(last_timestamp)>0;
CREATE TABLE ext_sink_pending (
    plugin_id TEXT PRIMARY KEY, revision TEXT NOT NULL, digest TEXT NOT NULL,
    device INTEGER NOT NULL, inode INTEGER NOT NULL, epoch TEXT NOT NULL, start_seq INTEGER NOT NULL,
    end_seq INTEGER NOT NULL, retention_floor INTEGER NOT NULL, end_time TEXT NOT NULL, end_id TEXT NOT NULL, filename TEXT NOT NULL,
    payload BLOB NOT NULL, record_count INTEGER NOT NULL
) WITHOUT ROWID;
CREATE TABLE ext_action_receipts (
    plugin_id TEXT NOT NULL, operation_id TEXT NOT NULL, intent_hash TEXT NOT NULL,
    record_id TEXT, PRIMARY KEY(plugin_id,operation_id)
) WITHOUT ROWID;
PRAGMA user_version=2;
