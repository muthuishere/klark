package config

import (
	"crypto/ed25519"
	"crypto/rand"
	"encoding/base64"
	"testing"
)

func TestPlatformKeys(t *testing.T) {
	pub, _, _ := ed25519.GenerateKey(rand.Reader)
	k := base64.StdEncoding.EncodeToString(pub)
	env := func(m map[string]string) func(string) string { return func(s string) string { return m[s] } }

	c, err := FromEnv(env(map[string]string{"HUB_PLATFORM_KEYS": "liveconv=" + k, "HUB_PUBLIC_URL": "https://hub.example/"}))
	if err != nil || len(c.PlatformKeys["liveconv"]) != ed25519.PublicKeySize || c.PlatformAudience != "https://hub.example" {
		t.Fatalf("keys %v aud %q err %v", c.PlatformKeys, c.PlatformAudience, err)
	}
	if _, err := FromEnv(env(map[string]string{"HUB_PLATFORM_KEYS": "liveconv=" + k})); err == nil {
		t.Fatal("keys without an audience accepted")
	}
	for _, bad := range []string{"liveconv", "=" + k, "x=notbase64!", "x=" + base64.StdEncoding.EncodeToString([]byte("short"))} {
		if _, err := FromEnv(env(map[string]string{"HUB_PLATFORM_KEYS": bad, "HUB_PUBLIC_URL": "https://h"})); err == nil {
			t.Fatalf("bad key %q accepted", bad)
		}
	}
	if c, _ := FromEnv(env(nil)); len(c.PlatformKeys) != 0 {
		t.Fatal("keys without HUB_PLATFORM_KEYS")
	}
}
