package chat

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"regexp"
	"sort"
	"strings"
	"testing"
)

// TestRenderRealTranscripts runs real agent transcripts (what "klark transcript" hands the hub; capture them with
// scripts/capture-render-fixtures.sh) through the chat pipeline the hub uses (ToItem for structured logs,
// Transform + ScreenItems for screens, then Linkify) and checks every row:
//
//   - nothing the agent or its harness injected leaks: harness tags, Devin citation and truncation markup,
//     the hub's own note, TUI chrome (banners, composers, status lines, sidebars);
//   - the HTML is safe: no script, no inline handlers, no loaded images, links only http(s)/mailto or file links;
//   - markdown fidelity: a GFM table in the source is a <table>, a heading is <hN>, a fenced block is a code
//     segment, and no raw markdown syntax ("## ", "|---|", "**x**") is left in the rendered prose.
//
// The fixtures are private conversations, so they live outside the repo:
// KLARK_RENDER_FIXTURES=<dir> go test ./chat -run TestRenderRealTranscripts -v
func TestRenderRealTranscripts(t *testing.T) {
	dir := os.Getenv("KLARK_RENDER_FIXTURES")
	if dir == "" {
		t.Skip("KLARK_RENDER_FIXTURES not set (see scripts/capture-render-fixtures.sh)")
	}
	files, _ := filepath.Glob(filepath.Join(dir, "*.json"))
	if len(files) == 0 {
		t.Fatalf("no fixtures in %s", dir)
	}
	sort.Strings(files)
	for _, f := range files {
		f := f
		t.Run(strings.TrimSuffix(filepath.Base(f), ".json"), func(t *testing.T) {
			var tr struct {
				Agent  string           `json:"agent"`
				Source string           `json:"source"`
				Items  []map[string]any `json:"items"`
			}
			b, err := os.ReadFile(f)
			if err != nil {
				t.Fatal(err)
			}
			if err := json.Unmarshal(b, &tr); err != nil {
				t.Fatal(err)
			}
			rows, sources := renderRows(tr.Agent, tr.Source, tr.Items)
			report := checkRows(t, tr.Agent, rows, sources, tr.Source == "screen")
			t.Logf("%s source=%s items=%d rows=%d %s", tr.Agent, tr.Source, len(tr.Items), len(rows), report)
		})
	}
}

// renderRows is the hub feed's conversion; sources holds each row's markdown source (for fidelity checks).
func renderRows(agent, source string, items []map[string]any) ([]Item, map[string]string) {
	sources := map[string]string{}
	var rows []Item
	if source == "screen" {
		raw := ""
		if len(items) > 0 {
			raw, _ = items[0]["text"].(string)
		}
		rows = ScreenItems(Transform(agent, raw))
	} else {
		for i, o := range items {
			if it := ToItem(o, i); it != nil {
				rows = append(rows, *it)
				sources[it.ID] = it.Text
			}
		}
	}
	href := KlarkHref("mac", "p1")
	for i := range rows {
		if rows[i].HTML != "" {
			rows[i].HTML = Linkify(rows[i].HTML, href)
		}
	}
	return rows, sources
}

var (
	// Markup the pipeline must remove. Checked in prose outside code spans and quotes, because agents also
	// talk about these tags (an agent explaining Devin's <ref_file> tags is content, not a leak).
	harnessTags = []string{
		"<system-reminder", "</system-reminder", "<ref_file", "<ref_snippet", "<truncation_notice",
		"<system_info", "<system_guidance", "<user-prompt-submit-hook", "<local-command-stdout",
		"<command-name>", "(klark hub:", "[REDACTED]",
	}
	// Tool output is the tool's own text (a file, a DB row, another pane's screen): only what the cleaner
	// promises to strip from it is checked.
	toolTags = []string{"<system-reminder", "(klark hub:"}
	// TUI chrome can only come from reading a screen.
	tuiChrome = []string{
		"ctrl+p commands", "ctrl+v to paste image", "esc twice to interrupt", "Ask Devin to build features",
		"Ask Codex to do anything", "╹▀▀", "⠀⣴⣾⣶⡄", "Free plan, use /upgrade",
	}
	codeOrQuote  = regexp.MustCompile("(?s)\x60{3}.*?\x60{3}|\x60[^\x60\n]*\x60|\"[^\"\n]*\"|“[^”\n]*”")
	unsafeHTML   = regexp.MustCompile("(?i)<script|<iframe|<img[^>]+src=|\\son[a-z]+=|href=\"javascript:")
	hrefRe       = regexp.MustCompile("href=\"([^\"]*)\"")
	srcTable     = regexp.MustCompile("(?m)^\\s*\\|?\\s*:?-{3,}:?\\s*(\\|\\s*:?-{3,}:?\\s*)+\\|?\\s*$")
	srcHeading   = regexp.MustCompile("(?m)^#{1,6} \\S")
	srcFence     = regexp.MustCompile("(?m)^\\s*(\x60{3,}|~{3,})")
	tagRe        = regexp.MustCompile("(?s)<pre.*?</pre>|<code.*?</code>|<[^>]+>")
	rawHeading   = regexp.MustCompile("(?m)^#{1,6} \\S")
	rawTableRule = regexp.MustCompile("\\|\\s*-{3,}\\s*\\|")
	rawBold      = regexp.MustCompile("\\*\\*[^*\\s][^*]*\\*\\*")
)

