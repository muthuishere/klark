package hub

import (
	"context"
	"encoding/json"
	"strings"
	"sync"
	"time"

	"github.com/muthuishere/klark/hub-go/internal/chat"
)

const callTimeout = 8 * time.Second

// ChatState is what the chat view draws for one pane.
type ChatState struct {
	Rows         []chat.Item   `json:"rows"`
	Rev          int64         `json:"rev"`
	Agent        string        `json:"agent"`
	Status       string        `json:"status"`
	Source       string        `json:"source"` // jsonl | screen | ""
	Reader       string        `json:"reader"`
	Error        string        `json:"error,omitempty"`
	Blocked      *chat.Blocked `json:"blocked,omitempty"`
	LastActivity string        `json:"lastActivity,omitempty"`
}

// Feed is the chat data for one pane. It is kept in memory after the last viewer leaves so coming back paints at
// once; one ~1 Hz poll per pane runs only while someone watches (lazy rule).
type Feed struct {
	rt           *Runtime
	machine, id  string
	mu           sync.Mutex
	items        []chat.Item
	version      int64
	source       string
	agent        string
	paneAgent    string
	status       string
	cursor       string
	reader       string
	errMsg       string
	screenText   string
	haveScreen   bool
	blocked      *chat.Blocked
	noStructured bool
	lastActivity time.Time
	local        []chat.Item
	firstSeen    map[string]time.Time
	cmdSeq       int
	watchers     map[*FeedWatcher]struct{}
	polling      bool
}

// FeedWatcher is told (on C) whenever the feed's version changes.
type FeedWatcher struct {
	C    chan struct{}
	feed *Feed
	once sync.Once
}

func (rt *Runtime) Feed(machineID, paneID string) *Feed {
	rt.mu.Lock()
	defer rt.mu.Unlock()
	k := machineID + "\x00" + paneID
	if rt.feeds == nil {
		rt.feeds = map[string]*Feed{}
	}
	f := rt.feeds[k]
	if f == nil {
		f = &Feed{rt: rt, machine: machineID, id: paneID, firstSeen: map[string]time.Time{}, watchers: map[*FeedWatcher]struct{}{}}
		rt.feeds[k] = f
	}
	return f
}

// State is a snapshot of the feed.
func (f *Feed) State() ChatState {
	f.mu.Lock()
	defer f.mu.Unlock()
	st := ChatState{Rows: append([]chat.Item{}, f.items...), Rev: f.version, Agent: firstNonEmpty(f.agent, f.paneAgent),
		Status: f.status, Source: f.source, Reader: f.reader, Error: f.errMsg, Blocked: f.blocked}
	if f.blocked != nil && len(f.blocked.Choices) > 0 {
		st.Status = "blocked"
	}
	if !f.lastActivity.IsZero() {
		st.LastActivity = f.lastActivity.UTC().Format(time.RFC3339)
	}
	return st
}

// Watch starts (or joins) the pane's poll. Close to leave; the last one out stops it.
func (f *Feed) Watch() *FeedWatcher {
	w := &FeedWatcher{C: make(chan struct{}, 1), feed: f}
	f.mu.Lock()
	f.watchers[w] = struct{}{}
	start := !f.polling
	f.polling = true
	f.mu.Unlock()
	if start {
		go f.pollLoop()
	}
	return w
}

func (w *FeedWatcher) Close() {
	w.once.Do(func() {
		w.feed.mu.Lock()
		delete(w.feed.watchers, w)
		w.feed.mu.Unlock()
	})
}

func (f *Feed) notify() {
	f.mu.Lock()
	ws := make([]*FeedWatcher, 0, len(f.watchers))
	for w := range f.watchers {
		ws = append(ws, w)
	}
	f.mu.Unlock()
	for _, w := range ws {
		select {
		case w.C <- struct{}{}:
		default:
		}
	}
}

