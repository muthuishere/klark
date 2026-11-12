package chat

import (
	"os"
	"strings"
	"testing"
	"time"
)

func fixture(t *testing.T, name string) string {
	t.Helper()
	b, err := os.ReadFile("testdata/" + name)
	if err != nil {
		t.Fatal(err)
	}
	return string(b)
}

func TestClaudeScreenDropsChromeKeepsContent(t *testing.T) {
	cleaned := Clean(fixture(t, "claude-code.screen.txt"))
	for _, chrome := range []string{"Welcome to Claude Code", "/help for help", "cwd: /Users/dev/app", "Tip:", "Thinking…", "Cogitating…",
		"Sautéed for", "? for shortcuts", "Context left until auto-compact", "accept edits on", "plan mode on",
		"bypass permissions on", "shift+tab", "Total cost", "Total duration", "23.4k tokens", "Claude Code v2",
		"───", "━━━", "═══", "- - - -", "╭", "╰", "│", "ctrl+r to expand", "esc to interrupt"} {
		if strings.Contains(cleaned, chrome) {
			t.Errorf("chrome %q survived", chrome)
		}
	}
	for _, keep := range []string{"> fix the login bug", "⏺ I'll look at the auth module first.", "⏺ Read(src/auth.rs)",
		"⎿  Read 120 lines", "⏺ Bash(cargo test auth)", "test result: ok. 4 passed", "… +12 lines",
		"| auth.rs | token check |", "|------|--------|", "  ---", "```rust", "// ----------------", "Done."} {
		if !strings.Contains(cleaned, keep) {
			t.Errorf("content %q lost", keep)
		}
	}
	if cleaned != strings.TrimRight(cleaned, "\n ") {
		t.Error("trailing whitespace")
	}
}

type tr struct{ role, kind, text, tool, summary string }

func flat(ts []ScreenTurn) []tr {
	var out []tr
	for _, t := range ts {
		out = append(out, tr{t.Role, t.Kind, t.Text, t.Tool, t.Summary})
	}
	return out
}

func TestClaudeTurns(t *testing.T) {
	turns := Transform("claude", fixture(t, "claude-code.screen.txt"))
	if len(turns) != 7 {
		t.Fatalf("%d turns: %+v", len(turns), flat(turns))
	}
	if turns[0].Role != "user" || turns[0].Text != "fix the login bug" {
		t.Errorf("%+v", turns[0])
	}
	if turns[2].Kind != "tool_use" || turns[2].Tool != "Read" || turns[2].Summary != "src/auth.rs" {
		t.Errorf("%+v", turns[2])
	}
	if turns[3].Kind != "tool_result" || turns[3].Text != "Read 120 lines" {
		t.Errorf("%+v", turns[3])
	}
	if turns[5].Text != "running 4 tests\ntest result: ok. 4 passed\n… +12 lines" {
		t.Errorf("%q", turns[5].Text)
	}
	a := turns[6].Text
	for _, want := range []string{"\n| file | change |\n|------|--------|\n", "\n---\n", "\n```rust\nfn login() {\n    // ----------------\n}\n```"} {
		if !strings.Contains(a, want) {
			t.Errorf("answer lacks %q", want)
		}
	}
	if !strings.HasPrefix(a, "All 4 auth tests pass. Summary:") || !strings.HasSuffix(a, "Done.") {
		t.Errorf("%q", a)
	}
}

func TestDevinTurns(t *testing.T) {
	turns := flat(Transform("devin", fixture(t, "devin.screen.txt")))
	want := []tr{
		{"user", "text", "add a health endpoint", "", ""},
		{"assistant", "text", "I'll add /healthz to the router.", "", ""},
		{"assistant", "tool_use", "", "Edit", "src/router.go"},
		{"assistant", "tool_result", "Added 6 lines", "", ""},
		{"assistant", "tool_use", "", "Bash", "go test ./..."},
		{"assistant", "tool_result", "ok   app/router  0.21s", "", ""},
		{"assistant", "text", "The endpoint is in place and tests pass.", "", ""},
	}
	if len(turns) != len(want) {
		t.Fatalf("%+v", turns)
	}
	for i := range want {
		if turns[i] != want[i] {
			t.Errorf("turn %d: %+v want %+v", i, turns[i], want[i])
		}
	}
	for _, chrome := range []string{"Devin CLI v1", "Welcome to Devin", "Working (5s", "⏎ send", "87% context left", "Context: 41% used", "Ctrl+C again", "ctrl+o to expand"} {
		if strings.Contains(Clean(fixture(t, "devin.screen.txt")), chrome) {
			t.Errorf("%q survived", chrome)
		}
	}
}

