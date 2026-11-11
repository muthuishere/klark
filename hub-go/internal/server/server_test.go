package server

import (
	"bufio"
	"bytes"
	"compress/gzip"
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"

	"github.com/muthuishere/klark/hub-go/internal/link"
	"github.com/muthuishere/klark/hub-go/internal/tokens"
)

const secret = "s3cret-token"

func newServer(t *testing.T) (*httptest.Server, *link.Registry, *[]map[string]any) {
	t.Helper()
	reg := link.NewRegistry()
	var got []map[string]any
	reg.OnMessage = func(_ *link.Conn, m map[string]any) { got = append(got, m) }
	s := &Server{Registry: reg, Tokens: tokens.Static{
		{Name: "mac", TokenSHA256: tokens.Sha256Hex(secret)},
		{Name: "fleet", TokenSHA256: tokens.Sha256Hex("fleet-token"), Fleet: true},
	}}
	ts := httptest.NewServer(s.Handler())
	t.Cleanup(ts.Close)
	return ts, reg, &got
}

func do(t *testing.T, method, url, token string, body []byte, hdr map[string]string) *http.Response {
	t.Helper()
	req, _ := http.NewRequest(method, url, bytes.NewReader(body))
	if token != "" {
		req.Header.Set("Authorization", "Bearer "+token)
	}
	for k, v := range hdr {
		req.Header.Set(k, v)
	}
	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { resp.Body.Close() })
	return resp
}

func hello(protocol int) []byte {
	b, _ := json.Marshal(map[string]any{"type": "hello", "protocol": protocol,
		"machine_id": "m1", "machine_name": "mac", "klark_version": "0.9.5"})
	return b
}

func openSession(t *testing.T, ts *httptest.Server) string {
	t.Helper()
	resp := do(t, "POST", ts.URL+"/agent/session", secret, hello(5), nil)
	if resp.StatusCode != 200 {
		t.Fatalf("open session: %d", resp.StatusCode)
	}
	var out struct {
		SessionID string `json:"session_id"`
		Heartbeat int    `json:"heartbeat_seconds"`
	}
	_ = json.NewDecoder(resp.Body).Decode(&out)
	if out.SessionID == "" {
		t.Fatalf("bad session reply %+v", out)
	}
	return out.SessionID
}

func TestSessionReplyCarriesTheHeartbeat(t *testing.T) {
	ts, _, _ := newServer(t)
	resp := do(t, "POST", ts.URL+"/agent/session", secret, hello(4), nil)
	var out map[string]any
	_ = json.NewDecoder(resp.Body).Decode(&out)
	if out["heartbeat_seconds"].(float64) != 15 || out["session_id"] == "" {
		t.Fatalf("bad reply %v", out)
	}
}

func TestProbe(t *testing.T) {
	ts, _, _ := newServer(t)
	if c := do(t, "GET", ts.URL+"/agent", "wrong", nil, nil).StatusCode; c != 401 {
		t.Fatalf("unknown token: %d", c)
	}
	if c := do(t, "GET", ts.URL+"/agent", secret, nil, nil).StatusCode; c != 200 {
		t.Fatalf("known token: %d", c)
	}
	if c := do(t, "GET", ts.URL+"/agent", "fleet-token", nil, nil).StatusCode; c != 400 {
		t.Fatalf("fleet token without a machine header: %d", c)
	}
	if c := do(t, "GET", ts.URL+"/agent", "fleet-token", nil, map[string]string{"X-Klark-Machine": "box-1"}).StatusCode; c != 200 {
		t.Fatalf("fleet token with a machine header: %d", c)
	}
}

func TestSessionRefusals(t *testing.T) {
	ts, _, _ := newServer(t)
	if c := do(t, "POST", ts.URL+"/agent/session", secret, hello(3), nil).StatusCode; c != 400 {
		t.Fatalf("protocol 3 should be refused: %d", c)
	}
	if c := do(t, "POST", ts.URL+"/agent/session", secret, []byte("nope"), nil).StatusCode; c != 400 {
		t.Fatalf("a non-hello body should be refused: %d", c)
	}
	if c := do(t, "POST", ts.URL+"/agent/events?session=nope", secret, nil, nil).StatusCode; c != 405 && c != 404 {
		t.Fatalf("unexpected: %d", c)
	}
	if c := do(t, "GET", ts.URL+"/agent/events?session=nope", secret, nil, nil).StatusCode; c != 404 {
		t.Fatalf("unknown session: %d", c)
	}
	sid := openSession(t, ts)
	if c := do(t, "GET", ts.URL+"/agent/events?session="+sid, "fleet-token", nil, map[string]string{"X-Klark-Machine": "x"}).StatusCode; c != 401 {
		t.Fatalf("another machine's token on this session: %d", c)
	}
}

