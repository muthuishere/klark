package stats

import (
	"encoding/json"
	"sort"
	"time"

	"github.com/muthuishere/klark/hub-go/internal/store"
)

// MaxPoints is the most points a series has per machine.
const MaxPoints = 120

var ranges = map[string]time.Duration{"1h": time.Hour, "6h": 6 * time.Hour, "24h": 24 * time.Hour, "7d": 7 * 24 * time.Hour}

// RangeDuration resolves ?range= ("" = 24h); ok is false for an unknown range.
func RangeDuration(name string) (string, time.Duration, bool) {
	if name == "" {
		name = "24h"
	}
	d, ok := ranges[name]
	return name, d, ok
}

// MachineRef is a machine the report lists (known to the hub, online or not).
type MachineRef struct {
	ID, Name string
	Online   bool
}

type Latest struct {
	TS                    int64         `json:"ts"`
	KlarkRSSBytes         int64         `json:"klarkRssBytes"`
	KlarkPeakRSSBytes     int64         `json:"klarkPeakRssBytes"`
	KlarkCPUPercent       float64       `json:"klarkCpuPercent"`
	UptimeSeconds         int64         `json:"uptimeSeconds"`
	Version               string        `json:"version"`
	HostMemTotalBytes     int64         `json:"hostMemTotalBytes"`
	HostMemAvailableBytes int64         `json:"hostMemAvailableBytes"`
	Load1                 float64       `json:"load1"`
	CPUs                  int64         `json:"cpus"`
	Panes                 int64         `json:"panes"`
	Agents                int64         `json:"agents"`
	AgentList             []AgentSample `json:"agentList"`
}

type Peak struct {
	KlarkRSSBytes      int64   `json:"klarkRssBytes"`
	KlarkCPUPercent    float64 `json:"klarkCpuPercent"`
	HostMemUsedPercent float64 `json:"hostMemUsedPercent"`
	Panes              int64   `json:"panes"`
	Agents             int64   `json:"agents"`
}

type Point struct {
	T                     int64   `json:"t"`
	KlarkRSSBytes         int64   `json:"klarkRssBytes"`
	KlarkCPUPercent       float64 `json:"klarkCpuPercent"`
	HostMemAvailableBytes int64   `json:"hostMemAvailableBytes"`
	Panes                 int64   `json:"panes"`
	Agents                int64   `json:"agents"`
}

type MachineReport struct {
	ID       string  `json:"id"`
	Name     string  `json:"name"`
	Online   bool    `json:"online"`
	HasStats bool    `json:"hasStats"`
	Latest   *Latest `json:"latest"`
	Peak     Peak    `json:"peak"`
	Series   []Point `json:"series"`
}

type Totals struct {
	Machines      int   `json:"machines"`
	Online        int   `json:"online"`
	Panes         int64 `json:"panes"`
	Agents        int64 `json:"agents"`
	KlarkRSSBytes int64 `json:"klarkRssBytes"`
}

type Report struct {
	Range           string          `json:"range"`
	IntervalSeconds int             `json:"intervalSeconds"` // how often the hub samples
	BucketSeconds   int             `json:"bucketSeconds"`   // width of one series point (0 = raw samples)
	Totals          Totals          `json:"totals"`
	Machines        []MachineReport `json:"machines"`
	Hub             *HubReport      `json:"hub"` // the hub process itself (machine id "hub" in the store)
}

func latestOf(r store.StatRow) *Latest {
	l := &Latest{TS: r.TS, KlarkRSSBytes: r.RSSBytes, KlarkPeakRSSBytes: r.PeakRSSBytes, KlarkCPUPercent: r.CPUPercent,
		UptimeSeconds: r.UptimeSeconds, Version: r.Version, HostMemTotalBytes: r.MemTotalBytes, HostMemAvailableBytes: r.MemAvailableBytes,
		Load1: r.Load1, CPUs: r.CPUs, Panes: r.Panes, Agents: r.Agents, AgentList: []AgentSample{}}
	_ = json.Unmarshal([]byte(r.AgentsJSON), &l.AgentList)
	if l.AgentList == nil {
		l.AgentList = []AgentSample{}
	}
	return l
}

