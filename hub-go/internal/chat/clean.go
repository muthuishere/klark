// Package chat turns what an agent shows (a structured transcript or a captured screen) into chat rows.
// Ported from the Blazor hub (hub/Services: TranscriptClean, ScreenTransformers, ChatMarkdown, AnsiHtml,
// CommandOutput, HubNote), which itself ported herdr-expose. Behaviour, not code, is shared.
package chat

import (
	"regexp"
	"strings"
	"unicode/utf8"
)

func rx(p string) *regexp.Regexp { return regexp.MustCompile(p) }

func ri(p string) *regexp.Regexp { return regexp.MustCompile("(?i)" + p) }

var chrome = []*regexp.Regexp{
	// A rule: eight or more horizontal line, dash or underscore characters and nothing else.
	ri(`^[\s─━═┄┅┈┉╌╍╴╶╸╺_=\-—–╭╮╰╯]{8,}$`),
	ri(`^\s*[❯›>▌]\s*$`),
	ri(`ctx\s+[\d.]+[KM]?\s+\d+%`),
	ri(`context left until auto-?compact`),
	ri(`\b\d+%\s+context\s+(left|remaining|used)\b`),
	ri(`^\s*context\s*(remaining|left|used|window)?\s*:?\s*[\d.]+\s*[%kK]`),
	ri(`\bcontext (remaining|low)\b.*\d`),
	ri(`\b(auto mode|bypass permissions|accept edits|plan mode|auto-accept edits)\b\s*(on|off)?\b.*(\(|$)`),
	ri(`shift\+tab to (cycle|toggle)`),
	ri(`^\s*[⏵⏸▸]{1,2}\s`),
	ri(`^\s*\?\s*for shortcuts`),
	ri(`\besc to (interrupt|cancel)\b`),
	ri(`\bctrl\+[a-z] to (expand|collapse|exit|quit|show)\b`),
	ri(`\b(press )?ctrl-?\+?c again to (exit|quit)\b`),
	ri(`^\s*⏎ send\b`),
	ri(`^\s*(⌃|ctrl\+)j newline\b`),
	ri(`^\s*Image in clipboard`),
	ri(`^\s*※\s*Tip:`),
	ri(`\bfor\s+(\d+h\s*)?(\d+m\s*)?\d+s\b.*·.*\bdone\b`),
	ri(`·\s*[↓↑v^]?\s*[\d.]+k?\s*tokens`),
	ri(`^\s*\S?\s*\w+(…|\.\.\.)\s*\(`),
	ri(`^\s*[·✻✽✶✳✢✦*•⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏]\s*\w+(…|\.\.\.)\s*$`),
	ri(`^\s*[↑↓]?\s*[\d.,]+\s*k?\s*tokens(\s*[·|].*)?\s*$`),
	ri(`^\s*(total\s+)?cost:\s*\$[\d.]+`),
	ri(`^\s*total (duration|code changes)\b.*:`),
	ri(`^\s*token usage:\s`),
	ri(`^\s*[✻*>_]*\s*Welcome to (Claude Code|Devin|Codex)`),
	ri(`^\s*(Claude Code|OpenAI Codex|Devin( CLI)?)\s*\(?v?\d+\.\d+`),
	ri(`^\s*>_\s*OpenAI Codex`),
	ri(`^\s*/help for help`),
	ri(`^\s*(cwd|directory):\s+[~/]\S*\s*$`),
	ri(`/model to change`),
	ri(`^\s*To get started, describe a task`),
	ri(`^\s*Model changed to\s`),
}

var (
	bannerBox  = ri(`Welcome to|/help for help|OpenAI Codex|Claude Code v?\d|Devin( CLI)? v?\d|cwd:\s`)
	ansiRe     = rx(`\x1b\[[0-?]*[ -/]*[@-~]|\x1b\][^\x07\x1b]*(\x07|\x1b\\)|\x1b[@-Z\\-_]`)
	expandHint = ri(`\s*\((ctrl|⌃)\+[a-z] to (expand|collapse|see all)\)`)
)

