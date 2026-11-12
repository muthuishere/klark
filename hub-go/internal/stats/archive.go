package stats

import (
	"bytes"
	"compress/gzip"
	"context"
	"crypto/rand"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"log"
	"strconv"
	"sync"
	"time"

	"github.com/muthuishere/klark/hub-go/internal/config"
	"github.com/muthuishere/klark/hub-go/internal/store"
)

const (
	settingHubID  = "hub.id"
	settingCursor = "archive.cursor"
	maxBackfill   = 48 * time.Hour
)

// Archiver copies the stored samples to a Sink as hourly gzip NDJSON files
// `analytics/YYYY/MM/DD/HH-<hubid>.ndjson.gz` (UTC hours; the S3 sink adds its prefix). Writing the same hour again
// overwrites the same object, so a re-run is idempotent.
type Archiver struct {
	Store *store.Store
	// Config returns the archive settings in force right now (admin-saved or environment).
	Config func() config.Storage
	// Every is how often the current hour is refreshed (default 10 min).
	Every time.Duration
	Now   func() time.Time
	// MakeSink builds the sink (default NewSink); tests can replace it.
	MakeSink func(config.Storage) (Sink, error)

	mu          sync.Mutex
	lastWrite   int64
	lastError   string
	filesWriten int64
	written     map[int64]int // hour -> samples in the last file written for it
}

// Status is what the admin page shows.
type Status struct {
	LastWriteMs  int64
	LastError    string
	FilesWritten int64
}

func (a *Archiver) now() time.Time {
	if a.Now != nil {
		return a.Now()
	}
	return time.Now()
}

func (a *Archiver) Status() Status {
	a.mu.Lock()
	defer a.mu.Unlock()
	return Status{a.lastWrite, a.lastError, a.filesWriten}
}

// HubID is this hub's stable short id used in file names.
func (a *Archiver) HubID() string {
	if v, ok := a.Store.Setting(settingHubID); ok && v != "" {
		return v
	}
	raw := make([]byte, 4)
	_, _ = rand.Read(raw)
	id := hex.EncodeToString(raw)
	_ = a.Store.SetSetting(settingHubID, id)
	return id
}

// ResetCursor makes the next cycle start over (after the destination changed).
func (a *Archiver) ResetCursor() {
	_ = a.Store.SetSetting(settingCursor, "")
	a.mu.Lock()
	a.written = nil
	a.mu.Unlock()
}

// Run cycles until ctx ends.
func (a *Archiver) Run(ctx context.Context) {
	every := a.Every
	if every <= 0 {
		every = 10 * time.Minute
	}
	t := time.NewTicker(every)
	defer t.Stop()
	first := time.After(30 * time.Second) // write what exists soon after start, then every interval
	for {
		select {
		case <-ctx.Done():
			return
		case <-first:
			a.Cycle(ctx)
		case <-t.C:
			a.Cycle(ctx)
		}
	}
}

func hourStart(t time.Time) time.Time { return t.UTC().Truncate(time.Hour) }

// Key is the archive object key of an hour.
func Key(hub string, hour time.Time) string {
	h := hour.UTC()
	return fmt.Sprintf("analytics/%04d/%02d/%02d/%02d-%s.ndjson.gz", h.Year(), int(h.Month()), h.Day(), h.Hour(), hub)
}

// Cycle writes every hour from the cursor up to the current one. A failure is recorded and retried next cycle.
func (a *Archiver) Cycle(ctx context.Context) {
	cfg := a.Config()
	mk := a.MakeSink
	if mk == nil {
		mk = NewSink
	}
	sink, err := mk(cfg)
	if err != nil {
		a.fail(err)
		return
	}
	if sink == nil {
		return
	}
	now := a.now()
	cur := hourStart(now)
	start := cur.Add(-24 * time.Hour)
	if v, _ := a.Store.Setting(settingCursor); v != "" {
		if ms, err := strconv.ParseInt(v, 10, 64); err == nil {
			start = time.UnixMilli(ms).UTC()
		}
	}
	if start.Before(cur.Add(-maxBackfill)) {
		start = cur.Add(-maxBackfill)
	}
	hub := a.HubID()
	for h := hourStart(start); !h.After(cur); h = h.Add(time.Hour) {
		if ctx.Err() != nil {
			return
		}
		if _, err := a.WriteHour(ctx, sink, hub, h); err != nil {
			a.fail(err)
			return
		}
		if h.Before(cur) {
			_ = a.Store.SetSetting(settingCursor, strconv.FormatInt(h.Add(time.Hour).UnixMilli(), 10))
		}
	}
	a.mu.Lock()
	a.lastError = ""
	a.mu.Unlock()
}

func (a *Archiver) fail(err error) {
	log.Printf("stats archive: %v", err)
	a.mu.Lock()
	a.lastError = err.Error()
	a.mu.Unlock()
}

