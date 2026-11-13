package app

import (
	"encoding/json"
	"strings"
	"testing"
	"time"

	"github.com/muthuishere/klark/hub-go/internal/auth"
	"github.com/muthuishere/klark/hub-go/internal/store"
	"github.com/muthuishere/klark/hub-go/internal/tokens"
)

// K1: two tenants on one hub cannot see or touch each other's machines,
// users or messages; the operator (an admin of the default tenant) sees both.
func TestTenantsCannotSeeEachOther(t *testing.T) {
	a, ts := newApp(t, nil)
	a.Runtime.TreePoll = 50 * time.Millisecond
	for _, id := range []string{"acme", "globex"} {
		if ok, err := a.Store.CreateTenant(store.Tenant{ID: id, Name: id}); !ok || err != nil {
			t.Fatalf("tenant %s: %v", id, err)
		}
	}
	hash, _ := auth.HashPassword("password1")
	for _, u := range []store.User{
		{Name: "alice", Hash: hash, Admin: true, Source: "local", Tenant: "acme"},
		{Name: "bob", Hash: hash, Source: "local", Tenant: "globex"},
	} {
		if err := a.Store.UpsertUser(u); err != nil {
			t.Fatal(err)
		}
	}
	for name, tenant := range map[string]string{"acme-box": "acme", "globex-box": "globex"} {
		if ok, err := a.Store.AddMachine(store.Machine{Name: name, SHA256: tokens.Sha256Hex(name + "-token"), CreatedBy: "admin", Tenant: tenant}); !ok || err != nil {
			t.Fatalf("machine %s: %v", name, err)
		}
	}
	a.Tokens.Invalidate()
	fa := connect(t, ts.URL, "acme-box-token", "", 5, machineBehaviour("r1"))
	defer fa.stop()
	fg := connect(t, ts.URL, "globex-box-token", "", 5, machineBehaviour("r1"))
	defer fg.stop()

	ids := func(c *client) []string {
		_, b := c.do("GET", "/api/machines", nil)
		var ms []machineOut
		_ = json.Unmarshal(b, &ms)
		out := []string{}
		for _, m := range ms {
			out = append(out, m.ID)
		}
		return out
	}

	alice := newClient(t, ts.URL)
	alice.login("alice", "password1")
	if got := ids(alice); strings.Join(got, ",") != "acme-box" {
		t.Fatalf("alice sees %v", got)
	}
	bob := newClient(t, ts.URL)
	bob.login("bob", "password1")
	if got := ids(bob); strings.Join(got, ",") != "globex-box" {
		t.Fatalf("bob sees %v", got)
	}

	// Every machine route answers 404 for another tenant's machine, as if absent.
	for _, rt := range [][2]string{
		{"GET", "/api/machines/globex-box/tree"},
		{"GET", "/api/machines/globex-box/feeds"},
		{"POST", "/api/machines/globex-box/call"},
		{"GET", "/api/machines/globex-box/panes/p1/chat"},
		{"POST", "/api/machines/globex-box/panes/p1/input"},
		{"POST", "/api/machines/globex-box/panes/p1/keys"},
		{"POST", "/api/machines/globex-box/panes/p1/send"},
	} {
		if r, b := alice.do(rt[0], rt[1], map[string]any{"method": "ping", "text": "x", "keys": []string{"enter"}}); r.StatusCode != 404 {
			t.Fatalf("alice %s %s = %d %s", rt[0], rt[1], r.StatusCode, b)
		}
	}
	if r, b := alice.do("GET", "/api/machines/acme-box/tree", nil); r.StatusCode != 200 {
		t.Fatalf("own machine %d %s", r.StatusCode, b)
	}

	// Messages: cannot reach the other tenant, and the log is per tenant.
	if r, b := alice.do("POST", "/api/messages", map[string]string{"to": "globex-box/default/rev", "text": "hi"}); r.StatusCode != 404 {
		t.Fatalf("message across tenants %d %s", r.StatusCode, b)
	}
	if r, _ := bob.do("POST", "/api/messages", map[string]string{"to": "globex-box/default/rev", "text": "for globex"}); r.StatusCode != 200 {
		t.Fatal("own message")
	}
	if _, b := alice.do("GET", "/api/messages", nil); strings.Contains(string(b), "for globex") {
		t.Fatalf("alice sees globex messages: %s", b)
	}
	if _, b := alice.do("GET", "/api/agents", nil); strings.Contains(string(b), "globex-box") {
		t.Fatalf("alice sees globex agents: %s", b)
	}

	// Admin: alice manages acme only.
	if _, b := alice.do("GET", "/api/admin/users", nil); strings.Contains(string(b), `"username":"bob"`) || strings.Contains(string(b), `"username":"admin"`) {
		t.Fatalf("alice's user list: %s", b)
	}
	if _, b := alice.do("GET", "/api/admin/machines", nil); strings.Contains(string(b), "globex-box") || strings.Contains(string(b), `"mac"`) {
		t.Fatalf("alice's machine list: %s", b)
	}
	if r, _ := alice.do("DELETE", "/api/admin/machines/globex-box", nil); r.StatusCode != 404 {
		t.Fatal("alice revoked globex's machine")
	}
	if r, _ := alice.do("DELETE", "/api/admin/users/bob", nil); r.StatusCode != 404 {
		t.Fatal("alice deleted globex's user")
	}
	if r, _ := alice.do("POST", "/api/admin/users", map[string]any{"username": "bob", "password": "password2"}); r.StatusCode != 409 {
		t.Fatal("alice took over bob")
	}
	if r, _ := alice.do("POST", "/api/admin/users", map[string]any{"username": "carol", "password": "password3", "tenant": "globex"}); r.StatusCode != 403 {
		t.Fatal("alice created a user in globex")
	}
	if r, _ := alice.do("POST", "/api/admin/users", map[string]any{"username": "carol", "password": "password3"}); r.StatusCode != 200 {
		t.Fatal("alice could not add to her own tenant")
	}
	if u, _ := a.Store.User("carol"); u.Tenant != "acme" {
		t.Fatalf("carol is in %q", u.Tenant)
	}
	if r, _ := alice.do("POST", "/api/admin/machines", map[string]any{"name": "acme-2"}); r.StatusCode != 201 {
		t.Fatal("alice could not add a machine")
	}
	if a.Store.MachineTenant("acme-2") != "acme" {
		t.Fatal("new machine not in acme")
	}
	for _, p := range []string{"/api/admin/analytics", "/api/admin/storage", "/api/admin/directory"} {
		if r, _ := alice.do("GET", p, nil); r.StatusCode != 403 {
			t.Fatalf("tenant admin reached %s", p)
		}
	}

	// The operator sees every tenant.
	op := newClient(t, ts.URL)
	op.login("admin", "adminadmin")
	if got := strings.Join(ids(op), ","); !strings.Contains(got, "acme-box") || !strings.Contains(got, "globex-box") || !strings.Contains(got, "mac") {
		t.Fatalf("operator sees %v", got)
	}
	if _, b := op.do("GET", "/api/messages", nil); !strings.Contains(string(b), "for globex") {
		t.Fatalf("operator message log: %s", b)
	}
}

