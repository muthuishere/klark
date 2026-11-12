package store

import (
	"database/sql"
	"errors"
	"strings"
	"time"
)

// ---- assets (ADR 0011 B): content-addressed bytes in the database, so every pod serves them ----------

// Asset is one recorded upload: who sent which bytes to which machine and pane.
type Asset struct {
	ID        int64
	SHA256    string
	Size      int64
	Mime      string
	Name      string
	Time      int64 // unix ms
	Machine   string
	Pane      string
	Direction string // to_agent | from_agent
	By        string
}

// PutAssetBlob stores the bytes under their SHA-256 once; an identical upload is kept once. existed reports a
// blob that was already there.
func (s *Store) PutAssetBlob(sha, mime string, data []byte) (existed bool, err error) {
	r, err := s.db.Exec(`INSERT OR IGNORE INTO asset_blobs (sha256, size, mime, data, created) VALUES (?,?,?,?,?)`,
		strings.ToLower(sha), int64(len(data)), mime, data, time.Now().UnixMilli())
	if err != nil {
		return false, err
	}
	n, _ := r.RowsAffected()
	return n == 0, nil
}

// AssetBlob reads the bytes of a hash.
func (s *Store) AssetBlob(sha string) ([]byte, error) {
	var b []byte
	err := s.db.QueryRow(`SELECT data FROM asset_blobs WHERE sha256 = ?`, strings.ToLower(sha)).Scan(&b)
	if errors.Is(err, sql.ErrNoRows) {
		return nil, ErrNotFound
	}
	return b, err
}

// AddAsset records an asset (its blob must exist).
func (s *Store) AddAsset(a Asset) error {
	_, err := s.db.Exec(`INSERT INTO assets (sha256, size, mime, name, ts, machine, pane_id, direction, by_user) VALUES (?,?,?,?,?,?,?,?,?)`,
		strings.ToLower(a.SHA256), a.Size, a.Mime, a.Name, a.Time, a.Machine, a.Pane, a.Direction, a.By)
	return err
}

// Assets lists the live records (ts >= since) of a hash, newest first.
func (s *Store) Assets(sha string, since int64) ([]Asset, error) {
	rows, err := s.db.Query(`SELECT id, sha256, size, mime, name, ts, machine, pane_id, direction, by_user FROM assets
		WHERE sha256 = ? AND ts >= ? ORDER BY ts DESC`, strings.ToLower(sha), since)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var out []Asset
	for rows.Next() {
		var a Asset
		if err := rows.Scan(&a.ID, &a.SHA256, &a.Size, &a.Mime, &a.Name, &a.Time, &a.Machine, &a.Pane, &a.Direction, &a.By); err != nil {
			return nil, err
		}
		out = append(out, a)
	}
	return out, rows.Err()
}

// SweepAssets deletes records older than before (unix ms), then every blob no remaining record names.
func (s *Store) SweepAssets(before int64) (records, blobs int64, err error) {
	r, err := s.db.Exec(`DELETE FROM assets WHERE ts < ?`, before)
	if err != nil {
		return 0, 0, err
	}
	records, _ = r.RowsAffected()
	r, err = s.db.Exec(`DELETE FROM asset_blobs WHERE sha256 NOT IN (SELECT sha256 FROM assets)`)
	if err != nil {
		return records, 0, err
	}
	blobs, _ = r.RowsAffected()
	return records, blobs, nil
}

// ---- device flow for `klark hub connect` (ADR 0012) -----------------------------------------------------

// ConnectRequest is a pending (or decided, not yet collected) machine approval. Only the SHA-256 of the
// poll id is stored; the machine's token is minted when the approved request is collected.
type ConnectRequest struct {
	PollSHA   string
	Code      string
	Machine   string
	Status    string // pending | approved | denied
	IP        string
	Created   int64 // unix ms
	Expires   int64 // unix ms
	DecidedBy string
}

const connectCols = `poll_sha, code, machine, status, ip, created_ms, expires_ms, decided_by`

func scanConnect(row interface{ Scan(...any) error }) (ConnectRequest, error) {
	var c ConnectRequest
	err := row.Scan(&c.PollSHA, &c.Code, &c.Machine, &c.Status, &c.IP, &c.Created, &c.Expires, &c.DecidedBy)
	if errors.Is(err, sql.ErrNoRows) {
		return c, ErrNotFound
	}
	return c, err
}

// AddConnectRequest stores a new request; false when its code is already taken.
func (s *Store) AddConnectRequest(c ConnectRequest) (bool, error) {
	r, err := s.db.Exec(`INSERT OR IGNORE INTO connect_requests (`+connectCols+`) VALUES (?,?,?,?,?,?,?,?)`,
		c.PollSHA, c.Code, c.Machine, c.Status, c.IP, c.Created, c.Expires, c.DecidedBy)
	if err != nil {
		return false, err
	}
	n, _ := r.RowsAffected()
	return n > 0, nil
}

// ConnectRequestByCode is the request a code names (expired ones included; the caller checks).
func (s *Store) ConnectRequestByCode(code string) (ConnectRequest, error) {
	return scanConnect(s.db.QueryRow(`SELECT `+connectCols+` FROM connect_requests WHERE code = ?`, code))
}

// ConnectRequestByPoll is the request a poll id's SHA-256 names.
func (s *Store) ConnectRequestByPoll(pollSHA string) (ConnectRequest, error) {
	return scanConnect(s.db.QueryRow(`SELECT `+connectCols+` FROM connect_requests WHERE poll_sha = ?`, pollSHA))
}

// DecideConnectRequest moves a pending, unexpired request to status; false when it was not pending any more.
func (s *Store) DecideConnectRequest(code, status, by string, now int64) (bool, error) {
	r, err := s.db.Exec(`UPDATE connect_requests SET status = ?, decided_by = ? WHERE code = ? AND status = 'pending' AND expires_ms > ?`,
		status, by, code, now)
	if err != nil {
		return false, err
	}
	n, _ := r.RowsAffected()
	return n == 1, nil
}

// TakeConnectRequest deletes the request when it has status; true for exactly one caller.
func (s *Store) TakeConnectRequest(pollSHA, status string) (bool, error) {
	r, err := s.db.Exec(`DELETE FROM connect_requests WHERE poll_sha = ? AND status = ?`, pollSHA, status)
	if err != nil {
		return false, err
	}
	n, _ := r.RowsAffected()
	return n == 1, nil
}

// DeleteConnectRequest removes a request whatever its state.
func (s *Store) DeleteConnectRequest(pollSHA string) error {
	_, err := s.db.Exec(`DELETE FROM connect_requests WHERE poll_sha = ?`, pollSHA)
	return err
}

// SweepConnectRequests drops requests that expired before now; it returns how many are left.
func (s *Store) SweepConnectRequests(now int64) (left int, err error) {
	if _, err = s.db.Exec(`DELETE FROM connect_requests WHERE expires_ms <= ?`, now); err != nil {
		return 0, err
	}
	err = s.db.QueryRow(`SELECT COUNT(*) FROM connect_requests`).Scan(&left)
	return left, err
}

// UpsertMachine stores a (non-fleet) machine token's hash, replacing the token of a machine with that name.
func (s *Store) UpsertMachine(name, sha, createdBy string) error {
	_, err := s.db.Exec(`INSERT INTO machines (name, token_sha256, fleet, created, created_by) VALUES (?,?,0,?,?)
		ON CONFLICT(name) DO UPDATE SET token_sha256 = excluded.token_sha256, fleet = 0, created = excluded.created, created_by = excluded.created_by`,
		name, strings.ToLower(sha), time.Now().Unix(), createdBy)
	return err
}
