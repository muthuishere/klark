// Package hub is the running side of the hub: what the connected machines said, and what the browsers ask of them.
// It sits on top of internal/link (the wire) and keeps the lazy rule of ADR 0001: a machine streams or is asked
// for its tree only while someone is looking.
package hub

import (
	"context"
	"encoding/json"
	"errors"
	"strconv"
	"sync"
	"time"

	"github.com/muthuishere/klark/hub-go/internal/link"
)

var (
	ErrOffline = errors.New("machine is offline")
	ErrTimeout = errors.New("the machine did not answer in time")
)

// RemoteError is an error the machine itself answered a request with.
type RemoteError struct{ Msg string }

func (e *RemoteError) Error() string { return e.Msg }

// AllowedCalls mirrors ALLOWED_CALLS in crates/klark-link/src/protocol.rs: the only socket methods a hub may invoke.
var AllowedCalls = map[string]bool{
	"ping": true, "session.snapshot": true, "pane.read": true, "agent.read": true, "pane.get": true,
	"workspace.list": true, "tab.list": true, "pane.list": true,
	"system.stats": true, // the hub's own analytics collector (internal/stats)
	"feed.list":    true, // read-only feed status (ADR 0015); feed.start / feed.stop stay off
}

// Info describes a machine for the machine list.
type Info struct {
	ID       string    `json:"id"`
	Name     string    `json:"name"`
	Online   bool      `json:"online"`
	Version  string    `json:"version"`
	Protocol int       `json:"protocol"`
	LastSeen time.Time `json:"lastSeen"`
}

type treeResult struct {
	rev  string
	snap json.RawMessage
	err  error
}

type machine struct {
	id string
	rt *Runtime

	mu       sync.Mutex
	conn     *link.Conn
	name     string
	version  string
	protocol int
	lastSeen time.Time
	nextID   int64
	pending  map[string]chan reply

	treeRev     string
	treeSnap    json.RawMessage
	treeChecked time.Time
	treeWait    []chan treeResult
	treeWatch   map[*TreeWatcher]struct{}
	treeLoop    bool

	panes map[string]*paneState
}

type reply struct {
	result json.RawMessage
	err    error
}

// Runtime tracks every machine that has linked since the hub started.
type Runtime struct {
	Reg *link.Registry
	// OnSeen is told when a machine links (to remember its version and protocol).
	OnSeen func(id, version string, protocol int)
	// TreePoll is how often a watched machine's tree is re-asked (default 2 s).
	TreePoll time.Duration
	// TreeFresh is how long an answer to get_tree is reused (default 1.5 s).
	TreeFresh time.Duration

	// FeedPoll is how often a watched chat is re-read (default 1 s).
	FeedPoll time.Duration

	mu       sync.Mutex
	machines map[string]*machine
	feeds    map[string]*Feed
}

// New wires a Runtime onto reg (it takes over reg.OnOpen and reg.OnMessage).
func New(reg *link.Registry) *Runtime {
	rt := &Runtime{Reg: reg, TreePoll: 2 * time.Second, TreeFresh: 1500 * time.Millisecond, FeedPoll: time.Second, machines: map[string]*machine{}}
	reg.OnOpen = rt.opened
	reg.OnMessage = rt.message
	return rt
}

func (rt *Runtime) get(id string) *machine {
	rt.mu.Lock()
	defer rt.mu.Unlock()
	m := rt.machines[id]
	if m == nil {
		m = &machine{id: id, rt: rt, pending: map[string]chan reply{}, panes: map[string]*paneState{}, treeWatch: map[*TreeWatcher]struct{}{}}
		rt.machines[id] = m
	}
	return m
}

func str(v any) string { s, _ := v.(string); return s }

func (rt *Runtime) opened(c *link.Conn) {
	m := rt.get(c.ID)
	m.mu.Lock()
	m.conn = c
	m.name, m.version = str(c.Hello["machine_name"]), str(c.Hello["klark_version"])
	p, _ := c.Hello["protocol"].(float64)
	m.protocol = int(p)
	m.lastSeen = time.Now()
	// the old session's requests will never be answered
	for id, ch := range m.pending {
		ch <- reply{err: errors.New("machine reconnected")}
		delete(m.pending, id)
	}
	for _, w := range m.treeWait {
		w <- treeResult{err: errors.New("machine reconnected")}
	}
	m.treeWait = nil
	var resub []string
	for pane, ps := range m.panes {
		ps.mu.Lock()
		if len(ps.viewers) > 0 {
			resub = append(resub, pane)
		}
		ps.mu.Unlock()
	}
	m.mu.Unlock()
	for _, pane := range resub { // the new session knows nothing of the old subscriptions
		c.Send(map[string]any{"type": "subscribe", "pane_id": pane})
	}
	if rt.OnSeen != nil {
		rt.OnSeen(c.ID, m.version, m.protocol)
	}
}

