package feed

import (
	"context"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/muthuishere/klark/hub-go/chat"
)

type fakeSrc struct {
	mu     sync.Mutex
	items  []chat.Item
	noTx   bool
	screen string
}

func (s *fakeSrc) Transcript(_ context.Context, cursor string) ([]chat.Item, string, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.noTx {
		return nil, "", ErrNoTranscript
	}
	n := 0
	if cursor != "" {
		n = len(cursor)
	}
	return append([]chat.Item{}, s.items[n:]...), string(make([]byte, len(s.items))), nil
}

func (s *fakeSrc) Screen(context.Context) (string, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.screen, nil
}

func TestTranscriptUpsertsAndSent(t *testing.T) {
	src := &fakeSrc{items: []chat.Item{{ID: "a", Role: "user", Kind: "text", Text: "hi"}}}
	f := New(src, Options{Agent: "claude", Href: chat.KlarkHref("m", "p")})
	ctx := context.Background()
	if ch, err := f.Poll(ctx); err != nil || !ch {
		t.Fatal(ch, err)
	}
	var sent Sent
	ev, ok := sent.Next(f.State())
	if !ok || !ev.Replace || len(ev.Rows) != 1 || ev.Source != "jsonl" {
		t.Fatalf("%+v", ev)
	}
	if _, ok := sent.Next(f.State()); ok {
		t.Fatal("same rev sent twice")
	}
	src.mu.Lock()
	src.items = append(src.items, chat.Item{ID: "b", Role: "assistant", Kind: "text", Text: "see src/x.rs", HTML: "<p>see src/x.rs</p>"})
	src.mu.Unlock()
	f.Poll(ctx)
	ev, ok = sent.Next(f.State())
	if !ok || ev.Replace || len(ev.Rows) != 1 || ev.Rows[0].ID != "b" {
		t.Fatalf("append: %+v", ev)
	}
	if want := `href="m/m/f?pane=p&amp;path=src%2Fx.rs"`; !strings.Contains(ev.Rows[0].HTML, want) {
		t.Fatalf("not linked: %s", ev.Rows[0].HTML)
	}
}

func TestScreenReplacesAndBlocked(t *testing.T) {
	src := &fakeSrc{noTx: true, screen: "> hello\n\n⏺ Hi.\n"}
	f := New(src, Options{Agent: "claude"})
	f.Poll(context.Background())
	var sent Sent
	ev, _ := sent.Next(f.State())
	if ev.Source != "screen" || !ev.Replace || len(ev.Rows) == 0 {
		t.Fatalf("%+v", ev)
	}
	src.mu.Lock()
	src.screen = "> hello\n\n⏺ Hi.\n\nDo you want to proceed?\n❯ 1. Yes\n  2. No\nEsc to cancel\n"
	src.mu.Unlock()
	f.Poll(context.Background())
	ev, ok := sent.Next(f.State())
	if !ok || !ev.Replace || ev.Blocked == nil || len(ev.Blocked.Choices) != 2 {
		t.Fatalf("%+v", ev)
	}
}

func TestWatchPollsWhileWatched(t *testing.T) {
	src := &fakeSrc{items: []chat.Item{{ID: "a", Role: "user", Kind: "text", Text: "hi"}}}
	f := New(src, Options{Poll: 5 * time.Millisecond})
	ctx, cancel := context.WithCancel(context.Background())
	c := f.Watch(ctx)
	select {
	case <-c:
	case <-time.After(time.Second):
		t.Fatal("no change notified")
	}
	go func() { _ = f.State() }()
	cancel()
	for range c {
	}
}
