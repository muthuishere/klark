package app

import (
	"bytes"
	"context"
	"net/http"
	"net/http/httptest"
	"os"
	"strings"
	"testing"
	"time"

	"github.com/muthuishere/klark/hub-go/internal/config"
	"github.com/muthuishere/klark/hub-go/internal/store"
	"github.com/muthuishere/klark/hub-go/internal/tokens"
)

const podSecret = "pod-secret-0123456789"

func pgURL(t *testing.T) string {
	t.Helper()
	base := os.Getenv("HUB_TEST_POSTGRES")
	if base == "" {
		t.Skip("HUB_TEST_POSTGRES is not set: the multi-pod tests need a PostgreSQL (e.g. docker run postgres:17-alpine)")
	}
	u, cleanup, err := store.TestSchema(base)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(cleanup)
	return u
}

func newPod(t *testing.T, dbURL, id string) (*App, *httptest.Server) {
	t.Helper()
	cfg := config.Config{Addr: ":0", DataDir: t.TempDir(), SessionKey: "shared-test-key", DB: "postgres", DatabaseURL: dbURL,
		PodID: id, PodURL: "http://placeholder", PodSecret: podSecret, StatsIntervalSeconds: 30, StatsRetentionDays: 14, SQLiteBackupKeep: 7,
		Storage:  config.Storage{Kind: "none"},
		Machines: []config.MachineEntry{{Name: "mac", SHA256: tokens.Sha256Hex(machineToken)}}}
	a, err := New(cfg, testUI)
	if err != nil {
		t.Fatal(err)
	}
	a.Auth.Insecure = true
	ts := httptest.NewServer(a.Handler())
	a.Cluster.SetURL(ts.URL)
	t.Cleanup(func() { ts.CloseClientConnections(); ts.Close(); _ = a.Close() })
	return a, ts
}

func TestPostgresNeedsSessionKeyAndPodSecret(t *testing.T) {
	u := pgURL(t)
	base := config.Config{DataDir: t.TempDir(), DB: "postgres", DatabaseURL: u, SessionKey: "k"}
	for name, mut := range map[string]func(*config.Config){
		"no url":         func(c *config.Config) { c.DatabaseURL = "" },
		"no session key": func(c *config.Config) { c.SessionKey = "" },
		"no pod secret":  func(c *config.Config) { c.PodURL = "http://x:7470" },
	} {
		c := base
		mut(&c)
		if _, err := New(c, testUI); err == nil {
			t.Errorf("%s: started", name)
		}
	}
}