func TestCodexScreens(t *testing.T) {
	cleaned := Clean(fixture(t, "codex.screen.txt"))
	for _, chrome := range []string{"OpenAI Codex", "/model to change", "directory:", "To get started", "▌", "⏎ send", "context left", "Token usage"} {
		if strings.Contains(cleaned, chrome) {
			t.Errorf("%q survived", chrome)
		}
	}
	turns := Turns(cleaned)
	if len(turns) != 2 || turns[0].Text != "explain main.go" || turns[1].Text != "main.go wires the HTTP server and the config loader." {
		t.Errorf("%+v", flat(turns))
	}
	idle := flat(Transform("codex", fixture(t, "codex-idle.screen.txt")))
	if len(idle) != 2 || idle[0].text != "hi" || idle[1].text != "Hi! What can I help you with?" {
		t.Errorf("%+v", idle)
	}
	ph := flat(Transform("codex", "› Explain this codebase\n\n• It is a hub.\n\n› Ask Codex to do anything\n"))
	if len(ph) != 2 || ph[0].text != "Explain this codebase" || ph[1].text != "It is a hub." {
		t.Errorf("%+v", ph)
	}
}

func TestPickReader(t *testing.T) {
	cases := []struct{ agent, file, want string }{
		{"claude", "claude-code.screen.txt", readerClaude}, {"", "claude-code.screen.txt", readerClaude},
		{"devin", "devin.screen.txt", readerDevin}, {"", "devin.screen.txt", readerDevin},
		{"codex", "codex.screen.txt", readerCodex}, {"", "codex-idle.screen.txt", readerCodex},
		{"agy", "agy.screen.txt", readerAgy}, {"Antigravity", "agy.screen.txt", readerAgy}, {"", "agy.screen.txt", readerAgy},
		{"", "generic-shell.screen.txt", readerGeneric}, {"aider", "generic-agent.screen.txt", readerGeneric},
	}
	for _, c := range cases {
		if got := PickReader(c.agent, fixture(t, c.file)); got != c.want {
			t.Errorf("%q %s = %s want %s", c.agent, c.file, got, c.want)
		}
	}
}

func TestBoxTableBecomesRealTable(t *testing.T) {
	turns := Transform("claude", fixture(t, "claude-table.screen.txt"))
	if turns[0].Text != "compare the transports" {
		t.Fatal(turns[0])
	}
	a := turns[1].Text
	if !strings.Contains(a, "| Transport | Direction | Status |\n|---|---|---|\n| NDJSON | hub → machine events | shipped |") ||
		!strings.Contains(a, "| POST segments | machine → hub segments, gzip | shipped |") || strings.Contains(a, "┌") {
		t.Errorf("%s", a)
	}
	items := ScreenItems(turns)
	h := Linkify(items[1].HTML, KlarkHref("box", "p1"))
	if !strings.Contains(h, "<table>") || !strings.Contains(h, "<th>Transport</th>") {
		t.Errorf("no table in %s", h)
	}
	if !strings.Contains(h, `href="m/box/f?pane=p1&amp;path=crates%2Fklark-link%2Fsrc%2Ftransport.rs#L42"`) {
		t.Errorf("no path link in %s", h)
	}
	for _, it := range items {
		if it.Kind == "tool_result" && (it.OK == nil || *it.OK) {
			t.Errorf("an error-shaped result must be marked failed: %+v", it)
		}
	}
}

func TestAgyTurns(t *testing.T) {
	turns := flat(Transform("agy", fixture(t, "agy.screen.txt")))
	want := [][2]string{{"user", "/model"}, {"assistant", "Model set to Gemini 3.8 Flash (Low)"}, {"user", "hi"}, {"assistant", "Hello! How can I help you today?"}}
	if len(turns) != len(want) {
		t.Fatalf("%+v", turns)
	}
	for i, w := range want {
		if turns[i].role != w[0] || turns[i].text != w[1] || turns[i].kind != "text" {
			t.Errorf("%d: %+v", i, turns[i])
		}
	}
	code := Transform("agy", fixture(t, "agy-code.screen.txt"))
	if len(code) != 2 {
		t.Fatalf("%+v", flat(code))
	}
	reply := code[1].Text
	for _, w := range []string{"```python\ndef is_prime(n: int) -> bool:\n    \"\"\"Return True if n is a prime number, else False.\"\"\"\n    if n < 2:\n        return False\n    for i in range(2, int(n**0.5) + 1):\n        if n % i == 0:\n            return False\n    return True\n```",
		"### 2. Find the Largest Prime in a Given List", "This function tests divisibility"} {
		if !strings.Contains(reply, w) {
			t.Errorf("reply lacks %q:\n%s", w, reply)
		}
	}
}

