package auth

import (
	"testing"

	"github.com/muthuishere/klark/hub-go/internal/config"
	"github.com/muthuishere/klark/hub-go/internal/store"
)

func TestStartupCheck(t *testing.T) {
	st, err := store.Open(t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	defer st.Close()
	s := New(st, config.Config{}, NewSigner([]byte("k")))
	if err := s.Seed(); err != nil {
		t.Fatal(err)
	}
	env := func(m map[string]string) func(string) string { return func(k string) string { return m[k] } }
	cases := []struct {
		addr, url string
		env       map[string]string
		ok        bool
	}{
		{"127.0.0.1:7470", "", nil, true},
		{"localhost:7470", "", nil, true},
		{":7470", "", nil, false},
		{"0.0.0.0:7470", "", nil, false},
		{":7470", "", map[string]string{"HUB_ALLOW_DEFAULT_ADMIN": "1"}, true},
		{":7470", "http://hub", map[string]string{"HUB_ALLOW_DEFAULT_ADMIN": "1", "HUB_REQUIRE_TLS": "1"}, false},
		{":7470", "https://hub", map[string]string{"HUB_ALLOW_DEFAULT_ADMIN": "1", "HUB_REQUIRE_TLS": "1"}, true},
	}
	for _, c := range cases {
		err := StartupCheck(config.Config{Addr: c.addr, PublicURL: c.url}, st, env(c.env))
		if (err == nil) != c.ok {
			t.Fatalf("%s %s %v: err=%v", c.addr, c.url, c.env, err)
		}
	}
	h, _ := HashPassword("a-real-password")
	_ = st.SetPassword(AdminName, h)
	if err := StartupCheck(config.Config{Addr: ":7470"}, st, env(nil)); err != nil {
		t.Fatalf("changed password still refused: %v", err)
	}
}
