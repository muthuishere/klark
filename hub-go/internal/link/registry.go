package link

import (
	"context"
	"crypto/rand"
	"crypto/subtle"
	"encoding/hex"
	"fmt"
	"sync"
	"time"

	"github.com/muthuishere/klark/hub-go/internal/tokens"
)

// Protocol versions this hub speaks (the machine's hello must be inside).
const (
	MinProtocol = 4
	MaxProtocol = 5
)

// Registry holds the live machine sessions.
type Registry struct {
	OfflineAfter time.Duration // a machine with no event stream this long is dropped
	Heartbeat    time.Duration // an idle event stream carries a ping this often
	// OnMessage receives every message a machine pushes (hello excluded).
	OnMessage func(c *Conn, msg map[string]any)
	// OnOpen is called when a machine opens a session.
	OnOpen func(c *Conn)

	mu       sync.Mutex
	machines map[string]*Conn
	sessions map[string]*Conn
}

// NewRegistry has the production timings: offline after 45 s, a heartbeat every 15 s.
func NewRegistry() *Registry {
	return &Registry{OfflineAfter: 45 * time.Second, Heartbeat: 15 * time.Second,
		machines: map[string]*Conn{}, sessions: map[string]*Conn{}}
}

// Open starts a session for machineID. A newer session for the same machine replaces the old one.
func (r *Registry) Open(machineID, token string, hello map[string]any) (*Conn, error) {
	if t, _ := hello["type"].(string); t != "hello" {
		return nil, fmt.Errorf("the body must be a hello message")
	}
	protocol, _ := hello["protocol"].(float64)
	if int(protocol) < MinProtocol || int(protocol) > MaxProtocol {
		return nil, fmt.Errorf("unsupported protocol %d; hub speaks %d-%d", int(protocol), MinProtocol, MaxProtocol)
	}
	raw := make([]byte, 16)
	_, _ = rand.Read(raw)
	conn := newConn(machineID, hex.EncodeToString(raw), tokens.Sha256Hex(token), r, hello)
	r.mu.Lock()
	if prev, ok := r.machines[machineID]; ok {
		r.removeLocked(prev)
	}
	r.machines[machineID] = conn
	r.sessions[conn.SessionID] = conn
	r.mu.Unlock()
	if r.OnOpen != nil {
		r.OnOpen(conn)
	}
	return conn, nil
}

// Session is the live session with this id whose machine token is token. wrongToken is true
// when the session exists but the token is another machine's.
func (r *Registry) Session(id, token string) (conn *Conn, wrongToken bool) {
	r.mu.Lock()
	c := r.sessions[id]
	r.mu.Unlock()
	if id == "" || c == nil {
		return nil, false
	}
	if subtle.ConstantTimeCompare([]byte(c.tokenSHA), []byte(tokens.Sha256Hex(token))) != 1 {
		return nil, true
	}
	return c, false
}

// Machines are the ids of the machines with a live session.
func (r *Registry) Machines() []string {
	r.mu.Lock()
	defer r.mu.Unlock()
	ids := make([]string, 0, len(r.machines))
	for id := range r.machines {
		ids = append(ids, id)
	}
	return ids
}

// Revoke drops a machine's session at once.
func (r *Registry) Revoke(machineID string) {
	r.mu.Lock()
	if c, ok := r.machines[machineID]; ok {
		r.removeLocked(c)
	}
	r.mu.Unlock()
}

func (r *Registry) removeLocked(c *Conn) {
	c.Close()
	delete(r.sessions, c.SessionID)
	if r.machines[c.ID] == c {
		delete(r.machines, c.ID)
	}
}

// Sweep drops machines whose event stream has been gone for OfflineAfter and rings quiet streams.
func (r *Registry) Sweep() {
	r.mu.Lock()
	defer r.mu.Unlock()
	for _, c := range r.sessions {
		if c.idleFor() > r.OfflineAfter {
			r.removeLocked(c)
			continue
		}
		c.pingIfQuiet(r.Heartbeat)
	}
}

// Run sweeps every 5 s until ctx ends.
func (r *Registry) Run(ctx context.Context) {
	t := time.NewTicker(5 * time.Second)
	defer t.Stop()
	for {
		select {
		case <-ctx.Done():
			return
		case <-t.C:
			r.Sweep()
		}
	}
}
