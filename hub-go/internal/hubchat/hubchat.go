// Package hubchat is the klark hub's own chat helpers (the hub note, local slash command output, file sizes);
// the reusable transcript code is in github.com/muthuishere/klark/hub-go/chat.
package hubchat

import (
	"fmt"
	"regexp"
	"strconv"
	"strings"

	"github.com/muthuishere/klark/hub-go/chat"
)

// MaxItems caps a chat's memory per pane.
const MaxItems = 2000

func rx(p string) *regexp.Regexp { return regexp.MustCompile(p) }

var (
	slashCommand = rx(`^/\S+( .*)?$`)
	promptGlyph  = rx(`^\s*[❭›>»$%#➜]\s*`)
	devinChrome  = rx(`Ask Devin to build features|^\s*SWE-[\w.-]+\s+\w+\s*$|^\s*(Claude|GPT|Gemini)[\w .-]*\s+(Low|Medium|High)\s*$`)
)

// IsSlashCommand: a one-line "/command args".
func IsSlashCommand(text string) bool {
	return text != "" && !strings.Contains(text, "\n") && slashCommand.MatchString(strings.TrimSpace(text))
}

func cleanLines(s string) []string {
	var out []string
	for _, l := range strings.Split(strings.ReplaceAll(chat.Clean(s), "\r\n", "\n"), "\n") {
		out = append(out, strings.TrimRight(l, " \t\r"))
	}
	return out
}

// ExtractCommandOutput is a slash command's output for agents whose structured log never records local commands
// (Devin): the lines on after that are not on before (counted, so a repeated line is new only as often as it
// grew), ANSI and agent chrome removed, the echo of command dropped, trimmed. Empty when nothing new.
func ExtractCommandOutput(before, after, command string) string {
	seen := map[string]int{}
	for _, l := range cleanLines(before) {
		seen[l]++
	}
	cmd := strings.TrimSpace(command)
	var fresh []string
	for _, l := range cleanLines(after) {
		if strings.TrimSpace(l) != "" && seen[l] > 0 {
			seen[l]--
			continue
		}
		if strings.TrimSpace(l) == "" && len(fresh) == 0 {
			continue
		}
		t := strings.TrimSpace(l)
		if t == cmd || promptGlyph.ReplaceAllString(t, "") == cmd {
			continue
		}
		if devinChrome.MatchString(l) {
			continue
		}
		fresh = append(fresh, l)
	}
	return strings.Trim(strings.Join(fresh, "\n"), "\n \t")
}

// ---- hub note ---------------------------------------------------------------------

// DefaultNote is the instruction added to what a person sends from the hub, so agents name files they create by
// absolute path (which the hub turns into links).
const DefaultNote = "when you create or change a file, state its absolute path so it can be opened from the hub."

var noteTail = rx(`\s*\(klark hub: [^)]*\)\s*$`)

// AppendNote adds the hub note to text (never to slash commands, empty text or text that has it already).
func AppendNote(text, body string) string {
	if strings.TrimSpace(body) == "" || strings.TrimSpace(text) == "" || strings.HasPrefix(strings.TrimLeft(text, " \t\n"), "/") {
		return text
	}
	if noteTail.MatchString(text) {
		return text
	}
	b := strings.NewReplacer("(", "[", ")", "]", "\r\n", " ", "\n", " ").Replace(strings.TrimSpace(body))
	return fmt.Sprintf("%s (klark hub: %s)", text, b)
}

// StripNote is the text as the person wrote it: the note, if it is at the end, removed.
func StripNote(text string) string { return noteTail.ReplaceAllString(text, "") }

// FileSize is "1.5 MB", "12 KB", "300 B".
func FileSize(b int64) string {
	switch {
	case b >= 1<<20:
		return strconv.FormatFloat(float64(b)/1048576, 'f', 1, 64) + " MB"
	case b >= 1024:
		return strconv.FormatInt(b/1024, 10) + " KB"
	}
	return strconv.FormatInt(b, 10) + " B"
}
