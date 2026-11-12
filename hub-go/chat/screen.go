package chat

import (
	"strings"
	"unicode/utf8"
)

// Reader names which screen transformer read a pane.
const (
	readerClaude  = "claude"
	readerDevin   = "devin"
	readerCodex   = "codex"
	readerAgy     = "agy"
	readerGeneric = "generic"
)

var (
	devinAgentLine = rx(`(?m)^• \S`)
	devinResult    = rx(`(?m)^\s{1,6}└ `)
	codexBanner    = rx(`(?m)^\s*>_\s*OpenAI Codex`)
)

// PickReader chooses the transformer for a pane: by the agent the machine names, else by the screen's markers.
// (Antigravity "agy" panes are read as generic: that transformer is not ported.)
func PickReader(agent, raw string) string {
	a := strings.ToLower(agent)
	switch {
	case strings.Contains(a, "claude"):
		return readerClaude
	case strings.Contains(a, "codex"):
		return readerCodex
	case strings.Contains(a, "devin"):
		return readerDevin
	case a == "agy" || strings.Contains(a, "antigravity"):
		return readerAgy
	}
	plain := StripAnsi(raw)
	switch {
	case codexBanner.MatchString(plain):
		return readerCodex
	case agyBanner.MatchString(plain):
		return readerAgy
	case strings.ContainsAny(plain, "⏺⎿"):
		return readerClaude
	case devinAgentLine.MatchString(plain) && devinResult.MatchString(plain):
		return readerDevin
	}
	return readerGeneric
}

// Transform turns a raw screen into turns with the transformer PickReader chose.
func Transform(agent, raw string) []ScreenTurn {
	reader := PickReader(agent, raw)
	raw = CutChoice(raw)
	switch reader {
	case readerClaude:
		return TurnsWith(boxTables(Clean(raw)), ">❯", "⏺●", "⎿")
	case readerDevin:
		return devin(raw)
	case readerCodex:
		return codex(raw)
	case readerAgy:
		return agy(raw)
	}
	return generic(raw)
}

func devin(raw string) []ScreenTurn { return TurnsWith(boxTables(Clean(raw)), "›>", "•", "└") }

var (
	codexPlaceholder = rx(`^\s*› (Ask Codex to do anything|Write tests for @filename|Explain this codebase|summarize recent commits|Implement \{feature\}|Find and fix a bug in @filename|Improve documentation in @filename|Use /skills to list available skills|Run /review on my current changes)\s*$`)
	codexFooter      = rx(`^\s*\S.*\s·\s(~|/)\S*(\s·\s.*)?$|^\s*←?\s*for agents\b`)
	codexWorked      = rx(`^\s*Worked for (\d+h\s*)?(\d+m\s*)?\d+s\b(\s*•.*)?$`)
	codexTurnLine    = rx(`^[›•] `)
)

// codex is Devin's markers, minus the idle composer placeholder, the footer, the timing line and (only while its
// banner is on screen) the greeting before the first turn.
func codex(raw string) []ScreenTurn {
	lines := strings.Split(strings.ReplaceAll(StripAnsi(raw), "\r\n", "\n"), "\n")
	banner := false
	for _, l := range lines {
		if codexBanner.MatchString(l) {
			banner = true
		}
	}
	if banner {
		idx := -1
		for i, l := range lines {
			if codexTurnLine.MatchString(l) {
				idx = i
				break
			}
		}
		if idx < 0 {
			lines = nil
		} else {
			lines = lines[idx:]
		}
	}
	var keep []string
	for _, l := range lines {
		if !codexFooter.MatchString(l) && !codexWorked.MatchString(l) {
			keep = append(keep, l)
		}
	}
	lastIdx := -1
	for i, l := range keep {
		if strings.TrimSpace(l) != "" {
			lastIdx = i
		}
	}
	if lastIdx >= 0 && codexPlaceholder.MatchString(keep[lastIdx]) {
		keep = append(keep[:lastIdx], keep[lastIdx+1:]...)
	}
	return devin(strings.Join(keep, "\n"))
}

// ---- box-drawn tables ---------------------------------------------------------

