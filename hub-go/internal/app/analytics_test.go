package app

import (
	"context"
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/muthuishere/klark/hub-go/internal/config"
)

func statsBehaviour(m *fakeMachine, msg map[string]any) {
	if msg["type"] == "call" && msg["method"] == "system.stats" {
		id, _ := msg["id"].(string)
		m.push(map[string]any{"type": "result", "id": id, "result": map[string]any{
			"klark":  map[string]any{"rss_bytes": 123456, "peak_rss_bytes": 234567, "cpu_percent": 4.5, "uptime_s": 60, "version": "9.9"},
			"host":   map[string]any{"mem_total_bytes": 1000, "mem_available_bytes": 600, "load1": 0.3, "cpus": 4},
			"panes":  map[string]any{"count": 2, "agents": 1},
			"agents": []map[string]any{{"pane_id": "p1", "name": "rev", "state": "idle", "rss_bytes": 99, "cpu_percent": 1}}}})
		return
	}
	machineBehaviour("rev1")(m, msg)
}

func TestAdminAnalyticsAndArchiveAPI(t *testing.T) {
	a, ts := newApp(t, nil)
	fm := connect(t, ts.URL, machineToken, "", 5, statsBehaviour)
	defer fm.stop()
	adm := newClient(t, ts.URL)
	adm.login("admin", "adminadmin")
	adm.do("POST", "/api/admin/users", map[string]any{"username": "bob", "password": "bobpassword"})
	bob := newClient(t, ts.URL)
	bob.login("bob", "bobpassword")
	anon := newClient(t, ts.URL)

	for _, p := range [][2]string{{"GET", "/api/admin/analytics"}, {"GET", "/api/admin/storage"}, {"PUT", "/api/admin/storage"}, {"DELETE", "/api/admin/storage"}, {"POST", "/api/admin/storage/test"}} {
		if r, _ := anon.do(p[0], p[1], map[string]any{}); r.StatusCode != 401 {
			t.Errorf("anon %v = %d", p, r.StatusCode)
		}
		if r, _ := bob.do(p[0], p[1], map[string]any{}); r.StatusCode != 403 {
			t.Errorf("user %v = %d", p, r.StatusCode)
		}
	}
	if r, _ := adm.do("GET", "/api/admin/analytics?range=3d", nil); r.StatusCode != 400 {
		t.Fatal("bad range")
	}
	waitFor(t, "the connect-time sample", func() bool { _, ok, _ := a.Store.LatestStat("mac"); return ok })
	_, b := adm.do("GET", "/api/admin/analytics?range=1h", nil) // asked again at once: cached, no new call
	var rep struct {
		Range           string
		IntervalSeconds int
		Totals          struct{ Machines, Online, Panes, Agents, KlarkRssBytes int }
		Machines        []struct {
			ID, Name string
			Online   bool
			HasStats bool
			Latest   *struct {
				KlarkRssBytes, UptimeSeconds int64
				Version                      string
				AgentList                    []struct{ PaneID, State string }
			}
			Peak   struct{ KlarkRssBytes int64 }
			Series []map[string]any
		}
	}
	if err := json.Unmarshal(b, &rep); err != nil {
		t.Fatalf("%s", b)
	}
	if rep.Range != "1h" || rep.IntervalSeconds != 30 || rep.Totals.Machines != 1 || rep.Totals.Online != 1 || rep.Totals.Panes != 2 || rep.Totals.Agents != 1 || rep.Totals.KlarkRssBytes != 123456 {
		t.Fatalf("%s", b)
	}
	m := rep.Machines[0]
	if m.ID != "mac" || !m.Online || !m.HasStats || m.Latest == nil || m.Latest.Version != "9.9" || m.Latest.UptimeSeconds != 60 ||
		len(m.Latest.AgentList) != 1 || m.Latest.AgentList[0].PaneID != "p1" || m.Peak.KlarkRssBytes != 123456 || len(m.Series) != 1 {
		t.Fatalf("%s", b)
	}
	if !strings.Contains(string(b), `"agentList":[{"paneId":"p1"`) || !strings.Contains(string(b), `"series":[{"t":`) {
		t.Fatalf("json names: %s", b)
	}

	// archive: off by default, folder, then s3 with a secret that never comes back
	_, b = adm.do("GET", "/api/admin/storage", nil)
	if !strings.Contains(string(b), `"kind":"none"`) || !strings.Contains(string(b), `"source":"env"`) || !strings.Contains(string(b), `"filesWritten":0`) {
		t.Fatalf("%s", b)
	}
	dir := filepath.Join(t.TempDir(), "out")
	if r, b := adm.do("PUT", "/api/admin/storage", map[string]any{"kind": "folder", "path": dir}); r.StatusCode != 200 || !strings.Contains(string(b), `"source":"admin"`) {
		t.Fatalf("%d %s", r.StatusCode, b)
	}
	if st, err := os.Stat(dir); err != nil || !st.IsDir() {
		t.Fatal("folder not created")
	}
	_, b = adm.do("POST", "/api/admin/storage/test", nil)
	if !strings.Contains(string(b), `"ok":true`) {
		t.Fatalf("%s", b)
	}
	a.Archive.Cycle(context.Background())
	if n := a.Archive.Status().FilesWritten; n < 1 {
		t.Fatalf("files %d", n)
	}
	if r, _ := adm.do("PUT", "/api/admin/storage", map[string]any{"kind": "s3", "s3": map[string]any{"bucket": "b", "accessKeyId": "k"}}); r.StatusCode != 400 {
		t.Fatal("s3 without a secret must be refused")
	}
	s3 := map[string]any{"bucket": "b", "region": "eu-west-1", "endpoint": "http://127.0.0.1:1", "accessKeyId": "AKIATESTKEY", "secretAccessKey": "supersecretvalue99", "pathStyle": true}
	r, b := adm.do("PUT", "/api/admin/storage", map[string]any{"kind": "s3", "path": "klarkhub/p", "s3": s3})
	if r.StatusCode != 200 || strings.Contains(string(b), "supersecretvalue99") || !strings.Contains(string(b), `"secretAccessKeySet":true`) || !strings.Contains(string(b), `"accessKeyId":"AKIATESTKEY"`) {
		t.Fatalf("%d %s", r.StatusCode, b)
	}
	delete(s3, "secretAccessKey")
	if r, b = adm.do("PUT", "/api/admin/storage", map[string]any{"kind": "s3", "path": "klarkhub/p", "s3": s3}); r.StatusCode != 200 || !strings.Contains(string(b), `"secretAccessKeySet":true`) {
		t.Fatalf("secret must be kept: %s", b)
	}
	if c, _ := a.storageConfig(); c.S3.SecretAccessKey != "supersecretvalue99" || c.Path != "klarkhub/p" {
		t.Fatal("kept secret")
	}
	_, b = adm.do("POST", "/api/admin/storage/test", nil) // nothing listens on port 1
	if !strings.Contains(string(b), `"ok":false`) || strings.Contains(string(b), "supersecretvalue99") {
		t.Fatalf("%s", b)
	}
	if r, b = adm.do("PUT", "/api/admin/storage", map[string]any{"kind": "s3", "path": "klarkhub/p", "clearSecretAccessKey": true, "s3": s3}); r.StatusCode != 400 {
		t.Fatalf("cleared secret must be refused: %s", b)
	}
	if _, b = adm.do("DELETE", "/api/admin/storage", nil); !strings.Contains(string(b), `"kind":"none"`) || !strings.Contains(string(b), `"source":"env"`) {
		t.Fatalf("%s", b)
	}
}

