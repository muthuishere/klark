package app

import (
	"context"
	"net/http"
	"os"
	"path/filepath"
	"strings"
	"sync/atomic"
	"time"

	"github.com/muthuishere/klark/hub-go/internal/config"
	"github.com/muthuishere/klark/hub-go/internal/stats"
)

// StartBackground runs the stats collector, the archive writer and the registry sweep until ctx ends.
func (a *App) StartBackground(ctx context.Context) {
	go a.Registry.Run(ctx)
	go a.Stats.Run(ctx) // only purges old samples; it never calls a machine
	go a.Archive.Run(ctx)
	go a.sweepAssets(ctx)
	if a.Backup != nil {
		go a.Backup.Run(ctx)
	}
}

func (a *App) initStats() {
	a.Stats = &stats.Collector{Src: a.Runtime, Store: a.Store,
		Interval:  time.Duration(a.Cfg.StatsIntervalSeconds) * time.Second,
		Retention: time.Duration(a.Cfg.StatsRetentionDays) * 24 * time.Hour}
	prev := a.Runtime.OnSeen
	a.Runtime.OnSeen = func(id, version string, protocol int) {
		if prev != nil {
			prev(id, version, protocol)
		}
		a.Stats.OnConnect(id) // the one sample a machine gets when it links
	}
	a.HubStats = &stats.HubSampler{Store: a.Store, Interval: a.Stats.Interval, Version: Version, Started: started, Counters: a.hubCounters}
	a.Archive = &stats.Archiver{Store: a.Store, Config: func() config.Storage { c, _ := a.storageConfig(); return c }}
}

func (a *App) adminAnalytics(w http.ResponseWriter, r *http.Request) {
	name, d, ok := stats.RangeDuration(r.URL.Query().Get("range"))
	if !ok {
		jsonErr(w, 400, "range must be 1h, 6h, 24h or 7d")
		return
	}
	// Lazy collection: an open analytics page is what makes the hub ask the machines (at most once per interval each).
	a.Stats.Refresh(r.Context(), 2*time.Second)
	a.HubStats.Sample() // the hub's own sample, same lazy rule
	var refs []stats.MachineRef
	for _, in := range a.Runtime.Machines() {
		refs = append(refs, stats.MachineRef{ID: in.ID, Name: in.Name, Online: in.Online})
	}
	rep, err := stats.Build(a.Store, refs, name, d, a.Stats.Interval, time.Now())
	if err != nil {
		jsonErr(w, 500, err.Error())
		return
	}
	jsonOut(w, 200, rep)
}

// started is when this process started (close enough: package init).
var started = time.Now()

// hubTraffic counts requests and open streams for the hub's own analytics (cheap atomics, no locks).
type hubTraffic struct{ requests, streams, viewers atomic.Int64 }

// countTraffic wraps the routes: every request is counted; an SSE stream (…/events) is open while it is served,
// and one with ?frames=1 is also a pane viewer (a terminal subscription).
func (a *App) countTraffic(h http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		a.traffic.requests.Add(1)
		if strings.HasSuffix(r.URL.Path, "/events") {
			a.traffic.streams.Add(1)
			defer a.traffic.streams.Add(-1)
			if r.URL.Query().Get("frames") == "1" {
				a.traffic.viewers.Add(1)
				defer a.traffic.viewers.Add(-1)
			}
		}
		h.ServeHTTP(w, r)
	})
}

func (a *App) hubCounters() stats.HubCounters {
	c := stats.HubCounters{Requests: a.traffic.requests.Load(), SSEStreams: a.traffic.streams.Load(), PaneViewers: a.traffic.viewers.Load()}
	for _, in := range a.Runtime.Machines() {
		if in.Online {
			c.MachineLinks++
		}
	}
	db := filepath.Join(a.Cfg.DataDir, "hub.db")
	if fi, err := os.Stat(db); err == nil {
		c.DBBytes = fi.Size()
	}
	if fi, err := os.Stat(db + "-wal"); err == nil {
		c.DBWALBytes = fi.Size()
	}
	c.AssetsBytes, _ = a.Store.AssetBytes()
	if a.Backup != nil {
		c.LastBackupMs = a.Backup.Status().LastBackupMs
	}
	if a.Archive != nil {
		c.LastAnalyticsWriteMs = a.Archive.Status().LastWriteMs
	}
	return c
}
