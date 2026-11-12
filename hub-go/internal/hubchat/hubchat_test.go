package hubchat

import (
	"strings"
	"testing"
)

func TestCommandOutput(t *testing.T) {
	rule := strings.Repeat("─", 80)
	composer := rule + "\n❭ Ask Devin to build features, fix bugs, or work on your code\n" + rule + "\nSWE-2 Flash Medium          Press Ctrl+O to view the full thinking trace"
	before := "❭ hi\n\nHi — what are we working on?\n\n"
	screen := "❭ hi\n\nHi — what are we working on?\n\n❭ /context\n\nContext Usage\n⛁ ⛁ ⛁ ⛁ ⛁ ⛁ ⛁ ⛁ ⛁ ⛁   swe-2-flash-medium · 31.6k/262.1k tokens (12.1%)\n⛶ ⛶ ⛶ ⛶ ⛶ ⛶ ⛶ ⛶ ⛶ ⛶   ⛶ Free: 230.5k (87.9%)\nNote: Token counts are estimates scaled by character ratio.\n\n"
	o := ExtractCommandOutput(before+composer, screen+composer, "/context")
	if !strings.HasPrefix(o, "Context Usage") || !strings.HasSuffix(o, "Note: Token counts are estimates scaled by character ratio.") ||
		strings.Contains(o, "Ask Devin") || strings.Contains(o, "SWE-2") || strings.Contains(o, "───") || strings.Contains(o, "/context") || strings.Contains(o, "working on") {
		t.Errorf("%q", o)
	}
	if got := ExtractCommandOutput(before+composer, before+composer, "/context"); got != "" {
		t.Errorf("nothing new: %q", got)
	}
	if got := ExtractCommandOutput(before+composer, before+"❭ /clear\n"+composer, "/clear"); got != "" {
		t.Errorf("only echo: %q", got)
	}
	if got := ExtractCommandOutput("line a\nline b\n", "line a\nline b\nline a\nline c\n", "/x"); got != "line a\nline c" {
		t.Errorf("%q", got)
	}
	if got := ExtractCommandOutput("old\n", "old\n\x1b[1mBold\x1b[0m result\n", "/x"); got != "Bold result" {
		t.Errorf("%q", got)
	}
	for in, want := range map[string]bool{"/context": true, "/model gpt": true, "/": false, "hello /context": false, "/a\nb": false, "/usr/bin/ls": true} {
		if IsSlashCommand(in) != want {
			t.Errorf("IsSlashCommand(%q)", in)
		}
	}
}

func TestHubNote(t *testing.T) {
	n := AppendNote("fix it", "name files (by path)")
	if n != "fix it (klark hub: name files [by path])" || StripNote(n) != "fix it" || AppendNote(n, "x") != n {
		t.Errorf("%q", n)
	}
	if AppendNote("/clear", "x") != "/clear" || AppendNote("hi", "") != "hi" || AppendNote(" ", "x") != " " {
		t.Error("never for slash, empty body, empty text")
	}
}
