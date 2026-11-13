package app

import (
	"bufio"
	"bytes"
	"compress/gzip"
	"encoding/base64"
	"encoding/json"
	"io"
	"net/http"
	"net/http/cookiejar"
	"net/http/httptest"
	"net/url"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
	"testing/fstest"
	"time"

	"github.com/muthuishere/klark/hub-go/internal/auth"
	"github.com/muthuishere/klark/hub-go/internal/config"
	"github.com/muthuishere/klark/hub-go/internal/tokens"
)

const machineToken = "machine-secret"

var testUI = fstest.MapFS{
	"index.html":         {Data: []byte(`<!doctype html><html><head><base href="/"><script>var t=1</script><script type="module" src="./assets/a-1.js"></script></head><body>app</body></html>`)},
	"assets/a-1.js":      {Data: []byte("console.log(1)")},
	"assets/index-2.css": {Data: []byte("body{}")},
}

func newApp(t *testing.T, mut func(*config.Config)) (*App, *httptest.Server) {
	t.Helper()
	cfg := config.Config{Addr: ":0", DataDir: t.TempDir(), SessionKey: "test-key",
		Machines: []config.MachineEntry{{Name: "mac", SHA256: tokens.Sha256Hex(machineToken)}}}
	if mut != nil {
		mut(&cfg)
	}
	a, err := New(cfg, testUI)
	if err != nil {
		t.Fatal(err)
	}
	a.Auth.Insecure = true
	ts := httptest.NewServer(a.Handler())
	t.Cleanup(func() { ts.CloseClientConnections(); ts.Close(); _ = a.Close() })
	return a, ts
}

type client struct {
	t    *testing.T
	base string
	hc   *http.Client
}

func newClient(t *testing.T, base string) *client {
	jar, _ := cookiejar.New(nil)
	return &client{t, base, &http.Client{Jar: jar, CheckRedirect: func(*http.Request, []*http.Request) error { return http.ErrUseLastResponse }}}
}

func (c *client) do(method, path string, body any) (*http.Response, []byte) {
	c.t.Helper()
	var rd io.Reader
	if body != nil {
		b, _ := json.Marshal(body)
		rd = bytes.NewReader(b)
	}
	req, _ := http.NewRequest(method, c.base+path, rd)
	if body != nil {
		req.Header.Set("Content-Type", "application/json")
	}
	resp, err := c.hc.Do(req)
	if err != nil {
		c.t.Fatal(err)
	}
	defer resp.Body.Close()
	b, _ := io.ReadAll(resp.Body)
	return resp, b
}

func (c *client) login(user, pw string) {
	c.t.Helper()
	if r, b := c.do("POST", "/api/auth/login", map[string]string{"username": user, "password": pw}); r.StatusCode != 200 {
		c.t.Fatalf("login %s: %d %s", user, r.StatusCode, b)
	}
}

func TestBasePathSPAHeadersAndGzip(t *testing.T) {
	_, ts := newApp(t, func(c *config.Config) { c.BasePath = "/klark" })
	c := newClient(t, ts.URL)

	r, b := c.do("GET", "/klark/", nil)
	if r.StatusCode != 200 || !strings.Contains(string(b), `<base href="/klark/">`) || r.Header.Get("Cache-Control") != "no-cache" {
		t.Fatalf("index: %d %v %s", r.StatusCode, r.Header, b)
	}
	if r2, b2 := c.do("GET", "/klark/m/mac/p/1", nil); r2.StatusCode != 200 || !strings.Contains(string(b2), `<base href="/klark/">`) {
		t.Fatal("SPA fallback")
	}
	if r2, _ := c.do("GET", "/klark/go?machine=mac&pane=1", nil); r2.StatusCode != 200 {
		t.Fatal("/go fallback")
	}
	if r2, _ := c.do("GET", "/klark/assets/a-1.js", nil); !strings.Contains(r2.Header.Get("Cache-Control"), "immutable") {
		t.Fatal("hashed asset must be immutable")
	}
	if r2, _ := c.do("GET", "/klark", nil); r2.StatusCode != 301 {
		t.Fatalf("/klark = %d", r2.StatusCode)
	}
	if r2, _ := c.do("GET", "/", nil); r2.StatusCode != 404 {
		t.Fatalf("outside the base = %d", r2.StatusCode)
	}
	if r2, _ := c.do("GET", "/healthz", nil); r2.StatusCode != 200 {
		t.Fatal("/healthz")
	}
	if r2, b2 := c.do("GET", "/klark/api/nope", nil); r2.StatusCode != 404 || !strings.Contains(string(b2), "error") {
		t.Fatal("unknown API must be JSON 404, not the SPA")
	}
	if r2, _ := c.do("GET", "/klark/agent", nil); r2.StatusCode != 401 {
		t.Fatalf("machine link under base: %d", r2.StatusCode)
	}
	for _, h := range []string{"X-Content-Type-Options", "X-Frame-Options", "Content-Security-Policy", "Referrer-Policy"} {
		if r.Header.Get(h) == "" {
			t.Errorf("missing %s", h)
		}
	}
	csp := r.Header.Get("Content-Security-Policy")
	if !strings.Contains(csp, "'sha256-") || strings.Contains(csp, "script-src 'self' 'unsafe-inline'") {
		t.Errorf("CSP should allow the inline theme script by hash only: %s", csp)
	}

	req, _ := http.NewRequest("GET", ts.URL+"/klark/", nil)
	req.Header.Set("Accept-Encoding", "gzip")
	resp, err := http.DefaultTransport.RoundTrip(req)
	if err != nil {
		t.Fatal(err)
	}
	defer resp.Body.Close()
	zr, err := gzip.NewReader(resp.Body)
	if resp.Header.Get("Content-Encoding") != "gzip" || err != nil {
		t.Fatalf("gzip: %v %v", resp.Header, err)
	}
	if body, _ := io.ReadAll(zr); !strings.Contains(string(body), "klark/") {
		t.Fatal("gzip body")
	}
}

