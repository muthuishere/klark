package store

import (
	"database/sql"
	"errors"
	"strings"
)

// Presence says which pod holds a machine's live link (table machine_presence; used with several pods).
type Presence struct {
	MachineID   string
	PodID       string
	PodURL      string
	SessionID   string
	Name        string
	Version     string
	Protocol    int
	HeartbeatMs int64
}

const presenceCols = `machine_id, pod_id, pod_url, session_id, name, version, protocol, heartbeat_ms`

func scanPresence(sc interface{ Scan(...any) error }) (Presence, error) {
	var p Presence
	err := sc.Scan(&p.MachineID, &p.PodID, &p.PodURL, &p.SessionID, &p.Name, &p.Version, &p.Protocol, &p.HeartbeatMs)
	return p, err
}

// UpsertPresence records that pod holds the machine as of nowMs. force (a session was just opened) always wins; a
// plain heartbeat only refreshes the pod's own row or takes over a row whose heartbeat is older than ttlMs... it is
// given as the cutoff: rows with heartbeat < nowMs-ExpiryMs are taken.
func (s *Store) UpsertPresence(p Presence, nowMs int64, force bool) error {
	p.HeartbeatMs = nowMs
	q := `INSERT INTO machine_presence (` + presenceCols + `) VALUES (?,?,?,?,?,?,?,?)
		ON CONFLICT(machine_id) DO UPDATE SET pod_id=excluded.pod_id, pod_url=excluded.pod_url, session_id=excluded.session_id,
		name=excluded.name, version=excluded.version, protocol=excluded.protocol, heartbeat_ms=excluded.heartbeat_ms`
	args := []any{p.MachineID, p.PodID, p.PodURL, p.SessionID, p.Name, p.Version, p.Protocol, p.HeartbeatMs}
	if !force {
		q += ` WHERE (machine_presence.pod_id = excluded.pod_id AND machine_presence.session_id = excluded.session_id)
			OR machine_presence.heartbeat_ms < ?`
		args = append(args, nowMs-PresenceExpiryMs)
	}
	_, err := s.db.Exec(q, args...)
	return err
}

// PresenceExpiryMs is how long a heartbeat keeps a machine present.
const PresenceExpiryMs = 15000

// PresenceOf is the machine's presence with a heartbeat at or after minHeartbeatMs.
func (s *Store) PresenceOf(machineID string, minHeartbeatMs int64) (Presence, bool, error) {
	p, err := scanPresence(s.db.QueryRow(`SELECT `+presenceCols+` FROM machine_presence WHERE machine_id = ? AND heartbeat_ms >= ?`, machineID, minHeartbeatMs))
	if errors.Is(err, sql.ErrNoRows) {
		return p, false, nil
	}
	return p, err == nil, err
}

func (s *Store) PresenceBySession(session string, minHeartbeatMs int64) (Presence, bool, error) {
	p, err := scanPresence(s.db.QueryRow(`SELECT `+presenceCols+` FROM machine_presence WHERE session_id = ? AND heartbeat_ms >= ?`, session, minHeartbeatMs))
	if errors.Is(err, sql.ErrNoRows) {
		return p, false, nil
	}
	return p, err == nil, err
}

func (s *Store) ListPresence(minHeartbeatMs int64) ([]Presence, error) {
	rows, err := s.db.Query(`SELECT `+presenceCols+` FROM machine_presence WHERE heartbeat_ms >= ? ORDER BY machine_id`, minHeartbeatMs)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var out []Presence
	for rows.Next() {
		p, err := scanPresence(rows)
		if err != nil {
			return nil, err
		}
		out = append(out, p)
	}
	return out, rows.Err()
}

// DeletePresenceExcept removes the pod's rows for machines not in keep (machines that left, or all of them at shutdown).
func (s *Store) DeletePresenceExcept(podID string, keep []string) error {
	q := `DELETE FROM machine_presence WHERE pod_id = ?`
	args := []any{podID}
	if len(keep) > 0 {
		q += ` AND machine_id NOT IN (` + strings.TrimSuffix(strings.Repeat("?,", len(keep)), ",") + `)`
		for _, k := range keep {
			args = append(args, k)
		}
	}
	_, err := s.db.Exec(q, args...)
	return err
}
