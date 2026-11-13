package store

import "time"

// AuditRow is one action taken through the hub: what and by whom, never the
// content (sizes only).
type AuditRow struct {
	Time    int64  `json:"time"`
	Tenant  string `json:"tenant"`
	Actor   string `json:"actor"`
	Machine string `json:"machine"`
	Target  string `json:"target"`
	Action  string `json:"action"`
	Status  int    `json:"status"`
	Bytes   int64  `json:"bytes"`
}

func (s *Store) Audit(a AuditRow) error {
	if a.Time == 0 {
		a.Time = time.Now().Unix()
	}
	_, err := s.db.Exec(`INSERT INTO audit (ts, tenant, actor, machine, target, action, status, bytes) VALUES (?,?,?,?,?,?,?,?)`,
		a.Time, tenantOr(a.Tenant), a.Actor, a.Machine, a.Target, a.Action, a.Status, a.Bytes)
	return err
}

// AuditRows returns the newest rows; tenant "" means every tenant.
func (s *Store) AuditRows(limit int, tenant string) ([]AuditRow, error) {
	rows, err := s.db.Query(`SELECT ts, tenant, actor, machine, target, action, status, bytes FROM audit
		WHERE ? = '' OR lower(tenant) = lower(?) ORDER BY id DESC LIMIT ?`, tenant, tenant, limit)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	out := []AuditRow{}
	for rows.Next() {
		var r AuditRow
		if err := rows.Scan(&r.Time, &r.Tenant, &r.Actor, &r.Machine, &r.Target, &r.Action, &r.Status, &r.Bytes); err != nil {
			return nil, err
		}
		out = append(out, r)
	}
	return out, rows.Err()
}
