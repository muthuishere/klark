// Package feed keeps one agent's chat up to date from a Source (its structured transcript, else its screen) and
// says what a stream must send next: rows upserted by id, or the whole chat with replace=true.
//
// It does no I/O of its own: the Source does, and the caller decides how events travel (SSE, websocket, ...).
package feed

import (
	"context"
	"errors"
	"sync"
	"time"

	"github.com/muthuishere/klark/hub-go/chat"
)

// ErrNoTranscript is what a Source's Transcript returns (or wraps) when the agent has no structured transcript;
// the feed then reads the screen from then on.
var ErrNoTranscript = errors.New("feed: no structured transcript")

// Source is where one agent's chat comes from.
type Source interface {
	// Transcript returns the rows after cursor ("" = from the start) and the cursor to pass next time. Rows are
	// immutable once returned (convert machine items with chat.ToItem).
	Transcript(ctx context.Context, cursor string) (items []chat.Item, next string, err error)
	// Screen returns the agent's recent screen as plain text (ANSI allowed).
	Screen(ctx context.Context) (raw string, err error)
}

// MaxItems caps the rows a feed keeps.
const MaxItems = 2000

// ChatState is what a chat view draws: the JSON of the `chat` event without replace.
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

// Options configure a Feed.
type Options struct {
	Agent string        // the agent's name (claude, codex, devin, agy, ...); picks the screen reader
	Poll  time.Duration // poll interval while watched; default 1s
	Href  chat.HrefFunc // links file paths in rows; nil leaves them as text
}

// Feed is one agent's chat. Safe for concurrent use.
type Feed struct {
	src  Source
	opt  Options
	mu   sync.Mutex
	st   ChatState
	cur  string
	noTx bool
	scr  string

	watchers int
	subs     map[chan struct{}]struct{}
}

// New makes a feed; nothing is read until Poll or Watch.
func New(src Source, opt Options) *Feed {
	if opt.Poll <= 0 {
		opt.Poll = time.Second
	}
	return &Feed{src: src, opt: opt, st: ChatState{Agent: opt.Agent}, subs: map[chan struct{}]struct{}{}}
}

// State is a snapshot (rows copied).
func (f *Feed) State() ChatState {
	f.mu.Lock()
	defer f.mu.Unlock()
	st := f.st
	st.Rows = append([]chat.Item{}, f.st.Rows...)
	return st
}

func (f *Feed) link(items []chat.Item) {
	if f.opt.Href == nil {
		return
	}
	for i := range items {
		items[i].HTML = chat.Linkify(items[i].HTML, f.opt.Href)
		for j := range items[i].Segments {
			items[i].Segments[j].HTML = chat.Linkify(items[i].Segments[j].HTML, f.opt.Href)
		}
	}
}

func (f *Feed) setErr(msg string) {
	if f.st.Error != msg {
		f.st.Error = msg
		f.st.Rev++
	}
}

// Poll reads the source once and reports whether the state changed. The screen also gives the blocked prompt.
func (f *Feed) Poll(ctx context.Context) (changed bool, err error) {
	f.mu.Lock()
	before, noTx, cur := f.st.Rev, f.noTx, f.cur
	f.mu.Unlock()
	defer func() {
		f.mu.Lock()
		changed = f.st.Rev != before
		f.mu.Unlock()
	}()

	if !noTx {
		items, next, terr := f.src.Transcript(ctx, cur)
		switch {
		case errors.Is(terr, ErrNoTranscript):
			f.mu.Lock()
			f.noTx = true
			f.mu.Unlock()
		case terr != nil:
			f.mu.Lock()
			f.setErr(terr.Error())
			f.mu.Unlock()
			return false, terr
		default:
			f.applyTranscript(items, next)
		}
	}
	raw, serr := f.src.Screen(ctx)
	f.mu.Lock()
	defer f.mu.Unlock()
	if serr != nil {
		if f.noTx {
			f.setErr(serr.Error())
		}
		return false, serr
	}
	if f.noTx {
		f.applyScreen(raw)
	}
	b := chat.ChoicePrompt(raw)
	if !sameBlocked(b, f.st.Blocked) {
		f.st.Blocked = b
		f.st.Rev++
	}
	return false, nil
}

