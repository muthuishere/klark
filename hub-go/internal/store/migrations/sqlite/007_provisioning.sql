-- K3: service tokens for a trusted platform (stored hashed, revocable), the
-- tenants each one created, and one-time pairing codes for those tenants.
CREATE TABLE IF NOT EXISTS service_tokens (
  id         TEXT PRIMARY KEY,
  name       TEXT NOT NULL,
  sha256     TEXT NOT NULL UNIQUE,
  created    INTEGER NOT NULL,
  created_by TEXT NOT NULL DEFAULT '',
  revoked    INTEGER NOT NULL DEFAULT 0,
  expires    INTEGER NOT NULL DEFAULT 0 -- unix seconds; 0 = never
);
ALTER TABLE tenants ADD COLUMN owner_token TEXT NOT NULL DEFAULT '';
CREATE TABLE IF NOT EXISTS pairing_codes (
  code_sha   TEXT PRIMARY KEY,
  tenant     TEXT NOT NULL,
  expires_ms INTEGER NOT NULL,
  used       INTEGER NOT NULL DEFAULT 0,
  created    INTEGER NOT NULL
);
