package auth

import (
	"crypto/rand"
	"crypto/rsa"
	"crypto/subtle"
	"encoding/base64"
	"encoding/json"
	"html"
	"math/big"
	"net/http"
	"net/url"
	"strings"
	"sync"
	"time"

	"github.com/golang-jwt/jwt/v5"

	"github.com/muthuishere/klark/hub-go/internal/config"
)

// Mock ADFS client credentials: the hub configures itself with these when HUB_MOCK_ADFS=1.
const (
	MockClientID     = "klark-hub"
	MockClientSecret = "mock-adfs-secret"
)

// MockADFS is an OAuth2/OIDC authorization-code provider (with PKCE) served by the hub itself under
// /mock-adfs/. For development and tests only: users have clear passwords in the environment.
type MockADFS struct {
	Users []config.MockUser
	// Issuer returns the issuer URL for a request (external base + base path + /mock-adfs).
	Issuer func(r *http.Request) string

	key  *rsa.PrivateKey
	kid  string
	lim  *Limiter
	mu   sync.Mutex
	code map[string]mockCode
	tok  map[string]string // access token -> user
}

type mockCode struct {
	user, nonce, challenge, redirect string
	expires                          time.Time
}

func NewMockADFS(users []config.MockUser, issuer func(*http.Request) string) (*MockADFS, error) {
	key, err := rsa.GenerateKey(rand.Reader, 2048)
	if err != nil {
		return nil, err
	}
	return &MockADFS{Users: users, Issuer: issuer, key: key, kid: randToken(6), lim: NewLimiter(10, 5*time.Minute),
		code: map[string]mockCode{}, tok: map[string]string{}}, nil
}

// Mount registers the provider's routes (paths relative to the hub's base path).
func (m *MockADFS) Mount(mux *http.ServeMux) {
	mux.HandleFunc("GET /mock-adfs/.well-known/openid-configuration", m.discovery)
	mux.HandleFunc("GET /mock-adfs/authorize", m.authorizeForm)
	mux.HandleFunc("POST /mock-adfs/authorize", m.authorizeSubmit)
	mux.HandleFunc("POST /mock-adfs/token", m.token)
	mux.HandleFunc("GET /mock-adfs/jwks", m.jwks)
	mux.HandleFunc("GET /mock-adfs/userinfo", m.userinfo)
}

func jsonOut(w http.ResponseWriter, v any) {
	w.Header().Set("Content-Type", "application/json")
	w.Header().Set("Cache-Control", "no-store")
	_ = json.NewEncoder(w).Encode(v)
}

func (m *MockADFS) discovery(w http.ResponseWriter, r *http.Request) {
	iss := m.Issuer(r)
	jsonOut(w, map[string]any{
		"issuer": iss, "authorization_endpoint": iss + "/authorize", "token_endpoint": iss + "/token",
		"jwks_uri": iss + "/jwks", "userinfo_endpoint": iss + "/userinfo",
		"response_types_supported": []string{"code"}, "subject_types_supported": []string{"public"},
		"id_token_signing_alg_values_supported": []string{"RS256"}, "code_challenge_methods_supported": []string{"S256"},
	})
}

func (m *MockADFS) jwks(w http.ResponseWriter, _ *http.Request) {
	e := big.NewInt(int64(m.key.E)).Bytes()
	jsonOut(w, map[string]any{"keys": []map[string]string{{
		"kty": "RSA", "use": "sig", "alg": "RS256", "kid": m.kid,
		"n": base64.RawURLEncoding.EncodeToString(m.key.N.Bytes()), "e": base64.RawURLEncoding.EncodeToString(e)}}})
}

const loginPage = `<!doctype html><html><head><meta charset="utf-8"><title>Mock ADFS</title>
<meta name="viewport" content="width=device-width,initial-scale=1">
<style>body{font:16px system-ui;max-width:22rem;margin:4rem auto;padding:0 1rem}input,button{display:block;width:100%;padding:.6rem;margin:.5rem 0;box-sizing:border-box}.err{color:#b00}</style></head>
<body><h1>Mock ADFS</h1><p>Development sign-in. Not a real identity provider.</p>%s
<form method="post" action="">%s<label>User<input name="username" autocomplete="username" autofocus></label>
<label>Password<input name="password" type="password" autocomplete="current-password"></label>
<button type="submit">Sign in</button></form></body></html>`

func hidden(q url.Values) string {
	var b strings.Builder
	for _, k := range []string{"response_type", "client_id", "redirect_uri", "scope", "state", "nonce", "code_challenge", "code_challenge_method"} {
		b.WriteString(`<input type="hidden" name="` + k + `" value="` + html.EscapeString(q.Get(k)) + `">`)
	}
	return b.String()
}

// validAuthorize checks the request is for our client and a redirect that ends at the hub's callback.
func validAuthorize(q url.Values) string {
	switch {
	case q.Get("response_type") != "code":
		return "response_type must be code"
	case q.Get("client_id") != MockClientID:
		return "unknown client_id"
	case !strings.HasSuffix(q.Get("redirect_uri"), "/auth/adfs/callback"):
		return "redirect_uri is not the hub's callback"
	case q.Get("code_challenge") == "" || q.Get("code_challenge_method") != "S256":
		return "PKCE (S256) is required"
	case q.Get("state") == "":
		return "state is required"
	}
	return ""
}