func TestGenericShell(t *testing.T) {
	turns := Transform("", fixture(t, "generic-shell.screen.txt"))
	var kinds []string
	for _, tt := range turns {
		kinds = append(kinds, tt.Kind)
	}
	if strings.Join(kinds, ",") != "command,output,command,output" || turns[0].Text != "cargo test -p klark-link files" || turns[2].Text != "git status --short" {
		t.Fatalf("%+v", flat(turns))
	}
	items := ScreenItems(turns)
	out := items[1].Segments[0]
	for _, w := range []string{`<span class="af2">ok</span>`, `<span class="af1">FAILED</span>`, `<span class="af2 a-b">   Compiling</span>`} {
		if !strings.Contains(out.HTML, w) {
			t.Errorf("ansi html lacks %q: %s", w, out.HTML)
		}
	}
	if strings.ContainsRune(out.HTML, '\x1b') || strings.ContainsRune(items[1].Text, '\x1b') || out.Lines != 11 {
		t.Errorf("lines=%d", out.Lines)
	}
	if !strings.Contains(Linkify(items[3].Segments[0].HTML, KlarkHref("box", "p1")), `class="fpath"`) {
		t.Error("path in output is not linked")
	}
}

func TestGenericAgentReflow(t *testing.T) {
	turns := Transform("aider", fixture(t, "generic-agent.screen.txt"))
	var user, answer string
	for _, tt := range turns {
		if tt.Role == "user" {
			user = tt.Text
		}
	}
	answer = turns[len(turns)-1].Text
	if user != "add a retry to the upload" {
		t.Errorf("user %q", user)
	}
	for _, w := range []string{"exponential backoff so that a transient network error does not fail the whole offer, and log each attempt so the operator can see what happened when the hub was briefly unreachable from the machine.",
		"- retries: 3\n- backoff: 250ms doubling", "```\n@@ -150,3 +150,7 @@\n-    let resp"} {
		if !strings.Contains(answer, w) {
			t.Errorf("answer lacks %q:\n%s", w, answer)
		}
	}
	if strings.Contains(answer, "────") {
		t.Error("rule survived")
	}
}

func TestAnsiHTML(t *testing.T) {
	cases := map[string]string{
		"<b>\x1b[31mx\x1b[0m":       `&lt;b&gt;<span class="af1">x</span>`,
		"\x1b[38;2;255;0;0mt\x1b[m": `<span style="color:rgb(255,0,0)">t</span>`,
		"\x1b[38;5;196mc":           `<span style="color:rgb(255,0,0)">c</span>`,
		"a\x1b[2K\x1b]0;title\x07b": "ab",
		"\x1b[1;32mgo\x1b[0m plain": `<span class="af2 a-b">go</span> plain`,
	}
	for in, want := range cases {
		if got := AnsiHTML(in); got != want {
			t.Errorf("%q -> %q want %q", in, got, want)
		}
	}
}

func TestKeepsContentLines(t *testing.T) {
	for _, line := range []string{"---", "> a genuine quoted line", "The build takes about 30s on this machine.", "It used 500 tokens to do that.",
		"We are done with the migration.", "Press Esc to close the dialog in the app we built.", "| a | b |", "┌───┬───┐", "│ a │ b │",
		"model accuracy is 41%", "1. Yes"} {
		if got := Clean(line); got != line {
			t.Errorf("%q became %q", line, got)
		}
	}
	fenced := "```\n────────────────\n❯\n? for shortcuts\n```"
	if Clean(fenced) != fenced {
		t.Error("fence not kept")
	}
	if got := Clean("╭──────────────╮\n│ > deploy it  │\n╰──────────────╯"); got != "> deploy it" {
		t.Errorf("%q", got)
	}
	if got := Clean("\x1b[1;32mhi\x1b[0m\n\x1b[2m? for shortcuts\x1b[0m"); got != "hi" {
		t.Errorf("%q", got)
	}
	if got := Clean("Model changed to gpt-6-luna low\n\n› hi"); got != "› hi" {
		t.Errorf("%q", got)
	}
}