func TestArchiveEnvAndOldMachineWithoutStats(t *testing.T) {
	dir := t.TempDir()
	a, ts := newApp(t, func(c *config.Config) {
		c.Storage = config.Storage{Kind: "folder", Path: dir}
		c.StatsIntervalSeconds, c.StatsRetentionDays = 30, 14
	})
	fm := connect(t, ts.URL, machineToken, "", 5, machineBehaviour("rev1")) // answers {"pong":true} to every call: no stats
	defer fm.stop()
	waitFor(t, "online", func() bool { return a.Runtime.Online("mac") })
	waitFor(t, "no-stats memory", func() bool { return a.Stats.HasNoStats("mac") })
	if !a.Stats.HasNoStats("mac") {
		t.Fatal("a machine that does not know system.stats must be remembered")
	}
	adm := newClient(t, ts.URL)
	adm.login("admin", "adminadmin")
	_, b := adm.do("GET", "/api/admin/analytics", nil)
	if !strings.Contains(string(b), `"hasStats":false`) || !strings.Contains(string(b), `"latest":null`) || !strings.Contains(string(b), `"range":"24h"`) {
		t.Fatalf("%s", b)
	}
	_, b = adm.do("GET", "/api/admin/storage", nil)
	if !strings.Contains(string(b), `"kind":"folder"`) || !strings.Contains(string(b), `"path":"`+dir) {
		t.Fatalf("%s", b)
	}
}

func calls(fm *fakeMachine) int {
	fm.mu.Lock()
	defer fm.mu.Unlock()
	n := 0
	for _, m := range fm.got {
		if m["type"] == "call" && m["method"] == "system.stats" {
			n++
		}
	}
	return n
}

// Analytics are lazy: besides the one sample at connect time, a machine is only called when an admin asks.
func TestStatsAreLazyNoCallsWithoutAnAdminRequest(t *testing.T) {
	a, ts := newApp(t, nil)
	a.Stats.Interval = 50 * time.Millisecond // would be a busy poll if anything polled
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	a.StartBackground(ctx)
	fm := connect(t, ts.URL, machineToken, "", 5, statsBehaviour)
	defer fm.stop()
	waitFor(t, "connect sample", func() bool { return calls(fm) == 1 })
	time.Sleep(600 * time.Millisecond)
	if n := calls(fm); n != 1 {
		t.Fatalf("%d system.stats calls with nobody looking (want only the connect one)", n)
	}
	adm := newClient(t, ts.URL)
	adm.login("admin", "adminadmin")
	adm.do("GET", "/api/admin/analytics", nil) // fresh sample: the last attempt is older than the interval
	waitFor(t, "admin-triggered call", func() bool { return calls(fm) == 2 })
	a.Stats.Interval = time.Hour
	for i := 0; i < 3; i++ { // many viewers inside one interval do not multiply calls
		adm.do("GET", "/api/admin/analytics", nil)
	}
	time.Sleep(200 * time.Millisecond)
	if n := calls(fm); n != 2 {
		t.Fatalf("%d calls", n)
	}
}