// Two hubs on one PostgreSQL: the machine is linked to pod A, the viewer is on pod B.
func TestTwoPodsMachineOnAViewerOnB(t *testing.T) {
	u := pgURL(t)
	a, tsA := newPod(t, u, "pod-a")
	b, tsB := newPod(t, u, "pod-b")
	a.Runtime.FeedPoll, b.Runtime.FeedPoll = 50*time.Millisecond, 50*time.Millisecond
	a.Runtime.TreePoll, b.Runtime.TreePoll = 50*time.Millisecond, 50*time.Millisecond
	fm := connect(t, tsA.URL, machineToken, "", 5, statsBehaviour)
	defer fm.stop()
	waitFor(t, "presence", func() bool {
		p, ok, _ := a.Store.PresenceOf("mac", 0)
		return ok && p.PodID == "pod-a" && p.SessionID == fm.session
	})
	waitFor(t, "connect sample", func() bool { _, ok, _ := a.Store.LatestStat("mac"); return ok })
	if b.Runtime.Online("mac") {
		t.Fatal("pod B must not hold the link")
	}

	v := newClient(t, tsB.URL) // a sign-in on B works on A too (shared key + users)
	v.login("admin", "adminadmin")
	if r, _ := newClient(t, tsA.URL).do("GET", "/api/auth/me", nil); r.StatusCode != 401 {
		t.Fatal("anonymous on A")
	}
	_, body := v.do("GET", "/api/machines", nil)
	if !strings.Contains(string(body), `"id":"mac"`) || !strings.Contains(string(body), `"online":true`) || !strings.Contains(string(body), `"version":"9.9"`) {
		t.Fatalf("machine list on B: %s", body)
	}
	// tree, via B
	r, body := v.do("GET", "/api/machines/mac/tree", nil)
	if r.StatusCode != 200 || !strings.Contains(string(body), `"workspaces"`) || r.Header.Get("X-Tree-Rev") != "rev1" {
		t.Fatalf("tree via B: %d %s", r.StatusCode, body)
	}
	if r2, _ := v.do("GET", "/api/machines/nope/tree", nil); r2.StatusCode != 404 {
		t.Fatal("unknown machine")
	}
	// a call, via B (a POST: the cross-origin check passed on B, the forwarded request is trusted on A)
	r, body = v.do("POST", "/api/machines/mac/call", map[string]any{"method": "ping"})
	if r.StatusCode != 200 || !strings.Contains(string(body), `"pong":true`) {
		t.Fatalf("call via B: %d %s", r.StatusCode, body)
	}
	// a pane stream, via B: chat rows arrive, and input typed on B reaches the machine on A
	chat, closeChat := openSSE(t, v, "/api/machines/mac/panes/p1/events")
	defer closeChat()
	if ev := next(t, chat, "chat"); !strings.Contains(ev.data, `"rows"`) {
		t.Fatalf("chat event %s", ev.data)
	}
	frames, closeFrames := openSSE(t, v, "/api/machines/mac/panes/p1/events?frames=1")
	defer closeFrames()
	waitFor(t, "subscribe on the machine of A", func() bool { return fm.count("subscribe") == 1 })
	fm.push(map[string]any{"type": "frame", "pane_id": "p1", "bytes": "aGVsbG8="})
	_ = next(t, frames, "frame")
	if r, _ := v.do("POST", "/api/machines/mac/panes/p1/input", map[string]any{"text": "hi"}); r.StatusCode != 204 {
		t.Fatalf("input via B: %d", r.StatusCode)
	}
	waitFor(t, "input at the machine", func() bool { return fm.count("input") == 1 })
	// messages and agents
	if r, body := v.do("POST", "/api/messages", map[string]any{"to": "mac/work/reviewer", "text": "hello"}); r.StatusCode != 200 {
		t.Fatalf("message via B: %d %s", r.StatusCode, body)
	}
	waitFor(t, "deliver at the machine", func() bool { return fm.count("deliver") == 1 })
	_, body = v.do("GET", "/api/agents", nil)
	if !strings.Contains(string(body), `"machine":"mac"`) {
		t.Fatalf("agents via B: %s", body)
	}
	// analytics on B show the machine of A
	_, body = v.do("GET", "/api/admin/analytics?range=1h", nil)
	if !strings.Contains(string(body), `"hasStats":true`) || !strings.Contains(string(body), `"online":true`) || !strings.Contains(string(body), `"klarkRssBytes":123456`) {
		t.Fatalf("analytics via B: %s", body)
	}
	// the machine's own link requests may land on B: they are proxied to A by session
	push := func(path string) int {
		req, _ := http.NewRequest("POST", tsB.URL+path, bytes.NewReader([]byte(`{"type":"pane_size","pane_id":"p1","cols":80,"rows":24}`+"\n")))
		req.Header.Set("Authorization", "Bearer "+machineToken)
		resp, err := http.DefaultClient.Do(req)
		if err != nil {
			t.Fatal(err)
		}
		resp.Body.Close()
		return resp.StatusCode
	}
	if c := push("/agent/push?session=" + fm.session + "&seq=900"); c != 204 {
		t.Fatalf("push landing on B: %d", c)
	}
	if c := push("/agent/push?session=nosuchsession&seq=1"); c != 404 {
		t.Fatalf("unknown session on B: %d", c)
	}
	// pod-to-pod requests need the secret
	for _, hdr := range []string{"", "wrong-secret-value-xx"} {
		req, _ := http.NewRequest("GET", tsA.URL+"/internal/agents", nil)
		if hdr != "" {
			req.Header.Set(podHeader, hdr)
		}
		resp, _ := http.DefaultClient.Do(req)
		resp.Body.Close()
		if resp.StatusCode != 403 {
			t.Fatalf("internal with %q: %d", hdr, resp.StatusCode)
		}
	}
	req, _ := http.NewRequest("GET", tsA.URL+"/api/machines", nil)
	req.Header.Set(podHeader, "bad")
	if resp, _ := http.DefaultClient.Do(req); resp.StatusCode != 403 {
		t.Fatal("a bad pod header must be refused on public routes too")
	}

	// the machine reconnects to B: B becomes the holder, A's view follows (presence, not the stale local session)
	fm2 := connect(t, tsB.URL, machineToken, "", 5, statsBehaviour)
	defer fm2.stop()
	waitFor(t, "presence moved to B", func() bool { p, _, _ := a.Store.PresenceOf("mac", 0); return p.PodID == "pod-b" })
	va := newClient(t, tsA.URL)
	va.login("admin", "adminadmin")
	if r, body := va.do("POST", "/api/machines/mac/call", map[string]any{"method": "ping"}); r.StatusCode != 200 || fm2.count("call") == 0 {
		t.Fatalf("call via A after the move: %d %s", r.StatusCode, body)
	}
	// presence expires when a pod stops heartbeating
	_ = a.Store.UpsertPresence(store.Presence{MachineID: "ghost", PodID: "dead", PodURL: "http://127.0.0.1:1"}, time.Now().UnixMilli()-60000, true)
	if _, ok := a.Cluster.remoteOwner("ghost"); ok {
		t.Fatal("an expired presence must not route")
	}
	_ = context.Background()
}
