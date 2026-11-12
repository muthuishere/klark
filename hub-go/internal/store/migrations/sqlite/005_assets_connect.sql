CREATE TABLE IF NOT EXISTS asset_blobs (
  sha256  TEXT PRIMARY KEY,
  size    INTEGER NOT NULL,
  mime    TEXT NOT NULL,
  data    BLOB NOT NULL,
  created INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS assets (
  id        INTEGER PRIMARY KEY AUTOINCREMENT,
  sha256    TEXT NOT NULL,
  size      INTEGER NOT NULL,
  mime      TEXT NOT NULL,
  name      TEXT NOT NULL,
  ts        INTEGER NOT NULL,
  machine   TEXT NOT NULL,
  pane_id   TEXT NOT NULL,
  direction TEXT NOT NULL,
  by_user   TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS assets_sha ON assets (sha256);
CREATE INDEX IF NOT EXISTS assets_ts ON assets (ts);
CREATE TABLE IF NOT EXISTS connect_requests (
  poll_sha   TEXT PRIMARY KEY,
  code       TEXT NOT NULL UNIQUE,
  machine    TEXT NOT NULL,
  status     TEXT NOT NULL,
  ip         TEXT NOT NULL DEFAULT '',
  created_ms INTEGER NOT NULL,
  expires_ms INTEGER NOT NULL,
  decided_by TEXT NOT NULL DEFAULT ''
);
CREATE INDEX IF NOT EXISTS connect_requests_expires ON connect_requests (expires_ms);