func TestPushDuplicateAndGzip(t *testing.T) {
	ts, _, got := newServer(t)
	sid := openSession(t, ts)
	url := ts.URL + "/agent/push?session=" + sid + "&seq="
	line := []byte(`{"type":"tree","rev":"1","snapshot":{}}` + "\n")
	if c := do(t, "POST", url+"1", secret, line, nil).StatusCode; c != 204 {
		t.Fatalf("first push: %d", c)
	}
	resp := do(t, "POST", url+"1", secret, line, nil)
	var dup map[string]bool
	_ = json.NewDecoder(resp.Body).Decode(&dup)
	if resp.StatusCode != 200 || !dup["duplicate"] {
		t.Fatalf("a retried seq must be a duplicate: %d %v", resp.StatusCode, dup)
	}
	var zb bytes.Buffer
	zw := gzip.NewWriter(&zb)
	_, _ = zw.Write([]byte(`{"type":"pong"}` + "\nnot json\n" + `{"type":"closed","pane_id":"p1","reason":"x"}` + "\n"))
	_ = zw.Close()
	if c := do(t, "POST", url+"2", secret, zb.Bytes(), map[string]string{"Content-Encoding": "gzip"}).StatusCode; c != 204 {
		t.Fatalf("gzip push: %d", c)
	}
	if c := do(t, "POST", url+"0", secret, line, nil).StatusCode; c != 400 {
		t.Fatalf("seq is required: %d", c)
	}
	if len(*got) != 3 { // tree, pong, closed; the bad line is skipped
		t.Fatalf("messages seen: %d %v", len(*got), *got)
	}
}

func TestEventsStreamDeliversAndResumes(t *testing.T) {
	ts, reg, _ := newServer(t)
	sid := openSession(t, ts)
	conn, _ := reg.Session(sid, secret)
	conn.Send(map[string]any{"type": "get_tree"})
	conn.Send(map[string]any{"type": "subscribe", "pane_id": "p1"})

	ctx, cancel := context.WithCancel(context.Background())
	req, _ := http.NewRequestWithContext(ctx, "GET", ts.URL+"/agent/events?session="+sid+"&after=0", nil)
	req.Header.Set("Authorization", "Bearer "+secret)
	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		t.Fatal(err)
	}
	if ct := resp.Header.Get("Content-Type"); ct != "application/x-ndjson" {
		t.Fatalf("content type %q", ct)
	}
	sc := bufio.NewScanner(resp.Body)
	read := func() map[string]any {
		if !sc.Scan() {
			t.Fatalf("stream ended: %v", sc.Err())
		}
		var m map[string]any
		_ = json.Unmarshal(sc.Bytes(), &m)
		return m
	}
	if m := read(); m["type"] != "get_tree" || m["seq"].(float64) != 1 {
		t.Fatalf("first message %v", m)
	}
	if m := read(); m["type"] != "subscribe" || m["seq"].(float64) != 2 {
		t.Fatalf("second message %v", m)
	}
	conn.Send(map[string]any{"type": "unsubscribe", "pane_id": "p1"}) // live, while held open
	if m := read(); m["seq"].(float64) != 3 {
		t.Fatalf("live message %v", m)
	}
	cancel()
	resp.Body.Close()

	// resume after seq 2: only seq 3 is sent again
	ctx2, cancel2 := context.WithTimeout(context.Background(), 3*time.Second)
	defer cancel2()
	req2, _ := http.NewRequestWithContext(ctx2, "GET", ts.URL+"/agent/events?session="+sid+"&after=2", nil)
	req2.Header.Set("Authorization", "Bearer "+secret)
	resp2, err := http.DefaultClient.Do(req2)
	if err != nil {
		t.Fatal(err)
	}
	defer resp2.Body.Close()
	sc2 := bufio.NewScanner(resp2.Body)
	if !sc2.Scan() || !strings.Contains(sc2.Text(), `"seq":3`) {
		t.Fatalf("resume should replay seq 3 first, got %q", sc2.Text())
	}
}

func TestIdleStreamGetsPingAndReplacedSessionEnds(t *testing.T) {
	ts, reg, _ := newServer(t)
	reg.Heartbeat = 50 * time.Millisecond
	sid := openSession(t, ts)
	req, _ := http.NewRequest("GET", ts.URL+"/agent/events?session="+sid, nil)
	req.Header.Set("Authorization", "Bearer "+secret)
	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		t.Fatal(err)
	}
	defer resp.Body.Close()
	time.Sleep(80 * time.Millisecond)
	reg.Sweep() // rings the quiet stream
	sc := bufio.NewScanner(resp.Body)
	if !sc.Scan() || !strings.Contains(sc.Text(), `"ping"`) {
		t.Fatalf("expected a ping line, got %q", sc.Text())
	}
	// a new session for the same machine replaces this one and ends its stream
	_ = openSession(t, ts)
	if sc.Scan() {
		t.Fatalf("the old stream should have ended, got %q", sc.Text())
	}
	if c := do(t, "GET", ts.URL+"/agent/events?session="+sid, secret, nil, nil).StatusCode; c != 404 {
		t.Fatalf("the replaced session should be gone: %d", c)
	}
}

func TestOfflineSweepAndRevoke(t *testing.T) {
	ts, reg, _ := newServer(t)
	reg.OfflineAfter = 20 * time.Millisecond
	sid := openSession(t, ts)
	time.Sleep(40 * time.Millisecond)
	reg.Sweep()
	if c, _ := reg.Session(sid, secret); c != nil {
		t.Fatal("a machine with no stream should be dropped")
	}
	sid = openSession(t, ts)
	reg.Revoke("mac")
	if c, _ := reg.Session(sid, secret); c != nil {
		t.Fatal("a revoked machine should be dropped at once")
	}
}
