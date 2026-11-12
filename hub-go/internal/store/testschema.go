package store

import (
	"crypto/rand"
	"database/sql"
	"encoding/hex"
	"net/url"
)

// TestSchema creates a fresh schema in the PostgreSQL at baseURL and returns a URL that uses it, plus a cleanup
// that drops it. It exists for tests (HUB_TEST_POSTGRES) so each test gets an empty database.
func TestSchema(baseURL string) (string, func(), error) {
	raw := make([]byte, 6)
	_, _ = rand.Read(raw)
	schema := "t_" + hex.EncodeToString(raw)
	db, err := sql.Open("pgx", baseURL)
	if err != nil {
		return "", nil, err
	}
	if _, err := db.Exec(`CREATE SCHEMA ` + schema); err != nil {
		_ = db.Close()
		return "", nil, err
	}
	u, err := url.Parse(baseURL)
	if err != nil {
		_ = db.Close()
		return "", nil, err
	}
	q := u.Query()
	q.Set("search_path", schema)
	u.RawQuery = q.Encode()
	return u.String(), func() { _, _ = db.Exec(`DROP SCHEMA ` + schema + ` CASCADE`); _ = db.Close() }, nil
}
