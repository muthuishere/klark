// Package stats is the hub's built-in analytics of the machines: a collector that asks every online machine for
// `system.stats` through the ordinary machine call path, a SQLite history with retention, the admin analytics
// query, and an optional archive (folder or S3) of hourly gzip NDJSON files for DuckDB.
package stats

import (
	"context"
	"encoding/json"
	"errors"
	"log"
	"strings"
	"sync"
	"time"

	"github.com/muthuishere/klark/hub-go/internal/hub"
	"github.com/muthuishere/klark/hub-go/internal/store"
)

// Method is the machine's JSON API method that answers a sample.
const Method = "system.stats"

// Source is the part of hub.Runtime the collector needs.
type Source interface {
	Machines() []hub.Info
	Call(ctx context.Context, machineID, method string, params any, timeout time.Duration) (json.RawMessage, error)
}

// Collector samples machines lazily: on demand while an admin looks, and once when a machine links. There is no
// polling of idle machines.
type Collector struct {
	Src       Source
	Store     *store.Store
	Interval  time.Duration // minimum time between two calls to one machine, default 30 s (HUB_STATS_INTERVAL_SECONDS, min 5)
	Retention time.Duration // default 14 days
	Timeout   time.Duration // per call, default 5 s
	Workers   int           // concurrent calls, default 4
	// RetryNoStats is how long a machine without system.stats is left alone (default 10 min).
	RetryNoStats time.Duration
	Now          func() time.Time

	mu       sync.Mutex
	noStats  map[string]time.Time // machine id -> do not ask before
	inflight map[string]bool
	lastTry  map[string]time.Time
}

func (c *Collector) defaults() {
	if c.Interval <= 0 { // the config already enforces the 5 s minimum
		c.Interval = 30 * time.Second
	}
	if c.Retention <= 0 {
		c.Retention = 14 * 24 * time.Hour
	}
	if c.Timeout <= 0 {
		c.Timeout = 5 * time.Second
	}
	if c.Workers <= 0 {
		c.Workers = 4
	}
	if c.RetryNoStats <= 0 {
		c.RetryNoStats = 10 * time.Minute
	}
	if c.Now == nil {
		c.Now = time.Now
	}
	c.mu.Lock()
	if c.noStats == nil {
		c.noStats = map[string]time.Time{}
		c.inflight = map[string]bool{}
		c.lastTry = map[string]time.Time{}
	}
	c.mu.Unlock()
}

// Run purges old samples hourly until ctx ends. It never calls a machine: samples are taken lazily by Refresh
// (an admin has the analytics page open) and by OnConnect (a machine linked).
func (c *Collector) Run(ctx context.Context) {
	c.defaults()
	c.Purge()
	purge := time.NewTicker(time.Hour)
	defer purge.Stop()
	for {
		select {
		case <-ctx.Done():
			return
		case <-purge.C:
			c.Purge()
		}
	}
}

// Purge drops samples older than the retention.
func (c *Collector) Purge() int64 {
	c.defaults()
	n, err := c.Store.PurgeStats(c.Now().Add(-c.Retention).UnixMilli())
	if err != nil {
		log.Printf("stats: purge: %v", err)
	}
	return n
}

// HasNoStats reports whether the machine is currently remembered as not answering system.stats.
func (c *Collector) HasNoStats(id string) bool {
	c.mu.Lock()
	defer c.mu.Unlock()
	until, ok := c.noStats[id]
	return ok && c.now().Before(until)
}

func (c *Collector) now() time.Time {
	if c.Now == nil {
		return time.Now()
	}
	return c.Now()
}

// Refresh samples the online machines whose last attempt is older than Interval (so any number of viewers make at
// most one call per machine per Interval). Calls run concurrently, bounded by Workers, each with the call
// timeout; Refresh returns when they are done or after wait, whichever is first (late calls still store their
// sample). Machines remembered as stat-less are left alone.
func (c *Collector) Refresh(ctx context.Context, wait time.Duration) { c.collect(ctx, false, wait) }

// CollectOnce samples every online machine now, regardless of the interval, and waits for the calls.
func (c *Collector) CollectOnce(ctx context.Context) { c.collect(ctx, true, 0) }

// OnConnect takes the one sample a machine gets when it links (in the background).
func (c *Collector) OnConnect(id string) {
	c.defaults()
	c.mu.Lock()
	delete(c.noStats, id) // a new link may be a newer klark
	c.mu.Unlock()
	for _, in := range c.Src.Machines() {
		if in.ID == id && in.Online {
			go c.sampleGuarded(context.Background(), in, true)
		}
	}
}

func (c *Collector) collect(ctx context.Context, force bool, wait time.Duration) {
	c.defaults()
	sem := make(chan struct{}, c.Workers)
	var wg sync.WaitGroup
	for _, in := range c.Src.Machines() {
		if !in.Online {
			continue
		}
		wg.Add(1)
		go func(in hub.Info) {
			defer wg.Done()
			select {
			case sem <- struct{}{}:
				defer func() { <-sem }()
			case <-ctx.Done():
				return
			}
			c.sampleGuarded(ctx, in, force)
		}(in)
	}
	done := make(chan struct{})
	go func() { wg.Wait(); close(done) }()
	if wait <= 0 {
		<-done
		return
	}
	t := time.NewTimer(wait)
	defer t.Stop()
	select {
	case <-done:
	case <-t.C:
	case <-ctx.Done():
	}
}

