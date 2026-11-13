-- K2: sign-in tickets from a trusted platform are used once; their ids are
-- kept until they expire.
CREATE TABLE IF NOT EXISTS used_tickets (
  jti     TEXT PRIMARY KEY,
  expires INTEGER NOT NULL
);