func TestStaticLoginRateLimitCSRF(t *testing.T) {
	hash, _ := auth.HashPassword("bobpass1")
	_, ts := newApp(t, func(c *config.Config) {
		c.Users = []config.User{{Name: "bob", Hash: hash}, {Name: "sam", Hash: auth.Sha256Hash("sampass1"), Admin: true}}
	})
	c := newClient(t, ts.URL)

	if r, _ := c.do("GET", "/api/machines", nil); r.StatusCode != 401 {
		t.Fatalf("unauthenticated = %d", r.StatusCode)
	}
	var p struct {
		Static bool
		Adfs   struct{ Enabled bool }
	}
	_, b := c.do("GET", "/api/auth/providers", nil)
	_ = json.Unmarshal(b, &p)
	if !p.Static || p.Adfs.Enabled {
		t.Fatalf("providers %s", b)
	}

	c.login("admin", "adminadmin") // built-in
	if _, b := c.do("GET", "/api/auth/me", nil); !strings.Contains(string(b), `"admin":true`) {
		t.Fatalf("me %s", b)
	}
	c.login("sam", "sampass1") // sha256: hash
	c.login("bob", "bobpass1") // bcrypt
	if _, b := c.do("GET", "/api/auth/me", nil); !strings.Contains(string(b), `"username":"bob"`) || !strings.Contains(string(b), `"admin":false`) {
		t.Fatalf("me %s", b)
	}
	if r, _ := c.do("GET", "/api/admin/users", nil); r.StatusCode != 403 {
		t.Fatalf("non-admin on admin API = %d", r.StatusCode)
	}

	// the cookie is HttpOnly + SameSite=Lax
	r, _ := c.do("POST", "/api/auth/login", map[string]string{"username": "bob", "password": "bobpass1"})
	ck := r.Header.Get("Set-Cookie")
	if !strings.Contains(ck, "HttpOnly") || !strings.Contains(ck, "SameSite=Lax") {
		t.Fatalf("cookie %s", ck)
	}

	// CSRF: a state-changing request from another origin is refused, even with a good cookie
	req, _ := http.NewRequest("POST", ts.URL+"/api/auth/logout", nil)
	req.Header.Set("Origin", "http://evil.example")
	resp, _ := c.hc.Do(req)
	if resp.StatusCode != 403 {
		t.Fatalf("cross-origin logout = %d", resp.StatusCode)
	}
	req, _ = http.NewRequest("POST", ts.URL+"/api/auth/logout", nil)
	req.Header.Set("Origin", ts.URL)
	if resp, _ = c.hc.Do(req); resp.StatusCode != 204 {
		t.Fatalf("same-origin logout = %d", resp.StatusCode)
	}
	if r, _ := c.do("GET", "/api/auth/me", nil); r.StatusCode != 401 {
		t.Fatal("logged out")
	}

	// rate limit: 5 failures lock the user out, even for the right password
	bad := newClient(t, ts.URL)
	for i := 0; i < 5; i++ {
		if r, _ := bad.do("POST", "/api/auth/login", map[string]string{"username": "bob", "password": "nope"}); r.StatusCode != 401 {
			t.Fatalf("attempt %d = %d", i, r.StatusCode)
		}
	}
	r, _ = bad.do("POST", "/api/auth/login", map[string]string{"username": "bob", "password": "bobpass1"})
	if r.StatusCode != 429 || r.Header.Get("Retry-After") == "" {
		t.Fatalf("locked out = %d", r.StatusCode)
	}
}

