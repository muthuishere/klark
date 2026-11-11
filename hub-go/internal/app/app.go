// Package app assembles the hub: config, storage, auth, machine link, API and the embedded UI, behind one
// handler mounted under HUB_BASE_PATH.
package app

import (
	"io/fs"
	"log"
	"net/http"
	"strings"
	"sync"
	"time"

	"github.com/muthuishere/klark/hub-go/internal/auth"
	"github.com/muthuishere/klark/hub-go/internal/chat"
	"github.com/muthuishere/klark/hub-go/internal/config"
	"github.com/muthuishere/klark/hub-go/internal/hub"
	"github.com/muthuishere/klark/hub-go/internal/link"
	"github.com/muthuishere/klark/hub-go/internal/server"
	"github.com/muthuishere/klark/hub-go/internal/store"
	"github.com/muthuishere/klark/hub-go/internal/tokens"
	"github.com/muthuishere/klark/hub-go/internal/ui"
)

// Version is set at build time (-ldflags "-X .../app.Version=...").
var Version = "dev"

// App is a running hub.
type App struct {
	Cfg      config.Config
	Store    *store.Store
	Auth     *auth.Service
	Registry *link.Registry
	Runtime  *hub.Runtime
	Tokens   *tokenSource
	// UI is the built app; nil uses the one embedded in the binary.
	UI fs.FS
	// AgentNote is appended to what people send from the hub (see chat.AppendNote); empty turns it off.
	AgentNote string

	files        *fileCache
	scriptHashes []string
	mux          *http.ServeMux
}

// New opens the database, seeds accounts and wires every route.
func New(cfg config.Config, uiFS fs.FS) (*App, error) {
	st, err := store.Open(cfg.DataDir)
	if err != nil {
		return nil, err
	}
	key, err := auth.LoadKey(cfg.SessionKey, cfg.DataDir)
	if err != nil {
		return nil, err
	}
	a := &App{Cfg: cfg, Store: st, UI: uiFS, AgentNote: chat.DefaultNote, files: newFileCache()}
	a.Auth = auth.New(st, cfg, auth.NewSigner(key))
	if err := a.Auth.Seed(); err != nil {
		return nil, err
	}
	if cfg.MockADFS {
		m, err := auth.NewMockADFS(cfg.MockUsers, func(r *http.Request) string { return a.Auth.ExternalBase(r) + "/mock-adfs" })
		if err != nil {
			return nil, err
		}
		a.Auth.Mock = m
		log.Printf("mock ADFS enabled at %s/mock-adfs (development only)", cfg.BasePath)
	}
	a.Registry = link.NewRegistry()
	a.Runtime = hub.New(a.Registry)
	a.Runtime.OnSeen = func(id, version string, protocol int) {
		log.Printf("machine %s connected (protocol %d, klark %s)", id, protocol, version)
		st.TouchMachine(id, version, protocol)
	}
	a.Tokens = &tokenSource{cfg: cfg, st: st}
	a.build()
	return a, nil
}

// Close releases the database.
func (a *App) Close() error { return a.Store.Close() }

func (a *App) build() {
	mux := http.NewServeMux()
	(&server.Server{Tokens: a.Tokens, Registry: a.Registry}).Mount(mux)
	a.Auth.Mount(mux)
	if a.Auth.Mock != nil {
		a.Auth.Mock.Mount(mux)
	}
	a.mountAPI(mux)
	mux.HandleFunc("/api/", func(w http.ResponseWriter, _ *http.Request) { jsonErr(w, http.StatusNotFound, "no such API route") })
	mux.HandleFunc("/agent/", func(w http.ResponseWriter, _ *http.Request) { jsonErr(w, http.StatusNotFound, "no such route") })
	uiFS := a.UI
	if uiFS == nil {
		uiFS = ui.Embedded()
	}
	a.scriptHashes = inlineScriptHashes(uiFS)
	mux.Handle("/", ui.HandlerFS(uiFS, a.Cfg.BasePath))
	a.mux = mux
	a.Auth.Local = mux
}

// Handler is the whole hub: the base path, security headers and gzip around the routes.
func (a *App) Handler() http.Handler {
	base := a.Cfg.BasePath
	var h http.Handler = a.mux
	if base != "" {
		inner := h
		h = http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			switch {
			case r.URL.Path == base:
				http.Redirect(w, r, base+"/", http.StatusMovedPermanently)
			case strings.HasPrefix(r.URL.Path, base+"/"):
				r2 := r.Clone(r.Context())
				r2.URL.Path = strings.TrimPrefix(r.URL.Path, base)
				r2.URL.RawPath = ""
				inner.ServeHTTP(w, r2)
			case r.URL.Path == "/healthz": // the container healthcheck needs no knowledge of the prefix
				inner.ServeHTTP(w, r)
			default:
				http.NotFound(w, r)
			}
		})
	}
	return securityHeaders(a.scriptHashes, gzipText(h))
}

// ---- machine tokens: HUB_MACHINES plus the machines table, cached ----------------------

type tokenSource struct {
	cfg config.Config
	st  *store.Store

	mu      sync.Mutex
	cached  []tokens.Machine
	fetched time.Time
}

func (t *tokenSource) Machines() []tokens.Machine {
	t.mu.Lock()
	defer t.mu.Unlock()
	if t.cached != nil && time.Since(t.fetched) < 5*time.Second {
		return t.cached
	}
	var out []tokens.Machine
	for _, m := range t.cfg.Machines {
		out = append(out, tokens.Machine{Name: m.Name, TokenSHA256: m.SHA256, Fleet: m.Fleet})
	}
	if rows, err := t.st.Machines(); err == nil {
		for _, m := range rows {
			out = append(out, tokens.Machine{Name: m.Name, TokenSHA256: m.SHA256, Fleet: m.Fleet})
		}
	} else {
		log.Printf("machines: %v", err)
	}
	t.cached, t.fetched = out, time.Now()
	return out
}

// Invalidate makes the next request read the table again (after an admin change).
func (t *tokenSource) Invalidate() {
	t.mu.Lock()
	t.cached = nil
	t.mu.Unlock()
}
