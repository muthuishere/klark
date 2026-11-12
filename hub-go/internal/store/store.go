// Package store is the hub's SQLite database (pure Go, no cgo): users, machines, messages, settings.
// Machine tokens are stored only as their SHA-256.
package store

import (
	"context"
	"database/sql"
	"embed"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strconv"
	"strings"
	"time"

	_ "github.com/jackc/pgx/v5/stdlib"
	_ "modernc.org/sqlite"
)

//go:embed migrations
var migrations embed.FS

// ErrNotFound is returned when a row does not exist.
var ErrNotFound = errors.New("not found")

// Store is the hub's database: SQLite (hub.db, the default) or PostgreSQL (several active pods). The queries are
// written once in the SQLite dialect with `?` placeholders; the Postgres connection rewrites the few differences
// (placeholders, INSERT OR IGNORE, the case-insensitive name key, rowid) in dbx.
type Store struct {
	db *dbx
	pg bool
}

// dbx is a *sql.DB that rewrites queries for PostgreSQL.
type dbx struct {
	*sql.DB
	pg bool
}

func (d *dbx) rewrite(q string) string {
	if !d.pg {
		return q
	}
	if strings.HasPrefix(q, "INSERT OR IGNORE INTO") {
		q = "INSERT INTO" + strings.TrimPrefix(q, "INSERT OR IGNORE INTO") + " ON CONFLICT DO NOTHING"
	}
	q = strings.Replace(q, "ON CONFLICT(name)", "ON CONFLICT ((lower(name)))", 1)
	q = strings.Replace(q, ", rowid DESC", ", id DESC", 1)
	var b strings.Builder
	n := 0
	for i := 0; i < len(q); i++ {
		if q[i] == '?' {
			n++
			b.WriteString("$" + strconv.Itoa(n))
		} else {
			b.WriteByte(q[i])
		}
	}
	return b.String()
}

func (d *dbx) Exec(q string, args ...any) (sql.Result, error) {
	return d.DB.Exec(d.rewrite(q), args...)
}
func (d *dbx) Query(q string, args ...any) (*sql.Rows, error) {
	return d.DB.Query(d.rewrite(q), args...)
}
func (d *dbx) QueryRow(q string, args ...any) *sql.Row { return d.DB.QueryRow(d.rewrite(q), args...) }

// Open opens (creating) dir/hub.db and applies the embedded migrations. Safe to call on every start.
func Open(dir string) (*Store, error) {
	if err := os.MkdirAll(dir, 0o750); err != nil {
		return nil, err
	}
	return OpenPath(filepath.Join(dir, "hub.db"))
}

// OpenPath opens a database file ("file::memory:?cache=shared" style DSNs are not used; pass a path).
func OpenPath(path string) (*Store, error) {
	dsn := "file:" + filepath.ToSlash(path) + "?_pragma=busy_timeout(5000)&_pragma=journal_mode(WAL)&_pragma=foreign_keys(1)"
	db, err := sql.Open("sqlite", dsn)
	if err != nil {
		return nil, err
	}
	db.SetMaxOpenConns(4)
	s := &Store{db: &dbx{DB: db}}
	if err := s.migrate(); err != nil {
		_ = db.Close()
		return nil, err
	}
	return s, nil
}

// OpenPostgres connects to PostgreSQL (url is a postgres:// URL; it is never logged) and applies the migrations
// under an advisory lock, so pods starting together do not race.
func OpenPostgres(url string) (*Store, error) {
	db, err := sql.Open("pgx", url)
	if err != nil {
		return nil, errors.New("postgres: invalid HUB_DATABASE_URL")
	}
	db.SetMaxOpenConns(10)
	db.SetConnMaxIdleTime(5 * time.Minute)
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()
	if err := db.PingContext(ctx); err != nil {
		_ = db.Close()
		return nil, fmt.Errorf("postgres: cannot connect: %s", scrubURL(err.Error(), url))
	}
	s := &Store{db: &dbx{DB: db, pg: true}, pg: true}
	if err := s.migrate(); err != nil {
		_ = db.Close()
		return nil, err
	}
	return s, nil
}

func scrubURL(msg, url string) string { return strings.ReplaceAll(msg, url, "***") }

// IsPostgres reports the backend.
func (s *Store) IsPostgres() bool { return s.pg }

// Snapshot writes a consistent copy of a SQLite database to path (VACUUM INTO; the file must not exist).
func (s *Store) Snapshot(path string) error {
	if s.pg {
		return errors.New("snapshots are for SQLite")
	}
	_, err := s.db.DB.Exec(`VACUUM INTO ?`, path)
	return err
}

func (s *Store) Close() error { return s.db.DB.Close() }

