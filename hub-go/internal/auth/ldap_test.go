package auth

import (
	"errors"
	"strings"
	"sync"
	"testing"

	"github.com/muthuishere/klark/hub-go/internal/config"
)

// fakeDir is an in-memory directory behind the LDAPConn interface.
type fakeDir struct {
	mu       sync.Mutex
	passwd   map[string]string // DN -> password
	entries  map[string]LDAPEntry
	searches []string // "base|filter"
	binds    []string
	// search answers a search; nil = no results.
	search func(base, filter string) []LDAPEntry
	down   bool
}

type fakeConn struct{ d *fakeDir }

func (d *fakeDir) dial(config.Directory) (LDAPConn, error) {
	if d.down {
		return nil, errors.New("connection refused")
	}
	return fakeConn{d}, nil
}

func (c fakeConn) Bind(dn, pw string) error {
	c.d.mu.Lock()
	defer c.d.mu.Unlock()
	c.d.binds = append(c.d.binds, dn)
	if p, ok := c.d.passwd[dn]; ok && p == pw && pw != "" {
		return nil
	}
	return ErrBadCredentials
}

func (c fakeConn) Search(base string, scope int, filter string, _ []string) ([]LDAPEntry, error) {
	c.d.mu.Lock()
	defer c.d.mu.Unlock()
	c.d.searches = append(c.d.searches, base+"|"+filter)
	if scope == ScopeBase {
		if e, ok := c.d.entries[base]; ok {
			return []LDAPEntry{e}, nil
		}
		return nil, errors.New("no such object")
	}
	if c.d.search != nil {
		return c.d.search(base, filter), nil
	}
	return nil, nil
}

func (c fakeConn) Close() {}

func entry(dn string, kv ...string) LDAPEntry {
	e := LDAPEntry{DN: dn, Attrs: map[string][]string{}}
	for i := 0; i+1 < len(kv); i += 2 {
		e.Attrs[strings.ToLower(kv[i])] = append(e.Attrs[strings.ToLower(kv[i])], kv[i+1])
	}
	return e
}

const aliceDN = "uid=alice,ou=people,dc=example,dc=com"

func newFake() *fakeDir {
	return &fakeDir{
		passwd: map[string]string{aliceDN: "alicepw", "cn=svc,dc=example,dc=com": "svcpw", "alice@corp.example.com": "alicepw"},
		entries: map[string]LDAPEntry{aliceDN: entry(aliceDN, "displayName", "Alice A", "cn", "alice",
			"memberOf", "CN=klark-admins,OU=Groups,DC=example,DC=com", "memberOf", "CN=staff,OU=Groups,DC=example,DC=com")},
	}
}

func TestLDAPDirectBindMemberOfAndAdminGroup(t *testing.T) {
	f := newFake()
	d := config.Directory{Host: "h", UserDNTemplate: "uid={0},ou=people,dc=example,dc=com", UseMemberOf: true, AdminGroup: "klark-admins"}
	u, err := LDAPAuthenticate(f.dial, d, "alice", "alicepw")
	if err != nil || u == nil {
		t.Fatalf("%v %v", u, err)
	}
	if u.Display != "Alice A" || !u.Admin || !AnyGroup(u.Groups, []string{"STAFF"}) {
		t.Fatalf("%+v", u)
	}
	// wrong password, unknown user, empty password (the anonymous-bind trap): all (nil, nil), and the empty one never reaches the server
	for _, c := range [][2]string{{"alice", "nope"}, {"bob", "x"}, {"alice", ""}, {"", "x"}} {
		if u, err := LDAPAuthenticate(f.dial, d, c[0], c[1]); u != nil || err != nil {
			t.Fatalf("%v: %+v %v", c, u, err)
		}
	}
	n := len(f.binds)
	LDAPAuthenticate(f.dial, d, "alice", "")
	if len(f.binds) != n {
		t.Fatal("an empty password reached the server")
	}
	// not admin when the admin group does not match
	d.AdminGroup = "other"
	if u, _ := LDAPAuthenticate(f.dial, d, "alice", "alicepw"); u == nil || u.Admin {
		t.Fatalf("%+v", u)
	}
}