func (f *Feed) pollLoop() {
	tick := time.NewTicker(f.rt.FeedPoll)
	defer tick.Stop()
	for {
		f.mu.Lock()
		if len(f.watchers) == 0 {
			f.polling = false
			f.mu.Unlock()
			return
		}
		before := f.version
		f.mu.Unlock()
		ctx, cancel := context.WithTimeout(context.Background(), 20*time.Second)
		f.PollOnce(ctx)
		cancel()
		f.mu.Lock()
		changed := f.version != before
		f.mu.Unlock()
		if changed {
			f.notify()
		}
		<-tick.C
	}
}

func (f *Feed) setError(msg string) {
	if f.errMsg == msg {
		return
	}
	f.errMsg = msg
	f.version++
}

// PollOnce reads the pane's chat once: the structured transcript when the machine has one (protocol >= 3), else
// the screen. A blocked agent's question always comes from its screen, where the options are drawn.
func (f *Feed) PollOnce(ctx context.Context) {
	rt := f.rt
	info, ok := rt.Info(f.machine)
	if !ok || !info.Online {
		f.mu.Lock()
		f.setError("machine offline")
		f.mu.Unlock()
		return
	}
	_, _ = rt.Tree(ctx, f.machine) // refreshes the pane's agent and status
	if p, found := rt.Pane(f.machine, f.id); found {
		f.mu.Lock()
		f.paneAgent = p.Agent
		if p.AgentStatus != f.status {
			f.status = p.AgentStatus
			f.version++
		}
		f.mu.Unlock()
	}
	f.mu.Lock()
	noStructured, cursor := f.noStructured, f.cursor
	f.mu.Unlock()

	structured := false
	if info.Protocol >= 3 && !noStructured {
		msg := map[string]any{"type": "get_transcript", "pane_id": f.id}
		if cursor != "" {
			msg["after"] = cursor
		}
		raw, err := rt.Request(ctx, f.machine, msg, callTimeout)
		switch {
		case err == nil:
			f.apply(raw)
			structured = true
		case isUnsupported(err):
			f.mu.Lock()
			f.noStructured = true
			f.mu.Unlock()
		default:
			f.mu.Lock()
			f.setError(err.Error())
			f.mu.Unlock()
		}
	}
	f.mu.Lock()
	needScreen := !structured && (info.Protocol < 3 || f.noStructured)
	f.mu.Unlock()
	if needScreen {
		f.readScreenText(ctx)
	}

	// A choice menu at the bottom of the pane means the agent waits for an answer, whatever status the machine
	// reports (agy, for one, reads as idle while it asks). The screen already read serves; else the visible rows.
	var blocked *chat.Blocked
	f.mu.Lock()
	isBlocked := f.status == "blocked"
	text, have := f.screenText, f.source == "screen" && f.haveScreen
	f.mu.Unlock()
	if !have {
		if t, err := rt.readText(ctx, f.machine, "pane.read", map[string]any{"pane_id": f.id, "source": "visible", "format": "text", "strip_ansi": true}); err == nil {
			text, have = t, true
		}
	}
	if have {
		if blocked = chat.ChoicePrompt(text); blocked == nil && isBlocked {
			if c := chat.Clean(text); c != "" {
				b := chat.BlockedPrompt(c)
				blocked = &b
			}
		}
	}
	f.mu.Lock()
	if !sameBlocked(blocked, f.blocked) {
		f.blocked = blocked
		f.version++
	}
	f.mu.Unlock()
}

func isUnsupported(err error) bool {
	re, ok := err.(*RemoteError)
	if !ok {
		return false
	}
	m := strings.ToLower(re.Msg)
	return strings.Contains(m, "unknown") || strings.Contains(m, "unsupported")
}