// migrate applies migrations/<dialect>/NNN_*.sql once each, in order; re-running is a no-op.
func (s *Store) migrate() error {
	dir := "migrations/sqlite"
	if s.pg {
		dir = "migrations/postgres"
	}
	ctx := context.Background()
	conn, err := s.db.DB.Conn(ctx)
	if err != nil {
		return err
	}
	defer conn.Close()
	if s.pg {
		if _, err := conn.ExecContext(ctx, `SELECT pg_advisory_lock(727274)`); err != nil {
			return err
		}
		defer conn.ExecContext(ctx, `SELECT pg_advisory_unlock(727274)`) //nolint:errcheck
	}
	if _, err := conn.ExecContext(ctx, `CREATE TABLE IF NOT EXISTS schema_migrations (name TEXT PRIMARY KEY, applied BIGINT NOT NULL)`); err != nil {
		return err
	}
	entries, err := migrations.ReadDir(dir)
	if err != nil {
		return err
	}
	names := make([]string, 0, len(entries))
	for _, e := range entries {
		names = append(names, e.Name())
	}
	sort.Strings(names)
	ph := "?"
	if s.pg {
		ph = "$1"
	}
	for _, n := range names {
		var done int
		if err := conn.QueryRowContext(ctx, `SELECT COUNT(*) FROM schema_migrations WHERE name = `+ph, n).Scan(&done); err != nil {
			return err
		}
		if done > 0 {
			continue
		}
		body, _ := migrations.ReadFile(dir + "/" + n)
		tx, err := conn.BeginTx(ctx, nil)
		if err != nil {
			return err
		}
		if _, err := tx.ExecContext(ctx, string(body)); err != nil {
			_ = tx.Rollback()
			return fmt.Errorf("migration %s: %w", n, err)
		}
		ins := `INSERT INTO schema_migrations (name, applied) VALUES (?, ?)`
		if s.pg {
			ins = `INSERT INTO schema_migrations (name, applied) VALUES ($1, $2)`
		}
		if _, err := tx.ExecContext(ctx, ins, n, time.Now().Unix()); err != nil {
			_ = tx.Rollback()
			return err
		}
		if err := tx.Commit(); err != nil {
			return err
		}
	}
	return nil
}

// ---- users -------------------------------------------------------------------

type User struct {
	Name        string `json:"username"`
	DisplayName string `json:"displayName"`
	Hash        string `json:"-"`
	Admin       bool   `json:"admin"`
	Source      string `json:"source"` // builtin, env, local, mockldap
	Created     int64  `json:"created"`
	// Groups are the directory groups of a mockldap user ("|" is not allowed in a name).
	Groups []string `json:"groups"`
}

func joinGroups(g []string) string { return strings.Join(g, "|") }

func splitGroups(s string) []string {
	out := []string{}
	for _, g := range strings.Split(s, "|") {
		if g = strings.TrimSpace(g); g != "" {
			out = append(out, g)
		}
	}
	return out
}

// UpsertUser inserts a user or replaces its hash, display name, role and source.
func (s *Store) UpsertUser(u User) error {
	if u.DisplayName == "" {
		u.DisplayName = u.Name
	}
	_, err := s.db.Exec(`INSERT INTO users (name, display_name, hash, admin, source, created, groups) VALUES (?,?,?,?,?,?,?)
		ON CONFLICT(name) DO UPDATE SET display_name=excluded.display_name, hash=excluded.hash, admin=excluded.admin, source=excluded.source, groups=excluded.groups`,
		u.Name, u.DisplayName, u.Hash, b2i(u.Admin), u.Source, time.Now().Unix(), joinGroups(u.Groups))
	return err
}

// InsertUser adds a user; false when the name is taken.
func (s *Store) InsertUser(u User) (bool, error) {
	if u.DisplayName == "" {
		u.DisplayName = u.Name
	}
	r, err := s.db.Exec(`INSERT OR IGNORE INTO users (name, display_name, hash, admin, source, created, groups) VALUES (?,?,?,?,?,?,?)`,
		u.Name, u.DisplayName, u.Hash, b2i(u.Admin), u.Source, time.Now().Unix(), joinGroups(u.Groups))
	if err != nil {
		return false, err
	}
	n, _ := r.RowsAffected()
	return n > 0, nil
}

func scanUser(sc interface{ Scan(...any) error }) (User, error) {
	var u User
	var admin int
	var groups string
	err := sc.Scan(&u.Name, &u.DisplayName, &u.Hash, &admin, &u.Source, &u.Created, &groups)
	u.Admin = admin != 0
	u.Groups = splitGroups(groups)
	return u, err
}

const userCols = `name, display_name, hash, admin, source, created, groups`

func (s *Store) User(name string) (User, error) {
	u, err := scanUser(s.db.QueryRow(`SELECT `+userCols+` FROM users WHERE lower(name) = lower(?)`, name))
	if errors.Is(err, sql.ErrNoRows) {
		return u, ErrNotFound
	}
	return u, err
}

func (s *Store) Users() ([]User, error) {
	rows, err := s.db.Query(`SELECT ` + userCols + ` FROM users ORDER BY name`)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var out []User
	for rows.Next() {
		u, err := scanUser(rows)
		if err != nil {
			return nil, err
		}
		out = append(out, u)
	}
	return out, rows.Err()
}

