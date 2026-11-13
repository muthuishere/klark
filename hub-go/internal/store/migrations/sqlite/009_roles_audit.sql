-- Access roles for hub accounts, and a record of actions taken through the hub.
ALTER TABLE users ADD COLUMN role TEXT NOT NULL DEFAULT 'member';
CREATE TABLE IF NOT EXISTS audit (
  id      INTEGER PRIMARY KEY AUTOINCREMENT,
  ts      INTEGER NOT NULL,
  tenant  TEXT NOT NULL,
  actor   TEXT NOT NULL,
  machine TEXT NOT NULL DEFAULT '',
  target  TEXT NOT NULL DEFAULT '',
  action  TEXT NOT NULL,
  status  INTEGER NOT NULL,
  bytes   INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS audit_tenant_ts ON audit (tenant, ts DESC);
