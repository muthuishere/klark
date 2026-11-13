package app

import (
	"encoding/json"
	"net/http"
	"strings"
	"testing"

	"github.com/muthuishere/klark/hub-go/internal/auth"
	"github.com/muthuishere/klark/hub-go/internal/config"
	"github.com/muthuishere/klark/hub-go/internal/store"
	"github.com/muthuishere/klark/hub-go/internal/tokens"
)

// svc calls the provisioning API with a service token.
func svc(t *testing.T, base, token, method, path string, body any) (int, map[string]any) {
	t.Helper()
	c := newClient(t, base)
	req, _ := http.NewRequest(method, base+path, jsonBody(body))
	req.Header.Set("Content-Type", "application/json")
	if token != "" {
		req.Header.Set("Authorization", "Bearer "+token)
	}
	resp, err := c.hc.Do(req)
	if err != nil {
		t.Fatal(err)
	}
	defer resp.Body.Close()
	var out map[string]any
	_ = json.NewDecoder(resp.Body).Decode(&out)
	return resp.StatusCode, out
}

func jsonBody(v any) *strings.Reader {
	if v == nil {
		return strings.NewReader("")
	}
	b, _ := json.Marshal(v)
	return strings.NewReader(string(b))
}

// K3: a platform provisions a tenant in one call, pairs a machine with a
// one-time code, lists and revokes it, and deletes the tenant; its token
// reaches only the tenants it created.
func TestProvisioningAPI(t *testing.T) {
	a, ts := newApp(t, func(c *config.Config) { c.PublicURL = "https://hub.example" })
	op := newClient(t, ts.URL)
	op.login("admin", "adminadmin")
	mint := func(name string) string {
		r, b := op.do("POST", "/api/admin/service-tokens", map[string]string{"name": name})
		var out map[string]string
		_ = json.Unmarshal(b, &out)
		if r.StatusCode != 201 || !strings.HasPrefix(out["token"], "khs_") {
			t.Fatalf("mint %d %s", r.StatusCode, b)
		}
		return out["token"]
	}
	liveconv, other := mint("liveconv"), mint("other")

	if code, _ := svc(t, ts.URL, "", "POST", "/api/provision/tenants", map[string]string{"id": "acme"}); code != 401 {
		t.Fatalf("no token = %d", code)
	}
	if code, out := svc(t, ts.URL, liveconv, "POST", "/api/provision/tenants", map[string]string{"id": "acme", "name": "Acme"}); code != 201 || out["id"] != "acme" {
		t.Fatalf("create %d %v", code, out)
	}
	if code, _ := svc(t, ts.URL, liveconv, "POST", "/api/provision/tenants", map[string]string{"id": "acme"}); code != 409 {
		t.Fatal("duplicate tenant")
	}
	for _, bad := range []string{"default", "A", "x", "has space", "-lead"} {
		if code, _ := svc(t, ts.URL, liveconv, "POST", "/api/provision/tenants", map[string]string{"id": bad}); code != 400 {
			t.Fatalf("tenant id %q accepted", bad)
		}
	}

	// Another service token cannot see or use acme.
	for _, rt := range [][2]string{
		{"POST", "/api/provision/tenants/acme/pairing-codes"},
		{"GET", "/api/provision/tenants/acme/machines"},
		{"DELETE", "/api/provision/tenants/acme"},
	} {
		if code, _ := svc(t, ts.URL, other, rt[0], rt[1], nil); code != 404 {
			t.Fatalf("other token %s %s = %d", rt[0], rt[1], code)
		}
	}

	// Pairing: a code, redeemed once by a machine, joins it to acme.
	code, out := svc(t, ts.URL, liveconv, "POST", "/api/provision/tenants/acme/pairing-codes", map[string]int{"ttl_seconds": 600})
	pc, _ := out["code"].(string)
	if code != 201 || len(pc) != 19 || !strings.Contains(out["connect"].(string), "--code "+pc) {
		t.Fatalf("pairing code %d %v", code, out)
	}
	status, red := svc(t, ts.URL, "", "POST", "/api/connect/redeem", map[string]string{"code": strings.ToLower(pc), "name": "laptop"})
	tok, _ := red["token"].(string)
	if status != 200 || tok == "" || red["tenant"] != "acme" {
		t.Fatalf("redeem %d %v", status, red)
	}
	if status, _ := svc(t, ts.URL, "", "POST", "/api/connect/redeem", map[string]string{"code": pc, "name": "laptop2"}); status != 403 {
		t.Fatal("a code was redeemed twice")
	}
	if a.Store.MachineTenant("laptop") != "acme" {
		t.Fatal("laptop not in acme")
	}
	fm := connect(t, ts.URL, tok, "", 5, machineBehaviour("r1"))
	defer fm.stop()
	waitFor(t, "laptop online", func() bool { return a.Runtime.Online("laptop") })
	req, _ := http.NewRequest("GET", ts.URL+"/api/provision/tenants/acme/machines", nil)
	req.Header.Set("Authorization", "Bearer "+liveconv)
	resp, _ := http.DefaultClient.Do(req)
	var ms []map[string]any
	_ = json.NewDecoder(resp.Body).Decode(&ms)
	resp.Body.Close()
	if len(ms) != 1 || ms[0]["name"] != "laptop" || ms[0]["online"] != true {
		t.Fatalf("machines %v", ms)
	}

	// The operator does not see acme's machine under 'default' users; an acme user does.
	_ = a.Store.UpsertUser(store.User{Name: "eve", Hash: mustHash(t, "password1"), Source: "local", Tenant: "acme"})
	eve := newClient(t, ts.URL)
	eve.login("eve", "password1")
	if _, b := eve.do("GET", "/api/machines", nil); !strings.Contains(string(b), `"id":"laptop"`) || strings.Contains(string(b), `"id":"mac"`) {
		t.Fatalf("eve sees %s", b)
	}

	// A name another tenant uses is refused, and the code is not spent.
	_, _ = a.Store.CreateTenant(store.Tenant{ID: "globex"})
	_ = a.Store.UpsertMachine("taken", tokens.Sha256Hex("x"), "admin", "globex")
	_, out = svc(t, ts.URL, liveconv, "POST", "/api/provision/tenants/acme/pairing-codes", nil)
	pc2 := out["code"].(string)
	if status, _ := svc(t, ts.URL, "", "POST", "/api/connect/redeem", map[string]string{"code": pc2, "name": "taken"}); status != 409 {
		t.Fatalf("name clash = %d", status)
	}
	if status, _ := svc(t, ts.URL, "", "POST", "/api/connect/redeem", map[string]string{"code": pc2, "name": "laptop-b"}); status != 200 {
		t.Fatal("code was spent by the refused clash")
	}

	// Revoke the machine: its link drops and its token stops working.
	if status, _ := svc(t, ts.URL, liveconv, "DELETE", "/api/provision/tenants/acme/machines/taken", nil); status != 404 {
		t.Fatal("revoked another tenant's machine")
	}
	if status, _ := svc(t, ts.URL, liveconv, "DELETE", "/api/provision/tenants/acme/machines/laptop", nil); status != 204 {
		t.Fatal("revoke")
	}
	waitFor(t, "laptop dropped", func() bool { return !a.Runtime.Online("laptop") })
	if tokens.Match(a.Tokens.Machines(), tok) != nil {
		t.Fatal("revoked token still matches")
	}

	// Delete the tenant: machines and users go; the token is then refused when revoked.
	if status, _ := svc(t, ts.URL, liveconv, "DELETE", "/api/provision/tenants/acme", nil); status != 204 {
		t.Fatal("delete tenant")
	}
	if a.Store.TenantExists("acme") || a.Store.MachineTenant("laptop-b") == "acme" {
		t.Fatal("tenant left behind")
	}
	if _, err := a.Store.User("eve"); err == nil {
		t.Fatal("tenant user left behind")
	}
	_, b := op.do("GET", "/api/admin/service-tokens", nil)
	var list []map[string]any
	_ = json.Unmarshal(b, &list)
	for _, st := range list {
		if st["name"] == "liveconv" {
			op.do("DELETE", "/api/admin/service-tokens/"+st["id"].(string), nil)
		}
	}
	if status, _ := svc(t, ts.URL, liveconv, "POST", "/api/provision/tenants", map[string]string{"id": "acme2"}); status != 401 {
		t.Fatal("revoked service token still works")
	}
}

// Service tokens are minted by the hub operator only, not a tenant's admin.
func TestServiceTokensAreOperatorOnly(t *testing.T) {
	a, ts := newApp(t, nil)
	_, _ = a.Store.CreateTenant(store.Tenant{ID: "acme"})
	_ = a.Store.UpsertUser(store.User{Name: "alice", Hash: mustHash(t, "password1"), Admin: true, Source: "local", Tenant: "acme"})
	c := newClient(t, ts.URL)
	c.login("alice", "password1")
	if r, _ := c.do("POST", "/api/admin/service-tokens", map[string]string{"name": "x"}); r.StatusCode != 403 {
		t.Fatalf("tenant admin minted a service token: %d", r.StatusCode)
	}
}

func mustHash(t *testing.T, pw string) string {
	t.Helper()
	h, err := auth.HashPassword(pw)
	if err != nil {
		t.Fatal(err)
	}
	return h
}
