package auth

import (
	"errors"
	"net"
	"strings"

	"github.com/muthuishere/klark/hub-go/internal/config"
	"github.com/muthuishere/klark/hub-go/internal/store"
)

// StartupCheck refuses settings that are only safe on this computer when the
// hub listens on other addresses: the built-in admin still on its default
// password (unless HUB_ALLOW_DEFAULT_ADMIN), and, with HUB_REQUIRE_TLS, a
// public URL that is not https.
func StartupCheck(cfg config.Config, st *store.Store, getenv func(string) string) error {
	if localOnly(cfg.Addr) {
		return nil
	}
	if !truthy(getenv("HUB_ALLOW_DEFAULT_ADMIN")) {
		if u, err := st.User(AdminName); err == nil && VerifyPassword(u.Hash, DefaultAdminPassword) {
			return errors.New("the built-in admin still has the default password and the hub listens on " + cfg.Addr +
				": set HUB_ADMIN_PASSWORD (or change it in /admin/users from this computer first); HUB_ALLOW_DEFAULT_ADMIN=1 skips this check for a throwaway hub")
		}
	}
	if truthy(getenv("HUB_REQUIRE_TLS")) && !strings.HasPrefix(strings.ToLower(cfg.PublicURL), "https://") {
		return errors.New("HUB_REQUIRE_TLS is set but HUB_PUBLIC_URL is not https://: serve the hub behind TLS and set its https URL")
	}
	return nil
}

// localOnly: the listen address accepts connections from this computer only.
func localOnly(addr string) bool {
	host, _, err := net.SplitHostPort(addr)
	if err != nil || host == "" {
		return false // ":7470" listens everywhere
	}
	if host == "localhost" {
		return true
	}
	ip := net.ParseIP(host)
	return ip != nil && ip.IsLoopback()
}

func truthy(v string) bool {
	switch strings.ToLower(strings.TrimSpace(v)) {
	case "1", "true", "yes", "on":
		return true
	}
	return false
}
