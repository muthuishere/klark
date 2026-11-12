CREATE TABLE IF NOT EXISTS machine_presence (
  machine_id   TEXT PRIMARY KEY,
  pod_id       TEXT NOT NULL,
  pod_url      TEXT NOT NULL,
  session_id   TEXT NOT NULL,
  name         TEXT NOT NULL DEFAULT '',
  version      TEXT NOT NULL DEFAULT '',
  protocol     INTEGER NOT NULL DEFAULT 0,
  heartbeat_ms BIGINT NOT NULL
);
CREATE INDEX IF NOT EXISTS machine_presence_session ON machine_presence (session_id);
