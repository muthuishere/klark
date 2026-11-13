-- K2: sign-in tickets from a trusted platform are used once; their ids are
-- kept until they expire.
CREATE TABLE IF NOT EXISTS used_tickets (
  jti     TEXT PRIMARY KEY,
  expires INTEGER NOT NULL
);
-- A tenant accepts sign-in tickets from one issuer only: the one bound to the
-- service token that created it.
ALTER TABLE service_tokens ADD COLUMN issuer TEXT NOT NULL DEFAULT '';
ALTER TABLE tenants ADD COLUMN issuer TEXT NOT NULL DEFAULT '';
