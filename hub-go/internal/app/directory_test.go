package app

import (
	"encoding/json"
	"errors"
	"net/url"
	"strings"
	"testing"

	"github.com/muthuishere/klark/hub-go/internal/auth"
	"github.com/muthuishere/klark/hub-go/internal/config"
)

func status(c *client, user, pw string) int {
	r, _ := c.do("POST", "/api/auth/login", map[string]string{"username": user, "password": pw})
	return r.StatusCode
}

func TestLocalIsTheDefaultAndMockLDAPIsOptIn(t *testing.T) {
	_, ts := newApp(t, nil)
	c := newClient(t, ts.URL)
	_, b := c.do("GET", "/api/auth/providers", nil)
	var p struct {
		Static    bool
		Directory struct{ Mode, Name string }
		Adfs      struct{ Enabled bool }
		Oidc      []any
	}
	if err := json.Unmarshal(b, &p); err != nil || !p.Static || p.Directory.Mode != "local" || p.Directory.Name != "Local" || p.Adfs.Enabled || p.Oidc == nil {
		t.Fatalf("providers %s", b)
	}
	if status(c, "user1", "user1pass") != 401 || status(newClient(t, ts.URL), "admin", "adminadmin") != 200 {
		t.Fatal("default: only the built-in admin, no mock users")
	}
	// opt in with HUB_AUTH_MODE=mockldap
	_, ts2 := newApp(t, func(c *config.Config) { c.Directory.Mode = "mockldap" })
	c2 := newClient(t, ts2.URL)
	_, b = c2.do("GET", "/api/auth/providers", nil)
	if err := json.Unmarshal(b, &p); err != nil || p.Directory.Mode != "mockldap" || p.Directory.Name != "Mock LDAP" {
		t.Fatalf("providers %s", b)
	}
	if status(c2, "user1", "user1pass") != 200 || status(newClient(t, ts2.URL), "user2", "user2pass") != 200 {
		t.Fatal("seeded mock users must sign in")
	}
	if status(newClient(t, ts2.URL), "user1", "wrong") != 401 || status(newClient(t, ts2.URL), "admin", "adminadmin") != 200 {
		t.Fatal("mock/builtin")
	}
	_, b = c2.do("GET", "/api/auth/me", nil)
	if !strings.Contains(string(b), `"username":"user1"`) || !strings.Contains(string(b), `"admin":false`) {
		t.Fatalf("me %s", b)
	}
}

