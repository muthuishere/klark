// Package link is the hub side of the machine link (ADR 0013): a machine opens a session,
// reads hub messages from one held NDJSON stream and pushes its own in POSTed segments.
package link

import (
	"bytes"
	"context"
	"encoding/json"
	"net/http"
	"sync"
	"sync/atomic"
	"time"
)

const outboxLimit = 4096

var pingLine = []byte("{\"type\":\"ping\"}\n")

type outEntry struct {
	seq  int64
	line []byte
}

// Conn is one machine's live session.
type Conn struct {
	ID        string
	SessionID string
	Hello     map[string]any

	tokenSHA string
	reg      *Registry

	closed    chan struct{}
	closeOnce sync.Once

	mu           sync.Mutex
	outbox       []outEntry
	outSeq       int64
	cancelStream context.CancelFunc
	streamGen    int64
	bell         chan struct{}
	pingDue      atomic.Bool
	streams      atomic.Int32
	lastWrite    atomic.Int64 // unix nanos
	lastSeen     atomic.Int64 // unix nanos

	pushMu   sync.Mutex
	lastPush int64
}

func newConn(id, sessionID, tokenSHA string, reg *Registry, hello map[string]any) *Conn {
	c := &Conn{ID: id, SessionID: sessionID, Hello: hello, tokenSHA: tokenSHA, reg: reg,
		closed: make(chan struct{}), bell: make(chan struct{}, 1)}
	now := time.Now().UnixNano()
	c.lastWrite.Store(now)
	c.lastSeen.Store(now)
	return c
}

func (c *Conn) seen() { c.lastSeen.Store(time.Now().UnixNano()) }

// IsClosed reports whether the hub dropped this session (replaced, revoked, offline).
func (c *Conn) IsClosed() bool {
	select {
	case <-c.closed:
		return true
	default:
		return false
	}
}

// Close drops the session; its stream ends.
func (c *Conn) Close() { c.closeOnce.Do(func() { close(c.closed) }) }

// idleFor is zero while an event stream is open, else how long since the machine was heard.
func (c *Conn) idleFor() time.Duration {
	if c.streams.Load() > 0 {
		return 0
	}
	return time.Since(time.Unix(0, c.lastSeen.Load()))
}

func (c *Conn) ring() {
	select {
	case c.bell <- struct{}{}:
	default:
	}
}

// Send queues a hub message for the machine; it carries the next seq.
func (c *Conn) Send(msg map[string]any) {
	c.mu.Lock()
	c.outSeq++
	out := make(map[string]any, len(msg)+1)
	for k, v := range msg {
		out[k] = v
	}
	out["seq"] = c.outSeq
	line, _ := json.Marshal(out)
	line = append(line, '\n')
	if len(c.outbox) >= outboxLimit {
		c.outbox = append(c.outbox[:0], c.outbox[len(c.outbox)/2:]...)
	}
	c.outbox = append(c.outbox, outEntry{c.outSeq, line})
	c.mu.Unlock()
	c.ring()
}

// Stream serves GET /agent/events until the request ends, the session closes or a newer
// stream for this session replaces it. Messages with seq <= after are not resent.
func (c *Conn) Stream(ctx context.Context, w http.ResponseWriter, after int64) {
	sctx, cancel := context.WithCancel(ctx)
	defer cancel()
	c.mu.Lock()
	if c.cancelStream != nil {
		c.cancelStream()
	}
	c.streamGen++
	gen := c.streamGen
	c.cancelStream = cancel
	kept := c.outbox[:0]
	for _, e := range c.outbox {
		if e.seq > after {
			kept = append(kept, e)
		}
	}
	c.outbox = kept
	c.mu.Unlock()

	c.streams.Add(1)
	c.seen()
	defer func() {
		c.seen()
		c.streams.Add(-1)
		c.mu.Lock()
		if c.streamGen == gen {
			c.cancelStream = nil
		}
		c.mu.Unlock()
	}()

	h := w.Header()
	h.Set("Content-Type", "application/x-ndjson")
	h.Set("Cache-Control", "no-cache")
	h.Set("X-Accel-Buffering", "no")
	w.WriteHeader(http.StatusOK)
	rc := http.NewResponseController(w)
	_ = rc.Flush()

	sent := after
	c.ring() // whatever is already queued
	for {
		select {
		case <-sctx.Done():
			return
		case <-c.closed:
			return
		case <-c.bell:
		}
		var batch bytes.Buffer
		c.mu.Lock()
		for _, e := range c.outbox {
			if e.seq > sent {
				batch.Write(e.line)
				sent = e.seq
			}
		}
		c.mu.Unlock()
		wrote := batch.Len() > 0
		if c.pingDue.Swap(false) && !wrote {
			batch.Write(pingLine)
			wrote = true
		}
		if !wrote {
			continue
		}
		if _, err := w.Write(batch.Bytes()); err != nil {
			return
		}
		if rc.Flush() != nil {
			return
		}
		c.lastWrite.Store(time.Now().UnixNano())
	}
}

// Push takes one segment (NDJSON, one machine message per line). A seq already taken is a
// retried push and is ignored; false means duplicate.
func (c *Conn) Push(seq int64, ndjson []byte) bool {
	c.seen()
	c.pushMu.Lock()
	defer c.pushMu.Unlock()
	if seq <= c.lastPush {
		return false
	}
	c.lastPush = seq
	for _, line := range bytes.Split(ndjson, []byte("\n")) {
		line = bytes.TrimRight(line, "\r")
		if len(bytes.TrimSpace(line)) == 0 {
			continue
		}
		var msg map[string]any
		if err := json.Unmarshal(line, &msg); err != nil {
			continue // a bad line is skipped, like the C# hub
		}
		if c.reg.OnMessage != nil {
			c.reg.OnMessage(c, msg)
		}
	}
	return true
}

func (c *Conn) pingIfQuiet(every time.Duration) {
	if c.streams.Load() == 0 {
		return
	}
	if time.Since(time.Unix(0, c.lastWrite.Load())) < every {
		return
	}
	c.pingDue.Store(true)
	c.ring()
}