func TestAdminUsersAndDirectory(t *testing.T) {
	_, ts := newApp(t, nil)
	c := newClient(t, ts.URL)
	c.login("admin", "adminadmin")
	if r, b := c.do("POST", "/api/admin/users", map[string]any{"username": "kim", "displayName": "Kim", "password": "kimkim12", "admin": false}); r.StatusCode != 200 {
		t.Fatalf("%d %s", r.StatusCode, b)
	}
	if r, _ := c.do("POST", "/api/admin/users", map[string]any{"username": "x", "password": "short"}); r.StatusCode != 400 {
		t.Fatal("short password accepted")
	}
	_, b := c.do("GET", "/api/admin/users", nil)
	if !strings.Contains(string(b), `"username":"kim"`) || strings.Contains(string(b), "hash") || strings.Contains(string(b), "$2") {
		t.Fatalf("users %s", b)
	}
	kim := newClient(t, ts.URL)
	kim.login("kim", "kimkim12")
	if r, _ := c.do("DELETE", "/api/admin/users/kim", nil); r.StatusCode != 204 {
		t.Fatal("delete")
	}
	if r, _ := kim.do("GET", "/api/machines", nil); r.StatusCode != 401 {
		t.Fatal("a deleted user's cookie must die")
	}
	if r, _ := c.do("DELETE", "/api/admin/users/admin", nil); r.StatusCode != 409 {
		t.Fatal("must not delete yourself")
	}
	if r, _ := c.do("PUT", "/api/admin/directory", map[string]any{"mode": "ldap", "host": "ldap.example.com", "userDnTemplate": "uid={0},dc=x"}); r.StatusCode != 200 {
		t.Fatal("put directory")
	}
	if _, b := c.do("GET", "/api/admin/directory", nil); !strings.Contains(string(b), `"mode":"ldap"`) || !strings.Contains(string(b), `"host":"ldap.example.com"`) {
		t.Fatalf("directory %s", b)
	}
	// machines: add (token shown once), connect with it, revoke
	r, b := c.do("POST", "/api/admin/machines", map[string]string{"name": "lab"})
	var added struct{ Token string }
	_ = json.Unmarshal(b, &added)
	if r.StatusCode != 201 || added.Token == "" {
		t.Fatalf("%d %s", r.StatusCode, b)
	}
	fm := connect(t, ts.URL, added.Token, "", 5, nil)
	defer fm.stop()
	if _, b := c.do("GET", "/api/machines", nil); !strings.Contains(string(b), `"id":"lab","name":"Mac","online":true`) {
		t.Fatalf("machines %s", b)
	}
	if r, _ := c.do("DELETE", "/api/admin/machines/lab", nil); r.StatusCode != 204 {
		t.Fatal("revoke")
	}
	resp, _ := http.Post(ts.URL+"/agent/session", "application/json", nil)
	_ = resp
	req, _ := http.NewRequest("GET", ts.URL+"/agent", nil)
	req.Header.Set("Authorization", "Bearer "+added.Token)
	time.Sleep(10 * time.Millisecond)
	if resp, _ := http.DefaultClient.Do(req); resp.StatusCode != 401 {
		t.Fatalf("a revoked token must stop working, got %d", resp.StatusCode)
	}
}

// ---- the full authorization-code flow against the mock ADFS, with no browser ---------------------