func TestMockLDAPAdminManagementEnvSeedAndGroups(t *testing.T) {
	a, ts := newApp(t, func(c *config.Config) {
		c.Directory.Mode = "mockldap"
		c.MockLDAPUsers = []config.MockLDAPUser{{Name: "ann", Password: "annpassword", Admin: true, Groups: []string{"devs"}}, {Name: "bo", Password: "bopassword", Groups: []string{"ops"}}}
	})
	// the env seed replaces the defaults
	if status(newClient(t, ts.URL), "user1", "user1pass") != 401 {
		t.Fatal("defaults must not be seeded when HUB_MOCK_USERS is set")
	}
	ann := newClient(t, ts.URL)
	if status(ann, "ann", "annpassword") != 200 {
		t.Fatal("ann")
	}
	if _, b := ann.do("GET", "/api/auth/me", nil); !strings.Contains(string(b), `"admin":true`) {
		t.Fatalf("ann %s", b)
	}
	// the hash is stored, never the password
	u, _ := a.Store.User("ann")
	if u.Source != "mockldap" || !strings.HasPrefix(u.Hash, "$2") || strings.Contains(u.Hash, "annpassword") || len(u.Groups) != 1 {
		t.Fatalf("%+v", u)
	}
	// the admin adds, changes and deletes mock users
	if r, b := ann.do("POST", "/api/admin/users", map[string]any{"username": "cy", "password": "cypassword", "source": "mockldap", "groups": []string{"guests"}}); r.StatusCode != 200 || !strings.Contains(string(b), `"source":"mockldap"`) || !strings.Contains(string(b), `"groups":["guests"]`) {
		t.Fatalf("%d %s", r.StatusCode, b)
	}
	if status(newClient(t, ts.URL), "cy", "cypassword") != 200 {
		t.Fatal("cy")
	}
	ann.do("POST", "/api/admin/users", map[string]any{"username": "cy", "password": "newpassword"})
	if status(newClient(t, ts.URL), "cy", "cypassword") != 401 || status(newClient(t, ts.URL), "cy", "newpassword") != 200 {
		t.Fatal("password change")
	}
	if u, _ := a.Store.User("cy"); u.Source != "mockldap" || len(u.Groups) != 1 {
		t.Fatalf("an update must keep source and groups: %+v", u)
	}
	if r, _ := ann.do("POST", "/api/admin/users", map[string]any{"username": "z", "password": "zzzzzzzz", "source": "bogus"}); r.StatusCode != 400 {
		t.Fatal("bad source")
	}
	// allowed groups apply to the mock directory too
	if r, _ := ann.do("PUT", "/api/admin/directory", map[string]any{"mode": "mockldap", "allowedGroups": []string{"OPS"}}); r.StatusCode != 200 {
		t.Fatal("put")
	}
	if status(newClient(t, ts.URL), "bo", "bopassword") != 200 || status(newClient(t, ts.URL), "cy", "newpassword") != 401 {
		t.Fatal("allowed groups")
	}
	if r, _ := ann.do("DELETE", "/api/admin/users/cy", nil); r.StatusCode != 204 {
		t.Fatal("delete")
	}
	if status(newClient(t, ts.URL), "cy", "newpassword") != 401 {
		t.Fatal("deleted user signs in")
	}
}

func TestDefaultMockUsersStayDeleted(t *testing.T) {
	a, ts := newApp(t, nil)
	c := newClient(t, ts.URL)
	c.login("admin", "adminadmin")
	c.do("DELETE", "/api/admin/users/user1", nil)
	if err := a.Auth.Seed(); err != nil { // a restart
		t.Fatal(err)
	}
	if status(newClient(t, ts.URL), "user1", "user1pass") != 401 {
		t.Fatal("user1 came back")
	}
}

func TestLocalModeHasNoDirectory(t *testing.T) {
	_, ts := newApp(t, func(c *config.Config) { c.Directory.Mode = "local" })
	c := newClient(t, ts.URL)
	if status(c, "user1", "user1pass") != 401 || status(newClient(t, ts.URL), "admin", "adminadmin") != 200 {
		t.Fatal("local mode")
	}
	if _, b := c.do("GET", "/api/auth/providers", nil); !strings.Contains(string(b), `"mode":"local"`) {
		t.Fatalf("%s", b)
	}
}

func fakeLDAP(a *App, down *bool) {
	const dn = "uid=lee,ou=people,dc=example,dc=com"
	a.Auth.Dial = func(config.Directory) (auth.LDAPConn, error) {
		if *down {
			return nil, errors.New("refused")
		}
		return fakeLDAPConn{dn}, nil
	}
}

type fakeLDAPConn struct{ dn string }

func (f fakeLDAPConn) Bind(dn, pw string) error {
	if dn == f.dn && pw == "leepw" || dn == "cn=svc,dc=example,dc=com" && pw == "svcsecret" {
		return nil
	}
	return auth.ErrBadCredentials
}
func (f fakeLDAPConn) Search(base string, scope int, _ string, _ []string) ([]auth.LDAPEntry, error) {
	if scope == auth.ScopeBase && base == f.dn {
		return []auth.LDAPEntry{{DN: f.dn, Attrs: map[string][]string{"displayname": {"Lee L"}, "memberof": {"CN=klark-admins,OU=g,DC=example,DC=com", "CN=eng,OU=g,DC=example,DC=com"}}}}, nil
	}
	return nil, nil
}
func (f fakeLDAPConn) Close() {}