// StripAnsi removes terminal escape sequences.
func StripAnsi(s string) string { return ansiRe.ReplaceAllString(s, "") }

func isChrome(line string) bool {
	if line == "" {
		return false
	}
	for _, re := range chrome {
		if re.MatchString(line) {
			return true
		}
	}
	return false
}

func first(t string) rune {
	r, _ := utf8.DecodeRuneInString(t)
	return r
}

func last(t string) rune {
	r, _ := utf8.DecodeLastRuneInString(t)
	return r
}

func countAny(t string, set string) int {
	n := 0
	for _, c := range t {
		if strings.ContainsRune(set, c) {
			n++
		}
	}
	return n
}

func isBoxTop(t string) bool {
	return t != "" && strings.ContainsRune("╭┌╔", first(t)) && countAny(t, "─━═") >= 3 && !strings.Contains(t, "┬")
}

func isBoxBottom(t string) bool {
	return t != "" && strings.ContainsRune("╰└╚", first(t)) && countAny(t, "─━═") >= 3 && !strings.Contains(t, "┴")
}

// boxInner is "│ > hello  │" -> " > hello": the inside of a box line.
func boxInner(line string) (string, bool) {
	t := strings.TrimSpace(line)
	if utf8.RuneCountInString(t) < 2 || !strings.ContainsRune("│┃║", first(t)) {
		return "", false
	}
	_, w := utf8.DecodeRuneInString(t)
	inner := t[w:]
	if l := last(t); strings.ContainsRune("│┃║", l) {
		inner = inner[:len(inner)-utf8.RuneLen(l)]
	}
	if strings.Contains(inner, "│") { // a table row is content
		return "", false
	}
	return strings.TrimRight(inner, " \t"), true
}

// Clean turns a captured screen into text a phone can read: strips ANSI, drops the agent's own chrome,
// trims trailing whitespace and collapses blank runs. Inside a fenced code block nothing is chrome.
func Clean(text string) string {
	var out []string
	blankRun := 0
	fenced := false
	var box []string
	inBox := false

	emit := func(line string) {
		if line == "" {
			blankRun++
			return
		}
		if blankRun > 0 && len(out) > 0 {
			out = append(out, "")
		}
		blankRun = 0
		out = append(out, line)
	}
	flushBox := func() {
		if !inBox {
			return
		}
		lines := box
		box, inBox = nil, false
		var inner []string
		for _, l := range lines {
			if s := strings.TrimSpace(l); s != "" {
				inner = append(inner, s)
			}
		}
		if len(inner) == 0 {
			return
		}
		allChrome := true
		for _, l := range inner {
			if bannerBox.MatchString(l) {
				return
			}
			if !isChrome(l) {
				allChrome = false
			}
		}
		if allChrome {
			return
		}
		for _, l := range lines {
			s := strings.TrimPrefix(l, " ")
			if !isChrome(s) {
				emit(strings.TrimRight(s, " \t"))
			}
		}
	}

	s := strings.ReplaceAll(strings.ReplaceAll(StripAnsi(text), "\r\n", "\n"), "\r", "\n")
	for _, raw := range strings.Split(s, "\n") {
		line := strings.TrimRight(raw, " \t ")
		if fenced {
			if strings.HasPrefix(strings.TrimLeft(line, " \t"), "```") {
				fenced = false
			}
			if blankRun > 0 && len(out) > 0 {
				out = append(out, "")
			}
			blankRun = 0
			out = append(out, line)
			continue
		}
		line = expandHint.ReplaceAllString(line, "")
		t := strings.TrimSpace(line)
		if inBox {
			if isBoxBottom(t) {
				flushBox()
				continue
			}
			if inside, ok := boxInner(line); ok {
				box = append(box, inside)
				continue
			}
			flushBox()
		}
		if isBoxTop(t) {
			inBox, box = true, nil
			continue
		}
		if isBoxBottom(t) {
			continue
		}
		if lone, ok := boxInner(line); ok {
			s := strings.TrimPrefix(lone, " ")
			if strings.TrimSpace(s) == "" || isChrome(s) || bannerBox.MatchString(s) {
				continue
			}
			emit(strings.TrimRight(s, " \t"))
			continue
		}
		if isChrome(line) {
			continue
		}
		if strings.HasPrefix(t, "```") {
			fenced = true
		}
		emit(line)
	}
	flushBox()
	return strings.Join(out, "\n")
}

