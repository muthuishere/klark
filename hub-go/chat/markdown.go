package chat

import (
	"bytes"
	"html"
	"net/url"
	"strings"

	"github.com/yuin/goldmark"
	"github.com/yuin/goldmark/ast"
	"github.com/yuin/goldmark/extension"
	extast "github.com/yuin/goldmark/extension/ast"
	"github.com/yuin/goldmark/parser"
	"github.com/yuin/goldmark/renderer"
	renderer_html "github.com/yuin/goldmark/renderer/html"
	"github.com/yuin/goldmark/text"
)

// Segment is a rendered piece of a message: "html", a "code" block (lang + code, drawn with a label and a copy
// button) or "ansi" command output (html with colour spans, plus its line count).
type Segment struct {
	Type  string `json:"type"`
	HTML  string `json:"html,omitempty"`
	Lang  string `json:"lang,omitempty"`
	Code  string `json:"code,omitempty"`
	Lines int    `json:"lines,omitempty"`
}

func newMD(hard bool) goldmark.Markdown {
	ropts := []renderer.Option{}
	if hard {
		ropts = append(ropts, renderer_html.WithHardWraps())
	}
	return goldmark.New(
		goldmark.WithExtensions(extension.Table, extension.Strikethrough, extension.TaskList, extension.Linkify),
		goldmark.WithParserOptions(parser.WithAutoHeadingID()),
		goldmark.WithRendererOptions(ropts...), // raw HTML is off by default (no html.WithUnsafe)
	)
}

var (
	mdChat   = newMD(false)
	mdScreen = newMD(true) // text from a screen keeps its line breaks
)

// SafeURL allows only http, https and mailto.
func SafeURL(u string) bool {
	u = strings.TrimSpace(u)
	if u == "" {
		return false
	}
	p, err := url.Parse(u)
	if err != nil {
		return false
	}
	switch strings.ToLower(p.Scheme) {
	case "http", "https", "mailto":
		return true
	}
	return false
}

// RenderMarkdown renders chat markdown: raw HTML is not emitted, links keep only http/https/mailto, images are
// never loaded (they become their alt text), so a transcript cannot make a viewer's browser fetch anything.
// Code blocks come back as their own segments.
func RenderMarkdown(src string, fromScreen bool) []Segment {
	md := mdChat
	if fromScreen {
		md = mdScreen
	}
	source := []byte(src)
	doc := md.Parser().Parse(text.NewReader(source))

	// Unsafe links and images lose their target but keep what they said.
	var bad []ast.Node
	_ = ast.Walk(doc, func(n ast.Node, entering bool) (ast.WalkStatus, error) {
		if !entering {
			return ast.WalkContinue, nil
		}
		switch v := n.(type) {
		case *ast.Image:
			bad = append(bad, n)
		case *ast.Link:
			if !SafeURL(string(v.Destination)) {
				bad = append(bad, n)
			}
		case *ast.AutoLink:
			u := string(v.URL(source))
			if v.AutoLinkType == ast.AutoLinkEmail {
				u = "mailto:" + u
			}
			if !SafeURL(u) {
				bad = append(bad, n)
			}
		}
		return ast.WalkContinue, nil
	})
	for _, n := range bad {
		var label string
		switch v := n.(type) {
		case *ast.AutoLink:
			label = string(v.URL(source))
		default:
			label = string(n.Text(source))
		}
		if label == "" {
			if l, ok := n.(*ast.Link); ok {
				label = string(l.Destination)
			}
		}
		parent := n.Parent()
		if parent == nil {
			continue
		}
		parent.ReplaceChild(parent, n, ast.NewString([]byte(label)))
	}
	_ = extast.TaskCheckBox{} // keep the extension import honest

	var segs []Segment
	var pending bytes.Buffer
	flush := func() {
		if pending.Len() > 0 {
			segs = append(segs, Segment{Type: "html", HTML: pending.String()})
			pending.Reset()
		}
	}
	for n := doc.FirstChild(); n != nil; n = n.NextSibling() {
		switch b := n.(type) {
		case *ast.FencedCodeBlock:
			flush()
			var code strings.Builder
			for i := 0; i < b.Lines().Len(); i++ {
				l := b.Lines().At(i)
				code.Write(l.Value(source))
			}
			lang := ""
			if b.Info != nil {
				lang = strings.TrimSpace(string(b.Info.Segment.Value(source)))
			}
			segs = append(segs, Segment{Type: "code", Lang: lang, Code: strings.TrimSuffix(code.String(), "\n")})
		case *ast.CodeBlock:
			flush()
			var code strings.Builder
			for i := 0; i < b.Lines().Len(); i++ {
				l := b.Lines().At(i)
				code.Write(l.Value(source))
			}
			segs = append(segs, Segment{Type: "code", Code: strings.TrimSuffix(code.String(), "\n")})
		default:
			_ = md.Renderer().Render(&pending, source, n)
		}
	}
	flush()
	return segs
}

// SegmentsHTML flattens segments into one HTML string (code blocks as <pre><code>), for a client that does not
// draw segments itself.
func SegmentsHTML(segs []Segment) string {
	var sb strings.Builder
	for _, s := range segs {
		switch s.Type {
		case "code":
			cls := ""
			if s.Lang != "" {
				cls = ` class="language-` + html.EscapeString(s.Lang) + `"`
			}
			sb.WriteString("<pre><code" + cls + ">" + html.EscapeString(s.Code) + "</code></pre>\n")
		case "ansi":
			sb.WriteString("<pre class=\"ansi\">" + s.HTML + "</pre>\n")
		default:
			sb.WriteString(s.HTML)
		}
	}
	return sb.String()
}
