package stats

import (
	"bytes"
	"compress/gzip"
	"context"
	"encoding/json"
	"errors"
	"io"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/muthuishere/klark/hub-go/internal/config"
	"github.com/muthuishere/klark/hub-go/internal/hub"
	"github.com/muthuishere/klark/hub-go/internal/store"
)

type fakeSrc struct {
	mu    sync.Mutex
	infos []hub.Info
	calls map[string]int
	reply func(id string) (json.RawMessage, error)
}

func (f *fakeSrc) Machines() []hub.Info { return f.infos }
func (f *fakeSrc) Call(_ context.Context, id, method string, _ any, _ time.Duration) (json.RawMessage, error) {
	f.mu.Lock()
	if f.calls == nil {
		f.calls = map[string]int{}
	}
	f.calls[id]++
	f.mu.Unlock()
	if method != Method {
		return nil, errors.New("wrong method")
	}
	return f.reply(id)
}

func newStore(t *testing.T) *store.Store {
	t.Helper()
	st, err := store.OpenPath(filepath.Join(t.TempDir(), "hub.db"))
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = st.Close() })
	return st
}

const sampleJSON = `{"ts_ms":%d,"klark":{"pid":1,"rss_bytes":104857600,"peak_rss_bytes":209715200,"cpu_percent":12.5,"uptime_s":99,"version":"0.9"},
"host":{"mem_total_bytes":1000,"mem_available_bytes":250,"load1":0.5,"cpus":8},"panes":{"count":3,"agents":1},
"agents":[{"pane_id":"p1","name":"rev","state":"working","rss_bytes":5000,"cpu_percent":3}]}`

func TestCollectorSamplesOnlineMachinesAndRemembersNoStats(t *testing.T) {
	st := newStore(t)
	clock := time.Date(2026, 1, 1, 10, 0, 0, 0, time.UTC)
	src := &fakeSrc{infos: []hub.Info{{ID: "a", Name: "Alpha", Online: true}, {ID: "old", Name: "Old", Online: true}, {ID: "off", Name: "Off", Online: false}},
		reply: func(id string) (json.RawMessage, error) {
			if id == "old" {
				return nil, &hub.RemoteError{Msg: "unknown method system.stats"}
			}
			return json.RawMessage(strings.Replace(sampleJSON, "%d", "0", 1)), nil
		}}
	c := &Collector{Src: src, Store: st, Now: func() time.Time { return clock }}
	c.CollectOnce(context.Background())
	c.CollectOnce(context.Background())
	if src.calls["off"] != 0 || src.calls["old"] != 1 || src.calls["a"] != 2 {
		t.Fatalf("calls %v: offline never asked, stat-less asked once then remembered", src.calls)
	}
	if !c.HasNoStats("old") || c.HasNoStats("a") {
		t.Fatal("no-stats memory")
	}
	row, ok, _ := st.LatestStat("a")
	if !ok || row.MachineName != "Alpha" || row.RSSBytes != 104857600 || row.PeakRSSBytes != 209715200 || row.CPUPercent != 12.5 ||
		row.MemAvailableBytes != 250 || row.Panes != 3 || row.Agents != 1 || row.TS != clock.UnixMilli() || !strings.Contains(row.AgentsJSON, `"paneId":"p1"`) {
		t.Fatalf("%+v", row)
	}
	if _, ok, _ := st.LatestStat("old"); ok {
		t.Fatal("stat-less machine stored a row")
	}
	// retried after 10 minutes
	clock = clock.Add(11 * time.Minute)
	c.CollectOnce(context.Background())
	if src.calls["old"] != 2 {
		t.Fatalf("retry after 10 min: %v", src.calls)
	}
}

func TestCollectorTimeoutOnlyMissesTheSampleAndMissingFieldsParse(t *testing.T) {
	st := newStore(t)
	src := &fakeSrc{infos: []hub.Info{{ID: "slow", Online: true}, {ID: "min", Online: true}}, reply: func(id string) (json.RawMessage, error) {
		if id == "slow" {
			return nil, hub.ErrTimeout
		}
		return json.RawMessage(`{"klark":{"rss_bytes":7}}`), nil
	}}
	c := &Collector{Src: src, Store: st}
	c.CollectOnce(context.Background())
	if c.HasNoStats("slow") {
		t.Fatal("a timeout must not blacklist a machine")
	}
	r, ok, _ := st.LatestStat("min")
	if !ok || r.RSSBytes != 7 || r.MachineName != "min" || r.TS == 0 || r.AgentsJSON != "[]" {
		t.Fatalf("%+v", r)
	}
	if _, ok := ParseSample(json.RawMessage(`{"pong":true}`)); ok {
		t.Fatal("not a stats result")
	}
}