// ScreenTurn is one turn read off a cleaned screen.
type ScreenTurn struct {
	Role, Kind, Text, Tool, Summary string
}

var toolHeader = rx(`^([\p{L}_][\w.:-]*)\((.*)\)\s*$`)

// Turns splits a cleaned screen into turns with Claude Code's default markers.
func Turns(cleaned string) []ScreenTurn { return TurnsWith(cleaned, ">❯›", "⏺●•", "⎿└") }

func dedent(l string) string { return strings.TrimPrefix(l, "  ") }

// TurnsWith: userMarks at column 0 is the person, agentMarks "⏺ Tool(args)" a tool call or other agent text,
// resultMarks its result. Text before any marker belongs to the agent. Nothing is reworded.
func TurnsWith(cleaned, userMarks, agentMarks, resultMarks string) []ScreenTurn {
	var turns []ScreenTurn
	role, kind, tool, summary := "assistant", "text", "", ""
	var buf []string
	fenced := false

	flush := func() {
		for len(buf) > 0 && buf[len(buf)-1] == "" {
			buf = buf[:len(buf)-1]
		}
		body := strings.Trim(strings.Join(buf, "\n"), "\n")
		if strings.TrimSpace(body) != "" || kind == "tool_use" {
			turns = append(turns, ScreenTurn{role, kind, body, tool, summary})
		}
		buf = nil
		role, kind, tool, summary = "assistant", "text", "", ""
	}

	for _, line := range strings.Split(cleaned, "\n") {
		if fenced {
			if strings.HasPrefix(strings.TrimLeft(line, " \t"), "```") {
				fenced = false
			}
			buf = append(buf, dedent(line))
			continue
		}
		if mk, rest, ok := marker(line, userMarks, agentMarks, resultMarks); ok {
			flush()
			switch mk {
			case "user":
				role = "user"
			case "result":
				kind = "tool_result"
			default:
				if m := toolHeader.FindStringSubmatch(rest); m != nil && len(rest) < 200 {
					sum := strings.TrimSpace(m[2])
					if utf8.RuneCountInString(sum) > 70 {
						sum = string([]rune(sum)[:67]) + "…"
					}
					turns = append(turns, ScreenTurn{"assistant", "tool_use", "", m[1], sum})
					continue
				}
			}
			buf = append(buf, rest)
			if strings.HasPrefix(strings.TrimLeft(rest, " \t"), "```") {
				fenced = true
			}
			continue
		}
		d := dedent(line)
		if kind == "tool_result" {
			d = strings.TrimLeft(line, " \t")
		}
		if strings.HasPrefix(strings.TrimLeft(d, " \t"), "```") {
			fenced = true
		}
		buf = append(buf, d)
	}
	flush()
	return turns
}

func marker(line, userMarks, agentMarks, resultMarks string) (kind, rest string, ok bool) {
	if utf8.RuneCountInString(line) < 2 {
		return "", "", false
	}
	c, w := utf8.DecodeRuneInString(line)
	if line[w] == ' ' {
		if strings.ContainsRune(userMarks, c) {
			rest = strings.TrimSpace(line[w+1:])
			return "user", rest, rest != ""
		}
		if strings.ContainsRune(agentMarks, c) {
			return "agent", line[w+1:], true
		}
	}
	t := strings.TrimLeft(line, " \t")
	if len(t) > 1 {
		c2, w2 := utf8.DecodeRuneInString(t)
		if strings.ContainsRune(resultMarks, c2) && t[w2] == ' ' && len(line)-len(t) <= 6 {
			return "result", strings.TrimSpace(t[w2+1:]), true
		}
	}
	return "", "", false
}

// ---- structured (jsonl/db) text ---------------------------------------------------

