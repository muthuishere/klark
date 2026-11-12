package auth

import (
	"crypto/tls"
	"errors"
	"fmt"
	"net"
	"strconv"
	"strings"
	"time"

	"github.com/go-ldap/ldap/v3"

	"github.com/muthuishere/klark/hub-go/internal/config"
)

// ErrBadCredentials is what an LDAPConn returns from Bind when the server says invalidCredentials.
var ErrBadCredentials = errors.New("invalid credentials")

// Search scopes.
const (
	ScopeBase    = 0
	ScopeSubtree = 2
)

// LDAPEntry is one search result; attribute names are lower case.
type LDAPEntry struct {
	DN    string
	Attrs map[string][]string
}

func (e LDAPEntry) first(attr string) string {
	if v := e.Attrs[strings.ToLower(attr)]; len(v) > 0 {
		return v[0]
	}
	return ""
}

// LDAPConn is the small slice of an LDAP client the sign-in needs (tests fake it).
type LDAPConn interface {
	// Bind returns ErrBadCredentials for invalidCredentials; any other error is a directory problem.
	Bind(dn, password string) error
	Search(base string, scope int, filter string, attrs []string) ([]LDAPEntry, error)
	Close()
}

// Dialer opens a connection for the settings (TLS / StartTLS included, not yet bound).
type Dialer func(d config.Directory) (LDAPConn, error)

type realConn struct{ c *ldap.Conn }

// DialLDAP is the production Dialer, on github.com/go-ldap/ldap/v3.
func DialLDAP(d config.Directory) (LDAPConn, error) {
	port := d.Port
	if port == 0 {
		port = 389
		if d.SSL {
			port = 636
		}
	}
	timeout := time.Duration(orInt(d.TimeoutSeconds, 10)) * time.Second
	host := strings.TrimSpace(d.Host)
	tc := &tls.Config{ServerName: host, MinVersion: tls.VersionTLS12, InsecureSkipVerify: d.SkipCertCheck} //nolint:gosec // operator's explicit choice
	scheme := "ldap"
	if d.SSL {
		scheme = "ldaps"
	}
	c, err := ldap.DialURL(scheme+"://"+net.JoinHostPort(host, strconv.Itoa(port)),
		ldap.DialWithDialer(&net.Dialer{Timeout: timeout}), ldap.DialWithTLSConfig(tc))
	if err != nil {
		return nil, err
	}
	c.SetTimeout(timeout)
	if d.StartTLS && !d.SSL {
		if err := c.StartTLS(tc); err != nil {
			c.Close()
			return nil, err
		}
	}
	return realConn{c}, nil
}

func (r realConn) Bind(dn, pw string) error {
	err := r.c.Bind(dn, pw)
	if ldap.IsErrorWithCode(err, ldap.LDAPResultInvalidCredentials) {
		return ErrBadCredentials
	}
	return err
}

func (r realConn) Search(base string, scope int, filter string, attrs []string) ([]LDAPEntry, error) {
	res, err := r.c.Search(ldap.NewSearchRequest(base, scope, ldap.NeverDerefAliases, 0, 0, false, filter, attrs, nil))
	if err != nil {
		return nil, err
	}
	out := make([]LDAPEntry, 0, len(res.Entries))
	for _, e := range res.Entries {
		le := LDAPEntry{DN: e.DN, Attrs: map[string][]string{}}
		for _, a := range e.Attributes {
			le.Attrs[strings.ToLower(a.Name)] = a.Values
		}
		out = append(out, le)
	}
	return out, nil
}

func (r realConn) Close() { _ = r.c.Close() }

func orInt(v, d int) int {
	if v <= 0 {
		return d
	}
	return v
}

// DirUser is who the directory vouched for.
type DirUser struct {
	Name, Display string
	Groups        []string
	Admin         bool
}

// ValidateDirectory reports a setting that can never work.
func ValidateDirectory(d config.Directory) error {
	switch strings.ToLower(strings.TrimSpace(d.Mode)) {
	case "", "mockldap", "ldap", "local":
	default:
		return fmt.Errorf("mode must be mockldap, ldap or local")
	}
	if d.Port < 0 || d.Port > 65535 {
		return errors.New("port must be 0-65535")
	}
	if d.TimeoutSeconds < 0 || d.TimeoutSeconds > 300 {
		return errors.New("timeoutSeconds must be 0-300")
	}
	if d.SSL && d.StartTLS {
		return errors.New("choose ldaps (ssl) or startTls, not both")
	}
	if d.UserDNTemplate != "" && !strings.Contains(d.UserDNTemplate, "{0}") {
		return errors.New("userDnTemplate must contain {0}")
	}
	if d.UserFilter != "" && !strings.Contains(d.UserFilter, "{0}") {
		return errors.New("userFilter must contain {0}")
	}
	if d.GroupFilter != "" && !strings.Contains(d.GroupFilter, "{0}") && !strings.Contains(d.GroupFilter, "{1}") {
		return errors.New("groupFilter must contain {0} (the user's DN) or {1} (the user name)")
	}
	return nil
}

// cnOf turns "CN=klark-admins,OU=Groups,DC=corp" into "klark-admins" ("" when the first RDN is not a cn).
func cnOf(dn string) string {
	first, _, _ := strings.Cut(dn, ",")
	k, v, ok := strings.Cut(first, "=")
	if ok && strings.EqualFold(strings.TrimSpace(k), "cn") {
		return strings.TrimSpace(v)
	}
	return ""
}

