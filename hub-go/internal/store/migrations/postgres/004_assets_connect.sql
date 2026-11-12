CREATE TABLE IF NOT EXISTS asset_blobs (
  sha256  TEXT PRIMARY KEY,
  size    BIGINT NOT NULL,
  mime    TEXT NOT NULL,
  data    BYTEA NOT NULL,
  created BIGINT NOT NULL
);
CREATE TABLE IF NOT EXISTS assets (
  id        BIGSERIAL PRIMARY KEY,
  sha256    TEXT NOT NULL,
  size      BIGINT NOT NULL,
  mime      TEXT NOT NULL,
  name      TEXT NOT NULL,
  ts        BIGINT NOT NULL,
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
  created_ms BIGINT NOT NULL,
  expires_ms BIGINT NOT NULL,
  decided_by TEXT NOT NULL DEFAULT ''
);
CREATE INDEX IF NOT EXISTS connect_requests_expires ON connect_requests (expires_ms);
