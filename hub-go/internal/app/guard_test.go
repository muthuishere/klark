package app

import (
	"encoding/json"
	"strings"
	"testing"

	"github.com/muthuishere/klark/hub-go/internal/auth"
	"github.com/muthuishere/klark/hub-go/internal/store"
	"github.com/muthuishere/klark/hub-go/internal/tokens"
)

func TestViewerWatchesButCannotAct(t *testing.T) {
	a, ts := newApp(t, nil)
	fm := connect(t, ts.URL, machineToken, "", 5, machineBehaviour("r1"))
	defer fm.stop()
	_ = a.Store.UpsertUser(store.User{Name: "vic", Hash: mustHash(t, "password1"), Source: "local", Role: "viewer"})
	c := newClient(t, ts.URL)
	c.login("vic", "password1")
	if r, _ := c.do("GET", "/api/machines/mac/tree", nil); r.StatusCode != 200 {
		t.Fatal("viewer cannot watch")
	}
	for _, p := range []string{"input", "keys", "send", "upload"} {
		if r, _ := c.do("POST", "/api/machines/mac/panes/p1/"+p, map[string]any{"text": "x", "keys": []string{"enter"}}); r.StatusCode != 403 {
			t.Fatalf("viewer %s = %d", p, r.StatusCode)
		}
	}
	if r, _ := c.do("POST", "/api/messages", map[string]string{"to": "mac/default/rev", "text": "hi"}); r.StatusCode != 403 {
		t.Fatal("viewer sent a message")
	}
	if u := (auth.User{Role: "viewer", Admin: true}); u.ReadOnly() {
		t.Fatal("an admin is never read-only")
	}
}

func TestEveryActionAndFileReadIsAudited(t *testing.T) {
	a, ts := newApp(t, nil)
	fm := connect(t, ts.URL, machineToken, "", 5, machineBehaviour("r1"))
	defer fm.stop()
	c := newClient(t, ts.URL)
	c.login("admin", "adminadmin")
	c.do("POST", "/api/machines/mac/panes/p1/input", map[string]string{"text": "ls"})
	c.do("POST", "/api/machines/mac/panes/p1/keys", map[string]any{"keys": []string{"enter"}})
	c.do("POST", "/api/messages", map[string]string{"to": "mac/default/rev", "text": "hi"})
	c.do("GET", "/api/files/nope", nil)
	_, b := c.do("GET", "/api/admin/audit", nil)
	var rows []store.AuditRow
	_ = json.Unmarshal(b, &rows)
	got := map[string]bool{}
	for _, r := range rows {
		got[r.Action] = true
		if r.Actor != "admin" {
			t.Fatalf("actor %q", r.Actor)
		}
	}
	for _, want := range []string{"input", "keys", "message", "file"} {
		if !got[want] {
			t.Fatalf("no audit row for %s: %s", want, b)
		}
	}
	if strings.Contains(string(b), `"ls"`) {
		t.Fatal("the audit log holds content")
	}

	// A tenant admin sees only its tenant's rows.
	_, _ = a.Store.CreateTenant(store.Tenant{ID: "acme"})
	_ = a.Store.UpsertUser(store.User{Name: "alice", Hash: mustHash(t, "password1"), Admin: true, Source: "local", Tenant: "acme"})
	al := newClient(t, ts.URL)
	al.login("alice", "password1")
	if _, b := al.do("GET", "/api/admin/audit", nil); strings.Contains(string(b), `"admin"`) {
		t.Fatalf("alice sees default's audit: %s", b)
	}
}

func TestActionBudgetPerUser(t *testing.T) {
	t.Setenv("HUB_USER_ACTIONS_PER_MINUTE", "3")
	_, ts := newApp(t, nil)
	fm := connect(t, ts.URL, machineToken, "", 5, machineBehaviour("r1"))
	defer fm.stop()
	c := newClient(t, ts.URL)
	c.login("admin", "adminadmin")
	codes := []int{}
	for i := 0; i < 4; i++ {
		r, _ := c.do("POST", "/api/machines/mac/panes/p1/keys", map[string]any{"keys": []string{"enter"}})
		codes = append(codes, r.StatusCode)
	}
	if codes[3] != 429 || codes[0] == 429 {
		t.Fatalf("codes %v", codes)
	}
}

// Requests for another tenant's machine are answered as unknown before any
// budget is charged, so they cannot exhaust that machine's budget.
func TestOtherTenantsCannotSpendAMachinesBudget(t *testing.T) {
	t.Setenv("HUB_MACHINE_ACTIONS_PER_MINUTE", "3")
	a, ts := newApp(t, nil)
	for _, id := range []string{"acme", "globex"} {
		_, _ = a.Store.CreateTenant(store.Tenant{ID: id})
	}
	_ = a.Store.UpsertUser(store.User{Name: "alice", Hash: mustHash(t, "password1"), Source: "local", Tenant: "acme"})
	_ = a.Store.UpsertUser(store.User{Name: "bob", Hash: mustHash(t, "password1"), Source: "local", Tenant: "globex"})
	_, _ = a.Store.AddMachine(store.Machine{Name: "gbox", SHA256: tokens.Sha256Hex("g-token"), Tenant: "globex"})
	a.Tokens.Invalidate()
	fm := connect(t, ts.URL, "g-token", "", 5, machineBehaviour("r1"))
	defer fm.stop()
	alice := newClient(t, ts.URL)
	alice.login("alice", "password1")
	for i := 0; i < 5; i++ {
		if r, _ := alice.do("POST", "/api/machines/gbox/panes/p1/keys", map[string]any{"keys": []string{"enter"}}); r.StatusCode != 404 {
			t.Fatalf("alice reached gbox: %d", r.StatusCode)
		}
	}
	bob := newClient(t, ts.URL)
	bob.login("bob", "password1")
	if r, _ := bob.do("POST", "/api/machines/gbox/panes/p1/keys", map[string]any{"keys": []string{"enter"}}); r.StatusCode == 429 {
		t.Fatal("another tenant spent gbox's budget")
	}
}

// The operator's actions on a tenant's machine show in that tenant's log.
func TestTenantLogShowsTheOperatorsActionsOnItsMachines(t *testing.T) {
	a, ts := newApp(t, nil)
	_, _ = a.Store.CreateTenant(store.Tenant{ID: "acme"})
	_ = a.Store.UpsertUser(store.User{Name: "alice", Hash: mustHash(t, "password1"), Admin: true, Source: "local", Tenant: "acme"})
	_, _ = a.Store.AddMachine(store.Machine{Name: "abox", SHA256: tokens.Sha256Hex("a-token"), Tenant: "acme"})
	a.Tokens.Invalidate()
	fm := connect(t, ts.URL, "a-token", "", 5, machineBehaviour("r1"))
	defer fm.stop()
	op := newClient(t, ts.URL)
	op.login("admin", "adminadmin")
	if r, _ := op.do("POST", "/api/machines/abox/panes/p1/keys", map[string]any{"keys": []string{"enter"}}); r.StatusCode >= 300 {
		t.Fatalf("operator keys %d", r.StatusCode)
	}
	alice := newClient(t, ts.URL)
	alice.login("alice", "password1")
	_, b := alice.do("GET", "/api/admin/audit", nil)
	if !strings.Contains(string(b), `"actor":"admin"`) || !strings.Contains(string(b), `"machine":"abox"`) {
		t.Fatalf("acme's log misses the operator's action: %s", b)
	}
}