func TestLDAPModeSignInSessionAndAdminAPI(t *testing.T) {
	down := false
	a, ts := newApp(t, func(c *config.Config) {
		c.Directory = config.Directory{Host: "ldap.example.com", UserDNTemplate: "uid={0},ou=people,dc=example,dc=com",
			UseMemberOf: true, AdminGroup: "klark-admins", BindDN: "cn=svc,dc=example,dc=com", BindPassword: "svcsecret"}
	})
	fakeLDAP(a, &down)
	c := newClient(t, ts.URL)
	if _, b := c.do("GET", "/api/auth/providers", nil); !strings.Contains(string(b), `"mode":"ldap"`) || !strings.Contains(string(b), `"name":"LDAP"`) {
		t.Fatalf("%s", b)
	}
	// the mock users do not exist outside mockldap mode; the built-in admin still does
	if status(c, "user1", "user1pass") != 401 {
		t.Fatal("mock user signed in under ldap")
	}
	if status(newClient(t, ts.URL), "admin", "adminadmin") != 200 {
		t.Fatal("builtin admin")
	}
	if status(newClient(t, ts.URL), "lee", "wrong") != 401 {
		t.Fatal("bad password")
	}
	if r, _ := c.do("POST", "/api/auth/login", map[string]string{"username": "lee", "password": "leepw"}); r.StatusCode != 200 {
		t.Fatal("lee")
	}
	_, b := c.do("GET", "/api/auth/me", nil)
	if !strings.Contains(string(b), `"displayName":"Lee L"`) || !strings.Contains(string(b), `"admin":true`) {
		t.Fatalf("me %s", b)
	}
	if u, err := a.Store.User("lee"); err == nil {
		t.Fatalf("an LDAP user must not be stored: %+v", u)
	}
	// directory down: 503, and it does not count as a failed password
	down = true
	if status(newClient(t, ts.URL), "lee", "leepw") != 503 {
		t.Fatal("outage")
	}
	down = false

	// admin settings API: the secret is never returned
	adm := newClient(t, ts.URL)
	adm.login("admin", "adminadmin")
	_, b = adm.do("GET", "/api/admin/directory", nil)
	if strings.Contains(string(b), "svcsecret") || !strings.Contains(string(b), `"bindPasswordSet":true`) || !strings.Contains(string(b), `"source":"env"`) {
		t.Fatalf("directory %s", b)
	}
	// test connection: current settings, and given settings (service password falls back to the current one)
	_, b = adm.do("POST", "/api/admin/directory/test", map[string]any{"username": "lee", "password": "leepw"})
	if !strings.Contains(string(b), `"ok":true`) || !strings.Contains(string(b), "Lee L") {
		t.Fatalf("test %s", b)
	}
	_, b = adm.do("POST", "/api/admin/directory/test", map[string]any{"username": "lee", "password": "no"})
	if !strings.Contains(string(b), `"ok":false`) {
		t.Fatalf("test %s", b)
	}
	_, b = adm.do("POST", "/api/admin/directory/test", map[string]any{"username": "", "settings": map[string]any{"host": "h", "bindDn": "cn=svc,dc=example,dc=com", "userDnTemplate": "uid={0},dc=x"}})
	if !strings.Contains(string(b), `"ok":true`) || !strings.Contains(string(b), "service account") {
		t.Fatalf("service test %s", b)
	}
	// PUT: validation, then override with allowed groups; the stored secret is kept when none is sent
	if r, _ := adm.do("PUT", "/api/admin/directory", map[string]any{"mode": "ldap", "host": "h", "userDnTemplate": "nope"}); r.StatusCode != 400 {
		t.Fatal("template without {0} accepted")
	}
	put := map[string]any{"mode": "ldap", "host": "ldap.example.com", "bindDn": "cn=svc,dc=example,dc=com", "userDnTemplate": "uid={0},ou=people,dc=example,dc=com",
		"useMemberOf": true, "adminGroup": "klark-admins", "allowedGroups": []string{"eng"}}
	r, b := adm.do("PUT", "/api/admin/directory", put)
	if r.StatusCode != 200 || strings.Contains(string(b), "svcsecret") || !strings.Contains(string(b), `"source":"admin"`) || !strings.Contains(string(b), `"bindPasswordSet":true`) {
		t.Fatalf("put %d %s", r.StatusCode, b)
	}
	if status(newClient(t, ts.URL), "lee", "leepw") != 200 {
		t.Fatal("lee in eng")
	}
	put["allowedGroups"] = []string{"nobody"}
	// clearing the stored secret falls back to HUB_LDAP_BIND_PASSWORD (still set in this test's environment)
	put["clearBindPassword"] = true
	_, b = adm.do("PUT", "/api/admin/directory", put)
	if !strings.Contains(string(b), `"bindPasswordSet":true`) {
		t.Fatalf("%s", b)
	}
	if v, _ := a.Store.Setting("directory"); strings.Contains(v, "bindPassword") {
		t.Fatalf("stored secret not cleared: %s", v)
	}
	if status(newClient(t, ts.URL), "lee", "leepw") != 401 {
		t.Fatal("allowed groups must refuse lee")
	}
	// reset: the environment applies again
	if _, b = adm.do("DELETE", "/api/admin/directory", nil); !strings.Contains(string(b), `"source":"env"`) {
		t.Fatalf("%s", b)
	}
	// switching to mockldap from the admin page seeds the mock users
	adm.do("PUT", "/api/admin/directory", map[string]any{"mode": "mockldap"})
	if status(newClient(t, ts.URL), "user1", "user1pass") != 200 {
		t.Fatal("mock users after switching")
	}
}

