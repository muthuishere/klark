// Package tokens matches machine tokens. Only a token's SHA-256 is ever stored.
package tokens

import (
	"crypto/sha256"
	"crypto/subtle"
	"encoding/hex"
	"strings"
)

// Machine is a machine (or a fleet of machines sharing one token) the hub accepts.
type Machine struct {
	Name        string
	TokenSHA256 string
	// Fleet tokens are shared by several machines; each one names itself in X-Klark-Machine.
	Fleet bool
}

// Source lists the machines the hub currently accepts (configuration, later hub.db).
type Source interface {
	Machines() []Machine
}

// Static is a fixed list of machines.
type Static []Machine

func (s Static) Machines() []Machine { return s }

// Sha256Hex is the lowercase hex SHA-256 of a token.
func Sha256Hex(token string) string {
	sum := sha256.Sum256([]byte(token))
	return hex.EncodeToString(sum[:])
}

// Match returns the machine whose token is token, compared in constant time.
func Match(machines []Machine, token string) *Machine {
	if token == "" {
		return nil
	}
	presented := []byte(Sha256Hex(token))
	var found *Machine
	for i := range machines {
		expected := []byte(strings.ToLower(strings.TrimSpace(machines[i].TokenSHA256)))
		if len(expected) == len(presented) && subtle.ConstantTimeCompare(expected, presented) == 1 {
			found = &machines[i]
		}
	}
	return found
}

// ResolveMachine is the id a request acts as: the machine's name, or for a fleet token
// "fleet:member" from the X-Klark-Machine header. A non-empty message means refused.
func ResolveMachine(m *Machine, header string) (id string, refused string) {
	if !m.Fleet {
		return m.Name, ""
	}
	member := make([]rune, 0, 64)
	for _, c := range header {
		if len(member) == 64 {
			break
		}
		if c < 128 && (c >= '0' && c <= '9' || c >= 'a' && c <= 'z' || c >= 'A' && c <= 'Z' || c == '-' || c == '_' || c == '.') {
			member = append(member, c)
		}
	}
	if len(member) == 0 {
		return "", "fleet tokens need an X-Klark-Machine header"
	}
	return m.Name + ":" + string(member), ""
}
