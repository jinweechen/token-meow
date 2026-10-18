CREATE TABLE pet_inbox_feed(
    sequence INTEGER PRIMARY KEY AUTOINCREMENT,
    inbox_id TEXT UNIQUE NOT NULL REFERENCES inbox_events(id) ON DELETE CASCADE
);
INSERT INTO pet_inbox_feed(inbox_id) SELECT id FROM inbox_events ORDER BY rowid;
CREATE TRIGGER pet_inbox_feed_after_insert AFTER INSERT ON inbox_events
BEGIN
    INSERT INTO pet_inbox_feed(inbox_id) VALUES(NEW.id);
END;
PRAGMA user_version=5;
