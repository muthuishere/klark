// Package config reads the hub's settings from the environment.
package config

import (
	"fmt"
	"os"
	"strings"
)

// User is a statically configured account.
type User struct {
	Name  string
	Hash  string // bcrypt hash, or "sha256:<hex>"
	Admin bool
}

// MachineEntry is a machine from HUB_MACHINES.
type MachineEntry struct {
	Name   string
	SHA256 string
	Fleet  bool
}

// OIDC configures an ADFS / OpenID Connect provider.
type OIDC struct {
	Issuer       string
	ClientID     string
	ClientSecret string
	Scopes       string
	AdminGroup   string
	Name         string
}

func (o OIDC) Enabled() bool { return o.Issuer != "" && o.ClientID != "" }

// MockUser is a user of the built-in mock ADFS (clear password: dev and tests only).
type MockUser struct {
	Name     string
	Password string
	Admin    bool
}

// Config is everything the hub reads from its environment.
type Config struct {
	Addr          string
	BasePath      string // "" for the root, else "/prefix" (no trailing slash)
	DataDir       string
	Machines      []MachineEntry
	Users         []User
	AdminPassword string
	SessionKey    string
	PublicURL     string
	OIDC          OIDC
	MockADFS      bool
	MockUsers     []MockUser
}

// NormalizeBase turns "klark/", "/klark" or "/" into "/klark" or "".
func NormalizeBase(p string) string {
	p = strings.Trim(strings.TrimSpace(p), "/")
	if p == "" {
		return ""
	}
	return "/" + p
}

// FromEnv builds the config from getenv (os.Getenv in production).
func FromEnv(getenv func(string) string) (Config, error) {
	c := Config{
		Addr:          orDefault(getenv("HUB_ADDR"), ":7470"),
		BasePath:      NormalizeBase(getenv("HUB_BASE_PATH")),
		DataDir:       orDefault(getenv("HUB_DATA_DIR"), "data"),
		AdminPassword: getenv("HUB_ADMIN_PASSWORD"),
		SessionKey:    getenv("HUB_SESSION_KEY"),
		PublicURL:     strings.TrimRight(getenv("HUB_PUBLIC_URL"), "/"),
		OIDC: OIDC{
			Issuer:       strings.TrimRight(getenv("HUB_OIDC_ISSUER"), "/"),
			ClientID:     getenv("HUB_OIDC_CLIENT_ID"),
			ClientSecret: getenv("HUB_OIDC_CLIENT_SECRET"),
			Scopes:       orDefault(getenv("HUB_OIDC_SCOPES"), "openid profile email"),
			AdminGroup:   getenv("HUB_OIDC_ADMIN_GROUP"),
			Name:         orDefault(getenv("HUB_OIDC_NAME"), "ADFS"),
		},
		MockADFS: truthy(getenv("HUB_MOCK_ADFS")),
	}
	var err error
	if c.Machines, err = parseMachines(getenv("HUB_MACHINES")); err != nil {
		return c, err
	}
	if c.Users, err = parseUsers(getenv("HUB_USERS")); err != nil {
		return c, err
	}
	if c.MockADFS {
		c.MockUsers, err = parseMockUsers(getenv("HUB_MOCK_ADFS_USERS"))
		if err != nil {
			return c, err
		}
	}
	return c, nil
}

func orDefault(v, d string) string {
	if strings.TrimSpace(v) == "" {
		return d
	}
	return strings.TrimSpace(v)
}

func truthy(v string) bool {
	switch strings.ToLower(strings.TrimSpace(v)) {
	case "1", "true", "yes", "on":
		return true
	}
	return false
}

// HUB_MACHINES="name=<sha256 hex>[,name*=<sha256 hex>]": a name ending in "*" is a fleet token.
func parseMachines(v string) ([]MachineEntry, error) {
	var out []MachineEntry
	for _, part := range strings.Split(v, ",") {
		part = strings.TrimSpace(part)
		if part == "" {
			continue
		}
		name, sha, ok := strings.Cut(part, "=")
		if !ok || name == "" || len(strings.TrimSpace(sha)) != 64 {
			return nil, fmt.Errorf("HUB_MACHINES: %q must be name=<64 hex sha256>", name)
		}
		out = append(out, MachineEntry{Name: strings.TrimSuffix(name, "*"), SHA256: strings.ToLower(strings.TrimSpace(sha)),
			Fleet: strings.HasSuffix(name, "*")})
	}
	return out, nil
}

// HUB_USERS="alice=<hash>:admin,bob=<hash>". A hash may contain ':' (sha256:hex), so the role is the
// text after the LAST ':' only when it is "admin" or "user".
func parseUsers(v string) ([]User, error) {
	var out []User
	for _, part := range splitTop(v) {
		name, rest, ok := strings.Cut(part, "=")
		name = strings.TrimSpace(name)
		if !ok || name == "" || rest == "" {
			return nil, fmt.Errorf("HUB_USERS: %q must be name=<hash>[:admin]", part)
		}
		u := User{Name: name, Hash: rest}
		if i := strings.LastIndex(rest, ":"); i > 0 {
			switch strings.ToLower(rest[i+1:]) {
			case "admin":
				u.Admin, u.Hash = true, rest[:i]
			case "user":
				u.Hash = rest[:i]
			}
		}
		out = append(out, u)
	}
	return out, nil
}

func parseMockUsers(v string) ([]MockUser, error) {
	if strings.TrimSpace(v) == "" {
		return []MockUser{{Name: "alice", Password: "alicepw", Admin: true}, {Name: "bob", Password: "bobpw"}}, nil
	}
	var out []MockUser
	for _, part := range splitTop(v) {
		name, rest, ok := strings.Cut(part, "=")
		if !ok || strings.TrimSpace(name) == "" || rest == "" {
			return nil, fmt.Errorf("HUB_MOCK_ADFS_USERS: %q must be name=password[:admin]", part)
		}
		u := MockUser{Name: strings.TrimSpace(name), Password: rest}
		if pw, role, ok := strings.Cut(rest, ":"); ok {
			u.Password, u.Admin = pw, strings.EqualFold(role, "admin")
		}
		out = append(out, u)
	}
	return out, nil
}

func splitTop(v string) []string {
	var out []string
	for _, p := range strings.Split(v, ",") {
		if p = strings.TrimSpace(p); p != "" {
			out = append(out, p)
		}
	}
	return out
}

// Getenv is os.Getenv, for callers that want the default.
func Getenv(k string) string { return os.Getenv(k) }
