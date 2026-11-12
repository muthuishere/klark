package app

import (
	"context"
	"net/http"
	"time"

	"github.com/muthuishere/klark/hub-go/internal/config"
	"github.com/muthuishere/klark/hub-go/internal/stats"
)

// StartBackground runs the stats collector, the archive writer and the registry sweep until ctx ends.
func (a *App) StartBackground(ctx context.Context) {
	go a.Registry.Run(ctx)
	if a.Cluster != nil {
		go a.Cluster.Run(ctx)
	}
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
	a.Archive = &stats.Archiver{Store: a.Store, Config: func() config.Storage { c, _ := a.storageConfig(); return c }}
}

func (a *App) adminAnalytics(w http.ResponseWriter, r *http.Request) {
	name, d, ok := stats.RangeDuration(r.URL.Query().Get("range"))
	if !ok {
		jsonErr(w, 400, "range must be 1h, 6h, 24h or 7d")
		return
	}
	// Lazy collection: an open analytics page is what makes the hub ask the machines (at most once per interval each).
	// With several pods each pod samples only its own machines: ask the others to do the same, in parallel.
	if a.Cluster != nil && !isForwarded(r) {
		go a.Cluster.refreshRemote(r.Context())
	}
	a.Stats.Refresh(r.Context(), 2*time.Second)
	var refs []stats.MachineRef
	have := map[string]bool{}
	for _, in := range a.Runtime.Machines() {
		refs = append(refs, stats.MachineRef{ID: in.ID, Name: in.Name, Online: in.Online})
		have[in.ID] = true
	}
	if a.Cluster != nil {
		for _, p := range a.Cluster.others() {
			if !have[p.MachineID] {
				refs = append(refs, stats.MachineRef{ID: p.MachineID, Name: firstOf(p.Name, p.MachineID), Online: true})
			}
		}
	}
	rep, err := stats.Build(a.Store, refs, name, d, a.Stats.Interval, time.Now())
	if err != nil {
		jsonErr(w, 500, err.Error())
		return
	}
	jsonOut(w, 200, rep)
}
