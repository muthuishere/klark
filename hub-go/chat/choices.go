package chat

import (
	"strings"
	"unicode/utf8"
)

// Choice prompts: the numbered, arrow-selected menus agents draw at the bottom of the pane when they wait for an
// answer (claude "Do you want to proceed? ❯ 1. Yes", codex "› 1. Yes, proceed", agy "Allow access to this file?
// 1. Yes, allow access … ↑/↓ Navigate", devin "❯ 1 file.mp3 … ↑↓ navigate"). They are found on the plain screen,
// shown as buttons, and cut out of the transcript so an option line never becomes a user bubble.

// Choice is one numbered option of a choice prompt.
type Choice struct {
	Key   string `json:"key"`
	Label string `json:"label"`
}

var (
	choiceLine = rx(`^\s*([❯›>▶➜→●]\s*)?(?:[·•]\s*)?(\d{1,2})(?:[.)]\s+|\s+)(\S.{0,119}?)\s*$`)
	choiceHint = ri(`↑\s*/?\s*↓|esc to cancel|enter to (?:select|confirm)|⏎\s*select|\bnavigate\b|tab to amend`)
	choiceRule = rx(`^[\s─━═╌┄\-_╭╮╰╯┌┐└┘├┤]*$`)
)

// plainChoiceLine strips ANSI and a surrounding box border.
func plainChoiceLine(s string) string {
	s = strings.TrimRight(StripAnsi(s), " \t\r")
	t := strings.TrimSpace(s)
	t = strings.TrimPrefix(t, "│")
	t = strings.TrimPrefix(t, "|")
	t = strings.TrimSuffix(t, "│")
	t = strings.TrimSuffix(t, "|")
	return strings.TrimSpace(t)
}

// findChoice locates a live choice prompt in the last lines of a screen. It answers the line index where the
// prompt (question included) starts, or -1. Strict on purpose: options must be numbered 1..n (n ≥ 2), sit at the
// bottom, and carry a selection marker or a navigation hint, so old numbered lists in the transcript never match.
func findChoice(lines []string) (int, *Blocked) {
	plain := make([]string, len(lines))
	for i, l := range lines {
		plain[i] = plainChoiceLine(l)
	}
	from := len(plain) - 30
	if from < 0 {
		from = 0
	}
	// The last option "1." in the window starts the candidate run.
	one := -1
	for i := len(plain) - 1; i >= from; i-- {
		if m := choiceLine.FindStringSubmatch(plain[i]); m != nil && m[2] == "1" {
			one = i
			break
		}
	}
	if one < 0 {
		return -1, nil
	}
	b := &Blocked{}
	marked, hinted := false, false
	last, next := one, 1
	for i := one; i < len(plain); i++ {
		p := plain[i]
		if p == "" {
			continue
		}
		if m := choiceLine.FindStringSubmatch(p); m != nil && m[2] == itoa(next) {
			key := m[2]
			b.Choices = append(b.Choices, Choice{Key: key, Label: strings.TrimSpace(m[3])})
			b.Options = append(b.Options, Option{Label: key + ". " + strings.TrimSpace(m[3]), Send: key})
			if m[1] != "" && b.Selected == "" {
				b.Selected, marked = key, true
			}
			last, next = i, next+1
			continue
		}
		if choiceHint.MatchString(p) {
			hinted = true
			continue
		}
		// A description line under an option (indented in the raw screen) is allowed; anything else after the
		// options must be the footer, which is short.
		if i-last > 6 {
			return -1, nil
		}
	}
	if len(b.Choices) < 2 || (!marked && !hinted) {
		return -1, nil
	}
	// The question: the text right above option 1, at most 8 lines across at most two blank gaps, up to a rule
	// or a transcript line (a reply bullet or a prompt), which stays in the transcript.
	start, q, gaps := one, []string{}, 0
	for i := one - 1; i >= 0 && len(q) < 8; i-- {
		p := plain[i]
		if p == "" {
			if len(q) > 0 && q[0] != "" {
				if gaps == 2 {
					break
				}
				gaps++
				q = append([]string{""}, q...)
			}
			continue
		}
		if choiceRule.MatchString(p) {
			start = i
			break
		}
		if r, _ := utf8.DecodeRuneInString(p); strings.ContainsRune("•⏺●›>❯⎿✻", r) {
			break
		}
		q = append([]string{p}, q...)
		start = i
	}
	for len(q) > 0 && q[0] == "" {
		q = q[1:]
	}
	// A box top or rule right above the question goes with the prompt.
	if start > 0 && plain[start-1] != "" && choiceRule.MatchString(plain[start-1]) {
		start--
	}
	b.Question = strings.Join(q, "\n")
	return start, b
}

func itoa(n int) string {
	if n < 10 {
		return string(rune('0' + n))
	}
	return string(rune('0'+n/10)) + string(rune('0'+n%10))
}

// ChoicePrompt finds a choice prompt at the bottom of a screen (raw or cleaned); nil when there is none.
func ChoicePrompt(screen string) *Blocked {
	_, b := findChoice(strings.Split(screen, "\n"))
	return b
}

// CutChoice drops a choice prompt at the bottom of a screen, so the transcript does not show it as messages.
func CutChoice(screen string) string {
	lines := strings.Split(screen, "\n")
	if at, b := findChoice(lines); b != nil {
		return strings.Join(lines[:at], "\n")
	}
	return screen
}
