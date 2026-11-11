package auth

import (
	"context"
	"encoding/json"
	"errors"
	"io"
	"log"
	"net"
	"net/http"
	"net/http/httptest"
	"net/url"
	"strconv"
	"strings"
	"sync"
	"time"

	"github.com/muthuishere/klark/hub-go/internal/config"
	"github.com/muthuishere/klark/hub-go/internal/store"
)

const (
	CookieName = "klark_session"
	oidcCookie = "klark_oidc"
	AdminName  = "admin"
	// DefaultAdminPassword is the built-in admin's password unless HUB_ADMIN_PASSWORD is set.
	DefaultAdminPassword = "adminadmin"
)

// User is the signed-in person a request acts as.
type User struct {
	Name    string `json:"username"`
	Display string `json:"displayName"`
	Admin   bool   `json:"admin"`
}

type ctxKey struct{}

// FromContext is the user Require put on the request.
func FromContext(ctx context.Context) (User, bool) {
	u, ok := ctx.Value(ctxKey{}).(User)
	return u, ok
}

// Service is the hub's sign-in: static credentials, optional ADFS/OIDC (or the mock), signed cookies.
type Service struct {
	Store    *store.Store
	Cfg      config.Config
	Signer   *Signer
	Limiter  *Limiter
	Mock     *MockADFS
	Insecure bool // never set Secure on cookies (plain-http tests)

	// Local serves the hub's own routes (without the base path) so the mock ADFS can be called in-process.
	Local http.Handler

	mu        sync.Mutex
	providers map[string]*oidcProvider
	dummy     string
}

func New(st *store.Store, cfg config.Config, signer *Signer) *Service {
	d, _ := HashPassword("not-a-real-password")
	return &Service{Store: st, Cfg: cfg, Signer: signer, Limiter: NewLimiter(5, 15*time.Minute),
		providers: map[string]*oidcProvider{}, dummy: d}
}

// Seed makes the built-in admin and the HUB_USERS accounts in hub.db. HUB_ADMIN_PASSWORD, when set, always wins.
func (s *Service) Seed() error {
	pw, forced := s.Cfg.AdminPassword, s.Cfg.AdminPassword != ""
	if !forced {
		pw = DefaultAdminPassword
	}
	if _, err := s.Store.User(AdminName); forced || errors.Is(err, store.ErrNotFound) {
		h, err := HashPassword(pw)
		if err != nil {
			return err
		}
		if err := s.Store.UpsertUser(store.User{Name: AdminName, DisplayName: "Administrator", Hash: h, Admin: true, Source: "builtin"}); err != nil {
			return err
		}
		if !forced {
			log.Printf("auth: built-in admin has the default password; set HUB_ADMIN_PASSWORD or change it in /admin/users")
		}
	}
	for _, u := range s.Cfg.Users {
		if err := s.Store.UpsertUser(store.User{Name: u.Name, Hash: u.Hash, Admin: u.Admin, Source: "env"}); err != nil {
			return err
		}
	}
	return nil
}

// ---- request helpers ----------------------------------------------------------

func isHTTPS(r *http.Request) bool {
	return r.TLS != nil || strings.EqualFold(firstOf(r.Header.Get("X-Forwarded-Proto")), "https")
}

func firstOf(v string) string {
	a, _, _ := strings.Cut(v, ",")
	return strings.TrimSpace(a)
}

// reqHost is the host the browser used (a reverse proxy's X-Forwarded-Host wins).
func reqHost(r *http.Request) string {
	if h := firstOf(r.Header.Get("X-Forwarded-Host")); h != "" {
		return h
	}
	return r.Host
}

// ExternalBase is the hub's public origin + base path, no trailing slash.
func (s *Service) ExternalBase(r *http.Request) string {
	if s.Cfg.PublicURL != "" {
		return s.Cfg.PublicURL + s.Cfg.BasePath
	}
	scheme := "http"
	if isHTTPS(r) {
		scheme = "https"
	}
	return scheme + "://" + reqHost(r) + s.Cfg.BasePath
}

func clientIP(r *http.Request) string {
	host, _, err := net.SplitHostPort(r.RemoteAddr)
	if err != nil {
		host = r.RemoteAddr
	}
	if ip := net.ParseIP(host); ip != nil && (ip.IsLoopback() || ip.IsPrivate()) {
		if x := firstOf(r.Header.Get("X-Forwarded-For")); x != "" {
			return x
		}
	}
	return host
}