func isTableTop(t string) bool {
	return utf8.RuneCountInString(t) > 2 && first(t) == '┌' && last(t) == '┐' && strings.Contains(t, "┬")
}
func isTableMid(t string) bool {
	return utf8.RuneCountInString(t) > 2 && first(t) == '├' && last(t) == '┤' && strings.Contains(t, "┼")
}
func isTableBottom(t string) bool {
	return utf8.RuneCountInString(t) > 2 && first(t) == '└' && last(t) == '┘' && strings.Contains(t, "┴")
}
func isTableRow(t string) bool {
	return utf8.RuneCountInString(t) > 2 && first(t) == '│' && last(t) == '│' && strings.Count(t, "│") >= 3
}

func trimBars(u string) string {
	u = strings.TrimPrefix(u, "│")
	return strings.TrimSuffix(u, "│")
}

func cells(u string) []string {
	parts := strings.Split(trimBars(u), "│")
	for i := range parts {
		parts[i] = strings.TrimSpace(parts[i])
	}
	return parts
}

// boxTables turns a TUI's box-drawn table (┌─┬─┐ │ a │ b │ ├─┼─┤ └─┴─┘; a cell that does not fit wraps onto more
// lines between the same separators) into a markdown pipe table, keeping its indent.
func boxTables(text string) string {
	lines := strings.Split(text, "\n")
	out := make([]string, 0, len(lines))
	fenced := false
	for i := 0; i < len(lines); i++ {
		t := strings.TrimSpace(lines[i])
		if strings.HasPrefix(t, "```") {
			fenced = !fenced
		}
		if fenced || !isTableTop(t) {
			out = append(out, lines[i])
			continue
		}
		indent := lines[i][:len(lines[i])-len(strings.TrimLeft(lines[i], " \t"))]
		groups := [][][]string{{}}
		j := i + 1
		closed, sawMid := false, false
		for ; j < len(lines); j++ {
			u := strings.TrimSpace(lines[j])
			if isTableBottom(u) {
				closed = true
				break
			}
			if isTableMid(u) {
				groups = append(groups, nil)
				sawMid = true
				continue
			}
			if !isTableRow(u) {
				break
			}
			groups[len(groups)-1] = append(groups[len(groups)-1], cells(u))
		}
		if !closed || len(groups[0]) == 0 {
			out = append(out, lines[i])
			continue
		}
		var rows [][]string
		if sawMid {
			for _, g := range groups {
				if len(g) > 0 {
					rows = append(rows, mergeRows(g))
				}
			}
		} else {
			rows = groups[0]
		}
		cols := 0
		for _, r := range rows {
			if len(r) > cols {
				cols = len(r)
			}
		}
		row := func(r []string) string {
			cs := make([]string, cols)
			for k := range cs {
				if k < len(r) {
					cs[k] = strings.ReplaceAll(r[k], "|", `\|`)
				}
			}
			return indent + "| " + strings.Join(cs, " | ") + " |"
		}
		if len(out) > 0 && strings.TrimSpace(out[len(out)-1]) != "" {
			out = append(out, "")
		}
		out = append(out, row(rows[0]), indent+"|"+strings.Repeat("---|", cols))
		for _, r := range rows[1:] {
			out = append(out, row(r))
		}
		out = append(out, "")
		i = j
	}
	return strings.Join(out, "\n")
}

func mergeRows(g [][]string) []string {
	n := 0
	for _, r := range g {
		if len(r) > n {
			n = len(r)
		}
	}
	res := make([]string, n)
	for k := 0; k < n; k++ {
		var parts []string
		for _, r := range g {
			if k < len(r) && r[k] != "" {
				parts = append(parts, r[k])
			}
		}
		res[k] = strings.Join(parts, " ")
	}
	return res
}

// ---- generic: shell prompts, command output, prose ------------------------------

var (
	userHostPrompt   = rx(`^(?:\([\w.-]+\)\s+)?[\w.-]+@[\w.-]+(?::[^\s$#%]*|\s+\S+)?\s*[$#%]\s?(.*)$`)
	barePrompt       = rx(`^[$%](?:\s(.*))?$`)
	ohMyZshPrompt    = rx(`^➜\s+\S+(?:\s+git:\([^)]*\))?(?:\s+✗)?\s?(.*)$`)
	starshipPrompt   = rx(`^❯\s?(.*)$`)
	powerShellPrompt = rx(`^PS [A-Za-z]:[^>]*>\s?(.*)$`)
	agentPrompt      = rx(`^[>›]\s(\S.*)$`)
)