func TestMockADFSAuthorizationCodeFlow(t *testing.T) {
	_, ts := newApp(t, func(c *config.Config) {
		c.MockADFS = true
		c.MockUsers = []config.MockUser{{Name: "alice", Password: "alicepw", Admin: true}, {Name: "bob", Password: "bobpw"}}
	})
	c := newClient(t, ts.URL)

	_, b := c.do("GET", "/api/auth/providers", nil)
	if !strings.Contains(string(b), `"enabled":true`) || !strings.Contains(string(b), `"loginUrl":"auth/adfs/login"`) {
		t.Fatalf("providers %s", b)
	}
	// discovery and jwks are served
	_, b = c.do("GET", "/mock-adfs/.well-known/openid-configuration", nil)
	if !strings.Contains(string(b), ts.URL+"/mock-adfs/authorize") {
		t.Fatalf("discovery %s", b)
	}
	if _, b = c.do("GET", "/mock-adfs/jwks", nil); !strings.Contains(string(b), `"kty":"RSA"`) {
		t.Fatalf("jwks %s", b)
	}

	// 1. the hub redirects to the IdP with state, nonce and a PKCE challenge
	r, _ := c.do("GET", "/auth/adfs/login", nil)
	if r.StatusCode != 302 {
		t.Fatalf("login = %d", r.StatusCode)
	}
	authURL, _ := url.Parse(r.Header.Get("Location"))
	q := authURL.Query()
	if !strings.HasSuffix(authURL.Path, "/mock-adfs/authorize") || q.Get("state") == "" || q.Get("nonce") == "" ||
		q.Get("code_challenge_method") != "S256" || q.Get("code_challenge") == "" || q.Get("response_type") != "code" {
		t.Fatalf("authorize url %s", authURL)
	}
	// 2. the IdP shows a login form
	r, b = c.do("GET", authURL.Path+"?"+authURL.RawQuery, nil)
	if r.StatusCode != 200 || !strings.Contains(string(b), `name="username"`) || !strings.Contains(string(b), q.Get("state")) {
		t.Fatalf("form %d %s", r.StatusCode, b)
	}
	// a wrong password is refused and gives no code
	form := url.Values{"username": {"alice"}, "password": {"wrong"}}
	for k := range q {
		form.Set(k, q.Get(k))
	}
	resp, _ := c.hc.PostForm(ts.URL+authURL.Path, form)
	if resp.StatusCode != 401 {
		t.Fatalf("bad password = %d", resp.StatusCode)
	}
	// 3. the right one redirects back to the hub's callback with a code
	form.Set("password", "alicepw")
	resp, _ = c.hc.PostForm(ts.URL+authURL.Path, form)
	if resp.StatusCode != 302 {
		t.Fatalf("credentials = %d", resp.StatusCode)
	}
	cb, _ := url.Parse(resp.Header.Get("Location"))
	if cb.Path != "/auth/adfs/callback" || cb.Query().Get("code") == "" || cb.Query().Get("state") != q.Get("state") {
		t.Fatalf("callback %s", cb)
	}
	// 4. a callback with a forged state is refused
	forged := *cb
	fq := forged.Query()
	fq.Set("state", "forged")
	forged.RawQuery = fq.Encode()
	if r, _ := c.do("GET", forged.Path+"?"+forged.RawQuery, nil); !strings.Contains(r.Header.Get("Location"), "error=adfs_failed") {
		t.Fatalf("forged state accepted: %v", r.Header)
	}
	if r, _ := c.do("GET", "/api/auth/me", nil); r.StatusCode != 401 {
		t.Fatal("signed in by a forged callback")
	}
	// the state cookie was cleared by the failure: start over for the real one
	r, _ = c.do("GET", "/auth/adfs/login", nil)
	authURL, _ = url.Parse(r.Header.Get("Location"))
	q = authURL.Query()
	form = url.Values{"username": {"alice"}, "password": {"alicepw"}}
	for k := range q {
		form.Set(k, q.Get(k))
	}
	resp, _ = c.hc.PostForm(ts.URL+authURL.Path, form)
	cb, _ = url.Parse(resp.Header.Get("Location"))
	// 5. the hub exchanges the code, verifies the ID token against the jwks and signs alice in
	r, _ = c.do("GET", cb.Path+"?"+cb.RawQuery, nil)
	if r.StatusCode != 302 || r.Header.Get("Location") != "/" {
		t.Fatalf("callback = %d %v", r.StatusCode, r.Header)
	}
	_, b = c.do("GET", "/api/auth/me", nil)
	if !strings.Contains(string(b), `"username":"alice"`) || !strings.Contains(string(b), `"admin":true`) {
		t.Fatalf("me %s", b)
	}
	if r, _ := c.do("GET", "/api/admin/users", nil); r.StatusCode != 200 {
		t.Fatal("the mock admin group must grant admin")
	}
	// 6. the code is single use
	other := newClient(t, ts.URL)
	if r, _ := other.do("GET", cb.Path+"?"+cb.RawQuery, nil); !strings.Contains(r.Header.Get("Location"), "adfs_failed") {
		t.Fatal("a code was replayed")
	}
	// a non-admin mock user
	bob := newClient(t, ts.URL)
	r, _ = bob.do("GET", "/auth/adfs/login", nil)
	au, _ := url.Parse(r.Header.Get("Location"))
	f2 := url.Values{"username": {"bob"}, "password": {"bobpw"}}
	for k := range au.Query() {
		f2.Set(k, au.Query().Get(k))
	}
	resp, _ = bob.hc.PostForm(ts.URL+au.Path, f2)
	cb2, _ := url.Parse(resp.Header.Get("Location"))
	bob.do("GET", cb2.Path+"?"+cb2.RawQuery, nil)
	if _, b := bob.do("GET", "/api/auth/me", nil); !strings.Contains(string(b), `"username":"bob"`) || !strings.Contains(string(b), `"admin":false`) {
		t.Fatalf("bob %s", b)
	}
}

