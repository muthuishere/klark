package app

import (
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"strings"
	"time"

	"github.com/muthuishere/klark/hub-go/internal/chat"
	"github.com/muthuishere/klark/hub-go/internal/hub"
)

type sse struct {
	w  http.ResponseWriter
	rc *http.ResponseController
}

func newSSE(w http.ResponseWriter) *sse {
	h := w.Header()
	h.Set("Content-Type", "text/event-stream")
	h.Set("Cache-Control", "no-cache, no-transform")
	h.Set("X-Accel-Buffering", "no")
	h.Set("Connection", "keep-alive")
	w.WriteHeader(http.StatusOK)
	s := &sse{w: w, rc: http.NewResponseController(w)}
	_ = s.rc.Flush()
	return s
}

// event writes one SSE event; data is a string (sent as is) or anything JSON.
func (s *sse) event(name string, data any) error {
	var d string
	switch v := data.(type) {
	case string:
		d = v
	default:
		b, _ := json.Marshal(v)
		d = string(b)
	}
	if _, err := fmt.Fprintf(s.w, "event: %s\ndata: %s\n\n", name, strings.ReplaceAll(d, "\n", "\ndata: ")); err != nil {
		return err
	}
	return s.rc.Flush()
}

func (s *sse) ping() error {
	if _, err := fmt.Fprint(s.w, ": ping\n\n"); err != nil {
		return err
	}
	return s.rc.Flush()
}

type treeEvent struct {
	Rev      string          `json:"rev"`
	Snapshot json.RawMessage `json:"snapshot"`
}

// machineEvents streams a machine's `tree` (when its revision changes) and `status` ({online}) events. The hub
// asks the machine for its tree only while this stream, or another watcher, is open.
func (a *App) machineEvents(w http.ResponseWriter, r *http.Request) {
	id, ok := a.machineOr404(w, r)
	if !ok {
		return
	}
	tw := a.Runtime.WatchTree(id)
	defer tw.Close()
	s := newSSE(w)
	_ = s.event("status", map[string]bool{"online": a.Runtime.Online(id)})
	keep := time.NewTicker(15 * time.Second)
	defer keep.Stop()
	for {
		select {
		case <-r.Context().Done():
			return
		case ev := <-tw.C:
			if ev.Online != nil {
				_ = s.event("status", map[string]bool{"online": *ev.Online})
			}
			if ev.Tree != nil {
				if s.event("tree", treeEvent{ev.Tree.Rev, ev.Tree.Snapshot}) != nil {
					return
				}
			}
		case <-keep.C:
			if s.ping() != nil {
				return
			}
		}
	}
}

// chatEvent is the `chat` event: rows are upserted by id; replace=true means they are the whole chat (rows
// the client holds that are not in it are gone).
type chatEvent struct {
	Rows    []chat.Item   `json:"rows"`
	Replace bool          `json:"replace,omitempty"`
	Rev     int64         `json:"rev"`
	Agent   string        `json:"agent"`
	Status  string        `json:"status"`
	Source  string        `json:"source"`
	Error   string        `json:"error,omitempty"`
	Blocked *chat.Blocked `json:"blocked,omitempty"`
}

// paneEvents is the pane's live stream. Always: `chat` ({rows}, upserted by id), `tree` and `status`. With
// ?frames=1 also `frame` ({data: base64 bytes, reset?: true to clear and repaint}) and `size` ({cols,rows}),
// and `closed` ({reason}); only then does the machine stream the pane.
func (a *App) paneEvents(w http.ResponseWriter, r *http.Request) {
	id, ok := a.machineOr404(w, r)
	if !ok {
		return
	}
	pane := r.PathValue("pane")
	wantFrames := r.URL.Query().Get("frames") == "1"

	var frames <-chan hub.Event
	if wantFrames {
		v, err := a.Runtime.Watch(id, pane)
		if err != nil {
			failure(w, err)
			return
		}
		frames = v.C
		defer v.Close()
	}
	feed := a.Runtime.Feed(id, pane)
	fw := feed.Watch()
	defer fw.Close()
	tw := a.Runtime.WatchTree(id)
	defer tw.Close()

	if feed.State().Rev == 0 && a.Runtime.Online(id) { // a first paint with the chat, not an empty one
		ctx, cancel := context.WithTimeout(r.Context(), 8*time.Second)
		feed.PollOnce(ctx)
		cancel()
	}
	s := newSSE(w)
	_ = s.event("status", map[string]bool{"online": a.Runtime.Online(id)})
	repaint := func() {
		ctx, cancel := context.WithTimeout(r.Context(), 8*time.Second)
		defer cancel()
		if screen, err := a.Runtime.ReadScreen(ctx, id, pane); err == nil {
			_ = s.event("frame", map[string]any{"data": encodeB64(screen), "reset": true})
		}
	}
	if wantFrames {
		repaint()
	}
	sent := sentChat{}
	sendChat := func() bool { return sent.send(s, feed.State()) }
	if !sendChat() {
		return
	}
	keep := time.NewTicker(15 * time.Second)
	defer keep.Stop()
	for {
		select {
		case <-r.Context().Done():
			return
		case <-fw.C:
			if !sendChat() {
				return
			}
		case ev := <-tw.C:
			if ev.Online != nil {
				_ = s.event("status", map[string]bool{"online": *ev.Online})
			}
			if ev.Tree != nil {
				_ = s.event("tree", treeEvent{ev.Tree.Rev, ev.Tree.Snapshot})
			}
		case ev := <-frames:
			var err error
			switch ev.Type {
			case "frame":
				err = s.event("frame", map[string]any{"data": ev.Data})
			case "size":
				err = s.event("size", map[string]int{"cols": ev.Cols, "rows": ev.Rows})
			case "closed":
				err = s.event("closed", map[string]string{"reason": ev.Data})
			case "resync":
				repaint()
			}
			if err != nil {
				return
			}
		case <-keep.C:
			if s.ping() != nil {
				return
			}
		}
	}
}

// sentChat remembers what a stream already holds: jsonl items are immutable, so only new ones are sent.
type sentChat struct {
	rev    int64
	n      int
	source string
	any    bool
}

func (c *sentChat) send(s *sse, st hub.ChatState) bool {
	if c.any && st.Rev == c.rev {
		return true
	}
	ev := chatEvent{Rows: st.Rows, Replace: true, Rev: st.Rev, Agent: st.Agent, Status: st.Status, Source: st.Source, Error: st.Error, Blocked: st.Blocked}
	if c.any && st.Source == "jsonl" && c.source == "jsonl" && len(st.Rows) >= c.n {
		ev.Rows, ev.Replace = st.Rows[c.n:], false
	}
	c.any, c.rev, c.source, c.n = true, st.Rev, st.Source, len(st.Rows)
	return s.event("chat", ev) == nil
}
