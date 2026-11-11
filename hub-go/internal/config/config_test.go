package config

import "testing"

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