func TestRetentionPurge(t *testing.T) {
	st := newStore(t)
	now := time.Date(2026, 1, 20, 0, 0, 0, 0, time.UTC)
	for _, d := range []int{20, 15, 13, 1} {
		_ = st.InsertStat(store.StatRow{MachineID: "a", TS: now.AddDate(0, 0, -d).UnixMilli()})
	}
	c := &Collector{Src: &fakeSrc{}, Store: st, Now: func() time.Time { return now }, Retention: 14 * 24 * time.Hour}
	if n := c.Purge(); n != 2 {
		t.Fatalf("purged %d", n)
	}
	rows, _ := st.StatsBetween(0, now.UnixMilli()+1)
	if len(rows) != 2 {
		t.Fatalf("%d left", len(rows))
	}
}

func TestBuildDownsamplesPeaksAndLatest(t *testing.T) {
	st := newStore(t)
	now := time.Date(2026, 1, 2, 0, 0, 0, 0, time.UTC)
	// 24 h at 30 s = 2880 samples, one spike
	for i := 0; i < 2880; i++ {
		ts := now.Add(-24*time.Hour + time.Duration(i)*30*time.Second + time.Second).UnixMilli()
		rss := int64(100)
		if i == 1000 {
			rss = 9999
		}
		_ = st.InsertStat(store.StatRow{MachineID: "a", MachineName: "Alpha", TS: ts, RSSBytes: rss, CPUPercent: float64(i % 50), MemTotalBytes: 1000, MemAvailableBytes: 400, Panes: 2, Agents: 1, AgentsJSON: `[{"paneId":"p","name":"n","state":"idle","rssBytes":1,"cpuPercent":0}]`})
	}
	_ = st.InsertStat(store.StatRow{MachineID: "gone", MachineName: "Gone", TS: now.Add(-time.Hour).UnixMilli(), RSSBytes: 5})
	name, d, ok := RangeDuration("")
	if !ok || name != "24h" {
		t.Fatal("default range")
	}
	rep, err := Build(st, []MachineRef{{ID: "a", Name: "Alpha", Online: true}, {ID: "none", Name: "NoStats", Online: true}}, name, d, 30*time.Second, now)
	if err != nil {
		t.Fatal(err)
	}
	if len(rep.Machines) != 3 || rep.Totals.Machines != 3 || rep.Totals.Online != 2 {
		t.Fatalf("%+v", rep.Totals)
	}
	var a, none, gone MachineReport
	for _, m := range rep.Machines {
		switch m.ID {
		case "a":
			a = m
		case "none":
			none = m
		case "gone":
			gone = m
		}
	}
	if len(a.Series) == 0 || len(a.Series) > 120 || rep.BucketSeconds != 720 {
		t.Fatalf("points %d bucket %d", len(a.Series), rep.BucketSeconds)
	}
	var spike bool
	for _, p := range a.Series {
		spike = spike || p.KlarkRSSBytes == 9999
	}
	if !spike || a.Peak.KlarkRSSBytes != 9999 || a.Peak.KlarkCPUPercent != 49 || a.Peak.HostMemUsedPercent != 60 || a.Peak.Agents != 1 {
		t.Fatalf("bucket max must keep the spike: %+v", a.Peak)
	}
	if !a.HasStats || a.Latest == nil || len(a.Latest.AgentList) != 1 || a.Latest.AgentList[0].State != "idle" {
		t.Fatalf("latest %+v", a.Latest)
	}
	if none.HasStats || none.Latest != nil || len(none.Series) != 0 || gone.Name != "Gone" || gone.Online {
		t.Fatal("stat-less and history-only machines")
	}
	if rep.Totals.KlarkRSSBytes != 100 || rep.Totals.Panes != 2 || rep.Totals.Agents != 1 {
		t.Fatalf("totals %+v", rep.Totals)
	}
	// a short range returns raw samples
	_, d1, _ := RangeDuration("1h")
	rep, _ = Build(st, nil, "1h", d1, 30*time.Second, now)
	for _, m := range rep.Machines {
		if m.ID == "a" && len(m.Series) != 120 {
			t.Fatalf("1h = %d samples", len(m.Series))
		}
	}
	if _, _, ok := RangeDuration("2d"); ok {
		t.Fatal("bad range accepted")
	}
}

