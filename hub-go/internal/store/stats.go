package store

import (
	"database/sql"
	"errors"
)

// StatRow is one sample of one machine (table machine_stats). Times are unix milliseconds.
type StatRow struct {
	MachineID         string
	MachineName       string
	TS                int64
	RSSBytes          int64
	PeakRSSBytes      int64
	CPUPercent        float64
	UptimeSeconds     int64
	Version           string
	MemTotalBytes     int64
	MemAvailableBytes int64
	Load1             float64
	CPUs              int64
	Panes             int64
	Agents            int64
	AgentsJSON        string // JSON array of {paneId,name,state,rssBytes,cpuPercent}
}

const statCols = `machine_id, machine_name, ts, rss_bytes, peak_rss_bytes, cpu_percent, uptime_s, version,
	mem_total_bytes, mem_available_bytes, load1, cpus, panes, agents, agents_json`

func (s *Store) InsertStat(r StatRow) error {
	if r.AgentsJSON == "" {
		r.AgentsJSON = "[]"
	}
	_, err := s.db.Exec(`INSERT INTO machine_stats (`+statCols+`) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)`,
		r.MachineID, r.MachineName, r.TS, r.RSSBytes, r.PeakRSSBytes, r.CPUPercent, r.UptimeSeconds, r.Version,
		r.MemTotalBytes, r.MemAvailableBytes, r.Load1, r.CPUs, r.Panes, r.Agents, r.AgentsJSON)
	return err
}

func scanStat(sc interface{ Scan(...any) error }) (StatRow, error) {
	var r StatRow
	err := sc.Scan(&r.MachineID, &r.MachineName, &r.TS, &r.RSSBytes, &r.PeakRSSBytes, &r.CPUPercent, &r.UptimeSeconds, &r.Version,
		&r.MemTotalBytes, &r.MemAvailableBytes, &r.Load1, &r.CPUs, &r.Panes, &r.Agents, &r.AgentsJSON)
	return r, err
}

// StatsBetween are the samples with from <= ts < to, oldest first (all machines).
func (s *Store) StatsBetween(from, to int64) ([]StatRow, error) {
	rows, err := s.db.Query(`SELECT `+statCols+` FROM machine_stats WHERE ts >= ? AND ts < ? ORDER BY machine_id, ts, id`, from, to)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var out []StatRow
	for rows.Next() {
		r, err := scanStat(rows)
		if err != nil {
			return nil, err
		}
		out = append(out, r)
	}
	return out, rows.Err()
}

// LatestStat is the newest sample of a machine; ok is false when it has none.
func (s *Store) LatestStat(machineID string) (StatRow, bool, error) {
	r, err := scanStat(s.db.QueryRow(`SELECT `+statCols+` FROM machine_stats WHERE machine_id = ? ORDER BY ts DESC, id DESC LIMIT 1`, machineID))
	if errors.Is(err, sql.ErrNoRows) {
		return r, false, nil
	}
	return r, err == nil, err
}

// PurgeStats deletes samples older than before (unix ms) and returns how many.
func (s *Store) PurgeStats(before int64) (int64, error) {
	r, err := s.db.Exec(`DELETE FROM machine_stats WHERE ts < ?`, before)
	if err != nil {
		return 0, err
	}
	return r.RowsAffected()
}
