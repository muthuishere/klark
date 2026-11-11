// Package ui serves the built React app (ui/ builds into internal/ui/dist), embedded in the binary.
package ui

import (
	"bytes"
	"embed"
	"io/fs"
	"mime"
	"net/http"
	"path"
	"regexp"
	"strings"
)

//go:embed all:dist
var dist embed.FS

// Placeholder is what the built index.html carries where the base href goes:
// <base href="__HUB_BASE_HREF__">. A literal <base href="..."> is replaced too, and when
// the page has neither, a base tag is inserted after <head>.
const Placeholder = "__HUB_BASE_HREF__"

var baseTag = regexp.MustCompile(`(?i)<base\s+[^>]*>`)

// Embedded is the UI built into this binary.
func Embedded() fs.FS {
	sub, _ := fs.Sub(dist, "dist")
	return sub
}

// IndexHTML is index.html with the base href for base ("" or "/prefix") filled in.
func IndexHTML(raw []byte, base string) []byte {
	href := base + "/"
	tag := []byte(`<base href="` + href + `">`)
	switch {
	case bytes.Contains(raw, []byte(Placeholder)):
		return bytes.ReplaceAll(raw, []byte(Placeholder), []byte(href))
	case baseTag.Match(raw):
		return baseTag.ReplaceAllLiteral(raw, tag)
	}
	lower := bytes.ToLower(raw)
	if i := bytes.Index(lower, []byte("<head>")); i >= 0 {
		i += len("<head>")
		return append(append(append([]byte{}, raw[:i]...), tag...), raw[i:]...)
	}
	return append(append([]byte{}, tag...), raw...)
}

// Handler serves the embedded app for requests whose prefix (the base path) is already stripped.
func Handler(base string) http.Handler { return HandlerFS(Embedded(), base) }

// missingUI is served when the binary was built before the React app (npm run build in ui/).
const missingUI = `<!doctype html><html><head><meta charset="utf-8"><base href="` + Placeholder + `"><title>klarkhub</title></head>` +
	`<body><p>klarkhub is running, but this binary was built without the UI. Run <code>npm run build</code> in hub-go/ui and rebuild.</p></body></html>`

// HandlerFS serves files from fsys. A path that is a file is served with long cache headers when it is a
// hashed build asset; any other path that is not an API path serves index.html (no-cache) so client-side
// routes work on reload.
func HandlerFS(fsys fs.FS, base string) http.Handler {
	raw, err := fs.ReadFile(fsys, "index.html")
	if err != nil || len(raw) == 0 {
		raw = []byte(missingUI) // a build without the UI (go build before npm run build)
	}
	index := IndexHTML(raw, base)
	serveIndex := func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "text/html; charset=utf-8")
		w.Header().Set("Cache-Control", "no-cache")
		if r.Method == http.MethodHead {
			return
		}
		_, _ = w.Write(index)
	}
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Method != http.MethodGet && r.Method != http.MethodHead {
			http.Error(w, "method not allowed", http.StatusMethodNotAllowed)
			return
		}
		p := strings.TrimPrefix(path.Clean("/"+r.URL.Path), "/")
		if p == "" || p == "index.html" {
			serveIndex(w, r)
			return
		}
		data, err := fs.ReadFile(fsys, p)
		if err != nil {
			// A missing file with an extension is a real 404 (a stale asset), not a client route.
			if path.Ext(p) != "" && strings.HasPrefix(p, "assets/") {
				http.NotFound(w, r)
				return
			}
			serveIndex(w, r)
			return
		}
		if ct := mime.TypeByExtension(path.Ext(p)); ct != "" {
			w.Header().Set("Content-Type", ct)
		} else {
			w.Header().Set("Content-Type", http.DetectContentType(data))
		}
		if strings.HasPrefix(p, "assets/") {
			w.Header().Set("Cache-Control", "public, max-age=31536000, immutable")
		} else {
			w.Header().Set("Cache-Control", "no-cache")
		}
		if r.Method == http.MethodHead {
			return
		}
		_, _ = w.Write(data)
	})
}