func checkRows(t *testing.T, agent string, rows []Item, sources map[string]string, screen bool) string {
	t.Helper()
	counts := map[string]int{}
	for _, r := range rows {
		counts[r.Role+"/"+r.Kind]++
		leak := func(where, text string, needles []string) {
			for _, l := range needles {
				if i := strings.Index(text, l); i >= 0 {
					lo := i - 60
					if lo < 0 {
						lo = 0
					}
					t.Errorf("%s row %s (%s/%s) leaks %q in %s: …%.140q", agent, r.ID, r.Role, r.Kind, l, where, text[lo:])
				}
			}
		}
		switch {
		case r.Role == "tool":
			leak("tool output", r.Text, toolTags)
		default:
			leak("text", codeOrQuote.ReplaceAllString(r.Text, ""), harnessTags)
		}
		if screen {
			leak("screen row", r.Text+"\n"+r.HTML, tuiChrome)
		}
		if r.HTML == "" {
			continue
		}
		if m := unsafeHTML.FindString(r.HTML); m != "" {
			t.Errorf("%s row %s unsafe HTML %q", agent, r.ID, m)
		}
		for _, m := range hrefRe.FindAllStringSubmatch(r.HTML, -1) {
			h := strings.ToLower(m[1])
			if !(strings.HasPrefix(h, "http://") || strings.HasPrefix(h, "https://") || strings.HasPrefix(h, "mailto:") ||
				strings.HasPrefix(h, "m/") || strings.HasPrefix(h, "#")) {
				t.Errorf("%s row %s link %q", agent, r.ID, m[1])
			}
		}
		if src, ok := sources[r.ID]; ok && r.Role == "assistant" && r.Kind == "text" {
			body := withoutFences(src)
			if srcTable.MatchString(body) && !strings.Contains(r.HTML, "<table") {
				t.Errorf("%s row %s: table in markdown, none rendered", agent, r.ID)
			}
			if srcHeading.MatchString(body) && !regexp.MustCompile("<h[1-6]").MatchString(r.HTML) {
				t.Errorf("%s row %s: heading in markdown, none rendered", agent, r.ID)
			}
			if srcFence.MatchString(src) {
				code := false
				for _, s := range r.Segments {
					code = code || s.Type == "code"
				}
				if !code {
					t.Errorf("%s row %s: fenced code in markdown, no code segment", agent, r.ID)
				}
			}
			prose := tagRe.ReplaceAllString(r.HTML, "\n")
			for name, re := range map[string]*regexp.Regexp{"heading": rawHeading, "table rule": rawTableRule, "bold": rawBold} {
				if m := re.FindString(prose); m != "" {
					t.Errorf("%s row %s: raw markdown %s left in prose: %q", agent, r.ID, name, m)
				}
			}
		}
	}
	keys := make([]string, 0, len(counts))
	for k := range counts {
		keys = append(keys, k)
	}
	sort.Strings(keys)
	var parts []string
	for _, k := range keys {
		parts = append(parts, fmt.Sprintf("%s=%d", k, counts[k]))
	}
	return strings.Join(parts, " ")
}

// withoutFences drops fenced code (a table or heading inside a code block is code, not markdown).
func withoutFences(s string) string {
	var out []string
	in := false
	for _, l := range strings.Split(s, "\n") {
		if srcFence.MatchString(l) {
			in = !in
			continue
		}
		if !in {
			out = append(out, l)
		}
	}
	return strings.Join(out, "\n")
}
