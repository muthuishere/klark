package app

import (
	"crypto/ed25519"
	"crypto/rand"
	"encoding/base64"
	"encoding/json"
	"net/http"
	"net/url"
	"strings"
	"testing"
	"time"

	"github.com/muthuishere/klark/hub-go/internal/auth"
	"github.com/muthuishere/klark/hub-go/internal/config"
	"github.com/muthuishere/klark/hub-go/internal/store"
)

func signTicket(t *testing.T, key ed25519.PrivateKey, tk auth.Ticket) string {
	t.Helper()
	payload, _ := json.Marshal(tk)
	return base64.RawURLEncoding.EncodeToString(payload) + "." + base64.RawURLEncoding.EncodeToString(ed25519.Sign(key, payload))
}

// K2: a platform user opens the hub with no second login; a forged, expired,
// replayed or misdirected ticket is refused.
func TestPlatformTicketSignIn(t *testing.T) {
	pub, priv, _ := ed25519.GenerateKey(rand.Reader)
	otherPub, otherPriv, _ := ed25519.GenerateKey(rand.Reader)
	_, strangerPriv, _ := ed25519.GenerateKey(rand.Reader)
	const origin = "https://app.liveconv.example"
	const aud = "https://hub.example"
	a, ts := newApp(t, func(c *config.Config) {
		c.PlatformKeys = map[string]ed25519.PublicKey{"liveconv": pub, "other": otherPub}
		c.PlatformAudience = aud
		c.PlatformOrigins = map[string]string{"liveconv": origin, "other": "https://other.example"}
	})
	_, _ = a.Store.CreateOwnedTenant(store.Tenant{ID: "acme"}, store.ServiceToken{ID: "st1", Issuer: "liveconv"})
	_, _ = a.Store.CreateOwnedTenant(store.Tenant{ID: "globex"}, store.ServiceToken{ID: "st2", Issuer: "other"})

	n := 0
	ticket := func(mut func(*auth.Ticket)) auth.Ticket {
		n++
		now := time.Now().Unix()
		tk := auth.Ticket{Issuer: "liveconv", Audience: aud, Subject: "priya", Name: "Priya", Tenant: "acme", Role: "member",
			IssuedAt: now, Expires: now + 60, ID: "t-" + string(rune('a'+n))}
		if mut != nil {
			mut(&tk)
		}
		return tk
	}
	post := func(c *client, raw, next, from string) *http.Response {
		form := url.Values{"t": {raw}, "next": {next}}
		req, _ := http.NewRequest("POST", ts.URL+"/auth/ticket", strings.NewReader(form.Encode()))
		req.Header.Set("Content-Type", "application/x-www-form-urlencoded")
		if from != "" {
			req.Header.Set("Origin", from)
		}
		r, err := c.hc.Do(req)
		if err != nil {
			t.Fatal(err)
		}
		r.Body.Close()
		return r
	}
	open := func(raw, next string) (*client, *http.Response) {
		c := newClient(t, ts.URL)
		return c, post(c, raw, next, origin)
	}

	good := signTicket(t, priv, ticket(nil))
	c, r := open(good, "/m/acme-box")
	if r.StatusCode != 303 || r.Header.Get("Location") != "/m/acme-box" {
		t.Fatalf("good ticket: %d %s", r.StatusCode, r.Header.Get("Location"))
	}
	_, b := c.do("GET", "/api/auth/me", nil)
	var me map[string]any
	_ = json.Unmarshal(b, &me)
	if me["username"] != "priya" || me["tenant"] != "acme" || me["role"] != "member" || me["admin"] != false {
		t.Fatalf("me = %s", b)
	}
	if r, _ := c.do("GET", "/api/machines", nil); r.StatusCode != 200 {
		t.Fatal("signed-in platform user cannot list machines")
	}

	refused := func(name, raw string) {
		t.Helper()
		c, r := open(raw, "/")
		if r.StatusCode != 303 || !strings.Contains(r.Header.Get("Location"), "error=ticket") {
			t.Fatalf("%s: %d %s", name, r.StatusCode, r.Header.Get("Location"))
		}
		if r, _ := c.do("GET", "/api/auth/me", nil); r.StatusCode != 401 {
			t.Fatalf("%s: signed in anyway", name)
		}
	}
	refused("replayed", good)
	refused("forged", signTicket(t, strangerPriv, ticket(nil)))
	refused("another issuer's tenant", signTicket(t, otherPriv, ticket(func(k *auth.Ticket) { k.Issuer = "other" })))
	refused("tampered", strings.Replace(good, good[:10], "eyJpc3MiOiJ4", 1))
	refused("expired", signTicket(t, priv, ticket(func(k *auth.Ticket) { k.IssuedAt -= 300; k.Expires = time.Now().Unix() - 1 })))
	refused("long-lived", signTicket(t, priv, ticket(func(k *auth.Ticket) { k.Expires = k.IssuedAt + 3600 })))
	refused("other hub", signTicket(t, priv, ticket(func(k *auth.Ticket) { k.Audience = "https://other.example" })))
	refused("unknown issuer", signTicket(t, priv, ticket(func(k *auth.Ticket) { k.Issuer = "mallory" })))
	refused("unknown tenant", signTicket(t, priv, ticket(func(k *auth.Ticket) { k.Tenant = "nope" })))
	refused("default tenant", signTicket(t, priv, ticket(func(k *auth.Ticket) { k.Tenant = "default"; k.Role = "admin" })))
	refused("bad role", signTicket(t, priv, ticket(func(k *auth.Ticket) { k.Role = "root" })))
	refused("garbage", "not-a-ticket")

	// The bad tickets above count against this address, as they should; the
	// limiter blocks the next try, so start the remaining checks fresh.
	_, blockedRes := open(signTicket(t, priv, ticket(nil)), "/")
	if !strings.Contains(blockedRes.Header.Get("Location"), "error=ticket") {
		t.Fatal("many bad tickets from one address were not rate limited")
	}
	a.Auth.Limiter = auth.NewLimiter(100, time.Minute)

	// next stays on this hub.
	for _, next := range []string{"//evil.example/x", "https://evil.example", "/\\evil.example", "/\t/evil.example", "/\n/evil.example", "/ /evil.example", "/x\\..\\/evil.example", "/%09/ok"} {
		if next == "/%09/ok" {
			continue // percent-encoded is a plain path segment, allowed
		}
		_, r := open(signTicket(t, priv, ticket(nil)), next)
		if loc := r.Header.Get("Location"); loc != "/" {
			t.Fatalf("next %q redirected to %q", next, loc)
		}
	}

	// A platform admin is an admin of its tenant, never the hub operator.
	c, _ = open(signTicket(t, priv, ticket(func(k *auth.Ticket) { k.Role = "admin" })), "/")
	if r, _ := c.do("GET", "/api/admin/users", nil); r.StatusCode != 200 {
		t.Fatal("platform admin cannot manage its tenant")
	}
	if r, _ := c.do("GET", "/api/admin/analytics", nil); r.StatusCode != 403 {
		t.Fatal("platform admin reached operator settings")
	}

	// Deleting the tenant ends its platform sessions.
	_ = a.Store.DeleteTenant("acme")
	if r, _ := c.do("GET", "/api/auth/me", nil); r.StatusCode != 401 {
		t.Fatal("session outlived its tenant")
	}
}