// shellCommand is the command typed after a shell prompt line ("" for an idle prompt); ok is false for any other line.
func shellCommand(plain string) (cmd string, ok bool) {
	for _, re := range []interface{ FindStringSubmatch(string) []string }{userHostPrompt, ohMyZshPrompt, starshipPrompt, powerShellPrompt, barePrompt} {
		if m := re.FindStringSubmatch(plain); m != nil {
			return strings.TrimSpace(m[1]), true
		}
	}
	return "", false
}

func lastCR(l string) string {
	if i := strings.LastIndex(l, "\r"); i >= 0 {
		return l[i+1:]
	}
	return l
}

func blank(s string) bool { return strings.TrimSpace(StripAnsi(s)) == "" }

// Generic reads any other agent or a plain shell: chrome stripped, shell prompts split into command turns,
// command output kept with its ANSI colours, prose shaped into paragraphs, lists, rules and code blocks.
func generic(raw string) []ScreenTurn {
	var turns []ScreenTurn
	var rawLines []string
	for _, l := range strings.Split(strings.ReplaceAll(raw, "\r\n", "\n"), "\n") {
		rawLines = append(rawLines, lastCR(l))
	}
	anyShell := false
	for _, l := range rawLines {
		if c, ok := shellCommand(strings.TrimRight(StripAnsi(l), " \t")); ok && c != "" {
			anyShell = true
		}
	}
	var buf []string
	mode := "prose"
	if anyShell {
		mode = "output"
	}
	flush := func() {
		for len(buf) > 0 && blank(buf[len(buf)-1]) {
			buf = buf[:len(buf)-1]
		}
		for len(buf) > 0 && blank(buf[0]) {
			buf = buf[1:]
		}
		if len(buf) > 0 {
			if mode == "output" {
				turns = append(turns, ScreenTurn{Role: "assistant", Kind: "output", Text: strings.Join(buf, "\n")})
			} else {
				plain := make([]string, len(buf))
				for i, l := range buf {
					plain[i] = StripAnsi(l)
				}
				if md := shapeMarkdown(strings.Join(plain, "\n")); strings.TrimSpace(md) != "" {
					turns = append(turns, ScreenTurn{Role: "assistant", Kind: "text", Text: md})
				}
			}
		}
		buf = nil
	}
	for _, line := range rawLines {
		plain := strings.TrimRight(StripAnsi(line), " \t")
		if isChrome(plain) {
			continue
		}
		if cmd, ok := shellCommand(plain); ok {
			flush()
			if cmd != "" {
				turns = append(turns, ScreenTurn{Role: "user", Kind: "command", Text: cmd})
			}
			mode = "output"
			continue
		}
		if m := agentPrompt.FindStringSubmatch(plain); m != nil {
			flush()
			turns = append(turns, ScreenTurn{Role: "user", Kind: "text", Text: strings.TrimSpace(m[1])})
			mode = "prose"
			continue
		}
		buf = append(buf, strings.TrimRight(line, " \t"))
	}
	flush()
	return turns
}

// ---- shapeTranscript, ported ----------------------------------------------------

const (
	ruleMin            = 8
	codeRunMin         = 2
	reflowMinFullLines = 3
	reflowRagged       = 12
	reflowMaxRun       = 60
	reflowMinWidth     = 40
	hardMinLines       = 2
)

var (
	boxDrawing = rx(`[─-╿▀-▟]`)
	gutter     = rx(`^\s*\d+\s+[-+−±]?\s?`)
	diffMarker = rx(`^\s*[-+−]\s?\S`)
	bareGutter = rx(`^\s*\d+$`)
	columns    = rx(`\S {2,}\S[\s\S]*?\S {2,}\S`)
	shellLike  = rx(`^\s*[$#>]\s\S`)
	pathish    = rx(`\S*/\S+/\S+`)
	codePunct  = rx("[{}\\[\\]()<>;=|$`~^*/\\\\]")
	blockStart = rx(`^(?:[-*+•▪◦]\s|\d+[.)]\s|[>›❯»→#|])`)
	bullet     = rx(`^(\s*)[-*\x{2022}\x{25aa}\x{25e6}\x{2013}\x{2014}]\s+(\S.*)$`)
	proseBull  = rx(`^\s*[-*\x{2022}]\s\p{L}[\p{L}\p{N}\s:,.'\x{2019}()%-]*$`)
	hunk       = rx(`^@@ .* @@`)
)

