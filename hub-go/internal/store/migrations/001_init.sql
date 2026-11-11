CREATE TABLE IF NOT EXISTS users (
  name         TEXT PRIMARY KEY COLLATE NOCASE,
  display_name TEXT NOT NULL DEFAULT '',
  hash         TEXT NOT NULL,
  admin        INTEGER NOT NULL DEFAULT 0,
  source       TEXT NOT NULL DEFAULT 'local',
  created      INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS machines (
  name         TEXT PRIMARY KEY COLLATE NOCASE,
  token_sha256 TEXT NOT NULL,
  fleet        INTEGER NOT NULL DEFAULT 0,
  created      INTEGER NOT NULL,
  created_by   TEXT NOT NULL DEFAULT '',
  last_seen    INTEGER NOT NULL DEFAULT 0,
  version      TEXT NOT NULL DEFAULT '',
  protocol     INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS messages (
  id      TEXT PRIMARY KEY,
  ts      INTEGER NOT NULL,
  sender  TEXT NOT NULL,
  from_   TEXT NOT NULL,
  to_     TEXT NOT NULL,
  text    TEXT NOT NULL,
  ok      INTEGER NOT NULL,
  error   TEXT NOT NULL DEFAULT ''
);
CREATE INDEX IF NOT EXISTS messages_ts ON messages (ts DESC);
CREATE TABLE IF NOT EXISTS settings (
  key   TEXT PRIMARY KEY,
  value TEXT NOT NULL
);
