ALTER TABLE ingested_records ADD COLUMN fact_id TEXT;
CREATE INDEX IF NOT EXISTS record_fact_reference ON ingested_records(fact_id);
CREATE INDEX IF NOT EXISTS turn_event_time ON turns(event_time);
CREATE INDEX IF NOT EXISTS item_event_time ON items(event_time);
CREATE INDEX IF NOT EXISTS inbox_event_time ON inbox_events(event_time);
PRAGMA user_version=3;