const ruleChars = "─━═-_=⎯‾▁·•"

func isRule(line string) bool {
	t := strings.TrimSpace(line)
	if utf8.RuneCountInString(t) < ruleMin || !strings.ContainsRune(ruleChars, first(t)) {
		return false
	}
	f := first(t)
	for _, c := range t {
		if c != f {
			return false
		}
	}
	return true
}

func isTableLine(line string) bool {
	return strings.HasPrefix(strings.TrimLeft(line, " \t"), "|") && strings.HasSuffix(strings.TrimRight(line, " \t"), "|")
}

func isCodeLike(line string) bool {
	if strings.TrimSpace(line) == "" || isRule(line) || isTableLine(line) {
		return false
	}
	if boxDrawing.MatchString(line) || gutter.MatchString(line) || (diffMarker.MatchString(line) && !proseBull.MatchString(line)) ||
		bareGutter.MatchString(line) || hunk.MatchString(line) || shellLike.MatchString(line) || pathish.MatchString(line) ||
		columns.MatchString(line) {
		return true
	}
	t := strings.TrimSpace(line)
	n := utf8.RuneCountInString(t)
	return n >= 8 && float64(len(codePunct.FindAllString(t, -1)))/float64(n) > 0.08
}

func indentOf(s string) string { return s[:len(s)-len(strings.TrimLeft(s, " \t"))] }

func runeLen(s string) int { return utf8.RuneCountInString(s) }

func detectWrapColumn(all []string) int {
	max := 0
	var lines []string
	for _, l := range all {
		if l != "" && !isCodeLike(l) {
			lines = append(lines, l)
			if runeLen(l) > max {
				max = runeLen(l)
			}
		}
	}
	if max < reflowMinWidth {
		return 0
	}
	n := 0
	for _, l := range lines {
		if runeLen(l) >= max-reflowRagged {
			n++
		}
	}
	if n >= reflowMinFullLines {
		return max
	}
	return 0
}

func detectHardColumn(all []string) int {
	max := 0
	var lines []string
	for _, l := range all {
		if !isRule(l) {
			lines = append(lines, l)
			if runeLen(l) > max {
				max = runeLen(l)
			}
		}
	}
	if max < reflowMinWidth {
		return 0
	}
	n := 0
	for _, l := range lines {
		if runeLen(l) == max {
			n++
		}
	}
	if n >= hardMinLines {
		return max
	}
	return 0
}

func seam(prev, next string) string {
	if strings.HasSuffix(prev, " ") || strings.HasPrefix(next, " ") {
		return " "
	}
	return ""
}

func unwrapHardCuts(lines []string, hardAt int) ([]string, []int) {
	if hardAt <= 0 {
		t := make([]int, len(lines))
		for i, l := range lines {
			t[i] = runeLen(l)
		}
		return lines, t
	}
	var out []string
	var tl []int
	for _, line := range lines {
		if len(out) > 0 && tl[len(tl)-1] >= hardAt && !isRule(out[len(out)-1]) && line != "" && !isRule(line) &&
			!blockStart.MatchString(line[len(indentOf(line)):]) {
			out[len(out)-1] = out[len(out)-1] + seam(out[len(out)-1], line) + line
			tl[len(tl)-1] = runeLen(line)
			continue
		}
		out = append(out, line)
		tl = append(tl, runeLen(line))
	}
	return out, tl
}

func isContinuation(prev, next string, wrapAt, prevLen int) bool {
	if prev == "" || next == "" || isRule(prev) || isRule(next) || isCodeLike(prev) || isCodeLike(next) {
		return false
	}
	delta := len(indentOf(next)) - len(indentOf(prev))
	if delta < 0 || delta > 4 {
		return false
	}
	body := next[len(indentOf(next)):]
	if blockStart.MatchString(body) {
		return false
	}
	word := body
	if sp := strings.IndexByte(body, ' '); sp >= 0 {
		word = body[:sp]
	}
	if word == "" {
		return false
	}
	return prevLen+1+runeLen(word) > wrapAt
}

type shaped struct {
	kind  string
	lines []string
}

