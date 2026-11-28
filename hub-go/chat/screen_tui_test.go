package chat

import (
	"strings"
	"testing"
)

func turnSig(ts []ScreenTurn) []string {
	var out []string
	for _, t := range ts {
		out = append(out, t.Role+"/"+t.Kind)
	}
	return out
}

func allText(ts []ScreenTurn) string {
	var b strings.Builder
	for _, t := range ts {
		b.WriteString(t.Text + "\n" + t.Tool + " " + t.Summary + "\n")
	}
	return b.String()
}

func TestDevin3000FreshSessionShowsNothing(t *testing.T) {
	raw := fixture(t, "devin3000-idle.screen.txt")
	if r := PickReader("devin", raw); r != readerDevin {
		t.Fatalf("reader %s", r)
	}
	if r := PickReader("", raw); r != readerDevin {
		t.Fatalf("detected reader %s", r)
	}
	if turns := Transform("devin", raw); len(turns) != 0 {
		t.Fatalf("banner leaked into chat: %+v", turns)
	}
}

func TestDevin3000Conversation(t *testing.T) {
	turns := Transform("devin", fixture(t, "devin3000.screen.txt"))
	sig := turnSig(turns)
	if len(sig) < 3 || sig[0] != "user/text" || sig[1] != "assistant/tool_use" || sig[2] != "assistant/text" {
		t.Fatalf("turns %v", sig)
	}
	if !strings.HasPrefix(turns[0].Text, "List the files in this folder") || !strings.Contains(turns[0].Text, "Do not change any files.") {
		t.Errorf("prompt (with its wrapped line) %q", turns[0].Text)
	}
	if turns[1].Tool != "Listed" || turns[1].Summary != "./" {
		t.Errorf("tool %+v", turns[1])
	}
	body := allText(turns)
	for _, chrome := range []string{"Devin CLI", "v3000", "Free plan", "Ask Devin to build", "SWE-2", "ctrl+v to paste", "⠀", "────", "➜"} {
		if strings.Contains(body, chrome) {
			t.Errorf("chrome %q left in %q", chrome, body)
		}
	}
	for _, keep := range []string{"Folder Contents", "Total files: 0", "| Property | Value |", "ls -la", "No files were changed."} {
		if !strings.Contains(body, keep) {
			t.Errorf("lost %q in %q", keep, body)
		}
	}
}

func TestDevin3000FooterWithContextUsage(t *testing.T) {
	raw := fixture(t, "devin3000.screen.txt") + "\nSWE-2 Flash Medium                     Context: 26k / 262k tokens (10%)\n"
	if strings.Contains(allText(Transform("devin", raw)), "262k tokens") {
		t.Error("context usage footer left in chat")
	}
}

func TestOpenCodeScreen(t *testing.T) {
	raw := fixture(t, "opencode-2.screen.txt")
	if r := PickReader("opencode", raw); r != readerOpenCode {
		t.Fatalf("reader %s", r)
	}
	if r := PickReader("", raw); r != readerOpenCode {
		t.Fatalf("detected reader %s", r)
	}
	turns := Transform("opencode", raw)
	body := allText(turns)
	for _, chrome := range []string{"ctrl+p commands", "╹▀", "Build auto", "tok/s", "Context", "105,836 tokens", "Connected", "┃"} {
		if strings.Contains(body, chrome) {
			t.Errorf("chrome %q left in chat", chrome)
		}
	}
	var user, steps int
	for _, tt := range turns {
		if tt.Role == "user" {
			user++
			if strings.Contains(tt.Text, "[klark msg from") && !strings.Contains(tt.Text, "/tmp/demo/scratch/notes.md and reply") {
				t.Errorf("wrapped prompt not joined: %q", tt.Text)
			}
		}
		if tt.Kind == "tool_use" && tt.Tool == "steps" {
			steps++
		}
	}
	if user == 0 || steps < 4 {
		t.Errorf("user turns %d, step rows %d: %v", user, steps, turnSig(turns))
	}
	for _, keep := range []string{"Replied to the hub with the notes path", "chat_message JSON", "Rendering recipe"} {
		if !strings.Contains(body, keep) {
			t.Errorf("lost %q", keep)
		}
	}
}

func TestOpenCodeBulletListAndShellRows(t *testing.T) {
	turns := Transform("opencode", fixture(t, "opencode-3.screen.txt"))
	var reply string
	var shells []ScreenTurn
	for _, tt := range turns {
		if tt.Kind == "text" && tt.Role == "assistant" {
			if strings.Contains(tt.Text, "Done.") {
				reply = tt.Text
			}
			if strings.Contains(tt.Text, "Shell finished") {
				t.Errorf("shell status became prose: %q", tt.Text)
			}
		}
		if tt.Kind == "tool_use" && tt.Tool == "shell" {
			shells = append(shells, tt)
		}
	}
	if strings.Contains(reply, "```") {
		t.Errorf("bullet list fenced as code:\n%s", reply)
	}
	for _, want := range []string{"- Adaptive: remote assignment", "- #3 — closed by its author for inactivity; not a confirmation of the feature\nbeing implemented, and a very long note that wraps onto a second line here.", "- Telemetry: counts exist"} {
		if !strings.Contains(reply, want) {
			t.Errorf("list item %q missing:\n%s", want, reply)
		}
	}
	if len(shells) != 1 || !strings.HasPrefix(shells[0].Summary, "JAVA_HOME=/opt/demo/jdk") {
		t.Errorf("shell rows: %v", shells)
	}
}

func TestOpenCodeScreenWithDialogDoesNotPanic(t *testing.T) {
	// A model picker drawn over the transcript: unreadable, but chrome is still removed and nothing panics.
	turns := Transform("opencode", fixture(t, "opencode.screen.txt"))
	if strings.Contains(allText(turns), "ctrl+p commands") {
		t.Error("status line left")
	}
}