// sampleGuarded skips a machine that is stat-less, already being asked, or (unless force) asked within Interval.
func (c *Collector) sampleGuarded(ctx context.Context, in hub.Info, force bool) {
	c.mu.Lock()
	until, skip := c.noStats[in.ID]
	skip = skip && c.now().Before(until)
	if !skip && c.inflight[in.ID] {
		skip = true
	}
	if !skip && !force && c.now().Sub(c.lastTry[in.ID]) < c.Interval {
		skip = true
	}
	if !skip {
		c.inflight[in.ID] = true
		c.lastTry[in.ID] = c.now()
	}
	c.mu.Unlock()
	if skip {
		return
	}
	defer func() { c.mu.Lock(); delete(c.inflight, in.ID); c.mu.Unlock() }()
	c.sample(ctx, in)
}

func (c *Collector) sample(ctx context.Context, in hub.Info) {
	cctx, cancel := context.WithTimeout(ctx, c.Timeout)
	defer cancel()
	raw, err := c.Src.Call(cctx, in.ID, Method, map[string]any{}, c.Timeout)
	if err != nil {
		var remote *hub.RemoteError
		switch {
		case errors.As(err, &remote) && isUnknownMethod(remote.Error()): // an older klark without system.stats
			c.markNoStats(in.ID)
		default: // offline, timeout, cancelled: just miss this sample
		}
		return
	}
	row, ok := ParseSample(raw)
	if !ok {
		c.markNoStats(in.ID)
		return
	}
	row.MachineID = in.ID
	row.MachineName = in.Name
	if row.MachineName == "" {
		row.MachineName = in.ID
	}
	if row.TS == 0 {
		row.TS = c.now().UnixMilli()
	}
	if err := c.Store.InsertStat(row); err != nil {
		log.Printf("stats: store sample of %s: %v", in.ID, err)
	}
}

// isUnknownMethod is a refusal that means the machine's klark does not have system.stats (not a transient
// failure such as its local server being unreachable).
func isUnknownMethod(msg string) bool {
	m := strings.ToLower(msg)
	for _, k := range []string{"unknown method", "unknown variant", "not allowed", "not allow-listed", "unsupported", "method not found"} {
		if strings.Contains(m, k) {
			return true
		}
	}
	return false
}

func (c *Collector) markNoStats(id string) {
	c.mu.Lock()
	c.noStats[id] = c.now().Add(c.RetryNoStats)
	c.mu.Unlock()
}

// AgentSample is one agent of a machine's sample.
type AgentSample struct {
	PaneID     string  `json:"paneId"`
	Name       string  `json:"name"`
	State      string  `json:"state"`
	RSSBytes   int64   `json:"rssBytes"`
	CPUPercent float64 `json:"cpuPercent"`
}

// ParseSample reads a system.stats result. Every field may be missing; ok is false when the result is not a
// JSON object or carries none of the known sections.
func ParseSample(raw json.RawMessage) (store.StatRow, bool) {
	var in struct {
		TSMs  float64 `json:"ts_ms"`
		Klark *struct {
			PID     float64 `json:"pid"`
			RSS     float64 `json:"rss_bytes"`
			PeakRSS float64 `json:"peak_rss_bytes"`
			CPU     float64 `json:"cpu_percent"`
			Uptime  float64 `json:"uptime_s"`
			Version string  `json:"version"`
		} `json:"klark"`
		Host *struct {
			MemTotal float64 `json:"mem_total_bytes"`
			MemAvail float64 `json:"mem_available_bytes"`
			Load1    float64 `json:"load1"`
			CPUs     float64 `json:"cpus"`
		} `json:"host"`
		Panes *struct {
			Count  float64 `json:"count"`
			Agents float64 `json:"agents"`
		} `json:"panes"`
		Agents []struct {
			PaneID string  `json:"pane_id"`
			Name   string  `json:"name"`
			State  string  `json:"state"`
			RSS    float64 `json:"rss_bytes"`
			CPU    float64 `json:"cpu_percent"`
		} `json:"agents"`
	}
	if json.Unmarshal(raw, &in) != nil || (in.Klark == nil && in.Host == nil && in.Panes == nil && in.Agents == nil) {
		return store.StatRow{}, false
	}
	r := store.StatRow{TS: int64(in.TSMs)}
	if k := in.Klark; k != nil {
		r.RSSBytes, r.PeakRSSBytes, r.CPUPercent, r.UptimeSeconds, r.Version = int64(k.RSS), int64(k.PeakRSS), k.CPU, int64(k.Uptime), k.Version
	}
	if h := in.Host; h != nil {
		r.MemTotalBytes, r.MemAvailableBytes, r.Load1, r.CPUs = int64(h.MemTotal), int64(h.MemAvail), h.Load1, int64(h.CPUs)
	}
	list := make([]AgentSample, 0, len(in.Agents))
	for _, a := range in.Agents {
		list = append(list, AgentSample{PaneID: a.PaneID, Name: a.Name, State: a.State, RSSBytes: int64(a.RSS), CPUPercent: a.CPU})
	}
	if p := in.Panes; p != nil {
		r.Panes, r.Agents = int64(p.Count), int64(p.Agents)
	} else {
		r.Agents = int64(len(list))
	}
	if in.Panes != nil && in.Panes.Agents == 0 && len(list) > 0 {
		r.Agents = int64(len(list))
	}
	b, _ := json.Marshal(list)
	r.AgentsJSON = string(b)
	return r, true
}