func gunzipLines(t *testing.T, b []byte) []map[string]any {
	t.Helper()
	zr, err := gzip.NewReader(bytes.NewReader(b))
	if err != nil {
		t.Fatal(err)
	}
	raw, _ := io.ReadAll(zr)
	var out []map[string]any
	for _, l := range strings.Split(strings.TrimSpace(string(raw)), "\n") {
		var m map[string]any
		if err := json.Unmarshal([]byte(l), &m); err != nil {
			t.Fatal(err)
		}
		out = append(out, m)
	}
	return out
}

func TestArchiveToFolderIsReadableAndIdempotent(t *testing.T) {
	st := newStore(t)
	dir := filepath.Join(t.TempDir(), "arch", "deep")
	now := time.Date(2026, 3, 4, 15, 20, 0, 0, time.UTC)
	for i := 0; i < 3; i++ {
		_ = st.InsertStat(store.StatRow{MachineID: "a", MachineName: "Alpha", TS: now.Add(-time.Duration(i) * time.Minute).UnixMilli(), RSSBytes: int64(100 + i), AgentsJSON: `[]`})
	}
	_ = st.InsertStat(store.StatRow{MachineID: "a", MachineName: "Alpha", TS: now.Add(-2 * time.Hour).UnixMilli(), RSSBytes: 1})
	ar := &Archiver{Store: st, Now: func() time.Time { return now }, Config: func() config.Storage { return config.Storage{Kind: "folder", Path: dir} }}
	ar.Cycle(context.Background())
	hub := ar.HubID()
	cur := filepath.Join(dir, "analytics", "2026", "03", "04", "15-"+hub+".ndjson.gz")
	b, err := os.ReadFile(cur)
	if err != nil {
		t.Fatal(err)
	}
	lines := gunzipLines(t, b)
	if len(lines) != 3 || lines[0]["machine"] != "Alpha" || lines[0]["hub"] != hub || lines[0]["klark_rss_bytes"] == nil || lines[0]["ts"] == nil {
		t.Fatalf("%v", lines)
	}
	if _, err := os.Stat(filepath.Join(dir, "analytics", "2026", "03", "04", "13-"+hub+".ndjson.gz")); err != nil {
		t.Fatal("the earlier hour was backfilled")
	}
	// a re-run of the same hour (even from a fresh process: no memory) gives the same bytes in the same object
	ar2 := &Archiver{Store: st, Now: func() time.Time { return now }, Config: ar.Config}
	ar2.ResetCursor()
	ar2.Cycle(context.Background())
	b2, _ := os.ReadFile(cur)
	if !bytes.Equal(b, b2) {
		t.Fatal("re-running an hour must overwrite the same object with the same bytes")
	}
	entries, _ := os.ReadDir(filepath.Dir(cur))
	if len(entries) != 2 {
		t.Fatalf("%d files in the day folder (no temp leftovers, no duplicates)", len(entries))
	}
	if s := ar.Status(); s.FilesWritten != 2 || s.LastWriteMs == 0 || s.LastError != "" {
		t.Fatalf("%+v", s)
	}
	// nothing new: no rewrite
	ar.Cycle(context.Background())
	if ar.Status().FilesWritten != 2 {
		t.Fatal("unchanged hour rewritten")
	}
	// folder sink never writes outside
	fs := &FolderSink{Root: dir}
	if err := fs.Put(context.Background(), "../escape", []byte("x")); err == nil {
		t.Fatal("escape allowed")
	}
	if ok, msg := ar.Test(context.Background(), config.Storage{Kind: "folder", Path: dir}); !ok {
		t.Fatal(msg)
	}
	if ok, _ := ar.Test(context.Background(), config.Storage{Kind: "none"}); ok {
		t.Fatal("none must not test ok")
	}
}

func TestArchiveErrorIsRecordedAndRetried(t *testing.T) {
	st := newStore(t)
	now := time.Date(2026, 3, 4, 15, 20, 0, 0, time.UTC)
	_ = st.InsertStat(store.StatRow{MachineID: "a", TS: now.UnixMilli()})
	bad := true
	ar := &Archiver{Store: st, Now: func() time.Time { return now }, Config: func() config.Storage { return config.Storage{Kind: "s3"} },
		MakeSink: func(config.Storage) (Sink, error) {
			if bad {
				return nil, errors.New("down")
			}
			return &FolderSink{Root: t.TempDir()}, nil
		}}
	ar.Cycle(context.Background())
	if ar.Status().LastError != "down" {
		t.Fatalf("%+v", ar.Status())
	}
	bad = false
	ar.Cycle(context.Background())
	if s := ar.Status(); s.LastError != "" || s.FilesWritten != 1 {
		t.Fatalf("%+v", s)
	}
}
