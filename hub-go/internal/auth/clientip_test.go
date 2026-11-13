package auth

import (
	"net"
	"net/http/httptest"
	"testing"
)

func TestForwardedAddressOnlyFromTrustedProxies(t *testing.T) {
	_, proxy, _ := net.ParseCIDR("10.0.0.5/32")
	trusted := []*net.IPNet{proxy}
	req := func(peer, xff string) string {
		r := httptest.NewRequest("GET", "/", nil)
		r.RemoteAddr = peer + ":1234"
		if xff != "" {
			r.Header.Set("X-Forwarded-For", xff)
		}
		return clientIP(r, trusted)
	}
	if got := req("192.168.1.9", "1.2.3.4"); got != "192.168.1.9" {
		t.Fatalf("untrusted private peer chose its address: %s", got)
	}
	if got := req("10.0.0.5", "6.6.6.6, 1.2.3.4"); got != "1.2.3.4" {
		t.Fatalf("trusted proxy: want the last hop, got %s", got)
	}
	if got := req("10.0.0.5", "1.2.3.4, 10.0.0.5"); got != "1.2.3.4" {
		t.Fatalf("trusted hops are skipped: %s", got)
	}
	if got := req("127.0.0.1", "1.2.3.4"); got != "127.0.0.1" {
		t.Fatalf("loopback is not trusted by default: %s", got)
	}
}

func TestDirectoryAdminsAreNotOperatorsByDefault(t *testing.T) {
	if (User{Admin: true, Tenant: "default", NotOperator: true}).Operator() {
		t.Fatal("a directory admin operates every tenant")
	}
	if !(User{Admin: true, Tenant: "default"}).Operator() {
		t.Fatal("a local admin of default must operate the hub")
	}
}