func writeJSON(w http.ResponseWriter, status int, v any) {
	w.Header().Set("Content-Type", "application/json")
	w.Header().Set("Cache-Control", "no-store")
	w.WriteHeader(status)
	_ = json.NewEncoder(w).Encode(v)
}

func apiError(w http.ResponseWriter, status int, msg string) {
	writeJSON(w, status, map[string]string{"error": msg})
}

func (s *Service) cookiePath() string {
	if s.Cfg.BasePath == "" {
		return "/"
	}
	return s.Cfg.BasePath + "/"
}

func (s *Service) setCookie(w http.ResponseWriter, r *http.Request, name, value string, maxAge int) {
	http.SetCookie(w, &http.Cookie{Name: name, Value: value, Path: s.cookiePath(), MaxAge: maxAge, HttpOnly: true,
		SameSite: http.SameSiteLaxMode, Secure: !s.Insecure && isHTTPS(r)})
}

func (s *Service) startSession(w http.ResponseWriter, r *http.Request, sess Session) error {
	sess.Expires = time.Now().Add(SessionTTL).Unix()
	tok, err := s.Signer.Sign(sess)
	if err != nil {
		return err
	}
	s.setCookie(w, r, CookieName, tok, int(SessionTTL.Seconds()))
	return nil
}

// Current is the signed-in user of a request, if any.
func (s *Service) Current(r *http.Request) (User, bool) {
	c, err := r.Cookie(CookieName)
	if err != nil {
		return User{}, false
	}
	var sess Session
	if s.Signer.Verify(c.Value, &sess) != nil || time.Now().Unix() > sess.Expires || sess.User == "" {
		return User{}, false
	}
	if sess.Source == "db" {
		u, err := s.Store.User(sess.User)
		if err != nil {
			return User{}, false // deleted: the cookie is dead
		}
		return User{Name: u.Name, Display: u.DisplayName, Admin: u.Admin}, true
	}
	return User{Name: sess.User, Display: sess.Display, Admin: sess.Admin}, true
}

// CSRF refuses a state-changing browser request that did not come from this hub's own origin. Cookies are
// SameSite=Lax as well; this is the second layer. Requests without an Origin (curl, tests) pass unless the
// browser says Sec-Fetch-Site: cross-site.
func CSRF(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch r.Method {
		case http.MethodGet, http.MethodHead, http.MethodOptions:
			next.ServeHTTP(w, r)
			return
		}
		if o := r.Header.Get("Origin"); o != "" && o != "null" {
			u, err := url.Parse(o)
			if err != nil || !strings.EqualFold(u.Host, reqHost(r)) {
				apiError(w, http.StatusForbidden, "cross-origin request refused")
				return
			}
		} else if o == "null" || r.Header.Get("Sec-Fetch-Site") == "cross-site" {
			apiError(w, http.StatusForbidden, "cross-origin request refused")
			return
		}
		next.ServeHTTP(w, r)
	})
}

// Require runs next for signed-in users only (admins only when admin is true).
func (s *Service) Require(admin bool, next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		u, ok := s.Current(r)
		if !ok {
			apiError(w, http.StatusUnauthorized, "sign in required")
			return
		}
		if admin && !u.Admin {
			apiError(w, http.StatusForbidden, "admin only")
			return
		}
		next.ServeHTTP(w, r.WithContext(context.WithValue(r.Context(), ctxKey{}, u)))
	})
}

// ---- routes -------------------------------------------------------------------

// Mount registers /api/auth/* and /auth/adfs/*.
func (s *Service) Mount(mux *http.ServeMux) {
	mux.Handle("GET /api/auth/providers", http.HandlerFunc(s.providersH))
	mux.Handle("POST /api/auth/login", CSRF(http.HandlerFunc(s.login)))
	mux.Handle("POST /api/auth/logout", CSRF(http.HandlerFunc(s.logout)))
	mux.Handle("GET /api/auth/me", http.HandlerFunc(s.me))
	mux.HandleFunc("GET /auth/adfs/login", s.adfsLogin)
	mux.HandleFunc("GET /auth/adfs/callback", s.adfsCallback)
}

func (s *Service) adfsEnabled() bool { return s.Cfg.OIDC.Enabled() || s.Mock != nil }

