package app

import (
	"bytes"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"io"
	"mime/multipart"
	"net/http"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/muthuishere/klark/hub-go/internal/config"
	"github.com/muthuishere/klark/hub-go/internal/store"
	"github.com/muthuishere/klark/hub-go/internal/tokens"
)

var pngBytes = append([]byte("\x89PNG\r\n\x1a\n"), bytes.Repeat([]byte{7}, 100)...)

func (c *client) upload(path, name string, data []byte) (*http.Response, []byte) {
	c.t.Helper()
	var buf bytes.Buffer
	mw := multipart.NewWriter(&buf)
	fw, _ := mw.CreateFormFile("file", name)
	_, _ = fw.Write(data)
	_ = mw.Close()
	req, _ := http.NewRequest("POST", c.base+path, &buf)
	req.Header.Set("Content-Type", mw.FormDataContentType())
	resp, err := c.hc.Do(req)
	if err != nil {
		c.t.Fatal(err)
	}
	defer resp.Body.Close()
	b, _ := io.ReadAll(resp.Body)
	return resp, b
}

func getWithToken(t *testing.T, url, token string) (int, []byte, http.Header) {
	t.Helper()
	req, _ := http.NewRequest("GET", url, nil)
	if token != "" {
		req.Header.Set("Authorization", "Bearer "+token)
	}
	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		t.Fatal(err)
	}
	defer resp.Body.Close()
	b, _ := io.ReadAll(resp.Body)
	return resp.StatusCode, b, resp.Header
}

func TestImageUploadFetchAssetAndServe(t *testing.T) {
	a, ts := newApp(t, func(c *config.Config) {
		c.Machines = append(c.Machines, config.MachineEntry{Name: "other", SHA256: tokens.Sha256Hex("other-secret")})
	})
	var mu sync.Mutex
	var fetched []byte
	var fetchMsg map[string]any
	fm := connect(t, ts.URL, machineToken, "", 5, func(m *fakeMachine, msg map[string]any) {
		if msg["type"] != "fetch_asset" {
			machineBehaviour("r1")(m, msg)
			return
		}
		sha, _ := msg["sha256"].(string)
		code, body, _ := getWithToken(t, ts.URL+"/api/assets/"+sha, machineToken)
		mu.Lock()
		fetchMsg, fetched = msg, body
		mu.Unlock()
		if code != 200 {
			m.push(map[string]any{"type": "result", "id": msg["id"], "error": "download failed"})
			return
		}
		m.push(map[string]any{"type": "result", "id": msg["id"], "result": map[string]any{"path": "/home/me/.klark/uploads/" + sha + ".png"}})
	})
	defer fm.stop()
	waitFor(t, "online", func() bool { return a.Runtime.Online("mac") })

	anon := newClient(t, ts.URL)
	if r, _ := anon.upload("/api/machines/mac/panes/p1/upload", "a.png", pngBytes); r.StatusCode != 401 {
		t.Fatalf("anonymous upload: %d", r.StatusCode)
	}
	c := newClient(t, ts.URL)
	c.login("admin", "adminadmin")

	r, b := c.upload("/api/machines/mac/panes/p1/upload", "../shot.png", pngBytes)
	if r.StatusCode != 200 {
		t.Fatalf("upload: %d %s", r.StatusCode, b)
	}
	sum := sha256.Sum256(pngBytes)
	sha := hex.EncodeToString(sum[:])
	var out struct{ Name, Path, Sha256, Mime string }
	_ = json.Unmarshal(b, &out)
	if out.Sha256 != sha || out.Mime != "image/png" || out.Name != "shot.png" || !strings.HasSuffix(out.Path, sha+".png") {
		t.Fatalf("reply: %s", b)
	}
	mu.Lock()
	if !bytes.Equal(fetched, pngBytes) || fetchMsg["pane_id"] != "p1" || fetchMsg["prompt"] != false || fetchMsg["mime"] != "image/png" {
		t.Fatalf("machine got %v (%d bytes)", fetchMsg, len(fetched))
	}
	mu.Unlock()

	// Served to signed-in people, to the machine it is addressed to, and to nobody else.
	if code, body, h := getWithToken(t, ts.URL+"/api/assets/"+sha, ""); code != 401 {
		t.Fatalf("anonymous get: %d %s %v", code, body, h)
	}
	if code, _, _ := getWithToken(t, ts.URL+"/api/assets/"+sha, "other-secret"); code != 403 {
		t.Fatalf("other machine: %d", code)
	}
	if code, _, _ := getWithToken(t, ts.URL+"/api/assets/"+sha, "wrong"); code != 401 {
		t.Fatalf("bad token: %d", code)
	}
	gr, gb := c.do("GET", "/api/assets/"+sha, nil)
	if gr.StatusCode != 200 || !bytes.Equal(gb, pngBytes) || gr.Header.Get("Content-Type") != "image/png" ||
		gr.Header.Get("X-Content-Type-Options") != "nosniff" || !strings.Contains(gr.Header.Get("Content-Security-Policy"), "sandbox") {
		t.Fatalf("person get: %d %v", gr.StatusCode, gr.Header)
	}

	// The same bytes again are stored once.
	if r, b := c.upload("/api/machines/mac/panes/p1/upload", "again.png", pngBytes); r.StatusCode != 200 {
		t.Fatalf("second upload: %d %s", r.StatusCode, b)
	}
	if existed, _ := a.Store.PutAssetBlob(sha, "image/png", pngBytes); !existed {
		t.Fatal("blob should be stored once")
	}

	// Type by content, not by name; size cap.
	if r, b := c.upload("/api/machines/mac/panes/p1/upload", "evil.png", []byte("<svg onload=alert(1)>")); r.StatusCode != 415 {
		t.Fatalf("svg as png: %d %s", r.StatusCode, b)
	}
	big := append(append([]byte{}, pngBytes...), make([]byte, maxImageBytes)...)
	if r, b := c.upload("/api/machines/mac/panes/p1/upload", "big.png", big); r.StatusCode != 413 {
		t.Fatalf("over 10 MB: %d %s", r.StatusCode, b)
	}
	if r, _ := c.upload("/api/machines/nope/panes/p1/upload", "a.png", pngBytes); r.StatusCode != 404 {
		t.Fatalf("unknown machine: %d", r.StatusCode)
	}

	// Retention: records past 24 h go, then the bytes no record names.
	if _, _, err := a.Store.SweepAssets(time.Now().Add(time.Hour).UnixMilli()); err != nil {
		t.Fatal(err)
	}
	if _, err := a.Store.AssetBlob(sha); err != store.ErrNotFound {
		t.Fatalf("blob after sweep: %v", err)
	}
	if gr, _ := c.do("GET", "/api/assets/"+sha, nil); gr.StatusCode != 403 {
		t.Fatalf("after sweep: %d", gr.StatusCode)
	}
}

