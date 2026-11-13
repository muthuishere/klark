-- K1: every user, machine and message belongs to one tenant. Existing rows go
-- to 'default', so a hub with one tenant works as before.
CREATE TABLE IF NOT EXISTS tenants (
  id      TEXT PRIMARY KEY COLLATE NOCASE,
  name    TEXT NOT NULL DEFAULT '',
  created INTEGER NOT NULL
);
INSERT OR IGNORE INTO tenants (id, name, created) VALUES ('default', 'Default', strftime('%s','now'));
ALTER TABLE users ADD COLUMN tenant TEXT NOT NULL DEFAULT 'default';
ALTER TABLE machines ADD COLUMN tenant TEXT NOT NULL DEFAULT 'default';
ALTER TABLE messages ADD COLUMN tenant TEXT NOT NULL DEFAULT 'default';
CREATE INDEX IF NOT EXISTS messages_tenant_ts ON messages (tenant, ts DESC);
ALTER TABLE connect_requests ADD COLUMN tenant TEXT NOT NULL DEFAULT 'default';
