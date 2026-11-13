package store

import (
	"database/sql"
	"errors"
	"strings"
	"time"
)

// ServiceToken lets a trusted platform provision tenants (K3). Only its
// SHA-256 is stored; it acts only on the tenants it created.
type ServiceToken struct {
	ID        string `json:"id"`
	Name      string `json:"name"`
	SHA256    string `json:"-"`
	Created   int64  `json:"created"`
	CreatedBy string `json:"createdBy"`
	Revoked   bool   `json:"revoked"`
	Expires   int64  `json:"expires"` // unix seconds; 0 = never
}

func (s *Store) AddServiceToken(t ServiceToken) error {
	_, err := s.db.Exec(`INSERT INTO service_tokens (id, name, sha256, created, created_by, expires) VALUES (?,?,?,?,?,?)`,
		t.ID, t.Name, strings.ToLower(t.SHA256), time.Now().Unix(), t.CreatedBy, t.Expires)
	return err
}

// ServiceTokenBySHA is the live (not revoked, not expired) token with this hash.
func (s *Store) ServiceTokenBySHA(sha string) (ServiceToken, error) {
	var t ServiceToken
	err := s.db.QueryRow(`SELECT id, name, created, created_by, expires FROM service_tokens
		WHERE sha256 = ? AND revoked = 0 AND (expires = 0 OR expires > ?)`,
		strings.ToLower(sha), time.Now().Unix()).Scan(&t.ID, &t.Name, &t.Created, &t.CreatedBy, &t.Expires)
	if errors.Is(err, sql.ErrNoRows) {
		return t, ErrNotFound
	}
	return t, err
}

func (s *Store) ServiceTokens() ([]ServiceToken, error) {
	rows, err := s.db.Query(`SELECT id, name, created, created_by, revoked, expires FROM service_tokens ORDER BY created`)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	out := []ServiceToken{}
	for rows.Next() {
		var t ServiceToken
		var rev int
		if err := rows.Scan(&t.ID, &t.Name, &t.Created, &t.CreatedBy, &rev, &t.Expires); err != nil {
			return nil, err
		}
		t.Revoked = rev != 0
		out = append(out, t)
	}
	return out, rows.Err()
}

func (s *Store) RevokeServiceToken(id string) error {
	r, err := s.db.Exec(`UPDATE service_tokens SET revoked = 1 WHERE id = ?`, id)
	if err != nil {
		return err
	}
	if n, _ := r.RowsAffected(); n == 0 {
		return ErrNotFound
	}
	return nil
}

// CreateOwnedTenant adds a tenant owned by a service token; false when the id is taken.
func (s *Store) CreateOwnedTenant(t Tenant, ownerToken string) (bool, error) {
	r, err := s.db.Exec(`INSERT OR IGNORE INTO tenants (id, name, created, owner_token) VALUES (?,?,?,?)`,
		t.ID, t.Name, time.Now().Unix(), ownerToken)
	if err != nil {
		return false, err
	}
	n, _ := r.RowsAffected()
	return n > 0, nil
}

// TenantOwnedBy reports whether tenant id was created by service token tokenID.
func (s *Store) TenantOwnedBy(id, tokenID string) bool {
	var n int
	_ = s.db.QueryRow(`SELECT count(*) FROM tenants WHERE lower(id) = lower(?) AND owner_token = ? AND owner_token != ''`, id, tokenID).Scan(&n)
	return n > 0
}

// AddPairingCode stores a one-time code (its hash) for a tenant.
func (s *Store) AddPairingCode(codeSHA, tenant string, expiresMs int64) error {
	_, err := s.db.Exec(`INSERT INTO pairing_codes (code_sha, tenant, expires_ms, created) VALUES (?,?,?,?)`,
		strings.ToLower(codeSHA), tenant, expiresMs, time.Now().Unix())
	return err
}

// TakePairingCode marks an unused, unexpired code used and returns its
// tenant; exactly one caller gets it.
func (s *Store) TakePairingCode(codeSHA string, nowMs int64) (string, error) {
	tx, err := s.db.Begin()
	if err != nil {
		return "", err
	}
	defer func() { _ = tx.Rollback() }()
	var tenant string
	err = tx.QueryRow(`SELECT tenant FROM pairing_codes WHERE code_sha = ? AND used = 0 AND expires_ms > ?`,
		strings.ToLower(codeSHA), nowMs).Scan(&tenant)
	if errors.Is(err, sql.ErrNoRows) {
		return "", ErrNotFound
	}
	if err != nil {
		return "", err
	}
	r, err := tx.Exec(`UPDATE pairing_codes SET used = 1 WHERE code_sha = ? AND used = 0`, strings.ToLower(codeSHA))
	if err != nil {
		return "", err
	}
	if n, _ := r.RowsAffected(); n != 1 {
		return "", ErrNotFound
	}
	return tenant, tx.Commit()
}

// MachinesOf lists a tenant's stored machines.
func (s *Store) MachinesOf(tenant string) ([]Machine, error) {
	all, err := s.Machines()
	if err != nil {
		return nil, err
	}
	out := []Machine{}
	for _, m := range all {
		if strings.EqualFold(m.Tenant, tenant) {
			out = append(out, m)
		}
	}
	return out, nil
}

// DeleteTenant removes a tenant and everything in it: machines, users,
// messages, pending pairing codes and requests. The default tenant stays.
func (s *Store) DeleteTenant(id string) error {
	if strings.EqualFold(id, DefaultTenant) {
		return errors.New("the default tenant cannot be deleted")
	}
	tx, err := s.db.Begin()
	if err != nil {
		return err
	}
	defer func() { _ = tx.Rollback() }()
	for _, q := range []string{
		`DELETE FROM assets WHERE lower(machine) IN (SELECT lower(name) FROM machines WHERE lower(tenant) = lower(?))`,
		`DELETE FROM machines WHERE lower(tenant) = lower(?)`,
		`DELETE FROM users WHERE lower(tenant) = lower(?)`,
		`DELETE FROM messages WHERE lower(tenant) = lower(?)`,
		`DELETE FROM pairing_codes WHERE lower(tenant) = lower(?)`,
		`DELETE FROM connect_requests WHERE lower(tenant) = lower(?)`,
		`DELETE FROM tenants WHERE lower(id) = lower(?)`,
	} {
		if _, err := tx.Exec(q, id); err != nil {
			return err
		}
	}
	return tx.Commit()
}

// LivePairingCode is the tenant of an unused, unexpired code, without using it.
func (s *Store) LivePairingCode(codeSHA string, nowMs int64) (string, bool) {
	var t string
	err := s.db.QueryRow(`SELECT tenant FROM pairing_codes WHERE code_sha = ? AND used = 0 AND expires_ms > ?`,
		strings.ToLower(codeSHA), nowMs).Scan(&t)
	return t, err == nil
}

// UseTicket records a sign-in ticket id; false when it was already used.
// Expired ids are dropped on the way.
func (s *Store) UseTicket(jti string, expires int64) (bool, error) {
	_, _ = s.db.Exec(`DELETE FROM used_tickets WHERE expires < ?`, time.Now().Unix())
	r, err := s.db.Exec(`INSERT OR IGNORE INTO used_tickets (jti, expires) VALUES (?,?)`, jti, expires)
	if err != nil {
		return false, err
	}
	n, _ := r.RowsAffected()
	return n == 1, nil
}