// shapeMarkdown is shapeTranscript (herdr-expose transcriptText.ts) with markdown as its output: blank runs
// collapse to one, the pane's hard and soft wraps are undone, long rules become rules, code-shaped runs become
// fenced blocks that keep their columns, and bullets become a real list. Nothing is reworded.
func shapeMarkdown(raw string) string {
	var rawLines []string
	for _, l := range strings.Split(raw, "\n") {
		rawLines = append(rawLines, strings.TrimRight(l, " \t"))
	}
	wrapAt := detectWrapColumn(rawLines)
	lines, tail := unwrapHardCuts(rawLines, detectHardColumn(rawLines))

	var items []shaped
	blankRun := 0
	lastProse, haveProse, lastProseLen, joinRun := "", false, 0, 0

	flushBlanks := func() {
		if blankRun == 0 {
			return
		}
		if len(items) > 0 {
			items = append(items, shaped{kind: "blank"})
		}
		blankRun, haveProse, joinRun = 0, false, 0
	}
	for i := 0; i < len(lines); i++ {
		line := lines[i]
		if line == "" {
			blankRun++
			continue
		}
		if isCodeLike(line) {
			j := i
			var run []string
			for j < len(lines) {
				if isCodeLike(lines[j]) {
					run = append(run, lines[j])
					j++
					continue
				}
				if lines[j] == "" && j+1 < len(lines) && isCodeLike(lines[j+1]) {
					run = append(run, "")
					j++
					continue
				}
				break
			}
			if len(run) >= codeRunMin {
				flushBlanks()
				items = append(items, shaped{"code", run})
				haveProse, joinRun = false, 0
				i = j - 1
				continue
			}
		}
		flushBlanks()
		if isRule(line) {
			items = append(items, shaped{kind: "rule"})
			haveProse, joinRun = false, 0
			continue
		}
		if m := bullet.FindStringSubmatch(line); m != nil {
			if n := len(items); n > 0 && items[n-1].kind == "list" {
				items[n-1].lines = append(items[n-1].lines, m[2])
			} else {
				items = append(items, shaped{"list", []string{m[2]}})
			}
			lastProse, haveProse, lastProseLen, joinRun = line, true, tail[i], 0
			continue
		}
		n := len(items)
		switch {
		case wrapAt > 0 && n > 0 && items[n-1].kind == "list" && haveProse && joinRun < reflowMaxRun &&
			len(indentOf(line)) > 0 && isContinuation(lastProse, line, wrapAt, lastProseLen):
			l := items[n-1].lines
			l[len(l)-1] += " " + strings.TrimLeft(line, " \t")
			joinRun++
		case wrapAt > 0 && n > 0 && items[n-1].kind == "text" && haveProse && joinRun < reflowMaxRun &&
			isContinuation(lastProse, line, wrapAt, lastProseLen):
			l := items[n-1].lines
			l[len(l)-1] += " " + strings.TrimLeft(line, " \t")
			joinRun++
		default:
			items = append(items, shaped{"text", []string{line}})
			joinRun = 0
		}
		lastProse, haveProse, lastProseLen = line, true, tail[i]
	}

	var sb strings.Builder
	for _, it := range items {
		switch it.kind {
		case "blank":
			sb.WriteString("\n")
		case "rule":
			sb.WriteString("\n* * *\n\n")
		case "list":
			for _, l := range it.lines {
				sb.WriteString("- " + l + "\n")
			}
		case "code":
			fence := "```"
			for _, l := range it.lines {
				if strings.Contains(l, "```") {
					fence = "````"
				}
			}
			sb.WriteString("\n" + fence + "\n")
			for _, l := range it.lines {
				sb.WriteString(l + "\n")
			}
			sb.WriteString(fence + "\n\n")
		default:
			sb.WriteString(strings.TrimLeft(it.lines[0], " \t") + "\n")
		}
	}
	return strings.Trim(sb.String(), "\n")
}

// ---- agy (Antigravity CLI) --------------------------------------------------------

var (
	agyBanner  = rx(`(?m)Antigravity CLI \d`)
	agyPrompt  = rx(`^>\s(\S.*)$`)
	agyLogo    = rx(`^\s*[▄▀][▄▀ ]*(\s{3,}\S.*)?$`)
	agyAccount = rx(`^\s*\S+@\S+\s+\(Antigravity[^)]*\)`)
	listMarker = rx(`^([-*+]|\d+[.)])\s+\S`)
	pythonLine = rx(`^\s*(def \w|class \w+.*:\s*$|import \w|from \w+ import )`)
	jsLine     = rx(`^\s*(function |const |let |var |export |=>)`)
)