// WriteHour writes (overwrites) the object of one hour; 0 samples writes nothing. For the same sample count as the
// last write of that hour in this process nothing is sent again.
func (a *Archiver) WriteHour(ctx context.Context, sink Sink, hub string, hour time.Time) (int, error) {
	hour = hourStart(hour)
	rows, err := a.Store.StatsBetween(hour.UnixMilli(), hour.Add(time.Hour).UnixMilli())
	if err != nil {
		return 0, err
	}
	if len(rows) == 0 {
		return 0, nil
	}
	a.mu.Lock()
	same := a.written[hour.UnixMilli()] == len(rows)
	a.mu.Unlock()
	if same {
		return len(rows), nil
	}
	data, err := EncodeHour(hub, rows)
	if err != nil {
		return 0, err
	}
	if err := sink.Put(ctx, Key(hub, hour), data); err != nil {
		return 0, err
	}
	a.mu.Lock()
	if a.written == nil {
		a.written = map[int64]int{}
	}
	a.written[hour.UnixMilli()] = len(rows)
	a.lastWrite = a.now().UnixMilli()
	a.filesWriten++
	a.mu.Unlock()
	return len(rows), nil
}

// EncodeHour is the gzip NDJSON of samples: one flat JSON object per line. The gzip header carries no time, so the
// same samples always give the same bytes.
func EncodeHour(hub string, rows []store.StatRow) ([]byte, error) {
	var buf bytes.Buffer
	zw, _ := gzip.NewWriterLevel(&buf, gzip.BestCompression)
	enc := json.NewEncoder(zw)
	for _, r := range rows {
		if r.MachineID == HubID {
			l := hubLatestOf(r)
			obj := map[string]any{"kind": "hub", "ts_ms": r.TS, "ts": time.UnixMilli(r.TS).UTC().Format(time.RFC3339), "hub": hub, "machine_id": HubID,
				"rss_bytes": l.RSSBytes, "peak_rss_bytes": l.PeakRSSBytes, "cpu_percent": l.CPUPercent, "heap_inuse_bytes": l.HeapInuseBytes,
				"heap_sys_bytes": l.HeapSysBytes, "goroutines": l.Goroutines, "gc_count": l.GCCount, "gc_pause_total_ms": l.GCPauseTotalMs,
				"uptime_s": l.UptimeSeconds, "version": l.Version, "machine_links": l.MachineLinks, "sse_streams": l.SSEStreams,
				"pane_viewers": l.PaneViewers, "requests": l.Requests, "requests_per_minute": l.RequestsPerMinute,
				"db_bytes": l.DBBytes, "db_wal_bytes": l.DBWALBytes, "assets_bytes": l.AssetsBytes}
			if err := enc.Encode(obj); err != nil {
				return nil, err
			}
			continue
		}
		var agents []AgentSample
		_ = json.Unmarshal([]byte(r.AgentsJSON), &agents)
		if agents == nil {
			agents = []AgentSample{}
		}
		obj := map[string]any{
			"kind": "machine", "ts_ms": r.TS, "ts": time.UnixMilli(r.TS).UTC().Format(time.RFC3339), "hub": hub, "machine": r.MachineName, "machine_id": r.MachineID,
			"klark_rss_bytes": r.RSSBytes, "klark_peak_rss_bytes": r.PeakRSSBytes, "klark_cpu_percent": r.CPUPercent,
			"klark_uptime_s": r.UptimeSeconds, "klark_version": r.Version,
			"host_mem_total_bytes": r.MemTotalBytes, "host_mem_available_bytes": r.MemAvailableBytes, "host_load1": r.Load1, "host_cpus": r.CPUs,
			"panes": r.Panes, "agents": r.Agents, "agent_list": agents,
		}
		if err := enc.Encode(obj); err != nil { // sorted keys: deterministic
			return nil, err
		}
	}
	if err := zw.Close(); err != nil {
		return nil, err
	}
	return buf.Bytes(), nil
}

// Test writes and deletes a small marker object in the destination described by cfg.
func (a *Archiver) Test(ctx context.Context, cfg config.Storage) (bool, string) {
	mk := a.MakeSink
	if mk == nil {
		mk = NewSink
	}
	sink, err := mk(cfg)
	if err != nil {
		return false, err.Error()
	}
	if sink == nil {
		return false, "The archive is off (kind none); choose folder or s3 first."
	}
	cctx, cancel := context.WithTimeout(ctx, 30*time.Second)
	defer cancel()
	key := "analytics/.klark-test-" + a.HubID()
	if err := sink.Put(cctx, key, []byte("klark archive test\n")); err != nil {
		return false, "Write failed: " + err.Error()
	}
	if err := sink.Delete(cctx, key); err != nil {
		return false, "Written, but the marker could not be deleted: " + err.Error()
	}
	return true, "Wrote and deleted a marker object; the archive destination works."
}
