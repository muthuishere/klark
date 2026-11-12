package chat

import (
	"encoding/json"
	"fmt"
	"hash/fnv"
	"strings"
	"time"
	"unicode/utf8"
)

const maxToolText = 4000

// Item is one chat row.
type Item struct {
	ID       string    `json:"id"`
	Role     string    `json:"role"` // user | assistant | tool
	Kind     string    `json:"kind"` // text | tool_use | tool_result | thinking | command | output
	Text     string    `json:"text,omitempty"`
	HTML     string    `json:"html,omitempty"` // markdown (assistant) or ANSI output, already safe HTML
	Segments []Segment `json:"-"`
	Lines    int       `json:"lines,omitempty"` // line count of command output
	Tool     string    `json:"tool,omitempty"`
	Input    string    `json:"input,omitempty"`
	Summary  string    `json:"summary,omitempty"`
	OK       *bool     `json:"ok,omitempty"`
	Time     string    `json:"time,omitempty"` // RFC 3339
	ToolUse  string    `json:"toolUseId,omitempty"`

	ts time.Time
}

// Cap shortens a tool's text.
func capText(s string) string {
	if utf8.RuneCountInString(s) <= maxToolText {
		return s
	}
	r := []rune(s)
	return string(r[:maxToolText]) + fmt.Sprintf("\n… (%d more characters)", len(r)-maxToolText)
}

var summaryKeys = []string{"command", "file_path", "path", "pattern", "url", "query", "description", "prompt", "notebook_path"}

// summarize is the one argument that says what a tool call did ("cargo test", "src/x.rs").
func summarize(input any) string {
	arg := ""
	switch v := input.(type) {
	case map[string]any:
		for _, k := range summaryKeys {
			if s, _ := v[k].(string); s != "" {
				arg = s
				break
			}
		}
	case string:
		arg = v
	}
	if arg == "" {
		return ""
	}
	arg = strings.TrimSpace(strings.NewReplacer("\r\n", " ", "\n", " ", "\r", " ").Replace(arg))
	if utf8.RuneCountInString(arg) > 70 {
		return string([]rune(arg)[:67]) + "…"
	}
	return arg
}

func boolOf(v any) bool { b, _ := v.(bool); return b }

func strOf(v any) string { s, _ := v.(string); return s }

func parseTS(v any) (time.Time, bool) {
	switch t := v.(type) {
	case string:
		for _, layout := range []string{time.RFC3339Nano, time.RFC3339} {
			if p, err := time.Parse(layout, t); err == nil {
				return p, true
			}
		}
	case float64:
		ms := int64(t)
		if ms > 10_000_000_000 {
			return time.UnixMilli(ms), true
		}
		return time.Unix(ms, 0), true
	}
	return time.Time{}, false
}

// noteTail is the klark hub note ("(klark hub: ...)") a person's message may end with; rows show the text without it.
var noteTail = rx(`\s*\(klark hub: [^)]*\)\s*$`)

func stripNote(text string) string { return noteTail.ReplaceAllString(text, "") }

var localOut = rx(`(?s)<local-command-(?:stdout|stderr)>(.*?)</local-command-(?:stdout|stderr)>`)

// localCommandOutput is the text inside local-command-stdout/stderr tags (ANSI removed); ok is false when there
// are none.
func localCommandOutput(text string) (string, bool) {
	if !strings.Contains(text, "<local-command-") {
		return "", false
	}
	var parts []string
	for _, m := range localOut.FindAllStringSubmatch(text, -1) {
		if p := strings.TrimSpace(StripAnsi(m[1])); p != "" {
			parts = append(parts, p)
		}
	}
	return strings.Join(parts, "\n"), true
}

func fence(s string) string {
	if strings.Contains(s, "```") {
		return "````"
	}
	return "```"
}

// ToItem converts one structured transcript item (what the machine's get_transcript returns) into a chat row.
// nil means the item never reaches the chat: system/meta items, harness wrappers and empty items.
func ToItem(o map[string]any, index int) *Item {
	role, kind := strOf(o["role"]), strOf(o["kind"])
	if role == "" {
		role = "assistant"
	}
	if kind == "" {
		kind = "text"
	}
	if role != "user" && role != "assistant" && role != "tool" {
		return nil
	}
	if role == "tool" && kind != "tool_result" {
		return nil
	}
	if kind != "text" && kind != "tool_use" && kind != "tool_result" && kind != "thinking" {
		return nil
	}
	if boolOf(o["meta"]) || boolOf(o["is_meta"]) || boolOf(o["isMeta"]) {
		return nil
	}
	id := strOf(o["id"])
	if id == "" {
		id = fmt.Sprintf("i%d", index)
	}
	text, tool := strOf(o["text"]), strOf(o["tool"])
	var ok *bool
	if b, has := o["ok"].(bool); has {
		ok = &b
	}
	it := &Item{ID: id, Role: role, Kind: kind, Tool: tool, OK: ok, ToolUse: strOf(o["tool_use_id"])}
	if ts, has := parseTS(o["ts"]); has {
		it.ts, it.Time = ts, ts.UTC().Format(time.RFC3339)
	}
	inputText := ""
	switch v := o["input"].(type) {
	case nil:
	case string:
		inputText = v
	default:
		b, _ := json.MarshalIndent(v, "", "  ")
		inputText = string(b)
	}
	// A slash command's own output (/context, /status ...) is logged as a "user" item wrapped in
	// <local-command-stdout>: it answers the command above it, so it is the agent's, in a code block.
	if role == "user" && kind == "text" {
		if out, has := localCommandOutput(text); has {
			if out == "" {
				return nil
			}
			role, it.Role = "assistant", "assistant"
			f := fence(out)
			text = f + "text\n" + capText(out) + "\n" + f
		}
	}
	switch kind {
	case "text", "thinking":
		text = cleanStructured(text)
		if role == "user" {
			text = stripNote(text)
		}
		if text == "" {
			return nil
		}
	case "tool_result":
		text = cleanStructured(text)
	}
	if kind == "tool_use" || kind == "tool_result" {
		text, inputText = capText(text), capText(inputText)
	}
	it.Text, it.Input, it.Summary = text, inputText, summarize(o["input"])
	if kind == "text" && role == "assistant" && text != "" {
		it.setMarkdown(RenderMarkdown(text, false))
	}
	return it
}

