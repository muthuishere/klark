package hub

import (
	"context"
	"encoding/json"
	"time"
)

// Tree is a machine's session tree (the `session.snapshot` of its `tree` message) and its revision.
type Tree struct {
	Rev      string          `json:"rev"`
	Snapshot json.RawMessage `json:"snapshot"`
}

func (m *machine) gotTree(rev string, snap json.RawMessage) {
	m.mu.Lock()
	if snap != nil { // tree_unchanged keeps what is held
		m.treeRev, m.treeSnap = rev, snap
	}
	m.treeChecked = time.Now()
	waiters := m.treeWait
	m.treeWait = nil
	res := treeResult{rev: m.treeRev, snap: m.treeSnap}
	m.mu.Unlock()
	for _, w := range waiters {
		w <- res
	}
}

// Tree returns the machine's tree. A recent answer is reused; otherwise the machine is asked with the revision
// the hub holds (`known_rev`) and answers `tree_unchanged` when nothing moved, so an idle tree costs nothing.
// Concurrent callers share one request.
func (rt *Runtime) Tree(ctx context.Context, machineID string) (Tree, error) {
	return rt.get(machineID).tree(ctx, rt.TreeFresh)
}

func (m *machine) tree(ctx context.Context, fresh time.Duration) (Tree, error) {
	c, err := m.live()
	if err != nil {
		return Tree{}, err
	}
	m.mu.Lock()
	if m.treeSnap != nil && time.Since(m.treeChecked) < fresh {
		t := Tree{Rev: m.treeRev, Snapshot: m.treeSnap}
		m.mu.Unlock()
		return t, nil
	}
	ch := make(chan treeResult, 1)
	first := len(m.treeWait) == 0
	m.treeWait = append(m.treeWait, ch)
	known := m.treeRev
	m.mu.Unlock()
	if first {
		msg := map[string]any{"type": "get_tree"}
		if known != "" {
			msg["known_rev"] = known
		}
		c.Send(msg)
	}
	t := time.NewTimer(5 * time.Second)
	defer t.Stop()
	select {
	case r := <-ch:
		return Tree{Rev: r.rev, Snapshot: r.snap}, r.err
	case <-t.C:
		m.dropWaiter(ch)
		return Tree{}, ErrTimeout
	case <-ctx.Done():
		m.dropWaiter(ch)
		return Tree{}, ctx.Err()
	}
}

func (m *machine) dropWaiter(ch chan treeResult) {
	m.mu.Lock()
	defer m.mu.Unlock()
	for i, w := range m.treeWait {
		if w == ch {
			m.treeWait = append(m.treeWait[:i], m.treeWait[i+1:]...)
			return
		}
	}
}

// TreeWatcher receives a Tree whenever the machine's revision changes, and an Online flag change.
type TreeWatcher struct {
	C      chan TreeEvent
	cancel func()
}

// TreeEvent is one change: a new tree, or the machine going on/offline.
type TreeEvent struct {
	Tree   *Tree
	Online *bool
}

func (w *TreeWatcher) Close() { w.cancel() }

// WatchTree polls the machine's tree (every Runtime.TreePoll) while at least one watcher exists,
// and not otherwise: nobody looking, nothing asked.
func (rt *Runtime) WatchTree(machineID string) *TreeWatcher {
	m := rt.get(machineID)
	w := &TreeWatcher{C: make(chan TreeEvent, 8)}
	m.mu.Lock()
	m.treeWatch[w] = struct{}{}
	start := !m.treeLoop
	m.treeLoop = true
	m.mu.Unlock()
	w.cancel = func() {
		m.mu.Lock()
		delete(m.treeWatch, w)
		m.mu.Unlock()
	}
	if start {
		go m.treeLoopRun()
	}
	return w
}

func (m *machine) treeLoopRun() {
	tick := time.NewTicker(m.rt.TreePoll)
	defer tick.Stop()
	var lastRev string
	var lastOnline *bool
	for {
		m.mu.Lock()
		if len(m.treeWatch) == 0 {
			m.treeLoop = false
			m.mu.Unlock()
			return
		}
		m.mu.Unlock()
		online := m.rt.Online(m.id)
		var ev TreeEvent
		changed := false
		if lastOnline == nil || *lastOnline != online {
			o := online
			lastOnline, ev.Online, changed = &o, &o, true
		}
		if online {
			ctx, cancel := context.WithTimeout(context.Background(), 6*time.Second)
			t, err := m.tree(ctx, 0)
			cancel()
			if err == nil && t.Rev != lastRev {
				lastRev = t.Rev
				ev.Tree, changed = &t, true
			}
		}
		if changed {
			m.mu.Lock()
			for w := range m.treeWatch {
				select {
				case w.C <- ev:
				default: // a slow watcher misses an intermediate tree; the next change carries the latest
				}
			}
			m.mu.Unlock()
		}
		<-tick.C
	}
}