func TestMockADFSUnderBasePath(t *testing.T) {
	_, ts := newApp(t, func(c *config.Config) {
		c.BasePath, c.MockADFS = "/klark", true
		c.MockUsers = []config.MockUser{{Name: "alice", Password: "pw"}}
	})
	c := newClient(t, ts.URL)
	r, _ := c.do("GET", "/klark/auth/adfs/login", nil)
	au, _ := url.Parse(r.Header.Get("Location"))
	if r.StatusCode != 302 || !strings.HasPrefix(au.Path, "/klark/mock-adfs/authorize") {
		t.Fatalf("%d %v", r.StatusCode, r.Header)
	}
	form := url.Values{"username": {"alice"}, "password": {"pw"}}
	for k := range au.Query() {
		form.Set(k, au.Query().Get(k))
	}
	resp, _ := c.hc.PostForm(ts.URL+au.Path, form)
	cb, _ := url.Parse(resp.Header.Get("Location"))
	if cb.Path != "/klark/auth/adfs/callback" {
		t.Fatalf("callback %s", cb)
	}
	r, _ = c.do("GET", cb.Path+"?"+cb.RawQuery, nil)
	if r.Header.Get("Location") != "/klark/" {
		t.Fatalf("%d %v", r.StatusCode, r.Header)
	}
	if _, b := c.do("GET", "/klark/api/auth/me", nil); !strings.Contains(string(b), "alice") {
		t.Fatalf("%s", b)
	}
}

// ---- a fake machine speaking the link over HTTP --------------------------------------------------

type fakeMachine struct {
	t       *testing.T
	base    string
	token   string
	session string
	cancel  func()
	handler func(m *fakeMachine, msg map[string]any)
	mu      sync.Mutex
	got     []map[string]any
	seq     atomic.Int64
}

func connect(t *testing.T, base, token, name string, protocol int, h func(*fakeMachine, map[string]any)) *fakeMachine {
	t.Helper()
	hello, _ := json.Marshal(map[string]any{"type": "hello", "protocol": protocol, "machine_id": "m1", "machine_name": "Mac", "klark_version": "9.9"})
	req, _ := http.NewRequest("POST", base+"/agent/session", bytes.NewReader(hello))
	req.Header.Set("Authorization", "Bearer "+token)
	if name != "" {
		req.Header.Set("X-Klark-Machine", name)
	}
	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		t.Fatal(err)
	}
	var s struct {
		Session string `json:"session_id"`
	}
	_ = json.NewDecoder(resp.Body).Decode(&s)
	resp.Body.Close()
	if resp.StatusCode != 200 || s.Session == "" {
		t.Fatalf("session: %d", resp.StatusCode)
	}
	fm := &fakeMachine{t: t, base: base, token: token, session: s.Session, handler: h}
	done := make(chan struct{})
	req2, _ := http.NewRequest("GET", base+"/agent/events?session="+s.Session+"&after=0", nil)
	req2.Header.Set("Authorization", "Bearer "+token)
	stream, err := http.DefaultClient.Do(req2)
	if err != nil {
		t.Fatal(err)
	}
	fm.cancel = func() { stream.Body.Close(); <-done }
	go func() {
		defer close(done)
		sc := bufio.NewScanner(stream.Body)
		sc.Buffer(make([]byte, 1<<20), 1<<24)
		for sc.Scan() {
			var m map[string]any
			if json.Unmarshal(sc.Bytes(), &m) != nil || m["type"] == "ping" {
				continue
			}
			fm.mu.Lock()
			fm.got = append(fm.got, m)
			fm.mu.Unlock()
			if fm.handler != nil {
				fm.handler(fm, m)
			}
		}
	}()
	return fm
}

func (f *fakeMachine) stop() { f.cancel() }

func (f *fakeMachine) push(msgs ...map[string]any) {
	var buf bytes.Buffer
	for _, m := range msgs {
		b, _ := json.Marshal(m)
		buf.Write(b)
		buf.WriteByte('\n')
	}
	req, _ := http.NewRequest("POST", f.base+"/agent/push?session="+f.session+"&seq="+itoa(int(f.seq.Add(1))), &buf)
	req.Header.Set("Authorization", "Bearer "+f.token)
	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		return
	}
	resp.Body.Close()
}

func (f *fakeMachine) count(typ string) int {
	f.mu.Lock()
	defer f.mu.Unlock()
	n := 0
	for _, m := range f.got {
		if m["type"] == typ {
			n++
		}
	}
	return n
}

func (f *fakeMachine) last(typ string) map[string]any {
	f.mu.Lock()
	defer f.mu.Unlock()
	for i := len(f.got) - 1; i >= 0; i-- {
		if f.got[i]["type"] == typ {
			return f.got[i]
		}
	}
	return nil
}

func waitFor(t *testing.T, what string, cond func() bool) {
	t.Helper()
	for i := 0; i < 300; i++ {
		if cond() {
			return
		}
		time.Sleep(10 * time.Millisecond)
	}
	t.Fatalf("timed out waiting for %s", what)
}

const snapshotJSON = `{"workspaces":[{"workspace_id":"w1","label":"main"}],"tabs":[{"tab_id":"t1","workspace_id":"w1"}],"panes":[{"pane_id":"p1","workspace_id":"w1","tab_id":"t1","agent":"claude","agent_status":"idle","cwd":"/Users/me/app"}]}`