func (s *Store) DeleteUser(name string) error {
	r, err := s.db.Exec(`DELETE FROM users WHERE lower(name) = lower(?)`, name)
	if err != nil {
		return err
	}
	if n, _ := r.RowsAffected(); n == 0 {
		return ErrNotFound
	}
	return nil
}

func (s *Store) SetPassword(name, hash string) error {
	r, err := s.db.Exec(`UPDATE users SET hash = ? WHERE lower(name) = lower(?)`, hash, name)
	if err != nil {
		return err
	}
	if n, _ := r.RowsAffected(); n == 0 {
		return ErrNotFound
	}
	return nil
}

// ---- machines ----------------------------------------------------------------

type Machine struct {
	Name      string `json:"name"`
	SHA256    string `json:"-"`
	Fleet     bool   `json:"fleet"`
	Created   int64  `json:"created"`
	CreatedBy string `json:"createdBy"`
	LastSeen  int64  `json:"lastSeen"`
	Version   string `json:"version"`
	Protocol  int    `json:"protocol"`
}

// AddMachine stores a machine token's hash; false when the name is taken.
func (s *Store) AddMachine(m Machine) (bool, error) {
	r, err := s.db.Exec(`INSERT OR IGNORE INTO machines (name, token_sha256, fleet, created, created_by) VALUES (?,?,?,?,?)`,
		m.Name, strings.ToLower(m.SHA256), b2i(m.Fleet), time.Now().Unix(), m.CreatedBy)
	if err != nil {
		return false, err
	}
	n, _ := r.RowsAffected()
	return n > 0, nil
}

func (s *Store) Machines() ([]Machine, error) {
	rows, err := s.db.Query(`SELECT name, token_sha256, fleet, created, created_by, last_seen, version, protocol FROM machines ORDER BY name`)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var out []Machine
	for rows.Next() {
		var m Machine
		var fleet int
		if err := rows.Scan(&m.Name, &m.SHA256, &fleet, &m.Created, &m.CreatedBy, &m.LastSeen, &m.Version, &m.Protocol); err != nil {
			return nil, err
		}
		m.Fleet = fleet != 0
		out = append(out, m)
	}
	return out, rows.Err()
}

func (s *Store) DeleteMachine(name string) error {
	r, err := s.db.Exec(`DELETE FROM machines WHERE lower(name) = lower(?)`, name)
	if err != nil {
		return err
	}
	if n, _ := r.RowsAffected(); n == 0 {
		return ErrNotFound
	}
	return nil
}

// TouchMachine records that a machine (stored or static) was seen, for the machine list after a restart.
func (s *Store) TouchMachine(name, version string, protocol int) {
	_, _ = s.db.Exec(`UPDATE machines SET last_seen = ?, version = ?, protocol = ? WHERE lower(name) = lower(?)`, time.Now().Unix(), version, protocol, name)
}

// ---- messages ----------------------------------------------------------------

type Message struct {
	ID     string `json:"id"`
	Time   int64  `json:"time"` // unix seconds
	Sender string `json:"sender"`
	From   string `json:"from"`
	To     string `json:"to"`
	Text   string `json:"text"`
	OK     bool   `json:"ok"`
	Error  string `json:"error,omitempty"`
}

func (s *Store) RecordMessage(m Message) error {
	_, err := s.db.Exec(`INSERT INTO messages (id, ts, sender, from_, to_, text, ok, error) VALUES (?,?,?,?,?,?,?,?)`,
		m.ID, m.Time, m.Sender, m.From, m.To, m.Text, b2i(m.OK), m.Error)
	return err
}

func (s *Store) RecentMessages(limit int) ([]Message, error) {
	rows, err := s.db.Query(`SELECT id, ts, sender, from_, to_, text, ok, error FROM messages ORDER BY ts DESC, rowid DESC LIMIT ?`, limit)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var out []Message
	for rows.Next() {
		var m Message
		var ok int
		if err := rows.Scan(&m.ID, &m.Time, &m.Sender, &m.From, &m.To, &m.Text, &ok, &m.Error); err != nil {
			return nil, err
		}
		m.OK = ok != 0
		out = append(out, m)
	}
	return out, rows.Err()
}

// ---- settings ----------------------------------------------------------------

func (s *Store) Setting(key string) (string, bool) {
	var v string
	if err := s.db.QueryRow(`SELECT value FROM settings WHERE key = ?`, key).Scan(&v); err != nil {
		return "", false
	}
	return v, true
}

func (s *Store) SetSetting(key, value string) error {
	_, err := s.db.Exec(`INSERT INTO settings (key, value) VALUES (?,?) ON CONFLICT(key) DO UPDATE SET value=excluded.value`, key, value)
	return err
}

func b2i(b bool) int {
	if b {
		return 1
	}
	return 0
}
