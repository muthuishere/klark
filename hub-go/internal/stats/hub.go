package stats

import (
	"encoding/json"
	"log"
	"runtime"
	"sync"
	"time"

	"github.com/muthuishere/klark/hub-go/internal/store"
)

// HubID is the reserved machine id under which the hub's own samples live in machine_stats (same retention,
// same archive). A hub row reuses the columns as: rss/peak/cpu/uptime/version as named, mem_available_bytes =
// Go heap in use, panes = goroutines, agents = open SSE streams, agents_json = the whole HubLatest object.
const HubID = "hub"

// HubCounters is what the app knows about itself that the stats package cannot see.
type HubCounters struct {
	MachineLinks         int64
	SSEStreams           int64
	PaneViewers          int64 // open frame (terminal) subscriptions
	Requests             int64 // total HTTP requests since start
	DBBytes, DBWALBytes  int64
	AssetsBytes          int64
	LastBackupMs         int64
	LastAnalyticsWriteMs int64
}

// HubLatest is one sample of the hub process.
type HubLatest struct {
	TS                   int64   `json:"ts"`
	RSSBytes             int64   `json:"rssBytes"` // 0 = not available on this OS
	PeakRSSBytes         int64   `json:"peakRssBytes"`
	CPUPercent           float64 `json:"cpuPercent"` // since the previous sample (since start for the first)
	HeapInuseBytes       int64   `json:"heapInuseBytes"`
	HeapSysBytes         int64   `json:"heapSysBytes"`
	SysBytes             int64   `json:"sysBytes"`
	Goroutines           int64   `json:"goroutines"`
	GCCount              int64   `json:"gcCount"`
	GCPauseTotalMs       float64 `json:"gcPauseTotalMs"`
	GCLastPauseMs        float64 `json:"gcLastPauseMs"`
	UptimeSeconds        int64   `json:"uptimeSeconds"`
	Version              string  `json:"version"`
	GoVersion            string  `json:"goVersion"`
	OS                   string  `json:"os"`
	MachineLinks         int64   `json:"machineLinks"`
	SSEStreams           int64   `json:"sseStreams"`
	PaneViewers          int64   `json:"paneViewers"`
	Requests             int64   `json:"requests"`
	RequestsPerMinute    float64 `json:"requestsPerMinute"` // since the previous sample
	DBBytes              int64   `json:"dbBytes"`
	DBWALBytes           int64   `json:"dbWalBytes"`
	AssetsBytes          int64   `json:"assetsBytes"`
	LastBackupMs         int64   `json:"lastBackupMs"`
	LastAnalyticsWriteMs int64   `json:"lastAnalyticsWriteMs"`
}

type HubPeak struct {
	RSSBytes   int64   `json:"rssBytes"`
	CPUPercent float64 `json:"cpuPercent"`
	Goroutines int64   `json:"goroutines"`
	HeapBytes  int64   `json:"heapBytes"`
}

type HubPoint struct {
	T          int64   `json:"t"`
	RSSBytes   int64   `json:"rssBytes"`
	CPUPercent float64 `json:"cpuPercent"`
	HeapBytes  int64   `json:"heapBytes"`
	Goroutines int64   `json:"goroutines"`
	Streams    int64   `json:"streams"`
}

type HubReport struct {
	Latest *HubLatest `json:"latest"`
	Peak   HubPeak    `json:"peak"`
	Series []HubPoint `json:"series"`
}

// HubSampler samples the hub process lazily (when the analytics page is served), at most once per Interval.
type HubSampler struct {
	Store    *store.Store
	Interval time.Duration
	Version  string
	Counters func() HubCounters // may be nil
	Now      func() time.Time
	Started  time.Time // process start; zero = when the sampler is first used

	mu       sync.Mutex
	last     time.Time
	lastCPU  time.Duration
	lastReqs int64
}

func (h *HubSampler) now() time.Time {
	if h.Now != nil {
		return h.Now()
	}
	return time.Now()
}

// Sample stores a sample unless one was taken within Interval; it reports whether it stored one.
func (h *HubSampler) Sample() bool {
	h.mu.Lock()
	defer h.mu.Unlock()
	now := h.now()
	if h.Started.IsZero() {
		h.Started = now
	}
	iv := h.Interval
	if iv <= 0 {
		iv = 30 * time.Second
	}
	if !h.last.IsZero() && now.Sub(h.last) < iv {
		return false
	}
	l := h.measure(now)
	if err := h.Store.InsertStat(hubRow(l)); err != nil {
		log.Printf("stats: store hub sample: %v", err)
		return false
	}
	return true
}

