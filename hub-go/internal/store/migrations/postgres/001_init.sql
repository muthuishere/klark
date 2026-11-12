CREATE TABLE IF NOT EXISTS users (
  name         TEXT PRIMARY KEY,
  display_name TEXT NOT NULL DEFAULT '',
  hash         TEXT NOT NULL,
  admin        INTEGER NOT NULL DEFAULT 0,
  source       TEXT NOT NULL DEFAULT 'local',
  created      BIGINT NOT NULL,
  groups       TEXT NOT NULL DEFAULT ''
);
CREATE UNIQUE INDEX IF NOT EXISTS users_name_lower ON users (lower(name));
CREATE TABLE IF NOT EXISTS machines (
  name         TEXT PRIMARY KEY,
  token_sha256 TEXT NOT NULL,
  fleet        INTEGER NOT NULL DEFAULT 0,
  created      BIGINT NOT NULL,
  created_by   TEXT NOT NULL DEFAULT '',
  last_seen    BIGINT NOT NULL DEFAULT 0,
  version      TEXT NOT NULL DEFAULT '',
  protocol     INTEGER NOT NULL DEFAULT 0
);
CREATE UNIQUE INDEX IF NOT EXISTS machines_name_lower ON machines (lower(name));
CREATE TABLE IF NOT EXISTS messages (
  id      TEXT PRIMARY KEY,
  ts      BIGINT NOT NULL,
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