// machineBehaviour answers the way a klark machine does.
func machineBehaviour(treeRev string) func(*fakeMachine, map[string]any) {
	return func(m *fakeMachine, msg map[string]any) {
		id, _ := msg["id"].(string)
		switch msg["type"] {
		case "get_tree":
			if msg["known_rev"] == treeRev {
				m.push(map[string]any{"type": "tree_unchanged", "rev": treeRev})
				return
			}
			var snap any
			_ = json.Unmarshal([]byte(snapshotJSON), &snap)
			m.push(map[string]any{"type": "tree", "rev": treeRev, "snapshot": snap})
		case "call":
			if msg["method"] == "feed.list" {
				m.push(map[string]any{"type": "result", "id": id, "result": map[string]any{"type": "feed_list", "feeds": []map[string]any{
					{"id": "f1", "label": "ci", "kind": "poll", "agents": []string{"work/rev"}, "state": "running", "running": true, "last_delivery": 1}}}})
				return
			}
			if msg["method"] == "pane.read" {
				m.push(map[string]any{"type": "result", "id": id, "result": map[string]any{"read": map[string]any{"text": "screen text"}}})
				return
			}
			m.push(map[string]any{"type": "result", "id": id, "result": map[string]any{"pong": true}})
		case "get_transcript":
			m.push(map[string]any{"type": "result", "id": id, "result": map[string]any{"source": "jsonl", "agent": "claude", "cursor": "2",
				"items": []map[string]any{{"id": "a", "role": "user", "kind": "text", "text": "hello"},
					{"id": "b", "role": "assistant", "kind": "text", "text": "see src/x.rs:3"}}}})
		case "list_agents":
			m.push(map[string]any{"type": "result", "id": id, "result": []map[string]any{{"session": "work", "agent": "claude", "name": "reviewer", "pane_id": "p1", "status": "idle"}}})
		case "deliver":
			m.push(map[string]any{"type": "result", "id": id, "result": map[string]any{"delivered": true}})
		case "get_file":
			p, _ := msg["path"].(string)
			m.push(map[string]any{"type": "result", "id": id, "result": map[string]any{"name": "x.rs", "path": p, "data": base64.StdEncoding.EncodeToString([]byte("fn main() {}\n"))}})
		}
	}
}