func TestImageUploadOffline(t *testing.T) {
	_, ts := newApp(t, nil)
	c := newClient(t, ts.URL)
	c.login("admin", "adminadmin")
	if r, _ := c.upload("/api/machines/mac/panes/p1/upload", "a.png", pngBytes); r.StatusCode != 503 {
		t.Fatalf("offline: %d", r.StatusCode)
	}
}

func TestSniffImage(t *testing.T) {
	cases := map[string]string{
		"\x89PNG\r\n\x1a\nxxxx":        "image/png",
		"\xff\xd8\xff\xe0":             "image/jpeg",
		"GIF89a..":                     "image/gif",
		"RIFF\x00\x00\x00\x00WEBPVP8 ": "image/webp",
		"\x00\x00\x00\x18ftypheic....": "image/heic",
		"\x00\x00\x00\x18ftypavif....": "image/avif",
		"<svg xmlns=...>":              "",
		"%PDF-1.7":                     "",
		"\x89PN":                       "",
	}
	for in, want := range cases {
		if got := sniffImage([]byte(in)); got != want {
			t.Errorf("sniff(%q) = %q, want %q", in, got, want)
		}
	}
	if n := cleanAssetName("C:\\x\\..\\a\"b.png"); n != "ab.png" {
		t.Errorf("clean name: %q", n)
	}
}

// ---- klark hub connect -------------------------------------------------------------------------------

type startReply struct {
	Code      string `json:"code"`
	PollID    string `json:"poll_id"`
	URL       string `json:"url"`
	ExpiresIn int    `json:"expires_in"`
	Interval  int    `json:"interval"`
}

