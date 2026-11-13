package auth

import (
	"crypto/ed25519"
	"encoding/base64"
	"encoding/json"
	"errors"
	"log"
	"net/http"
	"net/url"
	"strings"
	"time"
)

// Sign-in from a trusted platform (K2). The platform signs a short-lived,
// one-time ticket with its Ed25519 key; the browser opens
// a form posting to /auth/ticket from the platform's own origin; the hub checks it once and starts its
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

// PlatformSessionTTL: a platform's users sign in again (with a fresh ticket)
// after an hour, so a removal or role change on the platform is seen soon.
const PlatformSessionTTL = time.Hour

// ticketLogin is POST /auth/ticket (form field t, optional next). The form
// must come from the issuer's own origin (HUB_PLATFORM_ORIGINS): a page on
// another site cannot sign a visitor in with someone else's ticket.
func (s *Service) ticketLogin(w http.ResponseWriter, r *http.Request) {
	fail := func(why string) {
		log.Printf("auth: platform ticket refused: %s", why)
		http.Redirect(w, r, s.Cfg.BasePath+"/login?error=ticket", http.StatusSeeOther)
	}
	if len(s.Cfg.PlatformKeys) == 0 {
		http.NotFound(w, r)
		return
	}
	ip := clientIP(r, s.Cfg.TrustedProxies)
	if blocked, _ := s.Limiter.Blocked(ip); blocked {
		fail("too many bad tickets from " + ip)
		return
	}
	r.Body = http.MaxBytesReader(w, r.Body, 16<<10)
	if err := r.ParseForm(); err != nil {
		fail("unreadable form")
		return
	}
	t, err := CheckTicket(r.PostForm.Get("t"), s.Cfg.PlatformKeys, s.Cfg.PlatformAudience, time.Now())
	if err != nil {
		s.Limiter.Fail(ip)
		fail(err.Error())
		return
	}
	if origin := requestOrigin(r); origin == "" || origin != s.Cfg.PlatformOrigins[t.Issuer] {
		s.Limiter.Fail(ip)
		fail("form posted from " + origin + ", not " + s.Cfg.PlatformOrigins[t.Issuer])
		return
	}
	if !s.Store.TenantExists(t.Tenant) {
		fail("unknown tenant " + t.Tenant)
		return
	}
	if iss := s.Store.TenantIssuer(t.Tenant); iss != t.Issuer {
		fail("tenant " + t.Tenant + " does not accept tickets from " + t.Issuer)
		return
	}
	if cur, ok := s.Current(r); ok && (!strings.EqualFold(cur.Name, t.Subject) || !strings.EqualFold(cur.Tenant, t.Tenant)) {
		fail("this browser is signed in as someone else; sign out first")
		return
	}
	if fresh, err := s.Store.UseTicket(t.Issuer+":"+t.ID, t.Expires); err != nil || !fresh {
		fail("ticket already used")
		return
	}
	sess := Session{User: t.Subject, Display: t.Name, Admin: t.Role == "admin", Source: "platform:" + t.Issuer,
		Tenant: strings.ToLower(t.Tenant), Role: t.Role}
	if err := s.startSessionFor(w, r, sess, PlatformSessionTTL); err != nil {
		fail(err.Error())
		return
	}
	http.Redirect(w, r, s.Cfg.BasePath+safeNext(r.PostForm.Get("next")), http.StatusSeeOther)
}

// requestOrigin is the scheme://host the request says it came from: Origin,
// else the origin of Referer.
func requestOrigin(r *http.Request) string {
	o := r.Header.Get("Origin")
	if o == "" || o == "null" {
		ref, err := url.Parse(r.Header.Get("Referer"))
		if err != nil || ref.Scheme == "" || ref.Host == "" {
			return ""
		}
		return ref.Scheme + "://" + ref.Host
	}
	return strings.TrimRight(o, "/")
}

// safeNext keeps a redirect on this hub: a plain path, no scheme, no host, and
// no control character, space or backslash anywhere (browsers drop tabs and
// newlines and treat \ as /, which turns "/<TAB>/x" or "/\\x" into "//x").
func safeNext(next string) string {
	if !strings.HasPrefix(next, "/") || strings.HasPrefix(next, "//") {
		return "/"
	}
	for _, c := range next {
		if c < 0x21 || c == 0x7f || c == '\\' {
			return "/"
		}
	}
	u, err := url.Parse(next)
	if err != nil || u.Scheme != "" || u.Host != "" || u.Opaque != "" || strings.HasPrefix(u.Path, "//") {
		return "/"
	}
	return next
}
