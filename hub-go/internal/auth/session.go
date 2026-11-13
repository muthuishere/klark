package auth

import (
	"crypto/hmac"
	"crypto/rand"
	"crypto/sha256"
	"encoding/base64"
	"encoding/json"
	"errors"
	"os"
	"path/filepath"
	"strings"
	"time"
)

// Session is what the signed cookie carries.
type Session struct {
	User    string `json:"u"`
	Display string `json:"d,omitempty"`
	Admin   bool   `json:"a,omitempty"`
	Source  string `json:"s"`           // "db" (user row is checked on every request) or "oidc"
	Tenant  string `json:"t,omitempty"` // a session that is not "db"; empty = the default tenant
	Role    string `json:"r,omitempty"` // platform sessions: viewer | member | admin
	Expires int64  `json:"e"`
}

// Signer signs and verifies small JSON payloads: HMAC-SHA256, base64url.
type Signer struct{ key []byte }

func NewSigner(key []byte) *Signer { return &Signer{key: key} }

// LoadKey is the key from the environment value, else the one kept in dir/session.key (made on first use).
func LoadKey(fromEnv, dir string) ([]byte, error) {
	if fromEnv != "" {
		sum := sha256.Sum256([]byte(fromEnv))
		return sum[:], nil
	}
	path := filepath.Join(dir, "session.key")
	if b, err := os.ReadFile(path); err == nil && len(b) >= 32 {
		return b[:32], nil
	}
	if err := os.MkdirAll(dir, 0o750); err != nil {
		return nil, err
	}
	k := make([]byte, 32)
	if _, err := rand.Read(k); err != nil {
		return nil, err
	}
	return k, os.WriteFile(path, k, 0o600)
}

func (s *Signer) mac(data string) string {
	h := hmac.New(sha256.New, s.key)
	h.Write([]byte(data))
	return base64.RawURLEncoding.EncodeToString(h.Sum(nil))
}

// Sign returns payload.mac.
func (s *Signer) Sign(v any) (string, error) {
	b, err := json.Marshal(v)
	if err != nil {
		return "", err
	}
	p := base64.RawURLEncoding.EncodeToString(b)
	return p + "." + s.mac(p), nil
}

var errBadToken = errors.New("bad token")

// Verify checks the signature and decodes into v.
func (s *Signer) Verify(token string, v any) error {
	p, m, ok := strings.Cut(token, ".")
	if !ok || !hmac.Equal([]byte(m), []byte(s.mac(p))) {
		return errBadToken
	}
	b, err := base64.RawURLEncoding.DecodeString(p)
	if err != nil {
		return errBadToken
	}
	return json.Unmarshal(b, v)
}

// SessionTTL is how long a sign-in lasts.
const SessionTTL = 12 * time.Hour