// Without HUB_PLATFORM_KEYS the route does not exist.
func TestTicketRouteOffWithoutKeys(t *testing.T) {
	_, ts := newApp(t, nil)
	if r, _ := newClient(t, ts.URL).do("POST", "/auth/ticket", nil); r.StatusCode != 404 {
		t.Fatalf("ticket route without keys: %d", r.StatusCode)
	}
}

// A ticket is only accepted from the platform's own page, never by a link,
// and never replaces another person's session in the same browser.
func TestTicketOnlyFromThePlatformsPage(t *testing.T) {
	pub, priv, _ := ed25519.GenerateKey(rand.Reader)
	const aud, origin = "https://hub.example", "https://app.liveconv.example"
	a, ts := newApp(t, func(c *config.Config) {
		c.PlatformKeys = map[string]ed25519.PublicKey{"liveconv": pub}
		c.PlatformAudience = aud
		c.PlatformOrigins = map[string]string{"liveconv": origin}
	})
	_, _ = a.Store.CreateOwnedTenant(store.Tenant{ID: "acme"}, store.ServiceToken{ID: "st", Issuer: "liveconv"})
	n := 0
	tk := func(sub string) string {
		n++
		now := time.Now().Unix()
		return signTicket(t, priv, auth.Ticket{Issuer: "liveconv", Audience: aud, Subject: sub, Tenant: "acme", Role: "member",
			IssuedAt: now, Expires: now + 60, ID: "id-" + string(rune('a'+n))})
	}
	send := func(c *client, method, raw, from string) *http.Response {
		var req *http.Request
		if method == "GET" {
			req, _ = http.NewRequest("GET", ts.URL+"/auth/ticket?t="+raw, nil)
		} else {
			req, _ = http.NewRequest("POST", ts.URL+"/auth/ticket", strings.NewReader(url.Values{"t": {raw}}.Encode()))
			req.Header.Set("Content-Type", "application/x-www-form-urlencoded")
		}
		if from != "" {
			req.Header.Set("Origin", from)
		}
		r, _ := c.hc.Do(req)
		r.Body.Close()
		return r
	}
	signedIn := func(c *client) string {
		_, b := c.do("GET", "/api/auth/me", nil)
		var me map[string]any
		_ = json.Unmarshal(b, &me)
		s, _ := me["username"].(string)
		return s
	}
	if lc := newClient(t, ts.URL); send(lc, "GET", tk("priya"), origin) != nil && signedIn(lc) != "" {
		t.Fatal("a link signed someone in")
	}
	for _, from := range []string{"", "https://evil.example", "https://app.liveconv.example.evil.example"} {
		c := newClient(t, ts.URL)
		send(c, "POST", tk("priya"), from)
		if signedIn(c) != "" {
			t.Fatalf("a form from %q signed someone in", from)
		}
	}
	c := newClient(t, ts.URL)
	send(c, "POST", tk("priya"), origin)
	if signedIn(c) != "priya" {
		t.Fatal("the platform's own form did not sign in")
	}
	send(c, "POST", tk("mallory"), origin)
	if signedIn(c) != "priya" {
		t.Fatal("a ticket replaced another person's session")
	}
	// A platform session lasts an hour, not the hub's 12.
	r := send(newClient(t, ts.URL), "POST", tk("sam"), origin)
	for _, ck := range r.Cookies() {
		if ck.Name == auth.CookieName && ck.MaxAge > 3600 {
			t.Fatalf("platform session lasts %d s", ck.MaxAge)
		}
	}
}