func TestMachineDataAPI(t *testing.T) {
	a, ts := newApp(t, nil)
	a.Runtime.TreePoll = 50 * time.Millisecond
	a.Runtime.FeedPoll = 50 * time.Millisecond
	fm := connect(t, ts.URL, machineToken, "", 5, machineBehaviour("rev1"))
	defer fm.stop()
	c := newClient(t, ts.URL)
	c.login("admin", "adminadmin")

	_, b := c.do("GET", "/api/machines", nil)
	var ms []machineOut
	_ = json.Unmarshal(b, &ms)
	if len(ms) != 1 || ms[0].ID != "mac" || !ms[0].Online || ms[0].Protocol != 5 || ms[0].Version != "9.9" || ms[0].Name != "Mac" {
		t.Fatalf("machines %s", b)
	}
	if r, _ := c.do("GET", "/api/machines/ghost/tree", nil); r.StatusCode != 404 {
		t.Fatal("unknown machine")
	}

	// tree: asked once, then cached; ETag is the revision; tree_unchanged keeps the cache
	r, b := c.do("GET", "/api/machines/mac/tree", nil)
	if r.StatusCode != 200 || r.Header.Get("ETag") != `"rev1"` || !strings.Contains(string(b), `"pane_id":"p1"`) {
		t.Fatalf("tree %d %v %s", r.StatusCode, r.Header, b)
	}
	req, _ := http.NewRequest("GET", ts.URL+"/api/machines/mac/tree", nil)
	req.Header.Set("If-None-Match", `"rev1"`)
	resp, _ := c.hc.Do(req)
	if resp.StatusCode != 304 {
		t.Fatalf("etag = %d", resp.StatusCode)
	}
	time.Sleep(1700 * time.Millisecond) // past the freshness window: the hub asks again, with known_rev
	if _, b := c.do("GET", "/api/machines/mac/tree", nil); !strings.Contains(string(b), "p1") {
		t.Fatal("tree lost on tree_unchanged")
	}
	if fm.last("get_tree")["known_rev"] != "rev1" {
		t.Fatalf("get_tree must carry known_rev: %v", fm.last("get_tree"))
	}

	// call: allow-listed only
	if r, b := c.do("POST", "/api/machines/mac/call", map[string]any{"method": "ping"}); r.StatusCode != 200 || !strings.Contains(string(b), "pong") {
		t.Fatalf("call %d %s", r.StatusCode, b)
	}
	if r, _ := c.do("POST", "/api/machines/mac/call", map[string]any{"method": "pane.close"}); r.StatusCode != 403 {
		t.Fatal("mutating call allowed")
	}

	// feeds: listed read-only; the hub cannot start or stop one
	if r, b := c.do("GET", "/api/machines/mac/feeds", nil); r.StatusCode != 200 || !strings.Contains(string(b), `"label":"ci"`) || !strings.Contains(string(b), `"state":"running"`) {
		t.Fatalf("feeds %d %s", r.StatusCode, b)
	}
	if r, _ := c.do("GET", "/api/machines/ghost/feeds", nil); r.StatusCode != 404 {
		t.Fatal("feeds of an unknown machine")
	}
	for _, m := range []string{"feed.start", "feed.stop"} {
		if r, _ := c.do("POST", "/api/machines/mac/call", map[string]any{"method": m, "params": map[string]any{"id": "f1"}}); r.StatusCode != 403 {
			t.Fatalf("%s allowed from the hub", m)
		}
	}

	// chat: structured transcript, rendered, paths linked
	r, b = c.do("GET", "/api/machines/mac/panes/p1/chat", nil)
	var st struct {
		Rows []map[string]any
		Rev  int
	}
	_ = json.Unmarshal(b, &st)
	if r.StatusCode != 200 || len(st.Rows) != 2 || !strings.Contains(string(b), `class=\"fpath\"`) || !strings.Contains(string(b), `m/mac/f?pane=p1`) {
		t.Fatalf("chat %d %s", r.StatusCode, b)
	}

	// input / keys / send
	if r, _ := c.do("POST", "/api/machines/mac/panes/p1/input", map[string]any{"text": "ls"}); r.StatusCode != 204 {
		t.Fatal("input")
	}
	waitFor(t, "input", func() bool { return fm.count("input") == 1 })
	if r, _ := c.do("POST", "/api/machines/mac/panes/p1/keys", map[string]any{"keys": []string{"esc"}}); r.StatusCode != 204 {
		t.Fatal("keys")
	}
	c.do("POST", "/api/machines/mac/panes/p1/send", map[string]any{"text": "fix it"})
	waitFor(t, "send", func() bool { return fm.count("input") == 2 })
	if txt, _ := fm.last("input")["text"].(string); !strings.HasPrefix(txt, "fix it (klark hub:") {
		t.Fatalf("send text %q", txt)
	}

	// messages and agents
	if _, b := c.do("GET", "/api/agents", nil); !strings.Contains(string(b), `"address":"mac/work/reviewer"`) {
		t.Fatalf("agents %s", b)
	}
	if r, b := c.do("POST", "/api/messages", map[string]any{"to": "mac/work/reviewer", "text": "hi there"}); r.StatusCode != 200 {
		t.Fatalf("message %d %s", r.StatusCode, b)
	}
	if r, _ := c.do("POST", "/api/messages", map[string]any{"to": "bad", "text": "x"}); r.StatusCode != 400 {
		t.Fatal("bad address")
	}
	if r, _ := c.do("POST", "/api/messages", map[string]any{"to": "ghost/s/a", "text": "x"}); r.StatusCode != 404 {
		t.Fatal("unknown machine")
	}
	if fm.last("deliver")["text"] != "hi there" {
		t.Fatalf("deliver %v", fm.last("deliver"))
	}
	_, b = c.do("GET", "/api/messages", nil)
	var msgs []messageOut
	_ = json.Unmarshal(b, &msgs)
	if len(msgs) != 2 || msgs[1].OK != true || msgs[0].OK {
		t.Fatalf("messages %s", b)
	}

	// files: viewer data, refusal of secrets and paths outside the folder
	if _, b := c.do("GET", "/api/files/mac?pane=p1&path=src/x.rs", nil); !strings.Contains(string(b), `"view":"code"`) || !strings.Contains(string(b), `"size_label":"13 B"`) || !strings.Contains(string(b), `"lang":"rust"`) {
		t.Fatalf("file %s", b)
	}
	r, b = c.do("GET", "/api/files/mac/raw?pane=p1&path=src/x.rs", nil)
	if string(b) != "fn main() {}\n" || !strings.Contains(r.Header.Get("Content-Security-Policy"), "sandbox") {
		t.Fatalf("raw %q %v", b, r.Header)
	}
	if r, _ := c.do("GET", "/api/files/mac?pane=p1&path=.env", nil); r.StatusCode != 403 {
		t.Fatal("secret fetched")
	}
	if r, _ := c.do("GET", "/api/files/mac?pane=p1&path=/etc/passwd", nil); r.StatusCode != 403 {
		t.Fatal("outside folder fetched")
	}
	if _, b := c.do("GET", "/api/files/mac/markdown?pane=p1&path=src/x.rs", nil); !strings.Contains(string(b), `"html"`) {
		t.Fatalf("markdown %s", b)
	}

	// offline machine
	fm.stop()
	a.Runtime.Revoke("mac")
	if r, _ := c.do("POST", "/api/machines/mac/panes/p1/input", map[string]any{"text": "x"}); r.StatusCode != 503 {
		t.Fatalf("offline input = %d", r.StatusCode)
	}
}

