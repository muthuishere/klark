package chat

import (
	"strings"
	"unicode/utf8"
)

// Readers for the full-screen TUIs of Devin CLI (3000.x) and OpenCode (2.x). Both agents also keep a structured
// store that the machine reads first (klark transcript, source "db"); these run only when it has nothing: a brand
// new session, an older agent, or a store that cannot be read. The testdata screens (devin3000*.screen.txt, opencode*.screen.txt) copy the layouts of real captures with made-up content
// (devin3000*.screen.txt, opencode*.screen.txt).

const readerOpenCode = "opencode"

var (
	// Devin 3000: braille logo + "Devin CLI" / "v3000.x" banner, "❭ " prompts (the person), "✓ "/"✗ " tool steps,
	// unmarked reply text, box tables, a rule-framed composer and a model footer.
	devin3Banner      = rx(`(?m)^\s*[⠀-⣿]*\s*Devin CLI\s*$`)
	devin3Prompt      = rx(`^❭\s?(.*)$`)
	devin3Tool        = rx(`^\s{0,3}([✓✗✔✘])\s+(\S+)\s*(.*)$`)
	devin3Cwd         = rx(`^\s*⏺\s+[~/]\S*\s*$`)
	devin3Rule        = rx(`^\s*─{8,}.*─\s*$|^\s*─{8,}\s*$`)
	devin3Footer      = rx(`ctrl\+v to paste image|esc twice to interrupt|Context: \d+k? / \d+k? tokens|^\s*(NORMAL|ACCEPT EDITS|SMART|BYPASS|AUTONOMOUS)\b|^\s*Free plan, use /upgrade|^\s*(Cloud agents|No subagents)`)
	devin3Placeholder = rx(`^❭\s*(Ask Devin to build features|Guide Devin while it works|Run a shell command|Describe the task to hand off|Type anything to hand off)`)

	// OpenCode 2: "┃" bars frame the person's messages and the composer, the reply is indented text, "+ 2 commands,
	// 1 thought" are collapsed steps, "Build · model · 1m 13s" closes a turn, a right sidebar (title, Context, MCP,
	// LSP ...) shares the rows, and the composer is closed by "╹▀▀▀" over the status line.
	ocBar         = rx(`^\s{0,4}┃\s?\s?(.*)$`)
	ocComposerEnd = rx(`^\s*╹▀+`)
	ocSteps       = rx(`^\s*\+\s+(\d+\s+\w+(,\s*\d+\s+\w+)*)\s*$`)
	ocTurnMeta    = rx(`^\s*\S+(\s\S+)?\s·\s.+\s·\s\d+(\.\d+)?(ms|s|m)\b`)
	ocStatus      = rx(`ctrl\+p commands`)
	ocSidebar     = rx(`\s{4,}(Context|MCP|LSP|Modified Files|Todo)\s*$`)
)

func runeCut(s string, col int) string {
	if col <= 0 || utf8.RuneCountInString(s) <= col {
		return s
	}
	r := []rune(s)
	return string(r[:col])
}

func isDevin3(plain string) bool {
	return devin3Banner.MatchString(plain) || strings.Contains(plain, "\n❭ ")
}