func sameBlocked(a, b *chat.Blocked) bool {
	if a == nil || b == nil {
		return a == b
	}
	if a.Question != b.Question || a.Selected != b.Selected || len(a.Options) != len(b.Options) || len(a.Choices) != len(b.Choices) {
		return false
	}
	for i := range a.Options {
		if a.Options[i] != b.Options[i] {
			return false
		}
	}
	return true
}

// readText is a pane.read / agent.read answer's text.
func (rt *Runtime) readText(ctx context.Context, machineID, method string, params map[string]any) (string, error) {
	raw, err := rt.Call(ctx, machineID, method, params, callTimeout)
	if err != nil {
		return "", err
	}
	var r struct{ Read struct{ Text string } }
	_ = json.Unmarshal(raw, &r)
	return r.Read.Text, nil
}

func (f *Feed) readScreenText(ctx context.Context) {
	text, err := f.rt.readText(ctx, f.machine, "agent.read", map[string]any{"target": f.id, "source": "recent", "format": "text", "strip_ansi": true, "lines": 400})
	if err != nil || text == "" {
		text, err = f.rt.readText(ctx, f.machine, "pane.read", map[string]any{"pane_id": f.id, "source": "recent_unwrapped", "format": "text", "strip_ansi": true, "lines": 400})
		if err != nil {
			f.mu.Lock()
			f.setError(err.Error())
			f.mu.Unlock()
			return
		}
	}
	f.setScreen(text)
}

func (f *Feed) real() []chat.Item {
	local := map[string]bool{}
	for _, l := range f.local {
		local[l.ID] = true
	}
	var out []chat.Item
	for _, it := range f.items {
		if !local[it.ID] {
			out = append(out, it)
		}
	}
	return out
}

func (f *Feed) merge(real []chat.Item) []chat.Item {
	return chat.MergeLocal(real, f.local, f.firstSeen)
}

func linkItems(items []chat.Item, machine, pane string) {
	for i := range items {
		it := &items[i]
		if it.HTML != "" {
			it.HTML = chat.Linkify(it.HTML, machine, pane)
		}
		for j := range it.Segments {
			if it.Segments[j].HTML != "" {
				it.Segments[j].HTML = chat.Linkify(it.Segments[j].HTML, machine, pane)
			}
		}
	}
}

func idsEqual(a, b []chat.Item) bool {
	if len(a) != len(b) {
		return false
	}
	for i := range a {
		if a[i].ID != b[i].ID {
			return false
		}
	}
	return true
}

func (f *Feed) setScreen(raw string) {
	f.mu.Lock()
	defer f.mu.Unlock()
	f.setError("")
	if f.source == "screen" && f.haveScreen && f.screenText == raw {
		return
	}
	agent := firstNonEmpty(f.agent, f.paneAgent)
	f.source, f.reader, f.screenText, f.haveScreen, f.cursor = "screen", chat.PickReader(agent, raw), raw, true, ""
	items := chat.ScreenItems(chat.Transform(agent, raw))
	linkItems(items, f.machine, f.id)
	if len(items) == 0 || !idsEqual(items, f.real()) {
		f.lastActivity = time.Now()
	}
	f.items = f.merge(items)
	f.version++
}

