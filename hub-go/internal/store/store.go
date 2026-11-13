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
	"strings"
	"time"

	_ "modernc.org/sqlite"
)

//go:embed migrations
var migrations embed.FS

// ErrNotFound is returned when a row does not exist.
var ErrNotFound = errors.New("not found")

// Store is the hub's SQLite database (hub.db).
type Store struct {
	db *dbx
}

// dbx is the *sql.DB the queries run on.
type dbx struct {
	*sql.DB
}

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

// Snapshot writes a consistent copy of a SQLite database to path (VACUUM INTO; the file must not exist).
func (s *Store) Snapshot(path string) error {
	_, err := s.db.DB.Exec(`VACUUM INTO ?`, path)
	return err
}

func (s *Store) Close() error { return s.db.DB.Close() }

// migrate applies migrations/<dialect>/NNN_*.sql once each, in order; re-running is a no-op.
func (s *Store) migrate() error {
	dir := "migrations/sqlite"
	ctx := context.Background()
	conn, err := s.db.DB.Conn(ctx)
	if err != nil {
		return err
	}
	defer conn.Close()
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
	for _, n := range names {
		var done int
		if err := conn.QueryRowContext(ctx, `SELECT COUNT(*) FROM schema_migrations WHERE name = ?`, n).Scan(&done); err != nil {
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
	Tenant string   `json:"tenant"`
	Role   string   `json:"role"` // member (default) or viewer (read-only)
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
	_, err := s.db.Exec(`INSERT INTO users (name, display_name, hash, admin, source, created, groups, tenant, role) VALUES (?,?,?,?,?,?,?,?,?)
		ON CONFLICT(name) DO UPDATE SET display_name=excluded.display_name, hash=excluded.hash, admin=excluded.admin, source=excluded.source, groups=excluded.groups, tenant=excluded.tenant, role=excluded.role`,
		u.Name, u.DisplayName, u.Hash, b2i(u.Admin), u.Source, time.Now().Unix(), joinGroups(u.Groups), tenantOr(u.Tenant), roleOr(u.Role))
	return err
}

// InsertUser adds a user; false when the name is taken.
func (s *Store) InsertUser(u User) (bool, error) {
	if u.DisplayName == "" {
		u.DisplayName = u.Name
	}
	r, err := s.db.Exec(`INSERT OR IGNORE INTO users (name, display_name, hash, admin, source, created, groups, tenant, role) VALUES (?,?,?,?,?,?,?,?,?)`,
		u.Name, u.DisplayName, u.Hash, b2i(u.Admin), u.Source, time.Now().Unix(), joinGroups(u.Groups), tenantOr(u.Tenant), roleOr(u.Role))
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
	err := sc.Scan(&u.Name, &u.DisplayName, &u.Hash, &admin, &u.Source, &u.Created, &groups, &u.Tenant, &u.Role)
	u.Admin = admin != 0
	u.Groups = splitGroups(groups)
	return u, err
}

const userCols = `name, display_name, hash, admin, source, created, groups, tenant, role`

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
	Tenant    string `json:"tenant"`
}

// AddMachine stores a machine token's hash; false when the name is taken.
func (s *Store) AddMachine(m Machine) (bool, error) {
	r, err := s.db.Exec(`INSERT OR IGNORE INTO machines (name, token_sha256, fleet, created, created_by, tenant) VALUES (?,?,?,?,?,?)`,
		m.Name, strings.ToLower(m.SHA256), b2i(m.Fleet), time.Now().Unix(), m.CreatedBy, tenantOr(m.Tenant))
	if err != nil {
		return false, err
	}
	n, _ := r.RowsAffected()
	return n > 0, nil
}

func (s *Store) Machines() ([]Machine, error) {
	rows, err := s.db.Query(`SELECT name, token_sha256, fleet, created, created_by, last_seen, version, protocol, tenant FROM machines ORDER BY name`)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var out []Machine
	for rows.Next() {
		var m Machine
		var fleet int
		if err := rows.Scan(&m.Name, &m.SHA256, &fleet, &m.Created, &m.CreatedBy, &m.LastSeen, &m.Version, &m.Protocol, &m.Tenant); err != nil {
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
	Tenant string `json:"tenant"`
}

func (s *Store) RecordMessage(m Message) error {
	_, err := s.db.Exec(`INSERT INTO messages (id, ts, sender, from_, to_, text, ok, error, tenant) VALUES (?,?,?,?,?,?,?,?,?)`,
		m.ID, m.Time, m.Sender, m.From, m.To, m.Text, b2i(m.OK), m.Error, tenantOr(m.Tenant))
	return err
}

// RecentMessages returns the newest messages; tenant "" means every tenant.
func (s *Store) RecentMessages(limit int, tenant string) ([]Message, error) {
	rows, err := s.db.Query(`SELECT id, ts, sender, from_, to_, text, ok, error, tenant FROM messages
		WHERE ? = '' OR lower(tenant) = lower(?) ORDER BY ts DESC, rowid DESC LIMIT ?`, tenant, tenant, limit)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var out []Message
	for rows.Next() {
		var m Message
		var ok int
		if err := rows.Scan(&m.ID, &m.Time, &m.Sender, &m.From, &m.To, &m.Text, &ok, &m.Error, &m.Tenant); err != nil {
			return nil, err
		}
		m.OK = ok != 0
		out = append(out, m)
	}
	return out, rows.Err()
}

// ---- tenants -----------------------------------------------------------------

// DefaultTenant holds everything a hub had before tenants, env-configured
// machines and fleet tokens. Its admins operate the whole hub.
const DefaultTenant = "default"

func roleOr(r string) string {
	if r == "viewer" {
		return r
	}
	return "member"
}

func tenantOr(t string) string {
	if strings.TrimSpace(t) == "" {
		return DefaultTenant
	}
	return t
}

type Tenant struct {
	ID      string `json:"id"`
	Name    string `json:"name"`
	Created int64  `json:"created"`
}

// CreateTenant adds a tenant; false when the id is taken.
func (s *Store) CreateTenant(t Tenant) (bool, error) {
	r, err := s.db.Exec(`INSERT OR IGNORE INTO tenants (id, name, created) VALUES (?,?,?)`, t.ID, t.Name, time.Now().Unix())
	if err != nil {
		return false, err
	}
	n, _ := r.RowsAffected()
	return n > 0, nil
}

func (s *Store) TenantExists(id string) bool {
	var n int
	_ = s.db.QueryRow(`SELECT count(*) FROM tenants WHERE lower(id) = lower(?)`, id).Scan(&n)
	return n > 0
}

func (s *Store) Tenants() ([]Tenant, error) {
	rows, err := s.db.Query(`SELECT id, name, created FROM tenants ORDER BY id`)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	out := []Tenant{}
	for rows.Next() {
		var t Tenant
		if err := rows.Scan(&t.ID, &t.Name, &t.Created); err != nil {
			return nil, err
		}
		out = append(out, t)
	}
	return out, rows.Err()
}

// MachineTenant is the tenant of a machine id: its own row, else its fleet's
// row (fleet machines link as <fleet>:<host>), else "" (no tenant: the caller
// decides; machines from HUB_MACHINES are resolved before this).
func (s *Store) MachineTenant(id string) string {
	var t string
	if s.db.QueryRow(`SELECT tenant FROM machines WHERE lower(name) = lower(?)`, id).Scan(&t) == nil {
		return t
	}
	if fleet, _, ok := strings.Cut(id, ":"); ok && s.db.QueryRow(`SELECT tenant FROM machines WHERE lower(name) = lower(?) AND fleet = 1`, fleet).Scan(&t) == nil {
		return t
	}
	return ""
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
