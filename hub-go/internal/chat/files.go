package chat

import (
	"html"
	"net/http"
	"net/url"
	"regexp"
	"strconv"
	"strings"
	"unicode"
	"unicode/utf8"
)

const fileExt = `rs|cs|razor|go|py|js|mjs|cjs|ts|tsx|jsx|java|kt|kts|swift|rb|php|c|h|cc|cpp|hpp|m|mm|scala|clj|ex|exs|erl|hs|lua|pl|r|dart|zig|nim|sh|bash|zsh|fish|ps1|sql|md|markdown|mdx|txt|log|json|jsonc|yaml|yml|toml|ini|cfg|conf|xml|html|htm|css|scss|less|svg|png|jpe?g|gif|webp|pdf|csv|tsv|diff|patch|lock|proto|graphql|gradle|tf|hcl|vue|svelte|astro|ipynb|sln|csproj|fsproj|props|targets|mod|sum|dockerfile`

var (
	// RE2 has no lookaround: the caller checks the characters before and after a match.
	pathRe   = regexp.MustCompile(`(?i)((?:~/|\.{1,2}/|/)?(?:[\w@+-][\w.@+-]*/)+[\w@+-][\w.@+-]*|[\w@+-][\w.@+-]*\.(?:` + fileExt + `))(?::(\d{1,6})(?::\d{1,4})?)?`)
	knownExt = regexp.MustCompile(`(?i)\.(` + fileExt + `)$`)
	fsRoot   = regexp.MustCompile(`^/(Users|home|tmp|private|var|etc|opt|usr|srv|mnt|Volumes|workspace|workspaces|root|data|app|code|src)/`)
	htmlTag  = regexp.MustCompile(`(<[^>]*>)`)
)

// Found is a file reference in plain text.
type Found struct {
	Index, Length int
	Path          string
	Line          int // 0 when none
}

func isWord(r rune) bool { return r == '_' || unicode.IsLetter(r) || unicode.IsDigit(r) }

func badBefore(text string, i int) bool {
	if i == 0 {
		return false
	}
	r, _ := utf8.DecodeLastRuneInString(text[:i])
	return isWord(r) || strings.ContainsRune("/.~@:-", r)
}

func badAfter(text string, i int) bool {
	if i >= len(text) {
		return false
	}
	r, w := utf8.DecodeRuneInString(text[i:])
	if isWord(r) || strings.ContainsRune("/@+-", r) {
		return true
	}
	if r == '.' && i+w < len(text) {
		n, _ := utf8.DecodeRuneInString(text[i+w:])
		return isWord(n)
	}
	return false
}

// FindPaths are the file references in a run of plain text: absolute, "~/" and relative paths, with an optional
// ":line".
func FindPaths(text string) []Found {
	var out []Found
	pos := 0
	for pos < len(text) {
		m := pathRe.FindStringSubmatchIndex(text[pos:])
		if m == nil {
			break
		}
		start, end := pos+m[0], pos+m[1]
		pathEnd := pos + m[3]
		line := 0
		if m[4] >= 0 {
			line, _ = strconv.Atoi(text[pos+m[4] : pos+m[5]])
		}
		_, w := utf8.DecodeRuneInString(text[start:])
		if badBefore(text, start) {
			pos = start + w
			continue
		}
		if badAfter(text, end) {
			if m[4] >= 0 && !badAfter(text, pathEnd) { // backtrack: drop the ":line"
				end, line = pathEnd, 0
			} else {
				pos = start + w
				continue
			}
		}
		p := text[pos+m[2] : pathEnd]
		pos = end
		abs := strings.HasPrefix(p, "/") || strings.HasPrefix(p, "~/")
		slashes := strings.Count(p, "/")
		known := knownExt.MatchString(p)
		switch {
		case strings.Contains(p, "//") || strings.HasSuffix(p, "."):
			continue
		case !abs && !known && slashes < 2:
			continue
		case abs && slashes < 2 && !known:
			continue
		case strings.HasPrefix(p, "/") && !known && !fsRoot.MatchString(p):
			continue
		case strings.HasPrefix(p, "http") || strings.Contains(p, "..."):
			continue
		}
		out = append(out, Found{Index: start, Length: end - start, Path: p, Line: line})
	}
	return out
}

