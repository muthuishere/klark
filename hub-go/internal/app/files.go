package app

import (
	"context"
	"encoding/base64"
	"encoding/json"
	"net/http"
	"net/url"
	"path"
	"strings"
	"sync"
	"time"

	"github.com/muthuishere/klark/hub-go/internal/chat"
)

const (
	maxFileBytes = 10 << 20
	fileCacheTTL = 2 * time.Minute
	fileBudget   = 64 << 20
)

type cachedFile struct {
	bytes      []byte
	name, path string
	at         time.Time
}

type fileCache struct {
	mu sync.Mutex
	m  map[string]cachedFile
}

func newFileCache() *fileCache { return &fileCache{m: map[string]cachedFile{}} }

func (c *fileCache) get(k string) (cachedFile, bool) {
	c.mu.Lock()
	defer c.mu.Unlock()
	f, ok := c.m[k]
	return f, ok && time.Since(f.at) < fileCacheTTL
}

func (c *fileCache) put(k string, f cachedFile) {
	c.mu.Lock()
	defer c.mu.Unlock()
	c.m[k] = f
	total := 0
	for key, v := range c.m {
		if time.Since(v.at) > fileCacheTTL {
			delete(c.m, key)
			continue
		}
		total += len(v.bytes)
	}
	for total > fileBudget && len(c.m) > 1 { // drop the oldest
		var oldK string
		var oldT time.Time
		for key, v := range c.m {
			if oldK == "" || v.at.Before(oldT) {
				oldK, oldT = key, v.at
			}
		}
		total -= len(c.m[oldK].bytes)
		delete(c.m, oldK)
	}
}

// fetchFile gets a file a chat linked to from the machine (get_file, protocol 5). The hub refuses
// credential-shaped and out-of-folder paths first, and files over 10 MB; the machine checks again.
func (a *App) fetchFile(w http.ResponseWriter, r *http.Request) (cachedFile, bool) {
	id, ok := a.machineOr404(w, r)
	if !ok {
		return cachedFile{}, false
	}
	pane, p := r.URL.Query().Get("pane"), r.URL.Query().Get("path")
	if pane == "" || p == "" {
		jsonErr(w, 400, "pane and path are required")
		return cachedFile{}, false
	}
	cwd := ""
	if pi, found := a.Runtime.Pane(id, pane); found {
		cwd = pi.Cwd
	}
	if why := chat.Refusal(p, cwd); why != "" {
		jsonErr(w, 403, why)
		return cachedFile{}, false
	}
	key := id + "\n" + pane + "\n" + p
	if f, hit := a.files.get(key); hit {
		return f, true
	}
	info, _ := a.Runtime.Info(id)
	if info.Online && info.Protocol < 5 {
		jsonErr(w, 409, "machine "+id+" links with protocol "+itoa(info.Protocol)+"; update klark there to open files")
		return cachedFile{}, false
	}
	ctx, cancel := context.WithTimeout(r.Context(), 30*time.Second)
	defer cancel()
	raw, err := a.Runtime.Request(ctx, id, map[string]any{"type": "get_file", "pane_id": pane, "path": p, "max_bytes": maxFileBytes}, 30*time.Second)
	if err != nil {
		if re, isRemote := err.(interface{ Error() string }); isRemote && strings.Contains(strings.ToLower(re.Error()), "limit") {
			jsonErr(w, 413, re.Error())
			return cachedFile{}, false
		}
		failure(w, err)
		return cachedFile{}, false
	}
	var res struct{ Name, Path, Data string }
	_ = json.Unmarshal(raw, &res)
	data, err := base64.StdEncoding.DecodeString(res.Data)
	if err != nil {
		jsonErr(w, 502, "the machine sent a malformed file")
		return cachedFile{}, false
	}
	if len(data) > maxFileBytes {
		jsonErr(w, 413, "file is "+chat.FileSize(int64(len(data)))+"; limit 10 MB")
		return cachedFile{}, false
	}
	f := cachedFile{bytes: data, name: firstOf(res.Name, path.Base(p)), path: firstOf(res.Path, p), at: time.Now()}
	a.files.put(key, f)
	return f, true
}

func itoa(n int) string { b, _ := json.Marshal(n); return string(b) }

func countLines(b []byte) int {
	if len(b) == 0 {
		return 0
	}
	n := strings.Count(string(b), "\n")
	if b[len(b)-1] != '\n' {
		n++
	}
	return n
}

func (a *App) fileInfo(w http.ResponseWriter, r *http.Request) {
	f, ok := a.fetchFile(w, r)
	if !ok {
		return
	}
	k := chat.DetectFile(f.name, f.bytes)
	lines := 0
	if k.View != "image" && k.View != "pdf" && k.View != "binary" {
		lines = countLines(f.bytes)
	}
	q := url.Values{"pane": {r.URL.Query().Get("pane")}, "path": {r.URL.Query().Get("path")}}.Encode()
	raw := "api/files/" + url.PathEscape(r.PathValue("id")) + "/raw?" + q
	jsonOut(w, 200, map[string]any{"path": f.path, "name": f.name, "view": k.View, "lang": k.Lang, "mime": k.Mime,
		"size": len(f.bytes), "size_label": chat.FileSize(int64(len(f.bytes))), "lines": lines, "raw": raw, "download": raw + "&download=1"})
}

func (a *App) fileMarkdown(w http.ResponseWriter, r *http.Request) {
	f, ok := a.fetchFile(w, r)
	if !ok {
		return
	}
	jsonOut(w, 200, map[string]string{"html": chat.SegmentsHTML(chat.RenderMarkdown(string(f.bytes), false))})
}

// fileRaw serves the bytes, sandboxed by CSP: whatever a file holds, it can never run in the hub's origin.
func (a *App) fileRaw(w http.ResponseWriter, r *http.Request) {
	f, ok := a.fetchFile(w, r)
	if !ok {
		return
	}
	k := chat.DetectFile(f.name, f.bytes)
	download := r.URL.Query().Get("download") != ""
	h := w.Header()
	h.Set("X-Content-Type-Options", "nosniff")
	if k.View == "pdf" && !download {
		h.Set("Content-Security-Policy", "default-src 'none'; object-src 'self'")
	} else {
		h.Set("Content-Security-Policy", "default-src 'none'; img-src 'self' data:; style-src 'unsafe-inline'; sandbox")
	}
	h.Set("Cache-Control", "private, max-age=60")
	switch {
	case download || k.View == "binary":
		h.Set("Content-Type", "application/octet-stream")
		h.Set("Content-Disposition", `attachment; filename="`+strings.NewReplacer(`"`, "", "\r", "", "\n", "", "/", "_", `\`, "_").Replace(path.Base(f.name))+`"`)
	case k.View == "image" || k.View == "pdf":
		h.Set("Content-Type", k.Mime)
	default:
		h.Set("Content-Type", "text/plain; charset=utf-8")
	}
	_, _ = w.Write(f.bytes)
}