func (s *Service) providersH(w http.ResponseWriter, _ *http.Request) {
	adfs := map[string]any{"enabled": false}
	if s.adfsEnabled() {
		name := s.Cfg.OIDC.Name
		if s.Mock != nil && !s.Cfg.OIDC.Enabled() {
			name = "Mock ADFS"
		}
		adfs = map[string]any{"enabled": true, "name": name, "loginUrl": "auth/adfs/login"}
	}
	writeJSON(w, http.StatusOK, map[string]any{"static": true, "adfs": adfs})
}

func (s *Service) me(w http.ResponseWriter, r *http.Request) {
	u, ok := s.Current(r)
	if !ok {
		apiError(w, http.StatusUnauthorized, "not signed in")
		return
	}
	writeJSON(w, http.StatusOK, u)
}

func (s *Service) logout(w http.ResponseWriter, r *http.Request) {
	s.setCookie(w, r, CookieName, "", -1)
	w.WriteHeader(http.StatusNoContent)
}

func (s *Service) login(w http.ResponseWriter, r *http.Request) {
	var body struct{ Username, Password string }
	if err := json.NewDecoder(io.LimitReader(r.Body, 1<<16)).Decode(&body); err != nil {
		apiError(w, http.StatusBadRequest, "send {username, password}")
		return
	}
	name := strings.ToLower(strings.TrimSpace(body.Username))
	ukey, ipkey := "u:"+name, "ip:"+clientIP(r)
	for _, k := range []string{ukey, ipkey} {
		if blocked, wait := s.Limiter.Blocked(k); blocked {
			w.Header().Set("Retry-After", strconv.Itoa(int(wait.Seconds())+1))
			apiError(w, http.StatusTooManyRequests, "too many failed sign-ins; try again later")
			return
		}
	}
	u, err := s.Store.User(name)
	hash := s.dummy
	if err == nil {
		hash = u.Hash
	}
	if !VerifyPassword(hash, body.Password) || err != nil {
		s.Limiter.Fail(ukey)
		s.Limiter.Fail(ipkey)
		apiError(w, http.StatusUnauthorized, "wrong username or password")
		return
	}
	s.Limiter.Reset(ukey)
	if err := s.startSession(w, r, Session{User: u.Name, Source: "db"}); err != nil {
		apiError(w, http.StatusInternalServerError, "could not start a session")
		return
	}
	writeJSON(w, http.StatusOK, User{Name: u.Name, Display: u.DisplayName, Admin: u.Admin})
}

// ---- ADFS / OIDC ----------------------------------------------------------------

type oidcState struct {
	State, Nonce, Verifier, Redirect, Issuer, Return string
	Expires                                          int64
}

// issuerFor is the provider issuer: the configured one, or the mock served by this hub.
func (s *Service) issuerFor(r *http.Request) string {
	if s.Cfg.OIDC.Enabled() {
		return s.Cfg.OIDC.Issuer
	}
	return s.ExternalBase(r) + "/mock-adfs"
}

func (s *Service) clientCreds() (id, secret string) {
	if s.Cfg.OIDC.Enabled() {
		return s.Cfg.OIDC.ClientID, s.Cfg.OIDC.ClientSecret
	}
	return MockClientID, MockClientSecret
}

type localTransport struct {
	h    http.Handler
	base string
}

func (t localTransport) RoundTrip(req *http.Request) (*http.Response, error) {
	r2 := req.Clone(req.Context())
	r2.URL.Path = strings.TrimPrefix(r2.URL.Path, t.base)
	r2.RequestURI = r2.URL.RequestURI()
	rec := httptest.NewRecorder()
	t.h.ServeHTTP(rec, r2)
	resp := rec.Result()
	resp.Request = req
	return resp, nil
}

func (s *Service) httpClient(issuer string) *http.Client {
	if s.Mock != nil && !s.Cfg.OIDC.Enabled() && s.Local != nil {
		return &http.Client{Transport: localTransport{h: s.Local, base: s.Cfg.BasePath}, Timeout: 10 * time.Second}
	}
	return &http.Client{Timeout: 10 * time.Second}
}