func escapeData(s string) string { return strings.ReplaceAll(url.QueryEscape(s), "+", "%20") }

// FileHref is the viewer link for a path in a pane. It is relative (the page's <base href> carries the hub's
// base path).
func FileHref(machine, pane, path string, line int) string {
	h := "m/" + escapeData(machine) + "/f?pane=" + escapeData(pane) + "&path=" + escapeData(path)
	if line > 0 {
		h += "#L" + strconv.Itoa(line)
	}
	return h
}

// Linkify links file paths in rendered (already escaped) HTML: only in text nodes, never inside an existing link
// or a code block. Inline code chips are linked.
func Linkify(htmlIn, machine, pane string) string {
	var sb strings.Builder
	inA, inPre := 0, 0
	idx := htmlTag.FindAllStringIndex(htmlIn, -1)
	pos := 0
	emitText := func(part string) {
		if part == "" {
			return
		}
		if inA > 0 || inPre > 0 {
			sb.WriteString(part)
			return
		}
		text := html.UnescapeString(part)
		found := FindPaths(text)
		if len(found) == 0 {
			sb.WriteString(part)
			return
		}
		p := 0
		for _, f := range found {
			sb.WriteString(html.EscapeString(text[p:f.Index]))
			shown := text[f.Index : f.Index+f.Length]
			sb.WriteString(`<a class="fpath" href="` + html.EscapeString(FileHref(machine, pane, f.Path, f.Line)) +
				`" data-path="` + html.EscapeString(f.Path) + `">` + html.EscapeString(shown) + "</a>")
			p = f.Index + f.Length
		}
		sb.WriteString(html.EscapeString(text[p:]))
	}
	for _, m := range idx {
		emitText(htmlIn[pos:m[0]])
		tag := htmlIn[m[0]:m[1]]
		t := strings.ToLower(tag)
		switch {
		case strings.HasPrefix(t, "<a ") || t == "<a>":
			inA++
		case t == "</a>":
			if inA > 0 {
				inA--
			}
		case strings.HasPrefix(t, "<pre"):
			inPre++
		case t == "</pre>":
			if inPre > 0 {
				inPre--
			}
		}
		sb.WriteString(tag)
		pos = m[1]
	}
	emitText(htmlIn[pos:])
	return sb.String()
}

