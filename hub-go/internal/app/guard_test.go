package app

import (
	"encoding/json"
	"strings"
	"testing"

	"github.com/muthuishere/klark/hub-go/internal/auth"
	"github.com/muthuishere/klark/hub-go/internal/store"
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