func (s *Service) provider(ctx context.Context, issuer string) (*oidcProvider, error) {
	s.mu.Lock()
	p := s.providers[issuer]
	s.mu.Unlock()
	if p != nil {
		return p, nil
	}
	p, err := discover(ctx, s.httpClient(issuer), issuer)
	if err != nil {
		return nil, err
	}
	s.mu.Lock()
	s.providers[issuer] = p
	s.mu.Unlock()
	return p, nil
}

func safeReturn(p string) string {
	if strings.HasPrefix(p, "/") && !strings.HasPrefix(p, "//") && !strings.Contains(p, "\\") && !strings.ContainsAny(p, "\r\n") {
		return p
	}
	return ""
}

func (s *Service) adfsLogin(w http.ResponseWriter, r *http.Request) {
	if !s.adfsEnabled() {
		http.NotFound(w, r)
		return
	}
	issuer := s.issuerFor(r)
	p, err := s.provider(r.Context(), issuer)
	if err != nil {
		log.Printf("auth: adfs discovery failed: %v", err)
		http.Redirect(w, r, s.Cfg.BasePath+"/login?error=adfs_unavailable", http.StatusFound)
		return
	}
	st := oidcState{State: randToken(18), Nonce: randToken(18), Verifier: randToken(32),
		Redirect: s.ExternalBase(r) + "/auth/adfs/callback", Issuer: issuer, Return: safeReturn(r.URL.Query().Get("return")),
		Expires: time.Now().Add(10 * time.Minute).Unix()}
	tok, _ := s.Signer.Sign(st)
	s.setCookie(w, r, oidcCookie, tok, 600)
	id, _ := s.clientCreds()
	scopes := s.Cfg.OIDC.Scopes
	if scopes == "" {
		scopes = "openid profile email"
	}
	q := url.Values{"response_type": {"code"}, "client_id": {id}, "redirect_uri": {st.Redirect}, "scope": {scopes},
		"state": {st.State}, "nonce": {st.Nonce}, "code_challenge": {pkceChallenge(st.Verifier)}, "code_challenge_method": {"S256"}}
	sep := "?"
	if strings.Contains(p.AuthEndpoint, "?") {
		sep = "&"
	}
	http.Redirect(w, r, p.AuthEndpoint+sep+q.Encode(), http.StatusFound)
}

func (s *Service) adfsCallback(w http.ResponseWriter, r *http.Request) {
	fail := func(why string, err error) {
		log.Printf("auth: adfs callback refused (%s): %v", why, err)
		s.setCookie(w, r, oidcCookie, "", -1)
		http.Redirect(w, r, s.Cfg.BasePath+"/login?error=adfs_failed", http.StatusFound)
	}
	c, err := r.Cookie(oidcCookie)
	if err != nil {
		fail("no state cookie", err)
		return
	}
	var st oidcState
	if err := s.Signer.Verify(c.Value, &st); err != nil || time.Now().Unix() > st.Expires {
		fail("bad or expired state cookie", err)
		return
	}
	q := r.URL.Query()
	if e := q.Get("error"); e != "" {
		fail("provider error", errors.New(e))
		return
	}
	if q.Get("state") == "" || q.Get("state") != st.State {
		fail("state mismatch", errors.New("state"))
		return
	}
	p, err := s.provider(r.Context(), st.Issuer)
	if err != nil {
		fail("discovery", err)
		return
	}
	id, secret := s.clientCreds()
	raw, err := p.exchange(r.Context(), id, secret, q.Get("code"), st.Redirect, st.Verifier)
	if err != nil {
		fail("code exchange", err)
		return
	}
	ident, err := p.verifyIDToken(r.Context(), raw, id, st.Nonce)
	if err != nil {
		fail("id token", err)
		return
	}
	adminGroup := s.Cfg.OIDC.AdminGroup
	if !s.Cfg.OIDC.Enabled() {
		adminGroup = "klark-admins"
	}
	admin := false
	for _, g := range ident.Groups {
		if adminGroup != "" && strings.EqualFold(g, adminGroup) {
			admin = true
		}
	}
	if err := s.startSession(w, r, Session{User: ident.Name, Display: ident.Display, Admin: admin, Source: "oidc"}); err != nil {
		fail("session", err)
		return
	}
	s.setCookie(w, r, oidcCookie, "", -1)
	dest := s.Cfg.BasePath + "/"
	if st.Return != "" {
		dest = s.Cfg.BasePath + st.Return
	}
	http.Redirect(w, r, dest, http.StatusFound)
}