// devin3 reads Devin 3000. Everything before the first prompt (shell lines, logo, plan line) is dropped, and so is
// the composer at the bottom: rules, the cwd line, the idle placeholder and the footer.
func devin3(raw string) []ScreenTurn {
	lines := strings.Split(strings.ReplaceAll(StripAnsi(raw), "\r\n", "\n"), "\n")
	start := -1
	for i, l := range lines {
		if devin3Prompt.MatchString(l) && !devin3Placeholder.MatchString(l) {
			start = i
			break
		}
	}
	if start < 0 {
		return nil // a fresh session: only the banner and the composer
	}
	var turns []ScreenTurn
	var buf []string
	flush := func() {
		text := shapeMarkdown(boxTables(strings.Join(buf, "\n")))
		buf = nil
		if strings.TrimSpace(text) != "" {
			turns = append(turns, ScreenTurn{Role: "assistant", Kind: "text", Text: text})
		}
	}
	for i := start; i < len(lines); i++ {
		l := strings.TrimRight(lines[i], " 	")
		switch {
		case devin3Placeholder.MatchString(l), devin3Rule.MatchString(l), devin3Cwd.MatchString(l), devin3Footer.MatchString(l):
			continue
		case devin3Prompt.MatchString(l):
			flush()
			text := []string{strings.TrimSpace(devin3Prompt.FindStringSubmatch(l)[1])}
			for i+1 < len(lines) && strings.HasPrefix(lines[i+1], "  ") && strings.TrimSpace(lines[i+1]) != "" {
				i++
				text = append(text, strings.TrimSpace(lines[i]))
			}
			turns = append(turns, ScreenTurn{Role: "user", Kind: "text", Text: strings.Join(text, " ")})
		case devin3Tool.MatchString(l):
			flush()
			m := devin3Tool.FindStringSubmatch(l)
			turns = append(turns, ScreenTurn{Role: "assistant", Kind: "tool_use", Tool: m[2], Summary: strings.TrimSpace(m[3])})
		default:
			buf = append(buf, strings.TrimPrefix(l, " "))
		}
	}
	flush()
	return turns
}

func isOpenCode(plain string) bool {
	return ocStatus.MatchString(plain) && strings.Contains(plain, "╹▀")
}

// opencode reads OpenCode 2. The right sidebar is cut off at its column, the composer and status line are dropped,
// "┃" blocks become the person's turns, collapsed step lines become one tool row each, and the per-turn
// "Build · model · time" line is dropped.
func opencode(raw string) []ScreenTurn {
	lines := strings.Split(strings.ReplaceAll(StripAnsi(raw), "\r\n", "\n"), "\n")
	// The sidebar's column: where its section labels start.
	col := 0
	for _, l := range lines {
		if loc := ocSidebar.FindStringSubmatchIndex(l); loc != nil {
			c := utf8.RuneCountInString(l[:loc[2]])
			if c > 40 && (col == 0 || c < col) {
				col = c
			}
		}
	}
	// The composer: the "┃" block right above the last "╹▀▀▀", and everything after it.
	end := len(lines)
	for i := len(lines) - 1; i >= 0; i-- {
		if ocComposerEnd.MatchString(lines[i]) {
			end = i
			for end > 0 && ocBar.MatchString(lines[end-1]) {
				end--
			}
			break
		}
	}
	lines = lines[:end]

	var turns []ScreenTurn
	var buf, user []string
	flushText := func() {
		text := shapeMarkdown(boxTables(strings.Join(buf, "\n")))
		buf = nil
		if strings.TrimSpace(text) != "" {
			turns = append(turns, ScreenTurn{Role: "assistant", Kind: "text", Text: text})
		}
	}
	flushUser := func() {
		if len(user) > 0 {
			turns = append(turns, ScreenTurn{Role: "user", Kind: "text", Text: strings.Join(user, " ")})
		}
		user = nil
	}
	for _, l := range lines {
		l = strings.TrimRight(runeCut(l, col), " 	")
		if m := ocBar.FindStringSubmatch(l); m != nil {
			flushText()
			if t := strings.TrimSpace(m[1]); t != "" {
				user = append(user, t)
			}
			continue
		}
		flushUser()
		switch {
		case ocStatus.MatchString(l), ocTurnMeta.MatchString(l):
			continue
		case ocSteps.MatchString(l):
			flushText()
			turns = append(turns, ScreenTurn{Role: "assistant", Kind: "tool_use", Tool: "steps", Summary: ocSteps.FindStringSubmatch(l)[1]})
		default:
			buf = append(buf, strings.TrimPrefix(l, "     "))
		}
	}
	flushUser()
	flushText()
	// Before the first message the sidebar's session title can sit alone at the top: drop a leading text turn
	// that is only one short line ending in "+" (the title's new-session button).
	if len(turns) > 0 && turns[0].Kind == "text" && strings.HasSuffix(strings.TrimSpace(turns[0].Text), "+") && !strings.Contains(turns[0].Text, "\n") {
		turns = turns[1:]
	}
	return turns
}
