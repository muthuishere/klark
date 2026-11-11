package app

import (
	"compress/gzip"
	"crypto/sha256"
	"encoding/base64"
	"encoding/json"
	"io/fs"
	"net/http"
	"regexp"
	"strings"
	"sync"
)

func jsonOut(w http.ResponseWriter, status int, v any) {
	w.Header().Set("Content-Type", "application/json")
	w.Header().Set("Cache-Control", "no-store")
	w.WriteHeader(status)
	_ = json.NewEncoder(w).Encode(v)
}

func jsonErr(w http.ResponseWriter, status int, msg string) {
	jsonOut(w, status, map[string]string{"error": msg})
}

// securityHeaders sets the headers every response carries. The CSP lets the app load only its own files; inline
// styles are allowed (the app and the mock ADFS page use them), inline scripts are not.
func securityHeaders(scriptHashes []string, next http.Handler) http.Handler {
	script := "script-src 'self'"
	for _, h := range scriptHashes {
		script += " 'sha256-" + h + "'"
	}
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		h := w.Header()
		h.Set("X-Content-Type-Options", "nosniff")
		h.Set("X-Frame-Options", "DENY")
		h.Set("Referrer-Policy", "same-origin")
		h.Set("Cross-Origin-Opener-Policy", "same-origin")
		h.Set("Permissions-Policy", "camera=(), microphone=(), geolocation=()")
		h.Set("Content-Security-Policy", "default-src 'self'; "+script+"; style-src 'self' 'unsafe-inline'; "+
			"img-src 'self' data: blob:; font-src 'self' data:; connect-src 'self'; frame-ancestors 'none'; base-uri 'self'; object-src 'none'")
		if strings.EqualFold(firstValue(r.Header.Get("X-Forwarded-Proto")), "https") || r.TLS != nil {
			h.Set("Strict-Transport-Security", "max-age=31536000")
		}
		next.ServeHTTP(w, r)
	})
}

func firstValue(v string) string {
	a, _, _ := strings.Cut(v, ",")
	return strings.TrimSpace(a)
}

var gzPool = sync.Pool{New: func() any { return gzip.NewWriter(nil) }}

type gzipWriter struct {
	http.ResponseWriter
	zw     *gzip.Writer
	wrote  bool
	active bool
}

func compressible(ct string) bool {
	ct = strings.ToLower(ct)
	switch {
	case strings.HasPrefix(ct, "text/event-stream"), strings.Contains(ct, "ndjson"):
		return false
	case strings.HasPrefix(ct, "text/"), strings.Contains(ct, "json"), strings.Contains(ct, "javascript"),
		strings.Contains(ct, "xml"), strings.Contains(ct, "svg"), strings.Contains(ct, "wasm"):
		return true
	}
	return false
}

func (g *gzipWriter) WriteHeader(code int) {
	if g.wrote {
		return
	}
	g.wrote = true
	h := g.Header()
	if code != http.StatusNoContent && code != http.StatusNotModified && h.Get("Content-Encoding") == "" && compressible(h.Get("Content-Type")) {
		g.active = true
		h.Set("Content-Encoding", "gzip")
		h.Del("Content-Length")
		h.Add("Vary", "Accept-Encoding")
		g.zw = gzPool.Get().(*gzip.Writer)
		g.zw.Reset(g.ResponseWriter)
	}
	g.ResponseWriter.WriteHeader(code)
}

func (g *gzipWriter) Write(b []byte) (int, error) {
	if !g.wrote {
		if g.Header().Get("Content-Type") == "" {
			g.Header().Set("Content-Type", http.DetectContentType(b))
		}
		g.WriteHeader(http.StatusOK)
	}
	if g.active {
		return g.zw.Write(b)
	}
	return g.ResponseWriter.Write(b)
}

func (g *gzipWriter) Flush() {
	if g.active {
		_ = g.zw.Flush()
	}
	_ = http.NewResponseController(g.ResponseWriter).Flush()
}

func (g *gzipWriter) Unwrap() http.ResponseWriter { return g.ResponseWriter }

func (g *gzipWriter) close() {
	if g.active {
		_ = g.zw.Close()
		gzPool.Put(g.zw)
	}
}

// gzipText compresses text responses (HTML, JSON, JS, CSS, SVG) for clients that accept it. Event streams and the
// machine link's NDJSON are never touched, and the machine link is bypassed entirely.
func gzipText(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if !strings.Contains(r.Header.Get("Accept-Encoding"), "gzip") || r.Method == http.MethodHead ||
			strings.Contains(r.URL.Path, "/agent/") || strings.HasSuffix(r.URL.Path, "/agent") {
			next.ServeHTTP(w, r)
			return
		}
		g := &gzipWriter{ResponseWriter: w}
		defer g.close()
		next.ServeHTTP(g, r)
	})
}

func encodeB64(s string) string { return base64.StdEncoding.EncodeToString([]byte(s)) }

func jsonMarshal(v any) ([]byte, error) { return json.Marshal(v) }

var inlineScript = regexp.MustCompile(`(?is)<script(?:\s[^>]*)?>(.+?)</script>`)

// inlineScriptHashes are the CSP hashes of the inline <script> blocks of index.html (the theme bootstrap), so the
// policy can stay without 'unsafe-inline'.
func inlineScriptHashes(fsys fs.FS) []string {
	raw, err := fs.ReadFile(fsys, "index.html")
	if err != nil {
		return nil
	}
	var out []string
	for _, m := range inlineScript.FindAllSubmatch(raw, -1) {
		if strings.Contains(strings.ToLower(string(m[0][:strings.Index(string(m[0]), ">")])), "src=") {
			continue
		}
		sum := sha256.Sum256(m[1])
		out = append(out, base64.StdEncoding.EncodeToString(sum[:]))
	}
	return out
}
