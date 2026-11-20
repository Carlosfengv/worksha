CREATE TABLE observation_export_feed (
    seq INTEGER PRIMARY KEY AUTOINCREMENT,
    request_id TEXT NOT NULL UNIQUE,
    started_at TEXT NOT NULL,
    baseline INTEGER NOT NULL,
    projection TEXT NOT NULL
);

CREATE TABLE observation_export_retention (id INTEGER PRIMARY KEY CHECK(id=1), last_deleted INTEGER NOT NULL);

CREATE VIEW observation_export_projection AS
SELECT id, started_at, json_object(
    'agentId', value -> '$.agentId',
    'model', coalesce(nullif(value -> '$.routedModel','null'), value -> '$.model') -> '$',
    'lan', json_type(value,'$.client') = 'object', 'status', value -> '$.status',
    'httpStatus', value -> '$.httpStatus', 'durationMs', value -> '$.durationMs',
    'inputTokens', value -> '$.usage.inputTokens', 'outputTokens', value -> '$.usage.outputTokens'
) AS projection
FROM model_observations
WHERE CASE WHEN json_valid(value) THEN
    json_type(value,'$.agentId') = 'text' AND json_type(value,'$.status') = 'text'
    AND json_extract(value,'$.status') <> 'in_progress'
    AND coalesce(json_type(value,'$.routing.trace'),'null') = 'null'
END;

CREATE TRIGGER observation_export_insert AFTER INSERT ON model_observations
WHEN NOT EXISTS (SELECT 1 FROM observation_export_feed WHERE request_id=NEW.id)
BEGIN
    INSERT INTO observation_export_feed(request_id,started_at,baseline,projection)
    SELECT id,started_at,0,projection FROM observation_export_projection WHERE id=NEW.id;
END;

CREATE TRIGGER observation_export_update AFTER UPDATE ON model_observations
WHEN NOT EXISTS (SELECT 1 FROM observation_export_feed WHERE request_id=NEW.id)
BEGIN
    INSERT INTO observation_export_feed(request_id,started_at,baseline,projection)
    SELECT id,started_at,0,projection FROM observation_export_projection WHERE id=NEW.id;
END;

CREATE TRIGGER observation_export_delete AFTER DELETE ON model_observations BEGIN
    UPDATE observation_export_retention SET last_deleted=max(last_deleted,
        coalesce((SELECT seq FROM observation_export_feed WHERE request_id=OLD.id),0)) WHERE id=1;
    DELETE FROM observation_export_feed WHERE request_id=OLD.id;
END;
