ALTER TABLE tasks ADD COLUMN event_count INTEGER NOT NULL DEFAULT 0;
ALTER TABLE tasks ADD COLUMN raw_bytes INTEGER NOT NULL DEFAULT 0;
UPDATE tasks SET event_count=(SELECT count(*) FROM events WHERE task=tasks.id),raw_bytes=(SELECT coalesce(sum(length(CAST(raw AS BLOB))),0) FROM events WHERE task=tasks.id);
CREATE TRIGGER event_counts_ai AFTER INSERT ON events BEGIN UPDATE tasks SET event_count=event_count+1,raw_bytes=raw_bytes+length(CAST(new.raw AS BLOB)) WHERE id=new.task; END;
PRAGMA user_version=2;
