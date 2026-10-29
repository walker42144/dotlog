-- Pauses remain gaps in the timeline. A session groups only actual work slices.
CREATE TABLE record_session (
  id TEXT PRIMARY KEY,
  status TEXT NOT NULL CHECK(status IN ('active','paused','finished')),
  anchor_segment_id TEXT NOT NULL REFERENCES time_segment(id),
  publish_ready INTEGER NOT NULL DEFAULT 1 CHECK(publish_ready IN (0,1)),
  updated_at_ms INTEGER NOT NULL
) STRICT;
CREATE TABLE session_segment (
  segment_id TEXT PRIMARY KEY REFERENCES time_segment(id),
  session_id TEXT NOT NULL REFERENCES record_session(id)
) STRICT;
CREATE INDEX ix_session_segment_session ON session_segment(session_id);
INSERT INTO record_session(id,status,anchor_segment_id,updated_at_ms)
SELECT id,CASE WHEN status='active' THEN 'active' ELSE 'finished' END,id,updated_at_ms FROM time_segment;
INSERT INTO session_segment(segment_id,session_id) SELECT id,id FROM time_segment;
UPDATE record_session SET status='paused' WHERE id=(
 SELECT segment_id FROM time_point WHERE undone_at_ms IS NULL
 ORDER BY occurred_at_ms DESC,created_at_ms DESC,id DESC LIMIT 1
) AND (SELECT kind FROM time_point WHERE undone_at_ms IS NULL ORDER BY occurred_at_ms DESC,created_at_ms DESC,id DESC LIMIT 1)='pause'
AND NOT EXISTS(SELECT 1 FROM time_segment WHERE status='active');