func (it *Item) setMarkdown(segs []Segment) {
	it.Segments, it.HTML = segs, SegmentsHTML(segs)
}

func stableHash(s string) uint32 {
	h := fnv.New32a()
	_, _ = h.Write([]byte(s))
	return h.Sum32()
}

func errorShaped(s string) bool {
	l := strings.ToLower(s)
	return strings.HasPrefix(l, "error") || strings.Contains(s, "FAILED") || strings.Contains(s, "error[") || strings.Contains(s, "panicked at")
}

// ScreenItems turns screen turns into rows. Ids come from each turn's first line and its occurrence, so a row
// keeps its identity while the screen grows under it.
func ScreenItems(turns []ScreenTurn) []Item {
	var items []Item
	seen := map[string]int{}
	for _, t := range turns {
		plain := t.Text
		if t.Kind == "output" {
			plain = StripAnsi(t.Text)
		}
		if t.Role == "user" && t.Kind == "text" {
			plain = stripNote(plain)
		}
		head := plain
		if t.Kind == "tool_use" {
			head = t.Tool + "(" + t.Summary + ")"
		}
		head, _, _ = strings.Cut(head, "\n")
		base := fmt.Sprintf("s:%s:%08x", t.Kind, stableHash(head))
		n, had := seen[base]
		if had {
			n++
		}
		seen[base] = n
		it := Item{ID: fmt.Sprintf("%s:%d", base, n), Role: t.Role, Kind: t.Kind, Tool: t.Tool, Summary: t.Summary}
		if t.Kind != "tool_use" {
			it.Text = plain
		}
		switch {
		case t.Kind == "text" && t.Role == "assistant":
			it.setMarkdown(RenderMarkdown(t.Text, true))
		case t.Kind == "output":
			it.setMarkdown([]Segment{{Type: "ansi", HTML: AnsiHTML(t.Text), Lines: strings.Count(plain, "\n") + 1}})
			it.Lines = strings.Count(plain, "\n") + 1
		case t.Kind == "tool_result":
			ok := !errorShaped(plain) // a screen shows no exit status; an error-shaped result is the best evidence
			it.OK = &ok
		}
		items = append(items, it)
	}
	return items
}

// CommandItems are a local command and its output shown in the pane's chat: ids cmd-N and cmd-N-out.
func CommandItems(seq int, command, output string, ts time.Time) []Item {
	user := Item{ID: fmt.Sprintf("cmd-%d", seq), Role: "user", Kind: "text", Text: command, ts: ts, Time: ts.UTC().Format(time.RFC3339)}
	items := []Item{user}
	if output != "" {
		f := fence(output)
		text := f + "text\n" + capText(output) + "\n" + f
		out := Item{ID: user.ID + "-out", Role: "assistant", Kind: "text", Text: text, ts: ts.Add(time.Millisecond),
			Time: ts.Add(time.Millisecond).UTC().Format(time.RFC3339)}
		out.setMarkdown(RenderMarkdown(text, false))
		items = append(items, out)
	}
	return items
}

// MergeLocal places local command items among the real items by time: a command goes after every real item
// seen at or before it. A real item without a timestamp counts as seen when it first arrived (firstSeen).
func MergeLocal(real, local []Item, firstSeen map[string]time.Time) []Item {
	if len(local) == 0 {
		return real
	}
	now := time.Now()
	res := append([]Item{}, real...)
	sorted := append([]Item{}, local...)
	for i := 1; i < len(sorted); i++ { // insertion sort by time, stable
		for j := i; j > 0 && sorted[j].ts.Before(sorted[j-1].ts); j-- {
			sorted[j], sorted[j-1] = sorted[j-1], sorted[j]
		}
	}
	for _, e := range sorted {
		at := len(res)
		for i := range res {
			if strings.HasPrefix(res[i].ID, "cmd-") {
				continue
			}
			t := res[i].ts
			if t.IsZero() {
				f, ok := firstSeen[res[i].ID]
				if !ok {
					f = now
					firstSeen[res[i].ID] = f
				}
				t = f
			}
			if t.After(e.ts) {
				at = i
				break
			}
		}
		for at < len(res) && strings.HasPrefix(res[at].ID, "cmd-") && !res[at].ts.After(e.ts) {
			at++
		}
		res = append(res, Item{})
		copy(res[at+1:], res[at:])
		res[at] = e
	}
	return res
}
