package auth

import (
	"crypto/ed25519"
	"encoding/base64"
	"encoding/json"
	"errors"
	"log"
	"net/http"
	"strings"
	"time"
)

// Sign-in from a trusted platform (K2). The platform signs a short-lived,
// one-time ticket with its Ed25519 key; the browser opens
// /auth/ticket?t=<ticket>[&next=/path]; the hub checks it once and starts its
// own session. A ticket is base64url(payload JSON) "." base64url(signature
// over the payload bytes). No header-based sign-in: a header is one proxy
// mistake from a login bypass.

// Ticket is what a platform vouches for.
type Ticket struct {
	Issuer   string `json:"iss"`
	Audience string `json:"aud"`
	Subject  string `json:"sub"`  // the user name on this hub
	Name     string `json:"name"` // display name
	Tenant   string `json:"tenant"`
	Role     string `json:"role"` // viewer | member | admin
	IssuedAt int64  `json:"iat"`
	Expires  int64  `json:"exp"`
	ID       string `json:"jti"`
}

const (
	ticketMaxLife = 2 * time.Minute
	ticketSkew    = 30 * time.Second
)

var roles = map[string]bool{"viewer": true, "member": true, "admin": true}

// CheckTicket verifies a ticket's signature and claims (not its one-time use).
func CheckTicket(raw string, keys map[string]ed25519.PublicKey, audience string, now time.Time) (Ticket, error) {
	var t Ticket
	body, sig, ok := strings.Cut(raw, ".")
	if !ok {
		return t, errors.New("malformed ticket")
	}
	payload, err1 := base64.RawURLEncoding.DecodeString(body)
	signature, err2 := base64.RawURLEncoding.DecodeString(sig)
	if err1 != nil || err2 != nil || json.Unmarshal(payload, &t) != nil {
		return t, errors.New("malformed ticket")
	}
	key, known := keys[t.Issuer]
	if !known {
		return t, errors.New("unknown issuer")
	}
	if !ed25519.Verify(key, payload, signature) {
		return t, errors.New("bad signature")
	}
	switch {
	case t.Audience == "" || strings.TrimRight(t.Audience, "/") != strings.TrimRight(audience, "/"):
		return t, errors.New("ticket is for another hub")
	case t.Expires <= now.Unix():
		return t, errors.New("ticket expired")
	case t.IssuedAt > now.Add(ticketSkew).Unix():
		return t, errors.New("ticket issued in the future")
	case time.Duration(t.Expires-t.IssuedAt)*time.Second > ticketMaxLife:
		return t, errors.New("ticket lives longer than 2 minutes")
	case t.ID == "" || len(t.ID) > 128:
		return t, errors.New("ticket has no id")
	case t.Subject == "" || len(t.Subject) > 64 || strings.ContainsAny(t.Subject, " \t:,="):
		return t, errors.New("bad subject")
	case t.Tenant == "":
		return t, errors.New("ticket names no tenant")
	case strings.EqualFold(t.Tenant, "default"):
		// An admin of the default tenant operates the whole hub: never by ticket.
		return t, errors.New("a ticket cannot sign in to the default tenant")
	case !roles[t.Role]:
		return t, errors.New("role must be viewer, member or admin")
	}
	return t, nil
}

// ticketLogin is GET /auth/ticket?t=...&next=/path.
func (s *Service) ticketLogin(w http.ResponseWriter, r *http.Request) {
	fail := func(why string) {
		log.Printf("auth: platform ticket refused: %s", why)
		http.Redirect(w, r, s.Cfg.BasePath+"/login?error=ticket", http.StatusFound)
	}
	if len(s.Cfg.PlatformKeys) == 0 {
		http.NotFound(w, r)
		return
	}
	if blocked, _ := s.Limiter.Blocked(clientIP(r)); blocked {
		fail("too many bad tickets from " + clientIP(r))
		return
	}
	t, err := CheckTicket(r.URL.Query().Get("t"), s.Cfg.PlatformKeys, s.Cfg.PlatformAudience, time.Now())
	if err != nil {
		s.Limiter.Fail(clientIP(r))
		fail(err.Error())
		return
	}
	if !s.Store.TenantExists(t.Tenant) {
		fail("unknown tenant " + t.Tenant)
		return
	}
	if fresh, err := s.Store.UseTicket(t.Issuer+":"+t.ID, t.Expires); err != nil || !fresh {
		fail("ticket already used")
		return
	}
	sess := Session{User: t.Subject, Display: t.Name, Admin: t.Role == "admin", Source: "platform:" + t.Issuer,
		Tenant: strings.ToLower(t.Tenant), Role: t.Role}
	if err := s.startSession(w, r, sess); err != nil {
		fail(err.Error())
		return
	}
	http.Redirect(w, r, s.Cfg.BasePath+safeNext(r.URL.Query().Get("next")), http.StatusFound)
}

// safeNext keeps a redirect on this hub: a path, never a scheme or "//host".
func safeNext(next string) string {
	if !strings.HasPrefix(next, "/") || strings.HasPrefix(next, "//") || strings.HasPrefix(next, "/\\") || strings.ContainsAny(next, "\r\n") {
		return "/"
	}
	return next
}