func TestLDAPLoginSharesTheRateLimiter(t *testing.T) {
	down := false
	a, ts := newApp(t, func(c *config.Config) {
		c.Directory = config.Directory{Host: "h", UserDNTemplate: "uid={0},ou=people,dc=example,dc=com"}
	})
	fakeLDAP(a, &down)
	c := newClient(t, ts.URL)
	for i := 0; i < 5; i++ {
		if status(c, "lee", "bad") != 401 {
			t.Fatal("401 expected")
		}
	}
	if status(c, "lee", "leepw") != 429 {
		t.Fatal("limiter must lock lee out")
	}
}

func TestAdminDirectoryNeedsAdminAndCSRF(t *testing.T) {
	_, ts := newApp(t, func(c *config.Config) { c.Directory.Mode = "mockldap" })
	u := newClient(t, ts.URL)
	u.login("user1", "user1pass")
	for _, p := range [][2]string{{"GET", "/api/admin/directory"}, {"PUT", "/api/admin/directory"}, {"DELETE", "/api/admin/directory"}, {"POST", "/api/admin/directory/test"}} {
		if r, _ := u.do(p[0], p[1], map[string]any{}); r.StatusCode != 403 {
			t.Errorf("%v = %d", p, r.StatusCode)
		}
	}
}