func connectStartFor(t *testing.T, c *client, name string) startReply {
	t.Helper()
	r, b := c.do("POST", "/api/connect/start", map[string]string{"name": name})
	if r.StatusCode != 200 {
		t.Fatalf("start: %d %s", r.StatusCode, b)
	}
	var s startReply
	_ = json.Unmarshal(b, &s)
	return s
}

func poll(t *testing.T, c *client, pollID string) (int, map[string]string) {
	t.Helper()
	r, b := c.do("POST", "/api/connect/poll", map[string]string{"poll_id": pollID})
	var m map[string]string
	_ = json.Unmarshal(b, &m)
	return r.StatusCode, m
}

func TestConnectApproveRevoke(t *testing.T) {
	a, ts := newApp(t, nil)
	cli := newClient(t, ts.URL) // the klark CLI: no cookie
	s := connectStartFor(t, cli, "box-1")
	if len(s.Code) != 9 || s.Code[4] != '-' || len(s.PollID) != 64 || s.ExpiresIn != 600 || s.Interval != 2 ||
		s.URL != ts.URL+"/connect?code="+s.Code {
		t.Fatalf("start reply: %+v", s)
	}
	if code, m := poll(t, cli, s.PollID); code != 200 || m["status"] != "pending" {
		t.Fatalf("pending: %d %v", code, m)
	}
	// Approving needs a sign-in.
	if r, _ := cli.do("POST", "/api/connect/requests/"+s.Code+"/approve", nil); r.StatusCode != 401 {
		t.Fatalf("anonymous approve: %d", r.StatusCode)
	}
	admin := newClient(t, ts.URL)
	admin.login("admin", "adminadmin")
	r, b := admin.do("GET", "/api/connect/requests/"+strings.ToLower(strings.ReplaceAll(s.Code, "-", "")), nil)
	if r.StatusCode != 200 || !strings.Contains(string(b), `"machine":"box-1"`) || !strings.Contains(string(b), `"status":"pending"`) {
		t.Fatalf("get request: %d %s", r.StatusCode, b)
	}
	if r, b := admin.do("POST", "/api/connect/requests/"+s.Code+"/approve", nil); r.StatusCode != 200 {
		t.Fatalf("approve: %d %s", r.StatusCode, b)
	}
	if r, _ := admin.do("POST", "/api/connect/requests/"+s.Code+"/deny", nil); r.StatusCode != 409 {
		t.Fatalf("second decision: %d", r.StatusCode)
	}
	code, m := poll(t, cli, s.PollID)
	if code != 200 || m["status"] != "approved" || m["machine"] != "box-1" || !strings.HasPrefix(m["token"], "km_") {
		t.Fatalf("approved poll: %d %v", code, m)
	}
	token := m["token"]
	// Exactly once.
	if code, m := poll(t, cli, s.PollID); code != 404 || m["status"] != "unknown" {
		t.Fatalf("second poll: %d %v", code, m)
	}
	// Only the token's SHA-256 is stored, and the machine is listed.
	rows, _ := a.Store.Machines()
	found := false
	for _, row := range rows {
		if row.Name == "box-1" {
			found = row.SHA256 == tokens.Sha256Hex(token) && row.CreatedBy == "admin"
		}
	}
	if !found {
		t.Fatalf("machine row: %+v", rows)
	}
	if _, b := admin.do("GET", "/api/admin/machines", nil); !strings.Contains(string(b), `"name":"box-1"`) || strings.Contains(string(b), token) {
		t.Fatalf("admin machines: %s", b)
	}
	// The token links (the probe klark uses), and the machine appears online.
	if code, _, _ := getWithToken(t, ts.URL+"/agent", token); code != 200 {
		t.Fatalf("probe: %d", code)
	}
	fm := connect(t, ts.URL, token, "", 5, machineBehaviour("r1"))
	waitFor(t, "box-1 online", func() bool { return a.Runtime.Online("box-1") })
	if _, b := admin.do("GET", "/api/admin/machines", nil); !strings.Contains(string(b), `"name":"box-1","status":"online"`) {
		t.Fatalf("online in admin: %s", b)
	}
	// Revoke: the session drops and the token stops working.
	if r, _ := admin.do("DELETE", "/api/admin/machines/box-1", nil); r.StatusCode != 204 {
		t.Fatalf("revoke: %d", r.StatusCode)
	}
	waitFor(t, "box-1 offline", func() bool { return !a.Runtime.Online("box-1") })
	fm.stop()
	if code, _, _ := getWithToken(t, ts.URL+"/agent", token); code != 401 {
		t.Fatalf("revoked token probe: %d", code)
	}
	// HUB_MACHINES tokens keep working.
	if code, _, _ := getWithToken(t, ts.URL+"/agent", machineToken); code != 200 {
		t.Fatalf("HUB_MACHINES token: %d", code)
	}
}