func TestBlockedPrompts(t *testing.T) {
	box := "╭──────────────────────────────╮\n│ Do you want to proceed?      │\n│ ❯ 1. Yes                     │\n│   2. No                      │\n╰──────────────────────────────╯"
	b := BlockedPrompt(Clean(box))
	if b.Question != "Do you want to proceed?" || len(b.Options) != 2 || b.Options[0].Send != "1" || b.Options[1].Send != "2" {
		t.Errorf("%+v", b)
	}
	dev := BlockedPrompt("── File · Destination ──\n Which MP3 should I send?\n ❯ 1 ganesha-speech.mp3\n     1.5 MB, Aug 12\n · 2 kathir_clean.mp3\n     2.5 MB, Jul 26\n ·   Other (type your own)\n\n↑↓ navigate · ⏎select · esc cancel")
	if len(dev.Options) != 2 || dev.Options[0].Label != "1. ganesha-speech.mp3" || !strings.Contains(dev.Question, "Which MP3") {
		t.Errorf("%+v", dev)
	}
	yn := BlockedPrompt("Overwrite file? (y/n)")
	if len(yn.Options) != 2 || !yn.Options[0].Enter {
		t.Errorf("%+v", yn)
	}
}

func TestCleanStructured(t *testing.T) {
	cases := [][2]string{
		{"<command-name>/clear</command-name>\n<command-message>clear</command-message>\n<command-args></command-args>", "/clear"},
		{"<command-name>/model</command-name><command-args>opus</command-args>", "/model opus"},
		{"<local-command-stdout>Set model to opus</local-command-stdout>", ""},
		{"<system-reminder>\nnoise\n</system-reminder>", ""},
		{"fix it\n<system-reminder>be careful</system-reminder>", "fix it"},
		{"Caveat: The messages below were generated by the user while running local commands. DO NOT respond.", ""},
		{"<bash-input>ls</bash-input>", "! ls"}, {"plain text", "plain text"}, {"", ""},
	}
	for _, c := range cases {
		if got := cleanStructured(c[0]); got != c[1] {
			t.Errorf("%q -> %q want %q", c[0], got, c[1])
		}
	}
}

func TestToItem(t *testing.T) {
	never := []map[string]any{
		{"role": "system", "kind": "text", "text": "hook ran"}, {"role": "user", "kind": "meta", "text": "x"},
		{"role": "user", "kind": "text", "text": "x", "meta": true},
		{"role": "user", "kind": "text", "text": "<system-reminder>r</system-reminder>"},
		{"role": "assistant", "kind": "text", "text": "   "}, {"role": "assistant", "kind": "text"},
		{"role": "assistant", "kind": "thinking", "text": ""}, {"role": "tool", "kind": "text", "text": "x"},
	}
	for _, o := range never {
		if ToItem(o, 0) != nil {
			t.Errorf("%v reached the chat", o)
		}
	}
	if it := ToItem(map[string]any{"role": "user", "kind": "text", "text": "<command-name>/clear</command-name>"}, 0); it == nil || it.Text != "/clear" {
		t.Errorf("%+v", it)
	}
	tu := ToItem(map[string]any{"role": "assistant", "kind": "tool_use", "tool": "Bash", "input": map[string]any{"command": "ls"}}, 3)
	if tu == nil || tu.Summary != "ls" || tu.ID != "i3" {
		t.Errorf("%+v", tu)
	}
	if ToItem(map[string]any{"role": "tool", "kind": "tool_result", "text": "ok"}, 0) == nil || ToItem(map[string]any{"role": "user", "kind": "tool_result", "text": ""}, 0) == nil {
		t.Error("tool results dropped")
	}
	out := ToItem(map[string]any{"role": "user", "kind": "text", "text": "<local-command-stdout>ctx 12%</local-command-stdout>"}, 0)
	if out == nil || out.Role != "assistant" || !strings.Contains(out.Text, "```text\nctx 12%\n```") {
		t.Errorf("%+v", out)
	}
	md := ToItem(map[string]any{"role": "assistant", "kind": "text", "text": "hi **there**\n\n```go\nx := 1\n```"}, 0)
	if md == nil || len(md.Segments) != 2 || md.Segments[1].Type != "code" || md.Segments[1].Lang != "go" || !strings.Contains(md.HTML, "<strong>there</strong>") {
		t.Errorf("%+v", md)
	}
}