var (
	droppedTags = []*regexp.Regexp{}
	commandName = rx(`(?s)<command-name>\s*(.*?)\s*</command-name>`)
	commandArgs = rx(`(?s)<command-args>\s*(.*?)\s*</command-args>`)
	bashInput   = rx(`(?s)<bash-input>\s*(.*?)\s*</bash-input>`)
	caveat      = rx(`(?m)^Caveat: The messages below were generated by the user while running local commands.*$`)
	strayTag    = rx(`</?(system-reminder|local-command-stdout|local-command-stderr|command-name|command-args|command-message)>`)
)

func init() {
	for _, tag := range []string{"system-reminder", "local-command-stdout", "local-command-stderr", "command-message", "bash-stdout", "bash-stderr", "user-prompt-submit-hook"} {
		droppedTags = append(droppedTags, rx(`(?s)<`+tag+`>.*?</`+tag+`>`))
	}
}

// CleanStructured is the text of a structured transcript item as a person should see it: harness wrappers
// removed, "<command-name>/x</command-name>" shown as "/x args". Empty means nothing to show.
func CleanStructured(text string) string {
	if text == "" {
		return ""
	}
	if m := commandName.FindStringSubmatch(text); m != nil {
		out := m[1]
		if a := commandArgs.FindStringSubmatch(text); a != nil && a[1] != "" {
			out += " " + a[1]
		}
		return strings.TrimSpace(out)
	}
	if m := bashInput.FindStringSubmatch(text); m != nil {
		return "! " + m[1]
	}
	s := text
	for _, re := range droppedTags {
		s = re.ReplaceAllString(s, "")
	}
	s = caveat.ReplaceAllString(s, "")
	s = strayTag.ReplaceAllString(s, "")
	return strings.TrimSpace(s)
}

// ---- blocked prompts --------------------------------------------------------------

// Option is one quick reply a blocked screen offers.
type Option struct {
	Label string `json:"label"`
	Send  string `json:"send"`
	Enter bool   `json:"enter"`
}

// Blocked is a question an agent is blocked on, and the quick replies its screen offers.
type Blocked struct {
	Question string   `json:"question"`
	Options  []Option `json:"options"`
	// Choices is set for a numbered, arrow-selected menu (see ChoicePrompt); Selected is the highlighted key.
	Choices  []Choice `json:"choices,omitempty"`
	Selected string   `json:"selected,omitempty"`
}

var (
	numberedOption = rx(`^\s*(?:[❯>·]\s*)?(\d{1,2})(?:[.)]\s+|\s+)([^\s│|].{0,79})$`)
	yesNo          = ri(`\((?:y/n|yes/no)\)|\[(?:y/n|Y/n|y/N)\]`)
)

func tail(lines []string, n int) []string {
	if len(lines) > n {
		return lines[len(lines)-n:]
	}
	return lines
}

// BlockedPrompt reads the tail of a cleaned screen for a question and its numbered or yes/no options.
// Nothing is guessed: no options found means no buttons.
func BlockedPrompt(cleaned string) Blocked {
	if c := ChoicePrompt(cleaned); c != nil {
		return *c
	}
	lines := tail(strings.Split(cleaned, "\n"), 30)
	opts := []Option{}
	firstOpt := -1
	seen := map[string]bool{}
	for i, l := range lines {
		m := numberedOption.FindStringSubmatch(l)
		if m == nil || seen[m[1]] {
			continue
		}
		seen[m[1]] = true
		if firstOpt < 0 {
			firstOpt = i
		}
		opts = append(opts, Option{Label: m[1] + ". " + strings.TrimSpace(m[2]), Send: m[1]})
	}
	var q string
	if firstOpt >= 0 {
		q = strings.TrimSpace(strings.Join(tail(lines[:firstOpt], 8), "\n"))
	} else {
		q = strings.TrimSpace(strings.Join(tail(lines, 8), "\n"))
		for _, l := range tail(lines, 4) {
			if yesNo.MatchString(l) {
				opts = append(opts, Option{"Yes", "y", true}, Option{"No", "n", true})
				break
			}
		}
	}
	return Blocked{Question: q, Options: opts}
}
