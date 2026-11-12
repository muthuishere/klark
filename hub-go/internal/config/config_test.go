package config

import (
	"testing"
	"time"
)

func env(m map[string]string) func(string) string { return func(k string) string { return m[k] } }

func TestDefaultsAndBase(t *testing.T) {
	c, err := FromEnv(env(map[string]string{"HUB_BASE_PATH": "klark/"}))
	if err != nil || c.Addr != ":7470" || c.BasePath != "/klark" || c.DataDir != "data" {
		t.Fatalf("%+v %v", c, err)
	}
	for in, want := range map[string]string{"": "", "/": "", "/a/b/": "/a/b", " x ": "/x"} {
		if got := NormalizeBase(in); got != want {
			t.Errorf("NormalizeBase(%q)=%q want %q", in, got, want)
		}
	}
}

func TestUsersAndMachines(t *testing.T) {
	sha := "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
	c, err := FromEnv(env(map[string]string{
		"HUB_USERS":     "alice=sha256:abcd:admin, bob=$2a$10$xyz",
		"HUB_MACHINES":  "mac=" + sha + ",fleet*=" + sha,
		"HUB_MOCK_ADFS": "1", "HUB_MOCK_ADFS_USERS": "carol=pw:admin",
	}))
	if err != nil {
		t.Fatal(err)
	}
	if len(c.Users) != 2 || !c.Users[0].Admin || c.Users[0].Hash != "sha256:abcd" || c.Users[1].Hash != "$2a$10$xyz" || c.Users[1].Admin {
		t.Fatalf("%+v", c.Users)
	}
	if len(c.Machines) != 2 || !c.Machines[1].Fleet || c.Machines[1].Name != "fleet" {
		t.Fatalf("%+v", c.Machines)
	}
	if !c.MockADFS || c.MockUsers[0].Name != "carol" || !c.MockUsers[0].Admin {
		t.Fatalf("%+v", c.MockUsers)
	}
	if _, err := FromEnv(env(map[string]string{"HUB_MACHINES": "bad=1"})); err == nil {
		t.Fatal("bad machine accepted")
	}
}

func TestDirectoryMockUsersAndProviders(t *testing.T) {
	c, err := FromEnv(env(map[string]string{"HUB_LDAP_HOST": "ldap.x", "HUB_LDAP_PORT": "636", "HUB_LDAP_SSL": "true", "HUB_LDAP_ALLOWED_GROUPS": "a, b",
		"HUB_LDAP_BIND_PASSWORD": "s3", "HUB_MOCK_USERS": "u1=pw1:admin:g1|g2,u2=pw2", "HUB_OIDC_PROVIDERS": `[{"id":"Corp","issuer":"https://i/","clientId":"c","clientSecret":"env:CORP_SECRET"}]`,
		"CORP_SECRET": "shh"}))
	if err != nil {
		t.Fatal(err)
	}
	d := c.Directory
	if d.EffectiveMode() != "ldap" || d.Port != 636 || !d.SSL || len(d.AllowedGroups) != 2 || d.BindPassword != "s3" {
		t.Fatalf("%+v", d)
	}
	if len(c.MockLDAPUsers) != 2 || !c.MockLDAPUsers[0].Admin || len(c.MockLDAPUsers[0].Groups) != 2 || c.MockLDAPUsers[1].Password != "pw2" {
		t.Fatalf("%+v", c.MockLDAPUsers)
	}
	p := c.OIDCProviders[0]
	if p.ID != "corp" || p.Issuer != "https://i" || p.ClientSecret != "shh" || p.Name != "corp" || p.Scopes != "openid profile email" {
		t.Fatalf("%+v", p)
	}
	if c2, _ := FromEnv(env(nil)); c2.Directory.EffectiveMode() != "local" {
		t.Fatal("default mode")
	}
	for _, bad := range []map[string]string{{"HUB_AUTH_MODE": "x"}, {"HUB_LDAP_PORT": "abc"}, {"HUB_OIDC_PROVIDERS": "{"},
		{"HUB_OIDC_PROVIDERS": `[{"id":"adfs","issuer":"i","clientId":"c"}]`}, {"HUB_OIDC_PROVIDERS": `[{"id":"a","issuer":"i"}]`},
		{"HUB_OIDC_PROVIDERS": `[{"id":"a","issuer":"i","clientId":"c"},{"id":"a","issuer":"i","clientId":"c"}]`}, {"HUB_MOCK_USERS": "x"}} {
		if _, err := FromEnv(env(bad)); err == nil {
			t.Errorf("%v accepted", bad)
		}
	}
}

func TestStatsStorageAndDBEnv(t *testing.T) {
	c, err := FromEnv(env(nil))
	if err != nil || c.StatsIntervalSeconds != 30 || c.StatsRetentionDays != 14 || c.Storage.Kind != "none" || c.SQLiteBackupEvery != 6*time.Hour || c.SQLiteBackupKeep != 7 || c.DB != "sqlite" {
		t.Fatalf("%+v %v", c, err)
	}
	c, err = FromEnv(env(map[string]string{"HUB_STATS_INTERVAL_SECONDS": "1", "HUB_STATS_RETENTION_DAYS": "3", "HUB_BACKUP": "S3", "HUB_BACKUP_PATH": "klarkhub/prod",
		"HUB_BACKUP_S3_BUCKET": "b", "HUB_BACKUP_S3_ENDPOINT": "http://minio:9000", "HUB_BACKUP_S3_PATH_STYLE": "true",
		"HUB_BACKUP_S3_ACCESS_KEY_ID": "k", "HUB_BACKUP_S3_SECRET_ACCESS_KEY": "s", "HUB_SQLITE_BACKUP_EVERY": "1m", "HUB_SQLITE_BACKUP_KEEP": "3",
		"HUB_DB": "postgres", "HUB_DATABASE_URL": "postgres://x", "HUB_POD_URL": "http://10.0.0.1:7470/", "HUB_POD_SECRET": "s3cret"}))
	if err != nil || c.StatsIntervalSeconds != 5 || c.StatsRetentionDays != 3 || c.Storage.Kind != "s3" || c.Storage.Path != "klarkhub/prod" || !c.Storage.S3.PathStyle ||
		c.Storage.S3.SecretAccessKey != "s" || c.SQLiteBackupEvery != 5*time.Minute || c.SQLiteBackupKeep != 3 || c.DB != "postgres" || c.PodURL != "http://10.0.0.1:7470" {
		t.Fatalf("%+v %v", c, err)
	}
	for _, bad := range []map[string]string{{"HUB_BACKUP": "ftp"}, {"HUB_STATS_INTERVAL_SECONDS": "x"}, {"HUB_SQLITE_BACKUP_EVERY": "soon"}, {"HUB_DB": "mysql"}} {
		if _, err := FromEnv(env(bad)); err == nil {
			t.Errorf("%v accepted", bad)
		}
	}
}
