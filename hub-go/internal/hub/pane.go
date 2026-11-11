package hub

import "sync"

// Event is something the SSE endpoint forwards: frame (base64 bytes), closed (reason), size ("cols,rows" is
// carried in Cols/Rows), resync (this viewer fell behind: repaint from the screen).
type Event struct {
	Type       string
	Data       string
	Cols, Rows int
}

type paneState struct {
	mu         sync.Mutex
	machine    *machine
	id         string
	viewers    map[*Viewer]struct{}
	cols, rows int
}

func (m *machine) pane(id string) *paneState {
	m.mu.Lock()
	defer m.mu.Unlock()
	p := m.panes[id]
	if p == nil {
		p = &paneState{machine: m, id: id, viewers: map[*Viewer]struct{}{}}
		m.panes[id] = p
	}
	return p
}

// Viewer is one open view of a pane. The first viewer of a pane makes the machine start streaming it; closing the
// last one stops the stream (the lazy rule).
type Viewer struct {
	C      chan Event
	pane   *paneState
	once   sync.Once
	dropMu sync.Mutex
}

const viewerQueue = 64 // frames come in ~50 ms batches: a few seconds of output

// Watch starts viewing a pane. It fails when the machine is offline.
func (rt *Runtime) Watch(machineID, paneID string) (*Viewer, error) {
	m := rt.get(machineID)
	c, err := m.live()
	if err != nil {
		return nil, err
	}
	p := m.pane(paneID)
	v := &Viewer{C: make(chan Event, viewerQueue), pane: p}
	p.mu.Lock()
	first := len(p.viewers) == 0
	p.viewers[v] = struct{}{}
	cols, rows := p.cols, p.rows
	p.mu.Unlock()
	if first {
		c.Send(map[string]any{"type": "subscribe", "pane_id": paneID})
	}
	if cols > 0 {
		v.C <- Event{Type: "size", Cols: cols, Rows: rows}
	}
	return v, nil
}

// Viewers is how many views a pane has (for tests and the lazy rule).
func (rt *Runtime) Viewers(machineID, paneID string) int {
	p := rt.get(machineID).pane(paneID)
	p.mu.Lock()
	defer p.mu.Unlock()
	return len(p.viewers)
}

// Close leaves the pane; the last one out unsubscribes it.
func (v *Viewer) Close() {
	v.once.Do(func() {
		p := v.pane
		p.mu.Lock()
		delete(p.viewers, v)
		last := len(p.viewers) == 0
		if last {
			p.cols, p.rows = 0, 0
		}
		p.mu.Unlock()
		if last {
			if c, err := p.machine.live(); err == nil {
				c.Send(map[string]any{"type": "unsubscribe", "pane_id": p.id})
			}
		}
	})
}

func (p *paneState) setSize(cols, rows int) {
	p.mu.Lock()
	p.cols, p.rows = cols, rows
	p.mu.Unlock()
	p.broadcast(Event{Type: "size", Cols: cols, Rows: rows})
}

func (p *paneState) broadcast(ev Event) {
	p.mu.Lock()
	vs := make([]*Viewer, 0, len(p.viewers))
	for v := range p.viewers {
		vs = append(vs, v)
	}
	p.mu.Unlock()
	for _, v := range vs {
		v.push(ev)
	}
}

// push never blocks the machine link: a viewer that cannot keep up loses its queued output and gets one
// "resync" event, after which the SSE handler repaints it from the current screen.
func (v *Viewer) push(ev Event) {
	v.dropMu.Lock()
	defer v.dropMu.Unlock()
	select {
	case v.C <- ev:
		return
	default:
	}
	for {
		select {
		case <-v.C:
		default:
			v.C <- Event{Type: "resync"}
			return
		}
	}
}
