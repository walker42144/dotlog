-- DotLog schema v1（对应 docs/data-dictionary.md §4）

CREATE TABLE category (
  id             TEXT PRIMARY KEY,
  name           TEXT NOT NULL,
  color          TEXT NOT NULL DEFAULT '#8A8F98',
  parent_id      TEXT NOT NULL DEFAULT '',
  sort_order     INTEGER NOT NULL DEFAULT 0,
  archived_at_ms INTEGER,
  created_at_ms  INTEGER NOT NULL,
  updated_at_ms  INTEGER NOT NULL,
  UNIQUE (parent_id, name)
) STRICT;

CREATE TABLE tag (
  id              TEXT PRIMARY KEY,
  name            TEXT NOT NULL COLLATE NOCASE,
  color           TEXT,
  last_used_at_ms INTEGER,
  created_at_ms   INTEGER NOT NULL,
  updated_at_ms   INTEGER NOT NULL,
  UNIQUE (name)
) STRICT;

CREATE TABLE time_segment (
  id                  TEXT PRIMARY KEY,
  start_at_ms         INTEGER NOT NULL,
  end_at_ms           INTEGER,
  start_tz_offset_min INTEGER NOT NULL,
  end_tz_offset_min   INTEGER,
  title               TEXT    NOT NULL DEFAULT '',
  note                TEXT,
  status              TEXT    NOT NULL DEFAULT 'active'
                      CHECK (status IN ('active','closed','deleted')),
  category_id         TEXT REFERENCES category(id) ON DELETE SET NULL,
  is_focus            INTEGER NOT NULL DEFAULT 0 CHECK (is_focus IN (0,1)),
  is_ignored          INTEGER NOT NULL DEFAULT 0 CHECK (is_ignored IN (0,1)),
  source              TEXT    NOT NULL DEFAULT 'manual'
                      CHECK (source IN ('manual','hotkey','tray','system','plugin','import')),
  revision            INTEGER NOT NULL DEFAULT 1 CHECK (revision >= 1),
  created_at_ms       INTEGER NOT NULL,
  updated_at_ms       INTEGER NOT NULL,
  deleted_at_ms       INTEGER,
  CHECK (
    (status = 'active'  AND end_at_ms IS NULL)
    OR
    (status <> 'active' AND end_at_ms IS NOT NULL AND end_at_ms > start_at_ms)
  ),
  CHECK ((status = 'deleted') = (deleted_at_ms IS NOT NULL))
) STRICT;

CREATE UNIQUE INDEX ux_segment_single_active
  ON time_segment(status) WHERE status = 'active';
CREATE INDEX ix_segment_range  ON time_segment(start_at_ms, end_at_ms);
CREATE INDEX ix_segment_status ON time_segment(status, start_at_ms);
CREATE INDEX ix_segment_title  ON time_segment(title);
CREATE INDEX ix_segment_cat    ON time_segment(category_id);

CREATE TABLE time_point (
  id              TEXT PRIMARY KEY,
  occurred_at_ms  INTEGER NOT NULL,
  tz_offset_min   INTEGER NOT NULL,
  kind            TEXT NOT NULL
                  CHECK (kind IN ('start','punch','pause','resume','finish','undo','edit','backfill','delete')),
  source          TEXT NOT NULL
                  CHECK (source IN ('hotkey','button','tray','plugin','import','system')),
  segment_id      TEXT REFERENCES time_segment(id) ON DELETE SET NULL,
  payload         TEXT,
  undone_at_ms    INTEGER,
  created_at_ms   INTEGER NOT NULL
) STRICT;

CREATE INDEX ix_point_occurred ON time_point(occurred_at_ms);
CREATE INDEX ix_point_segment  ON time_point(segment_id);

CREATE TABLE segment_tag (
  segment_id    TEXT NOT NULL REFERENCES time_segment(id) ON DELETE CASCADE,
  tag_id        TEXT NOT NULL REFERENCES tag(id) ON DELETE CASCADE,
  created_at_ms INTEGER NOT NULL,
  PRIMARY KEY (segment_id, tag_id)
) STRICT;

CREATE TABLE integration_link (
  segment_id        TEXT NOT NULL REFERENCES time_segment(id) ON DELETE CASCADE,
  provider          TEXT NOT NULL,
  external_id       TEXT,
  state             TEXT NOT NULL
                    CHECK (state IN ('pending','published','failed','removed')),
  last_synced_at_ms INTEGER,
  last_error        TEXT,
  PRIMARY KEY (segment_id, provider),
  UNIQUE (provider, external_id)
) STRICT;

CREATE TABLE outbox_event (
  id              TEXT PRIMARY KEY,
  dedupe_key      TEXT UNIQUE,
  type            TEXT NOT NULL,
  payload         TEXT NOT NULL,
  state           TEXT NOT NULL DEFAULT 'pending'
                  CHECK (state IN ('pending','inflight','done','failed')),
  attempts        INTEGER NOT NULL DEFAULT 0,
  next_retry_at_ms INTEGER,
  last_error      TEXT,
  created_at_ms   INTEGER NOT NULL,
  updated_at_ms   INTEGER NOT NULL
) STRICT;

CREATE INDEX ix_outbox_due ON outbox_event(state, next_retry_at_ms);

CREATE TABLE setting (
  scope         TEXT NOT NULL,
  key           TEXT NOT NULL,
  value         TEXT NOT NULL,
  updated_at_ms INTEGER NOT NULL,
  PRIMARY KEY (scope, key)
) STRICT;

CREATE TABLE runtime_state (
  key           TEXT PRIMARY KEY,
  value         TEXT NOT NULL,
  updated_at_ms INTEGER NOT NULL
) STRICT;