func TestConnectDenyExpiryAndPolicy(t *testing.T) {
	a, ts := newApp(t, func(c *config.Config) {
		c.Users = append(c.Users, config.User{Name: "bob", Hash: "sha256:" + tokens.Sha256Hex("bobpassword")})
	})
	cli := newClient(t, ts.URL)
	admin := newClient(t, ts.URL)
	admin.login("admin", "adminadmin")

	// Deny.
	s := connectStartFor(t, cli, "box-2")
	if r, _ := admin.do("POST", "/api/connect/requests/"+s.Code+"/deny", nil); r.StatusCode != 200 {
		t.Fatalf("deny: %d", r.StatusCode)
	}
	if _, m := poll(t, cli, s.PollID); m["status"] != "denied" {
		t.Fatalf("denied poll: %v", m)
	}
	if code, _ := poll(t, cli, s.PollID); code != 404 {
		t.Fatalf("after denied: %d", code)
	}

	// Expiry: the page no longer finds it, approving fails, the poll says expired.
	now := time.Now()
	a.Now = func() time.Time { return now }
	s = connectStartFor(t, cli, "box-3")
	now = now.Add(connectLifetime + time.Second)
	if r, _ := admin.do("GET", "/api/connect/requests/"+s.Code, nil); r.StatusCode != 404 {
		t.Fatalf("expired get: %d", r.StatusCode)
	}
	if r, _ := admin.do("POST", "/api/connect/requests/"+s.Code+"/approve", nil); r.StatusCode != 404 {
		t.Fatalf("expired approve: %d", r.StatusCode)
	}
	if _, m := poll(t, cli, s.PollID); m["status"] != "expired" {
		t.Fatalf("expired poll: %v", m)
	}
	a.Now = nil

	// Bad names; HUB_MACHINES names cannot be taken over; someone else's machine needs an admin.
	if r, _ := cli.do("POST", "/api/connect/start", map[string]string{"name": "a b/c"}); r.StatusCode != 400 {
		t.Fatalf("bad name: %d", r.StatusCode)
	}
	s = connectStartFor(t, cli, "mac")
	if r, _ := admin.do("POST", "/api/connect/requests/"+s.Code+"/approve", nil); r.StatusCode != 409 {
		t.Fatalf("HUB_MACHINES name: %d", r.StatusCode)
	}
	if err := a.Store.UpsertMachine("box-4", tokens.Sha256Hex("t4"), "admin"); err != nil {
		t.Fatal(err)
	}
	bob := newClient(t, ts.URL)
	bob.login("bob", "bobpassword")
	s = connectStartFor(t, cli, "box-4")
	if r, _ := bob.do("POST", "/api/connect/requests/"+s.Code+"/approve", nil); r.StatusCode != 403 {
		t.Fatalf("bob re-keys admin's machine: %d", r.StatusCode)
	}
	s2 := connectStartFor(t, cli, "bobs-box")
	if r, b := bob.do("POST", "/api/connect/requests/"+s2.Code+"/approve", nil); r.StatusCode != 200 {
		t.Fatalf("bob approves a new machine: %d %s", r.StatusCode, b)
	}
	if _, m := poll(t, cli, s2.PollID); m["status"] != "approved" {
		t.Fatalf("bob's machine: %v", m)
	}
	if _, b := admin.do("GET", "/api/admin/machines", nil); !strings.Contains(string(b), `"name":"bobs-box","status":"offline","approvedBy":"bob"`) {
		t.Fatalf("approvedBy: %s", b)
	}
}

func TestConnectStartRateLimit(t *testing.T) {
	_, ts := newApp(t, nil)
	cli := newClient(t, ts.URL)
	for i := 0; i < 20; i++ {
		connectStartFor(t, cli, "box")
	}
	r, _ := cli.do("POST", "/api/connect/start", map[string]string{"name": "box"})
	if r.StatusCode != 429 || r.Header.Get("Retry-After") == "" {
		t.Fatalf("21st start: %d %v", r.StatusCode, r.Header)
	}
}
