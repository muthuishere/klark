// Package config reads the hub's settings from the environment.
package config

import (
	"encoding/json"
	"fmt"
	"os"
	"regexp"
	"strconv"
	"strings"
	"time"
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

// OIDCProvider is an additional OAuth/OIDC sign-in (HUB_OIDC_PROVIDERS), served at auth/oidc/{id}/login.
type OIDCProvider struct {
	ID            string `json:"id"`
	Name          string `json:"name"`
	Issuer        string `json:"issuer"`
	ClientID      string `json:"clientId"`
	ClientSecret  string `json:"clientSecret,omitempty"`
	Scopes        string `json:"scopes,omitempty"`
	AdminGroup    string `json:"adminGroup,omitempty"`
	GroupsClaim   string `json:"groupsClaim,omitempty"`
	UsernameClaim string `json:"usernameClaim,omitempty"`
}

// MockLDAPUser seeds the mock LDAP directory (clear password: only the environment seed ever holds it).
type MockLDAPUser struct {
	Name     string
	Password string
	Admin    bool
	Groups   []string
}

// Directory is the LDAP / directory sign-in setting set. The environment fills it from HUB_LDAP_*; an
// administrator can store a replacement (same JSON names) from /admin/directory.
type Directory struct {
	Mode            string   `json:"mode"` // mockldap | ldap | local ("" = ldap when Host is set, else local; mockldap only when asked for)
	Host            string   `json:"host"`
	Port            int      `json:"port"` // 0 = 389, or 636 with SSL
	SSL             bool     `json:"ssl"`
	StartTLS        bool     `json:"startTls"`
	SkipCertCheck   bool     `json:"skipCertCheck"`
	TimeoutSeconds  int      `json:"timeoutSeconds"` // 0 = 10
	BindDN          string   `json:"bindDn"`
	BindPassword    string   `json:"bindPassword,omitempty"` // never returned by any API
	UserDNTemplate  string   `json:"userDnTemplate"`
	UserSearchBase  string   `json:"userSearchBase"`
	UserFilter      string   `json:"userFilter"` // "" = (uid={0})
	GroupSearchBase string   `json:"groupSearchBase"`
	GroupFilter     string   `json:"groupFilter"`
	UseMemberOf     bool     `json:"useMemberOf"`
	DisplayNameAttr string   `json:"displayNameAttr"`
	AdminGroup      string   `json:"adminGroup"`
	AllowedGroups   []string `json:"allowedGroups"`
}

// S3Config is an S3 (or S3-compatible: MinIO, R2, ...) bucket. The key prefix is Storage.Path.
type S3Config struct {
	Bucket          string `json:"bucket"`
	Region          string `json:"region"`
	Endpoint        string `json:"endpoint"` // empty = AWS
	AccessKeyID     string `json:"accessKeyId"`
	SecretAccessKey string `json:"secretAccessKey,omitempty"` // never returned by any API
	PathStyle       bool   `json:"pathStyle"`
}

// Storage is the ONE storage target of the hub (HUB_BACKUP*): the SQLite backups under <Path>/sqlite/ and the
// analytics files under <Path>/analytics/. Kind none|folder|s3; Path is a local folder (folder) or a key prefix (s3).
type Storage struct {
	Kind string   `json:"kind"`
	Path string   `json:"path"`
	S3   S3Config `json:"s3"`
}

// EffectiveMode resolves an empty mode.
func (d Directory) EffectiveMode() string {
	switch strings.ToLower(strings.TrimSpace(d.Mode)) {
	case "mockldap", "ldap", "local":
		return strings.ToLower(strings.TrimSpace(d.Mode))
	}
	if strings.TrimSpace(d.Host) != "" {
		return "ldap"
	}
	return "local"
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
	Directory     Directory
	MockLDAPUsers []MockLDAPUser // HUB_MOCK_USERS; nil = the defaults (user1, user2)
	OIDCProviders []OIDCProvider

	StatsIntervalSeconds int // HUB_STATS_INTERVAL_SECONDS, default 30, min 5
	StatsRetentionDays   int // HUB_STATS_RETENTION_DAYS, default 14
	Storage              Storage
	SQLiteBackupEvery    time.Duration // HUB_SQLITE_BACKUP_EVERY, default 6h, min 5m
	SQLiteBackupKeep     int           // HUB_SQLITE_BACKUP_KEEP, default 7

	PodID string // HUB_POD_ID: names this hub instance as the SQLite backup owner; random when empty
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
	if c.Directory, err = parseDirectory(getenv); err != nil {
		return c, err
	}
	if c.MockLDAPUsers, err = parseMockLDAPUsers(getenv("HUB_MOCK_USERS")); err != nil {
		return c, err
	}
	if c.OIDCProviders, err = ParseOIDCProviders(getenv("HUB_OIDC_PROVIDERS"), getenv); err != nil {
		return c, err
	}
	if c.StatsIntervalSeconds, err = envInt(getenv, "HUB_STATS_INTERVAL_SECONDS", 30, 5); err != nil {
		return c, err
	}
	if c.StatsRetentionDays, err = envInt(getenv, "HUB_STATS_RETENTION_DAYS", 14, 1); err != nil {
		return c, err
	}
	if c.Storage, err = parseStorage(getenv); err != nil {
		return c, err
	}
	if c.SQLiteBackupEvery, err = envDuration(getenv, "HUB_SQLITE_BACKUP_EVERY", 6*time.Hour, 5*time.Minute); err != nil {
		return c, err
	}
	if c.SQLiteBackupKeep, err = envInt(getenv, "HUB_SQLITE_BACKUP_KEEP", 7, 1); err != nil {
		return c, err
	}
	c.PodID = strings.TrimSpace(getenv("HUB_POD_ID"))
	if c.MockADFS {
		c.MockUsers, err = parseMockUsers(getenv("HUB_MOCK_ADFS_USERS"))
		if err != nil {
			return c, err
		}
	}
	return c, nil
}

// envInt reads a whole number >= min; empty = def, a smaller number is raised to min.
func envInt(g func(string) string, name string, def, min int) (int, error) {
	v := strings.TrimSpace(g(name))
	if v == "" {
		return def, nil
	}
	n, err := strconv.Atoi(v)
	if err != nil {
		return 0, fmt.Errorf("%s: %q is not a number", name, v)
	}
	if n < min {
		n = min
	}
	return n, nil
}

func envDuration(g func(string) string, name string, def, min time.Duration) (time.Duration, error) {
	v := strings.TrimSpace(g(name))
	if v == "" {
		return def, nil
	}
	d, err := time.ParseDuration(v)
	if err != nil {
		return 0, fmt.Errorf("%s: %q is not a duration like 30m, 6h or 24h", name, v)
	}
	return max(d, min), nil
}

func parseStorage(g func(string) string) (Storage, error) {
	a := Storage{Kind: strings.ToLower(strings.TrimSpace(g("HUB_BACKUP"))), Path: strings.TrimSpace(g("HUB_BACKUP_PATH")),
		S3: S3Config{Bucket: strings.TrimSpace(g("HUB_BACKUP_S3_BUCKET")),
			Region: strings.TrimSpace(g("HUB_BACKUP_S3_REGION")), Endpoint: strings.TrimSpace(g("HUB_BACKUP_S3_ENDPOINT")),
			AccessKeyID: strings.TrimSpace(g("HUB_BACKUP_S3_ACCESS_KEY_ID")), SecretAccessKey: g("HUB_BACKUP_S3_SECRET_ACCESS_KEY"),
			PathStyle: truthy(g("HUB_BACKUP_S3_PATH_STYLE"))}}
	switch a.Kind {
	case "":
		a.Kind = "none"
	case "none", "folder", "s3":
	default:
		return a, fmt.Errorf("HUB_BACKUP: %q must be none, folder or s3", a.Kind)
	}
	return a, nil
}

func parseDirectory(g func(string) string) (Directory, error) {
	d := Directory{
		Mode: strings.ToLower(strings.TrimSpace(g("HUB_AUTH_MODE"))), Host: strings.TrimSpace(g("HUB_LDAP_HOST")),
		SSL: truthy(g("HUB_LDAP_SSL")), StartTLS: truthy(g("HUB_LDAP_STARTTLS")), SkipCertCheck: truthy(g("HUB_LDAP_SKIP_CERT_CHECK")),
		BindDN: strings.TrimSpace(g("HUB_LDAP_BIND_DN")), BindPassword: g("HUB_LDAP_BIND_PASSWORD"),
		UserDNTemplate: strings.TrimSpace(g("HUB_LDAP_USER_DN_TEMPLATE")), UserSearchBase: strings.TrimSpace(g("HUB_LDAP_USER_SEARCH_BASE")),
		UserFilter: strings.TrimSpace(g("HUB_LDAP_USER_FILTER")), GroupSearchBase: strings.TrimSpace(g("HUB_LDAP_GROUP_SEARCH_BASE")),
		GroupFilter: strings.TrimSpace(g("HUB_LDAP_GROUP_FILTER")), UseMemberOf: truthy(g("HUB_LDAP_USE_MEMBEROF")),
		DisplayNameAttr: strings.TrimSpace(g("HUB_LDAP_DISPLAY_NAME_ATTR")), AdminGroup: strings.TrimSpace(g("HUB_LDAP_ADMIN_GROUP")),
		AllowedGroups: splitTop(g("HUB_LDAP_ALLOWED_GROUPS")),
	}
	switch d.Mode {
	case "", "mockldap", "ldap", "local":
	default:
		return d, fmt.Errorf("HUB_AUTH_MODE: %q must be mockldap, ldap or local", d.Mode)
	}
	for _, kv := range []struct {
		env string
		to  *int
	}{{"HUB_LDAP_PORT", &d.Port}, {"HUB_LDAP_TIMEOUT_SECONDS", &d.TimeoutSeconds}} {
		if v := strings.TrimSpace(g(kv.env)); v != "" {
			n, err := strconv.Atoi(v)
			if err != nil || n < 0 || n > 65535 {
				return d, fmt.Errorf("%s: %q is not a number", kv.env, v)
			}
			*kv.to = n
		}
	}
	return d, nil
}

// HUB_MOCK_USERS="name=password[:admin][:group1|group2],...": the ":" parts after the password are "admin" and/or a
// "|"-separated group list. Passwords cannot contain ',' or ':'.
func parseMockLDAPUsers(v string) ([]MockLDAPUser, error) {
	var out []MockLDAPUser
	for _, part := range splitTop(v) {
		name, rest, ok := strings.Cut(part, "=")
		name = strings.TrimSpace(name)
		if !ok || name == "" || rest == "" {
			return nil, fmt.Errorf("HUB_MOCK_USERS: %q must be name=password[:admin][:group1|group2]", part)
		}
		f := strings.Split(rest, ":")
		u := MockLDAPUser{Name: name, Password: f[0]}
		for _, x := range f[1:] {
			if strings.EqualFold(x, "admin") {
				u.Admin = true
				continue
			}
			for _, g := range strings.Split(x, "|") {
				if g = strings.TrimSpace(g); g != "" {
					u.Groups = append(u.Groups, g)
				}
			}
		}
		if u.Password == "" {
			return nil, fmt.Errorf("HUB_MOCK_USERS: %q has an empty password", name)
		}
		out = append(out, u)
	}
	return out, nil
}

var providerID = regexp.MustCompile(`^[a-z0-9][a-z0-9_-]{0,31}$`)

// ParseOIDCProviders reads the HUB_OIDC_PROVIDERS JSON array. A clientSecret "env:NAME" is read from that variable.
func ParseOIDCProviders(v string, getenv func(string) string) ([]OIDCProvider, error) {
	if strings.TrimSpace(v) == "" {
		return nil, nil
	}
	var ps []OIDCProvider
	if err := json.Unmarshal([]byte(v), &ps); err != nil {
		return nil, fmt.Errorf("HUB_OIDC_PROVIDERS: %w", err)
	}
	seen := map[string]bool{}
	for i := range ps {
		p := &ps[i]
		p.ID = strings.ToLower(strings.TrimSpace(p.ID))
		p.Issuer = strings.TrimRight(strings.TrimSpace(p.Issuer), "/")
		switch {
		case !providerID.MatchString(p.ID):
			return nil, fmt.Errorf("HUB_OIDC_PROVIDERS: id %q must be 1-32 chars of a-z 0-9 _ -", p.ID)
		case p.ID == "adfs":
			return nil, fmt.Errorf("HUB_OIDC_PROVIDERS: id adfs is reserved for the HUB_OIDC_* provider")
		case seen[p.ID]:
			return nil, fmt.Errorf("HUB_OIDC_PROVIDERS: duplicate id %q", p.ID)
		case p.Issuer == "" || strings.TrimSpace(p.ClientID) == "":
			return nil, fmt.Errorf("HUB_OIDC_PROVIDERS: %q needs issuer and clientId", p.ID)
		}
		seen[p.ID] = true
		if n, ok := strings.CutPrefix(p.ClientSecret, "env:"); ok {
			p.ClientSecret = getenv(strings.TrimSpace(n))
		}
		if p.Name == "" {
			p.Name = p.ID
		}
		if strings.TrimSpace(p.Scopes) == "" {
			p.Scopes = "openid profile email"
		}
	}
	return ps, nil
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
