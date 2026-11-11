package auth

import (
	"context"
	"crypto/rand"
	"crypto/rsa"
	"crypto/sha256"
	"encoding/base64"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"math/big"
	"net/http"
	"net/url"
	"strings"
	"sync"
	"time"

	"github.com/golang-jwt/jwt/v5"
)

// oidcProvider is one discovered OpenID Connect provider (ADFS, or the mock).
type oidcProvider struct {
	Issuer        string `json:"issuer"`
	AuthEndpoint  string `json:"authorization_endpoint"`
	TokenEndpoint string `json:"token_endpoint"`
	JWKSURI       string `json:"jwks_uri"`

	client *http.Client
	mu     sync.Mutex
	keys   map[string]*rsa.PublicKey
	fetch  time.Time
}

// discover fetches {issuer}/.well-known/openid-configuration and checks the issuer matches.
func discover(ctx context.Context, c *http.Client, issuer string) (*oidcProvider, error) {
	req, _ := http.NewRequestWithContext(ctx, http.MethodGet, strings.TrimRight(issuer, "/")+"/.well-known/openid-configuration", nil)
	resp, err := c.Do(req)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()
	if resp.StatusCode != 200 {
		return nil, fmt.Errorf("discovery: %s", resp.Status)
	}
	p := &oidcProvider{client: c}
	if err := json.NewDecoder(io.LimitReader(resp.Body, 1<<20)).Decode(p); err != nil {
		return nil, err
	}
	if strings.TrimRight(p.Issuer, "/") != strings.TrimRight(issuer, "/") {
		return nil, fmt.Errorf("discovery: issuer %q does not match %q", p.Issuer, issuer)
	}
	if p.AuthEndpoint == "" || p.TokenEndpoint == "" || p.JWKSURI == "" {
		return nil, errors.New("discovery: missing endpoints")
	}
	return p, nil
}

// key finds the signing key for kid, refetching the JWKS when it is unknown (at most once a minute).
func (p *oidcProvider) key(ctx context.Context, kid string) (*rsa.PublicKey, error) {
	p.mu.Lock()
	defer p.mu.Unlock()
	if k, ok := p.keys[kid]; ok {
		return k, nil
	}
	if p.keys != nil && time.Since(p.fetch) < time.Minute {
		return nil, fmt.Errorf("unknown signing key %q", kid)
	}
	req, _ := http.NewRequestWithContext(ctx, http.MethodGet, p.JWKSURI, nil)
	resp, err := p.client.Do(req)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()
	var set struct {
		Keys []struct {
			Kty, Kid, N, E, Use string
		} `json:"keys"`
	}
	if err := json.NewDecoder(io.LimitReader(resp.Body, 1<<20)).Decode(&set); err != nil {
		return nil, err
	}
	keys := map[string]*rsa.PublicKey{}
	for _, k := range set.Keys {
		if k.Kty != "RSA" || (k.Use != "" && k.Use != "sig") {
			continue
		}
		n, err1 := base64.RawURLEncoding.DecodeString(k.N)
		e, err2 := base64.RawURLEncoding.DecodeString(k.E)
		if err1 != nil || err2 != nil {
			continue
		}
		keys[k.Kid] = &rsa.PublicKey{N: new(big.Int).SetBytes(n), E: int(new(big.Int).SetBytes(e).Int64())}
	}
	p.keys, p.fetch = keys, time.Now()
	if k, ok := keys[kid]; ok {
		return k, nil
	}
	return nil, fmt.Errorf("unknown signing key %q", kid)
}

// Identity is who an ID token says signed in.
type Identity struct {
	Name, Display string
	Groups        []string
}

// verifyIDToken checks signature (RS256/384/512 against the JWKS), issuer, audience, expiry and nonce.
func (p *oidcProvider) verifyIDToken(ctx context.Context, raw, clientID, nonce string) (*Identity, error) {
	claims := jwt.MapClaims{}
	_, err := jwt.ParseWithClaims(raw, claims, func(t *jwt.Token) (any, error) {
		kid, _ := t.Header["kid"].(string)
		return p.key(ctx, kid)
	}, jwt.WithValidMethods([]string{"RS256", "RS384", "RS512"}), jwt.WithIssuer(p.Issuer),
		jwt.WithAudience(clientID), jwt.WithExpirationRequired(), jwt.WithLeeway(time.Minute))
	if err != nil {
		return nil, err
	}
	if n, _ := claims["nonce"].(string); n == "" || n != nonce {
		return nil, errors.New("id token nonce does not match")
	}
	str := func(keys ...string) string {
		for _, k := range keys {
			if s, _ := claims[k].(string); s != "" {
				return s
			}
		}
		return ""
	}
	id := &Identity{Name: str("preferred_username", "upn", "unique_name", "email", "sub"), Display: str("name", "unique_name")}
	if id.Name == "" {
		return nil, errors.New("id token names no user")
	}
	for _, k := range []string{"groups", "roles", "group"} {
		switch v := claims[k].(type) {
		case []any:
			for _, g := range v {
				if s, ok := g.(string); ok {
					id.Groups = append(id.Groups, s)
				}
			}
		case string:
			id.Groups = append(id.Groups, v)
		}
	}
	return id, nil
}

// exchange trades an authorization code (+ PKCE verifier) for tokens.
func (p *oidcProvider) exchange(ctx context.Context, clientID, secret, code, redirect, verifier string) (idToken string, err error) {
	form := url.Values{"grant_type": {"authorization_code"}, "code": {code}, "redirect_uri": {redirect},
		"client_id": {clientID}, "code_verifier": {verifier}}
	if secret != "" {
		form.Set("client_secret", secret)
	}
	req, _ := http.NewRequestWithContext(ctx, http.MethodPost, p.TokenEndpoint, strings.NewReader(form.Encode()))
	req.Header.Set("Content-Type", "application/x-www-form-urlencoded")
	resp, err := p.client.Do(req)
	if err != nil {
		return "", err
	}
	defer resp.Body.Close()
	body, _ := io.ReadAll(io.LimitReader(resp.Body, 1<<20))
	if resp.StatusCode != 200 {
		return "", fmt.Errorf("token endpoint: %s", resp.Status)
	}
	var tok struct {
		IDToken string `json:"id_token"`
	}
	if err := json.Unmarshal(body, &tok); err != nil || tok.IDToken == "" {
		return "", errors.New("token endpoint returned no id_token")
	}
	return tok.IDToken, nil
}

func randToken(n int) string {
	b := make([]byte, n)
	_, _ = rand.Read(b)
	return base64.RawURLEncoding.EncodeToString(b)
}

func pkceChallenge(verifier string) string {
	sum := sha256.Sum256([]byte(verifier))
	return base64.RawURLEncoding.EncodeToString(sum[:])
}
