package stats

import (
	"bytes"
	"compress/gzip"
	"encoding/json"
	"io"
	"runtime"
	"strings"
	"testing"
	"time"
)

func TestMaxrssUnits(t *testing.T) {
	got := maxrssBytes(2048)
	want := int64(2048 * 1024)
	if runtime.GOOS == "darwin" {
		want = 2048
	}
	if got != want {
		t.Fatalf("maxrss %d, want %d", got, want)
	}
}

func TestHubSamplerIsLazyAndKeptApartFromMachines(t *testing.T) {
	st := newStore(t)
	clock := time.Date(2026, 1, 1, 10, 0, 0, 0, time.UTC)
	h := &HubSampler{Store: st, Interval: 30 * time.Second, Version: "v1", Now: func() time.Time { return clock }, Started: clock.Add(-time.Minute),
		Counters: func() HubCounters { return HubCounters{MachineLinks: 2, SSEStreams: 3, PaneViewers: 1, Requests: 60, AssetsBytes: 7} }}
	if !h.Sample() || h.Sample() {
		t.Fatal("first sample stored, second within the interval skipped")
	}
	clock = clock.Add(31 * time.Second)
	if !h.Sample() {
		t.Fatal("sample after the interval")
	}
	rep, err := Build(st, []MachineRef{{ID: "m", Name: "M", Online: true}}, "1h", time.Hour, 30*time.Second, clock)
	if err != nil {
		t.Fatal(err)
	}
	if len(rep.Machines) != 1 || rep.Machines[0].ID != "m" {
		t.Fatalf("hub must not be listed as a machine: %+v", rep.Machines)
	}
	if rep.Hub == nil || rep.Hub.Latest == nil || len(rep.Hub.Series) != 2 {
		t.Fatalf("hub report %+v", rep.Hub)
	}
	l := rep.Hub.Latest
	if l.Version != "v1" || l.MachineLinks != 2 || l.SSEStreams != 3 || l.PaneViewers != 1 || l.AssetsBytes != 7 || l.UptimeSeconds != 91 {
		t.Fatalf("latest %+v", l)
	}
	if l.Goroutines <= 0 || l.HeapInuseBytes <= 0 || rep.Hub.Peak.Goroutines <= 0 || rep.Hub.Series[1].Streams != 3 {
		t.Fatalf("runtime numbers missing: %+v %+v", l, rep.Hub)
	}
	if runtime.GOOS == "linux" || runtime.GOOS == "darwin" {
		if l.RSSBytes <= 0 || l.PeakRSSBytes < l.RSSBytes {
			t.Fatalf("rss %d peak %d", l.RSSBytes, l.PeakRSSBytes)
		}
	}
	if l.RequestsPerMinute != 0 { // 60 requests, none new since the previous sample
		t.Fatalf("rate %v", l.RequestsPerMinute)
	}

	rows, _ := st.StatsBetween(0, clock.UnixMilli()+1)
	data, err := EncodeHour("h1", rows)
	if err != nil {
		t.Fatal(err)
	}
	zr, _ := gzip.NewReader(bytes.NewReader(data))
	raw, _ := io.ReadAll(zr)
	line := strings.Split(strings.TrimSpace(string(raw)), "\n")[0]
	var obj map[string]any
	_ = json.Unmarshal([]byte(line), &obj)
	if obj["kind"] != "hub" || obj["machine_links"] != float64(2) {
		t.Fatalf("archive line %s", line)
	}
}