// agy reads Antigravity: "> text" is the person; everything after it up to the next prompt is the reply (it has no
// marker), "⎿" results are plain reply text. The logo, the account line, rules, the empty composer and the footer
// are dropped.
func agy(raw string) []ScreenTurn {
	lines := strings.Split(strings.ReplaceAll(StripAnsi(raw), "\r\n", "\n"), "\n")
	banner := false
	for _, l := range lines {
		if agyBanner.MatchString(l) {
			banner = true
		}
	}
	if banner {
		idx := -1
		for i, l := range lines {
			if agyPrompt.MatchString(l) {
				idx = i
				break
			}
		}
		if idx < 0 {
			lines = nil
		} else {
			lines = lines[idx:]
		}
	}
	var keep []string
	for _, l := range lines {
		if !agyLogo.MatchString(l) && !agyAccount.MatchString(l) {
			keep = append(keep, l)
		}
	}
	cleaned := Clean(strings.Join(keep, "\n"))
	var turns []ScreenTurn
	var buf []string
	flush := func() {
		for len(buf) > 0 && buf[len(buf)-1] == "" {
			buf = buf[:len(buf)-1]
		}
		for len(buf) > 0 && buf[0] == "" {
			buf = buf[1:]
		}
		if len(buf) > 0 {
			turns = append(turns, ScreenTurn{Role: "assistant", Kind: "text", Text: agyMarkdown(buf)})
		}
		buf = nil
	}
	for _, line := range strings.Split(cleaned, "\n") {
		if m := agyPrompt.FindStringSubmatch(line); m != nil {
			flush()
			turns = append(turns, ScreenTurn{Role: "user", Kind: "text", Text: strings.TrimSpace(m[1])})
			continue
		}
		t := strings.TrimLeft(line, " \t")
		if strings.HasPrefix(t, "⎿ ") {
			buf = append(buf, strings.TrimSpace(t[len("⎿ "):]))
			continue
		}
		buf = append(buf, dedent(line))
	}
	flush()
	return turns
}

func isAgyCode(line string, inList, marker bool) bool {
	return strings.HasPrefix(line, "  ") && !(inList && marker) && (!inList || isCodeLike(line))
}

// agyMarkdown puts agy's code runs (lines indented two columns past the reply's margin) back into fenced blocks.
func agyMarkdown(lines []string) string {
	var o []string
	inList := false
	for i := 0; i < len(lines); i++ {
		line := lines[i]
		if line == "" {
			o = append(o, "")
			continue
		}
		marker := listMarker.MatchString(strings.TrimLeft(line, " \t"))
		if !isAgyCode(line, inList, marker) {
			o = append(o, line)
			if line[0] != ' ' || marker {
				inList = marker
			}
			continue
		}
		var run []string
		j := i
		for j < len(lines) {
			l := lines[j]
			if l != "" && isAgyCode(l, inList, listMarker.MatchString(strings.TrimLeft(l, " \t"))) {
				run = append(run, l[2:])
				j++
				continue
			}
			if l == "" && j+1 < len(lines) && lines[j+1] != "" &&
				isAgyCode(lines[j+1], inList, listMarker.MatchString(strings.TrimLeft(lines[j+1], " \t"))) {
				run = append(run, "")
				j++
				continue
			}
			break
		}
		lang, f := "", "```"
		for _, l := range run {
			if lang == "" && pythonLine.MatchString(l) {
				lang = "python"
			}
		}
		if lang == "" {
			for _, l := range run {
				if jsLine.MatchString(l) {
					lang = "javascript"
					break
				}
			}
		}
		for _, l := range run {
			if strings.Contains(l, "```") {
				f = "````"
			}
		}
		if len(o) > 0 && o[len(o)-1] != "" {
			o = append(o, "")
		}
		o = append(o, f+lang)
		o = append(o, run...)
		o = append(o, f, "")
		i = j - 1
		inList = false
	}
	return strings.Trim(strings.Join(o, "\n"), "\n")
}