// A machine approved through `klark hub connect` joins the approver's tenant,
// and an admin of one tenant cannot re-approve (take over) another's machine.
func TestApprovedMachineJoinsTheApproversTenant(t *testing.T) {
	a, _ := newApp(t, nil)
	_, _ = a.Store.CreateTenant(store.Tenant{ID: "acme"})
	now := time.Now().UnixMilli()
	for _, c := range []store.ConnectRequest{
		{PollSHA: "p1", Code: "AAAA-1111", Machine: "laptop", Status: "pending", Created: now, Expires: now + 60000},
	} {
		if ok, err := a.Store.AddConnectRequest(c); !ok || err != nil {
			t.Fatal(err)
		}
	}
	if ok, _ := a.Store.DecideConnectRequest("AAAA-1111", "approved", "alice", "acme", now); !ok {
		t.Fatal("decide")
	}
	req, _ := a.Store.ConnectRequestByCode("AAAA-1111")
	if err := a.Store.UpsertMachine(req.Machine, tokens.Sha256Hex("t"), req.DecidedBy, req.Tenant); err != nil {
		t.Fatal(err)
	}
	if got := a.Store.MachineTenant("laptop"); got != "acme" {
		t.Fatalf("laptop is in %q", got)
	}
	globexAdmin := auth.User{Name: "g", Admin: true, Tenant: "globex"}
	if globexAdmin.Sees(a.Store.MachineTenant("laptop")) {
		t.Fatal("globex admin sees acme's laptop")
	}
}