// apply merges a get_transcript result: new items are appended after the cursor; a screen result replaces all.
func (f *Feed) apply(raw json.RawMessage) {
	var res struct {
		Source string           `json:"source"`
		Agent  string           `json:"agent"`
		Cursor json.RawMessage  `json:"cursor"`
		Items  []map[string]any `json:"items"`
	}
	_ = json.Unmarshal(raw, &res)
	cursor := ""
	if len(res.Cursor) > 0 && string(res.Cursor) != "null" {
		_ = json.Unmarshal(res.Cursor, &cursor)
		if cursor == "" {
			cursor = strings.Trim(string(res.Cursor), `"`)
		}
	}
	source := res.Source
	if source == "" {
		source = "jsonl"
	}
	f.mu.Lock()
	f.setError("")
	if res.Agent != f.agent {
		f.agent = res.Agent
		f.version++
	}
	f.mu.Unlock()
	if source == "screen" {
		var parts []string
		for _, it := range res.Items {
			if t := str(it["text"]); t != "" {
				parts = append(parts, t)
			}
		}
		f.setScreen(strings.Join(parts, "\n\n"))
		f.mu.Lock()
		f.cursor = cursor
		f.mu.Unlock()
		return
	}
	f.mu.Lock()
	defer f.mu.Unlock()
	replace := f.source != "jsonl"
	if !replace && len(res.Items) == 0 && cursor == f.cursor {
		return
	}
	var list []chat.Item
	if !replace {
		list = f.real()
	}
	seen := map[string]bool{}
	for _, it := range list {
		seen[it.ID] = true
	}
	n := len(list)
	oldCount := len(list)
	for _, o := range res.Items {
		it := chat.ToItem(o, n)
		n++
		if it == nil || seen[it.ID] {
			continue
		}
		seen[it.ID] = true
		one := []chat.Item{*it}
		linkItems(one, f.machine, f.id)
		list = append(list, one[0])
	}
	if len(list) > chat.MaxItems {
		list = list[len(list)-chat.MaxItems:]
	}
	f.source, f.reader, f.cursor = "jsonl", source, cursor
	if replace || len(list) != oldCount {
		f.lastActivity = time.Now()
		if len(list) > 0 {
			if t, err := time.Parse(time.RFC3339, list[len(list)-1].Time); err == nil {
				f.lastActivity = t
			}
		}
		f.items = f.merge(list)
		f.version++
	}
}

// SendLocalCommand sends a slash command to a pane whose agent never logs local commands (Devin, Codex, ...),
// and chats what it printed: the command at once, then its output read off the screen once that stops changing.
func (f *Feed) SendLocalCommand(ctx context.Context, command string) error {
	rt := f.rt
	before, _ := rt.readText(ctx, f.machine, "pane.read", map[string]any{"pane_id": f.id, "source": "visible", "format": "text", "strip_ansi": true})
	ts := time.Now()
	f.mu.Lock()
	f.cmdSeq++
	seq := f.cmdSeq
	f.mu.Unlock()
	if err := rt.Input(f.machine, f.id, command); err != nil {
		return err
	}
	if err := rt.Keys(f.machine, f.id, []string{"enter"}); err != nil {
		return err
	}
	f.addCommand(seq, command, "", ts)
	go func() {
		c, cancel := context.WithTimeout(context.Background(), 20*time.Second)
		defer cancel()
		var last, after string
		stableSince := time.Now()
		deadline := time.Now().Add(6 * time.Second)
		for time.Now().Before(deadline) {
			time.Sleep(250 * time.Millisecond)
			now, err := rt.readText(c, f.machine, "pane.read", map[string]any{"pane_id": f.id, "source": "visible", "format": "text", "strip_ansi": true})
			if err != nil {
				break
			}
			if now != last {
				last, stableSince = now, time.Now()
				continue
			}
			after = now
			if time.Since(stableSince) >= 700*time.Millisecond && chat.ExtractCommandOutput(before, now, command) != "" {
				break
			}
		}
		if after == "" {
			after = last
		}
		f.addCommand(seq, command, chat.ExtractCommandOutput(before, after, command), ts)
	}()
	return nil
}

func (f *Feed) addCommand(seq int, command, output string, ts time.Time) {
	f.mu.Lock()
	have := map[string]bool{}
	for _, l := range f.local {
		have[l.ID] = true
	}
	items := chat.CommandItems(seq, command, output, ts)
	linkItems(items, f.machine, f.id)
	for _, it := range items {
		if !have[it.ID] {
			f.local = append(f.local, it)
		}
	}
	for len(f.local) > 40 {
		f.local = f.local[1:]
	}
	f.items = f.merge(f.real())
	f.version++
	f.mu.Unlock()
	f.notify()
}