func (h *HubSampler) measure(now time.Time) HubLatest {
	var ms runtime.MemStats
	runtime.ReadMemStats(&ms)
	cpu, rss, peak := procUsage()
	var c HubCounters
	if h.Counters != nil {
		c = h.Counters()
	}
	l := HubLatest{TS: now.UnixMilli(), RSSBytes: rss, PeakRSSBytes: max(peak, rss),
		HeapInuseBytes: int64(ms.HeapInuse), HeapSysBytes: int64(ms.HeapSys), SysBytes: int64(ms.Sys),
		Goroutines: int64(runtime.NumGoroutine()), GCCount: int64(ms.NumGC), GCPauseTotalMs: float64(ms.PauseTotalNs) / 1e6,
		UptimeSeconds: int64(now.Sub(h.Started) / time.Second), Version: h.Version, GoVersion: runtime.Version(), OS: runtime.GOOS + "/" + runtime.GOARCH,
		MachineLinks: c.MachineLinks, SSEStreams: c.SSEStreams, PaneViewers: c.PaneViewers, Requests: c.Requests,
		DBBytes: c.DBBytes, DBWALBytes: c.DBWALBytes, AssetsBytes: c.AssetsBytes, LastBackupMs: c.LastBackupMs, LastAnalyticsWriteMs: c.LastAnalyticsWriteMs}
	if ms.NumGC > 0 {
		l.GCLastPauseMs = float64(ms.PauseNs[(ms.NumGC+255)%256]) / 1e6
	}
	since, prevCPU, prevReqs := now.Sub(h.Started), time.Duration(0), int64(0)
	if !h.last.IsZero() {
		since, prevCPU, prevReqs = now.Sub(h.last), h.lastCPU, h.lastReqs
	}
	if since > 0 {
		l.CPUPercent = float64(cpu-prevCPU) / float64(since) * 100
		l.RequestsPerMinute = float64(c.Requests-prevReqs) / since.Minutes()
	}
	h.last, h.lastCPU, h.lastReqs = now, cpu, c.Requests
	return l
}

func hubRow(l HubLatest) store.StatRow {
	b, _ := json.Marshal(l)
	return store.StatRow{MachineID: HubID, MachineName: HubID, TS: l.TS, RSSBytes: l.RSSBytes, PeakRSSBytes: l.PeakRSSBytes,
		CPUPercent: l.CPUPercent, UptimeSeconds: l.UptimeSeconds, Version: l.Version,
		MemAvailableBytes: l.HeapInuseBytes, Panes: l.Goroutines, Agents: l.SSEStreams, AgentsJSON: string(b)}
}

func hubLatestOf(r store.StatRow) *HubLatest {
	l := &HubLatest{}
	if json.Unmarshal([]byte(r.AgentsJSON), l) != nil || l.TS == 0 { // fall back to the columns
		*l = HubLatest{TS: r.TS, RSSBytes: r.RSSBytes, PeakRSSBytes: r.PeakRSSBytes, CPUPercent: r.CPUPercent, UptimeSeconds: r.UptimeSeconds,
			Version: r.Version, HeapInuseBytes: r.MemAvailableBytes, Goroutines: r.Panes, SSEStreams: r.Agents}
	}
	return l
}

func buildHub(st *store.Store, rs []store.StatRow, from int64, bucket time.Duration) (*HubReport, error) {
	hr := &HubReport{Series: []HubPoint{}}
	last, ok, err := st.LatestStat(HubID)
	if err != nil {
		return nil, err
	}
	if ok {
		hr.Latest = hubLatestOf(last)
	}
	pt := func(r store.StatRow) HubPoint {
		return HubPoint{T: r.TS, RSSBytes: r.RSSBytes, CPUPercent: r.CPUPercent, HeapBytes: r.MemAvailableBytes, Goroutines: r.Panes, Streams: r.Agents}
	}
	bms := max(bucket.Milliseconds(), 1)
	cur := int64(-1)
	for _, r := range rs {
		hr.Peak.RSSBytes = max(hr.Peak.RSSBytes, r.RSSBytes)
		hr.Peak.CPUPercent = max(hr.Peak.CPUPercent, r.CPUPercent)
		hr.Peak.Goroutines = max(hr.Peak.Goroutines, r.Panes)
		hr.Peak.HeapBytes = max(hr.Peak.HeapBytes, r.MemAvailableBytes)
		b := min(max((r.TS-from)/bms, 0), MaxPoints-1)
		if len(rs) <= MaxPoints || b != cur || len(hr.Series) == 0 {
			cur = b
			hr.Series = append(hr.Series, pt(r))
			continue
		}
		p, q := &hr.Series[len(hr.Series)-1], pt(r)
		p.T = q.T
		p.RSSBytes, p.CPUPercent, p.HeapBytes = max(p.RSSBytes, q.RSSBytes), max(p.CPUPercent, q.CPUPercent), max(p.HeapBytes, q.HeapBytes)
		p.Goroutines, p.Streams = max(p.Goroutines, q.Goroutines), max(p.Streams, q.Streams)
	}
	return hr, nil
}
