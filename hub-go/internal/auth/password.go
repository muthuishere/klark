// Package auth: static credentials, signed session cookies, an OIDC/ADFS client and a mock ADFS provider.
package auth

import (
	"crypto/sha256"
	"crypto/subtle"
	"encoding/hex"
	"strings"

	"golang.org/x/crypto/bcrypt"
)

// HashPassword is a bcrypt hash of pw.
func HashPassword(pw string) (string, error) {
	b, err := bcrypt.GenerateFromPassword([]byte(pw), bcrypt.DefaultCost)
	return string(b), err
}

// Sha256Hash is the "sha256:<hex>" form HUB_USERS also accepts.
func Sha256Hash(pw string) string {
	sum := sha256.Sum256([]byte(pw))
	return "sha256:" + hex.EncodeToString(sum[:])
}

// VerifyPassword checks pw against a bcrypt hash or a "sha256:<hex>" hash.
func VerifyPassword(stored, pw string) bool {
	switch {
	case strings.HasPrefix(stored, "sha256:"):
		want := strings.ToLower(strings.TrimSpace(strings.TrimPrefix(stored, "sha256:")))
		sum := sha256.Sum256([]byte(pw))
		got := hex.EncodeToString(sum[:])
		return len(want) == len(got) && subtle.ConstantTimeCompare([]byte(want), []byte(got)) == 1
	case strings.HasPrefix(stored, "$2"):
		return bcrypt.CompareHashAndPassword([]byte(stored), []byte(pw)) == nil
	}
	return false
}