func TestLDAPDNEscapingInTemplate(t *testing.T) {
	f := newFake()
	d := config.Directory{Host: "h", UserDNTemplate: "uid={0},ou=people,dc=example,dc=com"}
	LDAPAuthenticate(f.dial, d, "x,ou=admins", "pw")
	if got := f.binds[len(f.binds)-1]; strings.Contains(got, "x,ou=admins") || !strings.Contains(got, `x\,ou=admins`) {
		t.Fatalf("DN not escaped: %s", got)
	}
}

func TestLDAPSearchThenBindWithGroupSearch(t *testing.T) {
	f := newFake()
	f.search = func(base, filter string) []LDAPEntry {
		switch {
		case base == "ou=people,dc=example,dc=com" && filter == "(uid=alice)":
			return []LDAPEntry{entry(aliceDN, "cn", "alice")}
		case base == "ou=groups,dc=example,dc=com" && filter == "(member="+aliceDN+")":
			return []LDAPEntry{entry("cn=devs,ou=groups,dc=example,dc=com", "cn", "devs"), entry("cn=ops,ou=groups,dc=example,dc=com", "cn", "ops")}
		case base == "ou=people,dc=example,dc=com" && strings.Contains(filter, "dup"):
			return []LDAPEntry{entry("a"), entry("b")}
		}
		return nil
	}
	d := config.Directory{Host: "h", BindDN: "cn=svc,dc=example,dc=com", BindPassword: "svcpw", UserSearchBase: "ou=people,dc=example,dc=com",
		GroupSearchBase: "ou=groups,dc=example,dc=com", GroupFilter: "(member={0})", AdminGroup: "ops", DisplayNameAttr: "cn"}
	u, err := LDAPAuthenticate(f.dial, d, "alice", "alicepw")
	if err != nil || u == nil {
		t.Fatalf("%v %v", u, err)
	}
	if !u.Admin || !AnyGroup(u.Groups, []string{"devs"}) || u.Display != "alice" {
		t.Fatalf("%+v", u)
	}
	if u, err := LDAPAuthenticate(f.dial, d, "dup", "x"); u != nil || err != nil {
		t.Fatalf("ambiguous user must not sign in: %v %v", u, err)
	}
	// a hostile name is escaped in the filter, not interpreted
	f.searches = nil
	LDAPAuthenticate(f.dial, d, "*)(uid=*", "x")
	if len(f.searches) == 0 || strings.Contains(f.searches[0], "(uid=*)") || !strings.Contains(f.searches[0], `\2a\29\28uid=\2a`) {
		t.Fatalf("filter not escaped: %v", f.searches)
	}
	// a service account with the wrong password is a directory problem, not a wrong user password
	d.BindPassword = "bad"
	if _, err := LDAPAuthenticate(f.dial, d, "alice", "alicepw"); err == nil {
		t.Fatal("bad service bind must be an error")
	}
}

func TestLDAPUPNTemplateAndOutage(t *testing.T) {
	f := newFake()
	d := config.Directory{Host: "h", UserDNTemplate: "{0}@corp.example.com"}
	u, err := LDAPAuthenticate(f.dial, d, "alice", "alicepw")
	if err != nil || u == nil || u.Display != "alice" {
		t.Fatalf("%+v %v", u, err)
	}
	f.down = true
	if u, err := LDAPAuthenticate(f.dial, d, "alice", "alicepw"); u != nil || err == nil {
		t.Fatal("an outage is an error")
	}
	if _, err := LDAPAuthenticate(f.dial, config.Directory{}, "a", "b"); err == nil {
		t.Fatal("no host must be an error")
	}
}

func TestValidateDirectory(t *testing.T) {
	bad := []config.Directory{{Mode: "x"}, {Port: 70000}, {SSL: true, StartTLS: true}, {UserDNTemplate: "uid=x"}, {UserFilter: "(uid=x)"}, {GroupFilter: "(a=b)"}}
	for i, d := range bad {
		if ValidateDirectory(d) == nil {
			t.Errorf("%d accepted", i)
		}
	}
	if err := ValidateDirectory(config.Directory{Mode: "ldap", UserFilter: "(uid={0})", GroupFilter: "(memberUid={1})"}); err != nil {
		t.Fatal(err)
	}
}