func TestSecondOIDCProviderAgainstTheMockIdP(t *testing.T) {
	a, ts := newApp(t, func(c *config.Config) {
		c.MockADFS = true
		c.MockUsers = []config.MockUser{{Name: "alice", Password: "alicepw", Admin: true}, {Name: "bob", Password: "bobpw"}}
	})
	// the second provider's issuer is the mock IdP reached over real HTTP
	a.Auth.Cfg.OIDCProviders = []config.OIDCProvider{{ID: "corp", Name: "Corp SSO", Issuer: ts.URL + "/mock-adfs", ClientID: auth.MockClientID,
		ClientSecret: auth.MockClientSecret, Scopes: "openid profile", AdminGroup: "klark-admins"}}
	c := newClient(t, ts.URL)
	_, b := c.do("GET", "/api/auth/providers", nil)
	if !strings.Contains(string(b), `"oidc":[{"id":"corp","loginUrl":"auth/oidc/corp/login","name":"Corp SSO"}]`) || !strings.Contains(string(b), `"loginUrl":"auth/adfs/login"`) {
		t.Fatalf("providers %s", b)
	}
	if r, _ := c.do("GET", "/auth/oidc/nope/login", nil); r.StatusCode != 404 {
		t.Fatal("unknown provider")
	}
	signIn := func(user, pw string) *client {
		cl := newClient(t, ts.URL)
		r, _ := cl.do("GET", "/auth/oidc/corp/login?return=/messages", nil)
		if r.StatusCode != 302 {
			t.Fatalf("login = %d", r.StatusCode)
		}
		au, _ := url.Parse(r.Header.Get("Location"))
		q := au.Query()
		if q.Get("scope") != "openid profile" || !strings.HasSuffix(q.Get("redirect_uri"), "/auth/oidc/corp/callback") || q.Get("code_challenge_method") != "S256" {
			t.Fatalf("authorize %s", au)
		}
		form := url.Values{"username": {user}, "password": {pw}}
		for k := range q {
			form.Set(k, q.Get(k))
		}
		resp, err := cl.hc.PostForm(au.String(), form)
		if err != nil || resp.StatusCode != 302 {
			t.Fatalf("idp %v %v", resp, err)
		}
		cb, _ := url.Parse(resp.Header.Get("Location"))
		if cb.Path != "/auth/oidc/corp/callback" {
			t.Fatalf("callback %s", cb)
		}
		r, _ = cl.do("GET", cb.Path+"?"+cb.RawQuery, nil)
		if r.StatusCode != 302 || r.Header.Get("Location") != "/messages" {
			t.Fatalf("callback = %d %v", r.StatusCode, r.Header)
		}
		return cl
	}
	al := signIn("alice", "alicepw")
	if _, b := al.do("GET", "/api/auth/me", nil); !strings.Contains(string(b), `"username":"alice"`) || !strings.Contains(string(b), `"admin":true`) {
		t.Fatalf("alice %s", b)
	}
	if _, b := signIn("bob", "bobpw").do("GET", "/api/auth/me", nil); !strings.Contains(string(b), `"admin":false`) {
		t.Fatalf("bob %s", b)
	}
	// a state minted for one provider is refused by another's callback
	r, _ := c.do("GET", "/auth/adfs/login", nil)
	au, _ := url.Parse(r.Header.Get("Location"))
	form := url.Values{"username": {"alice"}, "password": {"alicepw"}}
	for k := range au.Query() {
		form.Set(k, au.Query().Get(k))
	}
	resp, _ := c.hc.PostForm(ts.URL+au.Path, form)
	cb, _ := url.Parse(resp.Header.Get("Location"))
	r, _ = c.do("GET", "/auth/oidc/corp/callback?"+cb.RawQuery, nil)
	if !strings.Contains(r.Header.Get("Location"), "error=oidc_failed&provider=corp") {
		t.Fatalf("cross-provider state accepted: %v", r.Header)
	}
	// the admin page shows each redirect URI
	adm := newClient(t, ts.URL)
	adm.login("admin", "adminadmin")
	_, b = adm.do("GET", "/api/admin/directory", nil)
	if !strings.Contains(string(b), `"redirectUri":"`+ts.URL+`/auth/oidc/corp/callback"`) || !strings.Contains(string(b), `"redirectUri":"`+ts.URL+`/auth/adfs/callback"`) ||
		!strings.Contains(string(b), `"clientSecretSet":true`) || strings.Contains(string(b), auth.MockClientSecret) {
		t.Fatalf("admin view %s", b)
	}
}