func (rt *Runtime) message(c *link.Conn, msg map[string]any) {
	m := rt.get(c.ID)
	m.mu.Lock()
	if m.conn != c {
		m.mu.Unlock()
		return
	}
	m.lastSeen = time.Now()
	m.mu.Unlock()
	switch str(msg["type"]) {
	case "tree":
		snap, _ := json.Marshal(msg["snapshot"])
		m.gotTree(str(msg["rev"]), snap)
	case "tree_unchanged":
		m.gotTree(str(msg["rev"]), nil)
	case "result":
		m.gotResult(str(msg["id"]), msg)
	case "frame":
		m.pane(str(msg["pane_id"])).broadcast(Event{Type: "frame", Data: str(msg["bytes"])})
	case "closed":
		m.pane(str(msg["pane_id"])).broadcast(Event{Type: "closed", Data: str(msg["reason"])})
	case "pane_size":
		cols, _ := msg["cols"].(float64)
		rows, _ := msg["rows"].(float64)
		m.pane(str(msg["pane_id"])).setSize(int(cols), int(rows))
	}
}

// Machines lists every machine that has linked since start, online or not.
func (rt *Runtime) Machines() []Info {
	rt.mu.Lock()
	ms := make([]*machine, 0, len(rt.machines))
	for _, m := range rt.machines {
		ms = append(ms, m)
	}
	rt.mu.Unlock()
	out := make([]Info, 0, len(ms))
	for _, m := range ms {
		out = append(out, m.info())
	}
	return out
}

func (m *machine) info() Info {
	m.mu.Lock()
	defer m.mu.Unlock()
	return Info{ID: m.id, Name: m.name, Online: m.conn != nil && !m.conn.IsClosed(), Version: m.version, Protocol: m.protocol, LastSeen: m.lastSeen}
}

// Info of one machine; ok is false when it never linked.
func (rt *Runtime) Info(id string) (Info, bool) {
	rt.mu.Lock()
	m := rt.machines[id]
	rt.mu.Unlock()
	if m == nil {
		return Info{}, false
	}
	return m.info(), true
}

// Online reports whether id has a live session.
func (rt *Runtime) Online(id string) bool {
	in, ok := rt.Info(id)
	return ok && in.Online
}

// Revoke drops the machine's session at once.
func (rt *Runtime) Revoke(id string) { rt.Reg.Revoke(id) }

func (m *machine) live() (*link.Conn, error) {
	m.mu.Lock()
	defer m.mu.Unlock()
	if m.conn == nil || m.conn.IsClosed() {
		return nil, ErrOffline
	}
	return m.conn, nil
}

func (m *machine) gotResult(id string, msg map[string]any) {
	m.mu.Lock()
	ch := m.pending[id]
	delete(m.pending, id)
	m.mu.Unlock()
	if ch == nil {
		return
	}
	if e := str(msg["error"]); e != "" {
		ch <- reply{err: &RemoteError{e}}
		return
	}
	b, _ := json.Marshal(msg["result"])
	ch <- reply{result: b}
}

// Request sends msg (type + fields) with a fresh id and waits for the machine's result.
func (rt *Runtime) Request(ctx context.Context, machineID string, msg map[string]any, timeout time.Duration) (json.RawMessage, error) {
	return rt.get(machineID).request(ctx, msg, timeout)
}

func (m *machine) request(ctx context.Context, msg map[string]any, timeout time.Duration) (json.RawMessage, error) {
	c, err := m.live()
	if err != nil {
		return nil, err
	}
	m.mu.Lock()
	m.nextID++
	id := strconv.FormatInt(m.nextID, 10)
	ch := make(chan reply, 1)
	m.pending[id] = ch
	m.mu.Unlock()
	out := make(map[string]any, len(msg)+1)
	for k, v := range msg {
		out[k] = v
	}
	out["id"] = id
	c.Send(out)
	t := time.NewTimer(timeout)
	defer t.Stop()
	select {
	case r := <-ch:
		return r.result, r.err
	case <-t.C:
		err = ErrTimeout
	case <-ctx.Done():
		err = ctx.Err()
	}
	m.mu.Lock()
	delete(m.pending, id)
	m.mu.Unlock()
	return nil, err
}

// Call is an allow-listed socket read (pane.read, agent.read, ...), answered with the machine's result.
func (rt *Runtime) Call(ctx context.Context, machineID, method string, params any, timeout time.Duration) (json.RawMessage, error) {
	if !AllowedCalls[method] {
		return nil, errors.New("method not allowed: " + method)
	}
	return rt.Request(ctx, machineID, map[string]any{"type": "call", "method": method, "params": params}, timeout)
}

func (rt *Runtime) send(machineID string, msg map[string]any) error {
	c, err := rt.get(machineID).live()
	if err != nil {
		return err
	}
	c.Send(msg)
	return nil
}

// Input types text into a pane; Keys sends logical keys.
func (rt *Runtime) Input(machineID, pane, text string) error {
	return rt.send(machineID, map[string]any{"type": "input", "pane_id": pane, "text": text})
}

func (rt *Runtime) Keys(machineID, pane string, keys []string) error {
	return rt.send(machineID, map[string]any{"type": "keys", "pane_id": pane, "keys": keys})
}

// ReadScreen is the pane's current screen with colours (so a new viewer is not blank).
func (rt *Runtime) ReadScreen(ctx context.Context, machineID, pane string) (string, error) {
	raw, err := rt.Call(ctx, machineID, "pane.read", map[string]any{"pane_id": pane, "source": "visible", "format": "ansi", "strip_ansi": false}, 8*time.Second)
	if err != nil {
		return "", err
	}
	var r struct{ Read struct{ Text string } }
	_ = json.Unmarshal(raw, &r)
	return r.Read.Text, nil
}