// CredentialLike: secrets are never fetched (port of klark-link files::credential_like).
func CredentialLike(path string) bool {
	lp := "/" + strings.TrimLeft(strings.ToLower(strings.ReplaceAll(path, `\`, "/")), "/")
	for _, d := range []string{"/.ssh/", "/.aws/", "/.gnupg/", "/.kube/", "/.docker/", "/.config/gcloud/", "/vault/", "/.password-store/"} {
		if strings.Contains(lp, d) {
			return true
		}
	}
	b := lp[strings.LastIndex(lp, "/")+1:]
	if b == ".env" || strings.HasSuffix(b, ".env") || strings.HasPrefix(b, ".env.") || strings.HasPrefix(b, "id_") ||
		b == ".netrc" || b == ".npmrc" || b == ".pypirc" {
		return true
	}
	for _, s := range []string{".pem", ".key", ".p12", ".pfx"} {
		if strings.HasSuffix(b, s) {
			return true
		}
	}
	for _, s := range []string{"credential", "secret", "token"} {
		if strings.Contains(b, s) {
			return true
		}
	}
	return false
}

var homeOf = regexp.MustCompile(`^/(?:Users|home)/[^/]+`)

func unprivate(p string) string {
	for _, d := range []string{"tmp", "var", "etc"} {
		pre := "/private/" + d
		if p == pre || strings.HasPrefix(p, pre+"/") {
			return p[8:]
		}
	}
	return p
}

// Refusal is why the hub will not ask a machine for this path, or "". Allowed: relative to the pane's cwd (no
// escaping with ..), "~/...", or absolute under the cwd or the cwd's home directory.
func Refusal(path, cwd string) string {
	path = strings.TrimSpace(path)
	if path == "" {
		return "no path given"
	}
	if len(path) > 1024 || strings.ContainsRune(path, 0) {
		return "invalid path"
	}
	if CredentialLike(path) {
		return path + " looks like a credential; refusing"
	}
	segs := strings.Split(path, "/")
	hasDots := false
	for _, s := range segs {
		if s == ".." {
			hasDots = true
		}
	}
	if hasDots && !strings.HasPrefix(path, "/") {
		return path + " leaves the pane's folder; refusing"
	}
	if strings.HasPrefix(path, "~/") || !strings.HasPrefix(path, "/") {
		return ""
	}
	if hasDots {
		return path + " is not a plain path; refusing"
	}
	root := unprivate(strings.TrimRight(cwd, "/"))
	if root != "" {
		if plain := unprivate(path); plain == root || strings.HasPrefix(plain, root+"/") {
			return ""
		}
	}
	if home := homeOf.FindString(root); home != "" && strings.HasPrefix(path, home+"/") {
		return ""
	}
	return path + " is outside the pane's folder and home; refusing"
}

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

// FileKind is what the viewer draws for a file.
type FileKind struct {
	View string `json:"view"` // code | markdown | csv | tsv | json | diff | image | pdf | binary
	Lang string `json:"lang"`
	Mime string `json:"mime"`
}

var byExt = map[string]string{
	"rs": "rust", "cs": "csharp", "go": "go", "py": "python", "js": "javascript", "mjs": "javascript", "cjs": "javascript",
	"ts": "typescript", "tsx": "typescript", "jsx": "javascript", "java": "java", "kt": "kotlin", "swift": "swift", "rb": "ruby",
	"php": "php", "c": "c", "h": "c", "cc": "cpp", "cpp": "cpp", "hpp": "cpp", "sh": "bash", "bash": "bash", "zsh": "bash",
	"ps1": "powershell", "sql": "sql", "json": "json", "jsonc": "json", "yaml": "yaml", "yml": "yaml", "toml": "ini", "ini": "ini",
	"cfg": "ini", "conf": "ini", "xml": "xml", "html": "xml", "htm": "xml", "svg": "xml", "css": "css", "scss": "scss",
	"md": "markdown", "markdown": "markdown", "mdx": "markdown", "diff": "diff", "patch": "diff", "dockerfile": "dockerfile",
	"proto": "protobuf", "tf": "hcl", "hcl": "hcl", "lua": "lua", "dart": "dart", "zig": "zig", "razor": "xml", "vue": "xml",
}

var byName = map[string]string{
	"dockerfile": "dockerfile", "containerfile": "dockerfile", "makefile": "makefile", "gnumakefile": "makefile",
	"justfile": "makefile", ".gitignore": "bash", ".dockerignore": "bash", "gemfile": "ruby", "rakefile": "ruby",
	"cmakelists.txt": "cmake", "taskfile.yml": "yaml", "go.mod": "go",
}

// DetectFile decides how to draw a file from its name and first bytes.
func DetectFile(name string, head []byte) FileKind {
	if len(head) > 8192 {
		head = head[:8192]
	}
	sniff := http.DetectContentType(head)
	ext := ""
	if i := strings.LastIndex(name, "."); i >= 0 {
		ext = strings.ToLower(name[i+1:])
	}
	switch {
	case strings.HasPrefix(sniff, "image/") && sniff != "image/svg+xml":
		return FileKind{View: "image", Mime: sniff}
	case sniff == "application/pdf" || ext == "pdf":
		return FileKind{View: "pdf", Mime: "application/pdf"}
	case ext == "svg":
		return FileKind{View: "image", Lang: "xml", Mime: "image/svg+xml"}
	case looksBinary(head):
		return FileKind{View: "binary", Mime: "application/octet-stream"}
	}
	lang, ok := byName[strings.ToLower(name)]
	if !ok {
		lang = byExt[ext]
	}
	view := "code"
	switch ext {
	case "md", "markdown", "mdx":
		view = "markdown"
	case "csv":
		view = "csv"
	case "tsv":
		view = "tsv"
	case "json", "jsonc", "ipynb":
		view = "json"
	case "diff", "patch":
		view = "diff"
	}
	return FileKind{View: view, Lang: lang, Mime: "text/plain; charset=utf-8"}
}

func looksBinary(head []byte) bool {
	if len(head) > 8192 {
		head = head[:8192]
	}
	for _, b := range head {
		if b == 0 {
			return true
		}
	}
	for trim := 0; trim < 4 && trim <= len(head); trim++ {
		if utf8.Valid(head[:len(head)-trim]) {
			return false
		}
	}
	return true
}