func TestMarkdownIsSafe(t *testing.T) {
	h := SegmentsHTML(RenderMarkdown("hi <script>alert(1)</script> [x](javascript:alert(1)) ![img](http://evil/x.png) [ok](https://a.b/c) <https://auto.link>", false))
	for _, bad := range []string{"<script", "javascript:", "<img", "evil"} {
		if strings.Contains(h, bad) {
			t.Errorf("%q in %s", bad, h)
		}
	}
	if !strings.Contains(h, `href="https://a.b/c"`) || !strings.Contains(h, `href="https://auto.link"`) {
		t.Errorf("safe links lost: %s", h)
	}
}

func TestFileLinks(t *testing.T) {
	var got []string
	for _, f := range FindPaths("see src/auth.rs:42, ~/notes/todo.md and /Users/me/app/main.go; not and/or, /agent/events, w/o, http://x.com/a/b.go, foo.bar") {
		got = append(got, f.Path)
	}
	want := "src/auth.rs ~/notes/todo.md /Users/me/app/main.go"
	if strings.Join(got, " ") != want {
		t.Errorf("%q want %q", strings.Join(got, " "), want)
	}
	if f := FindPaths("src/a.rs:7"); len(f) != 1 || f[0].Line != 7 {
		t.Errorf("%+v", f)
	}
	h := Linkify(`<p>edit <code>src/x.rs</code> and <a href="/y">src/y.rs</a></p><pre>src/z.rs</pre>`, KlarkHref("m", "p"))
	if !strings.Contains(h, `<code><a class="fpath"`) || strings.Count(h, `class="fpath"`) != 1 {
		t.Errorf("%s", h)
	}
}

func TestRefusal(t *testing.T) {
	cases := []struct{ path, cwd, refused string }{
		{"src/a.rs", "/Users/me/app", ""},
		{"../x", "/Users/me/app", "leaves"},
		{"/Users/me/app/a.go", "/Users/me/app", ""},
		{"/Users/me/other/a.go", "/Users/me/app", ""}, // same home
		{"/etc/passwd", "/Users/me/app", "outside"},
		{"/Users/me/.ssh/id_rsa", "/Users/me/app", "credential"},
		{"~/notes.md", "", ""},
		{"config/.env", "/x", "credential"},
		{"/private/tmp/a.txt", "/tmp", ""},
		{"/a/../b", "/a", "not a plain path"},
		{"", "/a", "no path"},
	}
	for _, c := range cases {
		got := Refusal(c.path, c.cwd)
		if (c.refused == "") != (got == "") || !strings.Contains(got, c.refused) {
			t.Errorf("%q in %q: %q want %q", c.path, c.cwd, got, c.refused)
		}
	}
}

func TestDetectFile(t *testing.T) {
	cases := []struct{ name, head, view, lang string }{
		{"a.go", "package main", "code", "go"}, {"README.md", "# hi", "markdown", "markdown"},
		{"a.csv", "a,b", "csv", ""}, {"x.png", "\x89PNG\r\n\x1a\n....", "image", ""}, {"blob", "a\x00b", "binary", ""},
		{"Dockerfile", "FROM x", "code", "dockerfile"}, {"s", "#!/usr/bin/env python\n", "code", ""},
	}
	for _, c := range cases {
		k := DetectFile(c.name, []byte(c.head))
		if k.View != c.view || k.Lang != c.lang {
			t.Errorf("%s: %+v", c.name, k)
		}
	}
}

func TestMergeLocalPlacesCommandsByTime(t *testing.T) {
	// a command shows after the real items seen at or before it
	items := ToItem(map[string]any{"id": "a", "role": "user", "kind": "text", "text": "hi", "ts": "2026-01-01T10:00:00Z"}, 0)
	next := ToItem(map[string]any{"id": "b", "role": "assistant", "kind": "text", "text": "yo", "ts": "2026-01-01T10:00:10Z"}, 1)
	cmd := CommandItems(1, "/context", "out", items.ts.Add(5*1e9))
	got := MergeLocal([]Item{*items, *next}, cmd, map[string]time.Time{})
	var ids []string
	for _, g := range got {
		ids = append(ids, g.ID)
	}
	if strings.Join(ids, ",") != "a,cmd-1,cmd-1-out,b" {
		t.Errorf("%v", ids)
	}
}