func inList(list []string, v string) bool {
	for _, x := range list {
		if strings.EqualFold(strings.TrimSpace(x), v) {
			return true
		}
	}
	return false
}

// AnyGroup reports whether groups contains any of wanted (case-insensitive; a wanted name also matches a full DN).
func AnyGroup(groups, wanted []string) bool {
	for _, w := range wanted {
		if w = strings.TrimSpace(w); w != "" && inList(groups, w) {
			return true
		}
	}
	return false
}

// LDAPAuthenticate checks a user the way the old hub did: the server verifies the password by a bind as the
// user (direct bind from a DN template, or search-then-bind), then groups come from a group search and/or
// memberOf. It returns (nil, nil) for a wrong name or password and an error for directory trouble. Allowed
// groups are NOT applied here. The password is never stored.
func LDAPAuthenticate(dial Dialer, d config.Directory, username, password string) (*DirUser, error) {
	if username == "" || password == "" || strings.ContainsRune(username, 0) || strings.ContainsRune(password, 0) {
		return nil, nil // an empty password would be an anonymous bind on many servers
	}
	if strings.TrimSpace(d.Host) == "" {
		return nil, errors.New("no LDAP host is configured")
	}
	if d.UserSearchBase == "" && d.UserDNTemplate == "" {
		return nil, errors.New("set userDnTemplate or userSearchBase")
	}
	var dn string
	var entry *LDAPEntry
	nameAttr := strings.TrimSpace(d.DisplayNameAttr)
	attrs := []string{"displayName", "cn", "memberOf"}
	if nameAttr != "" {
		attrs = append(attrs, nameAttr)
	}

	if d.UserSearchBase != "" {
		finder, err := dial(d)
		if err != nil {
			return nil, err
		}
		defer finder.Close()
		if d.BindDN != "" {
			if err := finder.Bind(d.BindDN, d.BindPassword); err != nil {
				return nil, fmt.Errorf("service account bind: %w", err)
			}
		}
		filter := strings.ReplaceAll(orStr(d.UserFilter, "(uid={0})"), "{0}", ldap.EscapeFilter(username))
		found, err := finder.Search(d.UserSearchBase, ScopeSubtree, filter, attrs)
		if err != nil {
			return nil, err
		}
		if len(found) != 1 {
			return nil, nil
		}
		entry, dn = &found[0], found[0].DN
	} else {
		dn = strings.ReplaceAll(d.UserDNTemplate, "{0}", ldap.EscapeDN(username))
	}

	conn, err := dial(d)
	if err != nil {
		return nil, err
	}
	defer conn.Close()
	if err := conn.Bind(dn, password); err != nil {
		if errors.Is(err, ErrBadCredentials) {
			return nil, nil
		}
		return nil, err
	}
	if entry == nil {
		// A UPN bind ("user@corp") is not a DN we can read; the entry is then simply unknown.
		if res, err := conn.Search(dn, ScopeBase, "(objectClass=*)", attrs); err == nil && len(res) > 0 {
			entry = &res[0]
		}
	}

	u := &DirUser{Name: username, Display: username}
	if entry != nil {
		for _, a := range []string{nameAttr, "displayName", "cn"} {
			if a != "" && entry.first(a) != "" {
				u.Display = entry.first(a)
				break
			}
		}
	}
	seen := map[string]bool{}
	add := func(g string) {
		if g != "" && !seen[strings.ToLower(g)] {
			seen[strings.ToLower(g)] = true
			u.Groups = append(u.Groups, g)
		}
	}
	if d.UseMemberOf && entry != nil {
		for _, m := range entry.Attrs["memberof"] {
			add(cnOf(m))
			add(m)
		}
	}
	if d.GroupSearchBase != "" {
		memberDN := dn
		if entry != nil {
			memberDN = entry.DN
		}
		filter := orStr(d.GroupFilter, "(|(member={0})(uniqueMember={0})(memberUid={1}))")
		filter = strings.ReplaceAll(strings.ReplaceAll(filter, "{0}", ldap.EscapeFilter(memberDN)), "{1}", ldap.EscapeFilter(username))
		gc := conn
		if d.BindDN != "" { // a service account usually reads groups better than the user can
			if sc, err := dial(d); err == nil {
				defer sc.Close()
				if sc.Bind(d.BindDN, d.BindPassword) == nil {
					gc = sc
				}
			}
		}
		groups, err := gc.Search(d.GroupSearchBase, ScopeSubtree, filter, []string{"cn"})
		if err != nil {
			return nil, fmt.Errorf("group search: %w", err)
		}
		for _, g := range groups {
			add(g.first("cn"))
			add(g.DN)
		}
	}
	u.Admin = d.AdminGroup != "" && inList(u.Groups, d.AdminGroup)
	return u, nil
}

func orStr(v, d string) string {
	if strings.TrimSpace(v) == "" {
		return d
	}
	return v
}

// LDAPCheckService connects and binds as the service account (or anonymously): the "no test user" test.
func LDAPCheckService(dial Dialer, d config.Directory) error {
	if strings.TrimSpace(d.Host) == "" {
		return errors.New("no LDAP host is configured")
	}
	c, err := dial(d)
	if err != nil {
		return err
	}
	defer c.Close()
	if d.BindDN != "" {
		if err := c.Bind(d.BindDN, d.BindPassword); err != nil {
			return fmt.Errorf("service account bind: %w", err)
		}
	}
	return nil
}
