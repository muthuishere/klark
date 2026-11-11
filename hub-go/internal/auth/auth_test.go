package auth

import (
	"testing"
	"time"
)

func TestPasswordsAndLimiter(t *testing.T) {
	h, _ := HashPassword("pw")
	if !VerifyPassword(h, "pw") || VerifyPassword(h, "x") || !VerifyPassword(Sha256Hash("pw"), "pw") || VerifyPassword(Sha256Hash("pw"), "x") || VerifyPassword("plain", "plain") {
		t.Fatal("password verification")
	}
	now := time.Now()
	l := NewLimiter(2, time.Minute)
	l.Now = func() time.Time { return now }
	l.Fail("k")
	l.Fail("k")
	if b, _ := l.Blocked("k"); !b {
		t.Fatal("should block")
	}
	now = now.Add(2 * time.Minute)
	if b, _ := l.Blocked("k"); b {
		t.Fatal("window should expire")
	}
}

func TestSignerRejectsTampering(t *testing.T) {
	s := NewSigner([]byte("k"))
	tok, _ := s.Sign(Session{User: "a", Source: "db"})
	var out Session
	if s.Verify(tok, &out) != nil || out.User != "a" {
		t.Fatal("round trip")
	}
	if s.Verify("x"+tok, &out) == nil || NewSigner([]byte("other")).Verify(tok, &out) == nil {
		t.Fatal("tampering accepted")
	}
}