func (f *Feed) applyTranscript(items []chat.Item, next string) {
	f.mu.Lock()
	defer f.mu.Unlock()
	f.setErr("")
	replace := f.st.Source != "jsonl"
	if !replace && len(items) == 0 && next == f.cur {
		return
	}
	var list []chat.Item
	if !replace {
		list = f.st.Rows
	}
	seen := map[string]bool{}
	for _, it := range list {
		seen[it.ID] = true
	}
	old := len(list)
	fresh := []chat.Item{}
	for _, it := range items {
		if !seen[it.ID] {
			seen[it.ID] = true
			fresh = append(fresh, it)
		}
	}
	f.link(fresh)
	list = append(append([]chat.Item{}, list...), fresh...)
	if len(list) > MaxItems {
		list = list[len(list)-MaxItems:]
	}
	f.st.Source, f.st.Reader, f.cur = "jsonl", "jsonl", next
	if replace || len(list) != old {
		f.st.Rows = list
		f.st.LastActivity = time.Now().UTC().Format(time.RFC3339)
		f.st.Rev++
	}
}

func (f *Feed) applyScreen(raw string) {
	f.setErr("")
	if f.st.Source == "screen" && f.scr == raw {
		return
	}
	f.scr = raw
	items := chat.ScreenItems(chat.Transform(f.opt.Agent, raw))
	f.link(items)
	f.st.Source, f.st.Reader, f.st.Rows = "screen", chat.PickReader(f.opt.Agent, raw), items
	f.st.LastActivity = time.Now().UTC().Format(time.RFC3339)
	f.st.Rev++
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
	for i := range a.Choices {
		if a.Choices[i] != b.Choices[i] {
			return false
		}
	}
	return true
}

// Watch returns a channel told (coalesced) whenever the state changes. The feed polls only while at least one
// watcher's ctx is live; the channel closes when ctx ends.
func (f *Feed) Watch(ctx context.Context) <-chan struct{} {
	c := make(chan struct{}, 1)
	f.mu.Lock()
	f.subs[c] = struct{}{}
	f.watchers++
	start := f.watchers == 1
	f.mu.Unlock()
	if start {
		go f.loop()
	}
	go func() {
		<-ctx.Done()
		f.mu.Lock()
		delete(f.subs, c)
		f.watchers--
		f.mu.Unlock()
		close(c)
	}()
	return c
}

func (f *Feed) loop() {
	t := time.NewTicker(f.opt.Poll)
	defer t.Stop()
	for {
		f.mu.Lock()
		if f.watchers == 0 {
			f.mu.Unlock()
			return
		}
		f.mu.Unlock()
		ctx, cancel := context.WithTimeout(context.Background(), 20*time.Second)
		changed, _ := f.Poll(ctx)
		cancel()
		if changed {
			f.mu.Lock()
			for c := range f.subs {
				select {
				case c <- struct{}{}:
				default:
				}
			}
			f.mu.Unlock()
		}
		<-t.C
	}
}

// Event is the `chat` event a stream sends.
type Event struct {
	Rows    []chat.Item   `json:"rows"`
	Replace bool          `json:"replace,omitempty"`
	Rev     int64         `json:"rev"`
	Agent   string        `json:"agent"`
	Status  string        `json:"status"`
	Source  string        `json:"source"`
	Error   string        `json:"error,omitempty"`
	Blocked *chat.Blocked `json:"blocked,omitempty"`
}

// Sent remembers what one stream already holds. jsonl rows are immutable, so after the first event only new rows
// go out (upserted by id); anything else is sent whole with Replace. The zero value is ready; one per stream.
type Sent struct {
	rev    int64
	n      int
	source string
	any    bool
}

// Next is the event to send for st, or ok=false when the stream already has this revision.
func (c *Sent) Next(st ChatState) (ev Event, ok bool) {
	if c.any && st.Rev == c.rev {
		return Event{}, false
	}
	ev = Event{Rows: st.Rows, Replace: true, Rev: st.Rev, Agent: st.Agent, Status: st.Status, Source: st.Source, Error: st.Error, Blocked: st.Blocked}
	if c.any && st.Source == "jsonl" && c.source == "jsonl" && len(st.Rows) >= c.n {
		ev.Rows, ev.Replace = st.Rows[c.n:], false
	}
	c.any, c.rev, c.source, c.n = true, st.Rev, st.Source, len(st.Rows)
	return ev, true
}