// readEvents reads SSE events from a stream until stop is closed.
type sseEvent struct{ name, data string }

func openSSE(t *testing.T, c *client, path string) (<-chan sseEvent, func()) {
	t.Helper()
	req, _ := http.NewRequest("GET", c.base+path, nil)
	resp, err := c.hc.Do(req)
	if err != nil || resp.StatusCode != 200 {
		t.Fatalf("sse %v %v", err, resp)
	}
	ch := make(chan sseEvent, 256)
	go func() {
		defer close(ch)
		sc := bufio.NewScanner(resp.Body)
		sc.Buffer(make([]byte, 1<<20), 1<<24)
		var ev sseEvent
		for sc.Scan() {
			l := sc.Text()
			switch {
			case strings.HasPrefix(l, "event: "):
				ev.name = l[7:]
			case strings.HasPrefix(l, "data: "):
				ev.data += l[6:]
			case l == "" && ev.name != "":
				ch <- ev
				ev = sseEvent{}
			}
		}
	}()
	return ch, func() { resp.Body.Close() }
}

func next(t *testing.T, ch <-chan sseEvent, name string) sseEvent {
	t.Helper()
	timeout := time.After(5 * time.Second)
	for {
		select {
		case ev, ok := <-ch:
			if !ok {
				t.Fatalf("stream ended waiting for %s", name)
			}
			if ev.name == name {
				return ev
			}
		case <-timeout:
			t.Fatalf("timed out waiting for %s event", name)
		}
	}
}

func TestPaneStreamIsLazyAndCountsViewers(t *testing.T) {
	a, ts := newApp(t, nil)
	a.Runtime.TreePoll = 50 * time.Millisecond
	a.Runtime.FeedPoll = 50 * time.Millisecond
	fm := connect(t, ts.URL, machineToken, "", 5, machineBehaviour("rev1"))
	defer fm.stop()
	c := newClient(t, ts.URL)
	c.login("admin", "adminadmin")

	// a chat-only viewer never makes the machine stream the pane
	chatOnly, closeChat := openSSE(t, c, "/api/machines/mac/panes/p1/events")
	ev := next(t, chatOnly, "chat")
	var ce struct {
		Rows    []map[string]any
		Replace bool
	}
	_ = json.Unmarshal([]byte(ev.data), &ce)
	if len(ce.Rows) != 2 || !ce.Replace {
		t.Fatalf("first chat event %s", ev.data)
	}
	time.Sleep(200 * time.Millisecond)
	if fm.count("subscribe") != 0 {
		t.Fatal("subscribed without anyone watching the terminal")
	}

	// the first terminal viewer subscribes; the second does not; the last one out unsubscribes
	v1, close1 := openSSE(t, c, "/api/machines/mac/panes/p1/events?frames=1")
	waitFor(t, "subscribe", func() bool { return fm.count("subscribe") == 1 })
	v2, close2 := openSSE(t, c, "/api/machines/mac/panes/p1/events?frames=1")
	waitFor(t, "2 viewers", func() bool { return a.Runtime.Viewers("mac", "p1") == 2 })
	if fm.count("subscribe") != 1 {
		t.Fatal("second viewer must not subscribe again")
	}
	reset := next(t, v1, "frame") // the repaint
	if !strings.Contains(reset.data, `"reset":true`) {
		t.Fatalf("first frame must be a reset: %s", reset.data)
	}
	fm.push(map[string]any{"type": "pane_size", "pane_id": "p1", "cols": 100, "rows": 30})
	fm.push(map[string]any{"type": "frame", "pane_id": "p1", "bytes": base64.StdEncoding.EncodeToString([]byte("hello\r\n"))})
	if sz := next(t, v1, "size"); !strings.Contains(sz.data, `"cols":100`) {
		t.Fatalf("size %s", sz.data)
	}
	for _, v := range []<-chan sseEvent{v1, v2} {
		f := next(t, v, "frame")
		for strings.Contains(f.data, `"reset":true`) {
			f = next(t, v, "frame")
		}
		var d struct{ Data string }
		_ = json.Unmarshal([]byte(f.data), &d)
		if raw, _ := base64.StdEncoding.DecodeString(d.Data); string(raw) != "hello\r\n" {
			t.Fatalf("frame %q", raw)
		}
	}
	close1()
	time.Sleep(150 * time.Millisecond)
	if fm.count("unsubscribe") != 0 {
		t.Fatal("unsubscribed while a viewer remains")
	}
	close2()
	waitFor(t, "unsubscribe", func() bool { return fm.count("unsubscribe") == 1 })
	closeChat()

	// a tree stream pulls the tree only while open
	before := fm.count("get_tree")
	time.Sleep(300 * time.Millisecond)
	if fm.count("get_tree") != before {
		t.Fatal("the hub kept asking for the tree with nobody watching")
	}
}