// Build is the admin analytics report: machines are the hub's known machines; machines that only have history
// in the range are added.
func Build(st *store.Store, machines []MachineRef, rangeName string, d time.Duration, interval time.Duration, now time.Time) (Report, error) {
	from, to := now.Add(-d).UnixMilli(), now.UnixMilli()+1
	rows, err := st.StatsBetween(from, to)
	if err != nil {
		return Report{}, err
	}
	by := map[string][]store.StatRow{}
	var hubRows []store.StatRow
	for _, r := range rows {
		if r.MachineID == HubID {
			hubRows = append(hubRows, r)
			continue
		}
		by[r.MachineID] = append(by[r.MachineID], r)
	}
	refs := append([]MachineRef(nil), machines...)
	known := map[string]bool{}
	for _, m := range refs {
		known[m.ID] = true
	}
	for id, rs := range by {
		if !known[id] {
			refs = append(refs, MachineRef{ID: id, Name: rs[len(rs)-1].MachineName})
		}
	}
	sort.Slice(refs, func(i, j int) bool { return refs[i].Name+"\x00"+refs[i].ID < refs[j].Name+"\x00"+refs[j].ID })

	rep := Report{Range: rangeName, IntervalSeconds: int(interval / time.Second), Machines: []MachineReport{}}
	bucket := d / MaxPoints
	if rep.Hub, err = buildHub(st, hubRows, from, bucket); err != nil {
		return rep, err
	}
	for _, ref := range refs {
		mr := MachineReport{ID: ref.ID, Name: ref.Name, Online: ref.Online, Series: []Point{}}
		if mr.Name == "" {
			mr.Name = ref.ID
		}
		rs := by[ref.ID]
		last, ok, err := st.LatestStat(ref.ID)
		if err != nil {
			return rep, err
		}
		if ok {
			mr.HasStats, mr.Latest = true, latestOf(last)
		}
		mr.Peak = peakOf(rs)
		var raw bool
		mr.Series, raw = downsample(rs, from, bucket)
		if !raw && len(rs) > 0 {
			rep.BucketSeconds = int(bucket / time.Second)
		}
		rep.Machines = append(rep.Machines, mr)
		rep.Totals.Machines++
		if ref.Online {
			rep.Totals.Online++
			if mr.Latest != nil {
				rep.Totals.Panes += mr.Latest.Panes
				rep.Totals.Agents += mr.Latest.Agents
				rep.Totals.KlarkRSSBytes += mr.Latest.KlarkRSSBytes
			}
		}
	}
	return rep, nil
}

func peakOf(rs []store.StatRow) Peak {
	var p Peak
	for _, r := range rs {
		p.KlarkRSSBytes = max(p.KlarkRSSBytes, r.RSSBytes)
		p.KlarkCPUPercent = max(p.KlarkCPUPercent, r.CPUPercent)
		p.Panes = max(p.Panes, r.Panes)
		p.Agents = max(p.Agents, r.Agents)
		if r.MemTotalBytes > 0 {
			used := float64(r.MemTotalBytes-r.MemAvailableBytes) / float64(r.MemTotalBytes) * 100
			p.HostMemUsedPercent = max(p.HostMemUsedPercent, used)
		}
	}
	return p
}

// downsample keeps the samples as they are when there are at most MaxPoints, else folds them into buckets of
// width bucket: the max of rss, cpu, panes and agents, and the lowest available memory (the worst moment).
// raw reports that the samples were returned unchanged.
func downsample(rs []store.StatRow, from int64, bucket time.Duration) ([]Point, bool) {
	pts := make([]Point, 0, min(len(rs), MaxPoints))
	if len(rs) <= MaxPoints {
		for _, r := range rs {
			pts = append(pts, Point{T: r.TS, KlarkRSSBytes: r.RSSBytes, KlarkCPUPercent: r.CPUPercent, HostMemAvailableBytes: r.MemAvailableBytes, Panes: r.Panes, Agents: r.Agents})
		}
		return pts, true
	}
	bms := bucket.Milliseconds()
	if bms < 1 {
		bms = 1
	}
	cur := int64(-1)
	for _, r := range rs {
		b := (r.TS - from) / bms
		b = min(max(b, 0), MaxPoints-1)
		if b != cur || len(pts) == 0 {
			cur = b
			pts = append(pts, Point{T: r.TS, KlarkRSSBytes: r.RSSBytes, KlarkCPUPercent: r.CPUPercent, HostMemAvailableBytes: r.MemAvailableBytes, Panes: r.Panes, Agents: r.Agents})
			continue
		}
		p := &pts[len(pts)-1]
		p.T = r.TS
		p.KlarkRSSBytes = max(p.KlarkRSSBytes, r.RSSBytes)
		p.KlarkCPUPercent = max(p.KlarkCPUPercent, r.CPUPercent)
		p.HostMemAvailableBytes = minNonZero(p.HostMemAvailableBytes, r.MemAvailableBytes)
		p.Panes = max(p.Panes, r.Panes)
		p.Agents = max(p.Agents, r.Agents)
	}
	return pts, false
}

func minNonZero(a, b int64) int64 {
	switch {
	case a == 0:
		return b
	case b == 0:
		return a
	}
	return min(a, b)
}
