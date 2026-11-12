package chat

import (
	"strings"
	"testing"
)

func TestChoicePromptsFromAgentScreens(t *testing.T) {
	cases := []struct {
		file, agent, selected, question string
		labels                          []string
	}{
		{"agy-choice.screen.txt", "agy", "1", "File access\nRead: /Users/me/notes/todo.md\nReason: outside workspace",
			[]string{"Yes, allow access", "Yes, and always allow non-workspace access", "No, deny access"}},
		{"claude-choice.screen.txt", "claude", "1", "Bash command",
			[]string{"Yes", "Yes, and don't ask again for rm commands in /Users/me/app", "No, and tell Claude what to do differently (esc)"}},
		{"codex-choice.screen.txt", "codex", "1", "Would you like to run the following command?",
			[]string{"Yes, proceed (y)", "Yes, and don't ask again for this command (a)", "No, and tell Codex what to do differently (esc)"}},
	}
	for _, c := range cases {
		raw := fixture(t, c.file)
		b := ChoicePrompt(raw)
		if b == nil {
			t.Fatalf("%s: no choice prompt", c.file)
		}
		if b.Selected != c.selected || len(b.Choices) != len(c.labels) {
			t.Fatalf("%s: %+v", c.file, b)
		}
		for i, l := range c.labels {
			if b.Choices[i].Key != itoa(i+1) || b.Choices[i].Label != l {
				t.Errorf("%s choice %d: %+v", c.file, i, b.Choices[i])
			}
		}
		if !strings.Contains(b.Question, strings.Split(c.question, "\n")[0]) {
			t.Errorf("%s question %q", c.file, b.Question)
		}
		// The menu never shows up as transcript turns (option 1 used to be a user bubble).
		for _, it := range ScreenItems(Transform(c.agent, raw)) {
			if strings.Contains(it.Text+it.HTML, "1. Yes") || strings.Contains(it.Text+it.HTML, "Yes, allow access") {
				t.Errorf("%s: choice line in transcript item %+v", c.file, it)
			}
		}
	}
	// The question keeps the context the menu is about: agy's path, claude's command.
	if b := ChoicePrompt(fixture(t, "agy-choice.screen.txt")); b.Question != "File access\nRead: /Users/me/notes/todo.md\nReason: outside workspace\n\nAllow access to this file?" {
		t.Errorf("agy question %q", b.Question)
	}
	if b := ChoicePrompt(fixture(t, "claude-choice.screen.txt")); !strings.Contains(b.Question, "rm -rf build") || !strings.HasSuffix(b.Question, "Do you want to proceed?") {
		t.Errorf("claude question %q", b.Question)
	}
	// codex: the reply above the question stays in the transcript.
	items := ScreenItems(Transform("codex", fixture(t, "codex-choice.screen.txt")))
	if len(items) == 0 || !strings.Contains(items[len(items)-1].Text+items[len(items)-1].HTML, "remove the build directory") {
		t.Errorf("codex transcript %+v", items)
	}
}

func TestChoicePromptIgnoresOrdinaryLists(t *testing.T) {
	for _, s := range []string{
		"Steps:\n1. Install\n2. Build\n3. Run\n\nDone.\n> ",
		"Here is the plan\n1. Read\n2. Write",                     // numbered list without marker or hint
		"> 1. only one option\n↑/↓ navigate",                      // a single option is not a menu
		"❯ 2. Yes\n  3. No\nEsc to cancel",                        // must start at 1
		"❯ 1. Yes\n  2. No\n" + strings.Repeat("text line\n", 12), // scrolled away: not live
	} {
		if b := ChoicePrompt(s); b != nil {
			t.Errorf("%q: unexpected %+v", s, b)
		}
	}
	for _, f := range []string{"agy.screen.txt", "claude-code.screen.txt", "codex.screen.txt", "devin.screen.txt", "generic-shell.screen.txt"} {
		if b := ChoicePrompt(fixture(t, f)); b != nil {
			t.Errorf("%s: unexpected %+v", f, b)
		}
	}
}

func TestChoicePromptSelectedNotFirst(t *testing.T) {
	b := ChoicePrompt("Pick one\n  1. Alpha\n› 2. Beta\n  3. Gamma\n")
	if b == nil || b.Selected != "2" || b.Question != "Pick one" {
		t.Fatalf("%+v", b)
	}
}