func (m *MockADFS) authorizeForm(w http.ResponseWriter, r *http.Request) {
	q := r.URL.Query()
	if msg := validAuthorize(q); msg != "" {
		http.Error(w, msg, http.StatusBadRequest)
		return
	}
	w.Header().Set("Content-Type", "text/html; charset=utf-8")
	w.Header().Set("Cache-Control", "no-store")
	_, _ = w.Write([]byte(strings.Replace(strings.Replace(loginPage, "%s", "", 1), "%s", hidden(q), 1)))
}

func (m *MockADFS) authorizeSubmit(w http.ResponseWriter, r *http.Request) {
	if err := r.ParseForm(); err != nil {
		http.Error(w, "bad form", http.StatusBadRequest)
		return
	}
	q := r.PostForm
	if msg := validAuthorize(q); msg != "" {
		http.Error(w, msg, http.StatusBadRequest)
		return
	}
	name := strings.TrimSpace(q.Get("username"))
	if blocked, _ := m.lim.Blocked(name); blocked {
		http.Error(w, "too many failures", http.StatusTooManyRequests)
		return
	}
	var user *config.MockUser
	for i := range m.Users {
		if strings.EqualFold(m.Users[i].Name, name) &&
			subtle.ConstantTimeCompare([]byte(m.Users[i].Password), []byte(q.Get("password"))) == 1 {
			user = &m.Users[i]
		}
	}
	if user == nil {
		m.lim.Fail(name)
		w.Header().Set("Content-Type", "text/html; charset=utf-8")
		w.WriteHeader(http.StatusUnauthorized)
		_, _ = w.Write([]byte(strings.Replace(strings.Replace(loginPage, "%s", `<p class="err">Wrong user or password.</p>`, 1), "%s", hidden(q), 1)))
		return
	}
	code := randToken(24)
	m.mu.Lock()
	m.code[code] = mockCode{user: user.Name, nonce: q.Get("nonce"), challenge: q.Get("code_challenge"),
		redirect: q.Get("redirect_uri"), expires: time.Now().Add(2 * time.Minute)}
	m.mu.Unlock()
	u, _ := url.Parse(q.Get("redirect_uri"))
	v := u.Query()
	v.Set("code", code)
	v.Set("state", q.Get("state"))
	u.RawQuery = v.Encode()
	http.Redirect(w, r, u.String(), http.StatusFound)
}

func (m *MockADFS) token(w http.ResponseWriter, r *http.Request) {
	if err := r.ParseForm(); err != nil {
		oauthErr(w, "invalid_request")
		return
	}
	f := r.PostForm
	if f.Get("grant_type") != "authorization_code" || f.Get("client_id") != MockClientID ||
		subtle.ConstantTimeCompare([]byte(f.Get("client_secret")), []byte(MockClientSecret)) != 1 {
		oauthErr(w, "invalid_client")
		return
	}
	m.mu.Lock()
	c, ok := m.code[f.Get("code")]
	delete(m.code, f.Get("code")) // single use
	m.mu.Unlock()
	if !ok || time.Now().After(c.expires) || c.redirect != f.Get("redirect_uri") ||
		subtle.ConstantTimeCompare([]byte(pkceChallenge(f.Get("code_verifier"))), []byte(c.challenge)) != 1 {
		oauthErr(w, "invalid_grant")
		return
	}
	var u config.MockUser
	for _, x := range m.Users {
		if x.Name == c.user {
			u = x
		}
	}
	claims := jwt.MapClaims{"iss": m.Issuer(r), "sub": u.Name, "aud": MockClientID, "iat": time.Now().Unix(),
		"exp": time.Now().Add(10 * time.Minute).Unix(), "nonce": c.nonce, "preferred_username": u.Name, "name": u.Name,
		"email": u.Name + "@mock-adfs.invalid"}
	if u.Admin {
		claims["groups"] = []string{"klark-admins"}
	} else {
		claims["groups"] = []string{"klark-users"}
	}
	tk := jwt.NewWithClaims(jwt.SigningMethodRS256, claims)
	tk.Header["kid"] = m.kid
	id, err := tk.SignedString(m.key)
	if err != nil {
		oauthErr(w, "server_error")
		return
	}
	access := randToken(24)
	m.mu.Lock()
	m.tok[access] = u.Name
	m.mu.Unlock()
	jsonOut(w, map[string]any{"access_token": access, "token_type": "Bearer", "expires_in": 600, "id_token": id})
}

func oauthErr(w http.ResponseWriter, code string) {
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(http.StatusBadRequest)
	_ = json.NewEncoder(w).Encode(map[string]string{"error": code})
}

func (m *MockADFS) userinfo(w http.ResponseWriter, r *http.Request) {
	h := r.Header.Get("Authorization")
	m.mu.Lock()
	name, ok := m.tok[strings.TrimSpace(strings.TrimPrefix(h, "Bearer "))]
	m.mu.Unlock()
	if !ok {
		w.WriteHeader(http.StatusUnauthorized)
		return
	}
	jsonOut(w, map[string]string{"sub": name, "preferred_username": name, "name": name})
}
