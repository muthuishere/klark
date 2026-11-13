package app

import (
	"context"
	"encoding/json"
	"errors"
	"io"
	"net/http"
	"sort"
	"strings"
	"time"

	"github.com/muthuishere/klark/hub-go/internal/auth"
	"github.com/muthuishere/klark/hub-go/internal/hub"
	"github.com/muthuishere/klark/hub-go/internal/hubchat"
	"github.com/muthuishere/klark/hub-go/internal/store"
)

func (a *App) mountAPI(mux *http.ServeMux) {
	user := func(f http.HandlerFunc) http.Handler { return auth.CSRF(a.Auth.Require(false, f)) }
	admin := func(f http.HandlerFunc) http.Handler { return auth.CSRF(a.Auth.Require(true, f)) }
	// Hub-wide settings: the operator only (an admin of the default tenant), not a tenant's admin.
	operator := func(f http.HandlerFunc) http.Handler {
		return admin(func(w http.ResponseWriter, r *http.Request) {
			if u, _ := auth.FromContext(r.Context()); !u.Operator() {
				jsonErr(w, http.StatusForbidden, "hub operator only")
				return
			}
			f(w, r)
		})
	}

	a.mountProvisioning(mux)
	mux.Handle("GET /api/machines", user(a.listMachines))
	mux.Handle("GET /api/machines/{id}/tree", user(a.getTree))
	mux.Handle("POST /api/machines/{id}/call", user(a.postCall))
	mux.Handle("GET /api/machines/{id}/events", user(a.machineEvents))
	mux.Handle("GET /api/machines/{id}/panes/{pane}/chat", user(a.getChat))
	mux.Handle("GET /api/machines/{id}/panes/{pane}/events", user(a.paneEvents))
	mux.Handle("POST /api/machines/{id}/panes/{pane}/input", user(a.postInput))
	mux.Handle("POST /api/machines/{id}/panes/{pane}/keys", user(a.postKeys))
	mux.Handle("POST /api/machines/{id}/panes/{pane}/send", user(a.postSend))
	mux.Handle("POST /api/machines/{id}/panes/{pane}/upload", user(a.postUpload))

	mux.Handle("GET /api/assets/{sha}", http.HandlerFunc(a.getAsset)) // machine token or cookie, checked inside

	// `klark hub connect` (ADR 0012): start and poll need no sign-in (rate limited); the decision does.
	mux.HandleFunc("POST /api/connect/start", a.connectStart)
	mux.HandleFunc("POST /api/connect/poll", a.connectPoll)
	mux.Handle("GET /api/connect/requests/{code}", user(a.connectGet))
	mux.Handle("POST /api/connect/requests/{code}/approve", user(a.connectDecide(true)))
	mux.Handle("POST /api/connect/requests/{code}/deny", user(a.connectDecide(false)))

	mux.Handle("GET /api/files/{id}", user(a.fileInfo))
	mux.Handle("GET /api/files/{id}/markdown", user(a.fileMarkdown))
	mux.Handle("GET /api/files/{id}/raw", user(a.fileRaw))

	mux.Handle("GET /api/agents", user(a.listAgents))
	mux.Handle("GET /api/messages", user(a.listMessages))
	mux.Handle("POST /api/messages", user(a.postMessage))

	mux.Handle("GET /api/admin/users", admin(a.adminUsers))
	mux.Handle("POST /api/admin/users", admin(a.adminSaveUser))
	mux.Handle("DELETE /api/admin/users/{name}", admin(a.adminDeleteUser))
	mux.Handle("GET /api/admin/machines", admin(a.adminMachines))
	mux.Handle("POST /api/admin/machines", admin(a.adminAddMachine))
	mux.Handle("DELETE /api/admin/machines/{id}", admin(a.adminRevokeMachine))
	mux.Handle("GET /api/admin/directory", operator(a.adminDirectory))
	mux.Handle("PUT /api/admin/directory", operator(a.adminPutDirectory))
	mux.Handle("DELETE /api/admin/directory", operator(a.adminResetDirectory))
	mux.Handle("POST /api/admin/directory/test", operator(a.adminTestDirectory))
	mux.Handle("GET /api/admin/analytics", operator(a.adminAnalytics))
	mux.Handle("GET /api/admin/storage", operator(a.adminStorage))
	mux.Handle("PUT /api/admin/storage", operator(a.adminPutStorage))
	mux.Handle("DELETE /api/admin/storage", operator(a.adminResetStorage))
	mux.Handle("POST /api/admin/storage/test", operator(a.adminTestStorage))
}

// failure turns an error from the machine link into an HTTP answer.
func failure(w http.ResponseWriter, err error) {
	var remote *hub.RemoteError
	switch {
	case errors.Is(err, hub.ErrOffline):
		jsonErr(w, http.StatusServiceUnavailable, "machine is offline")
	case errors.Is(err, hub.ErrTimeout), errors.Is(err, context.DeadlineExceeded):
		jsonErr(w, http.StatusGatewayTimeout, "the machine did not answer in time")
	case errors.Is(err, context.Canceled):
		jsonErr(w, http.StatusRequestTimeout, "request cancelled")
	case errors.As(err, &remote):
		jsonErr(w, http.StatusBadGateway, remote.Msg)
	default:
		jsonErr(w, http.StatusInternalServerError, err.Error())
	}
}

func decode(w http.ResponseWriter, r *http.Request, v any) bool {
	if err := json.NewDecoder(io.LimitReader(r.Body, 1<<20)).Decode(v); err != nil {
		jsonErr(w, http.StatusBadRequest, "invalid JSON body")
		return false
	}
	return true
}

type machineOut struct {
	ID       string  `json:"id"`
	Name     string  `json:"name"`
	Online   bool    `json:"online"`
	Version  string  `json:"version"`
	Protocol int     `json:"protocol"`
	LastSeen *string `json:"lastSeen"`
}

// knownMachine: a machine the hub has heard of or is configured to accept.
func (a *App) knownMachine(id string) bool {
	if _, ok := a.Runtime.Info(id); ok {
		return true
	}
	for _, m := range a.Tokens.Machines() {
		if !m.Fleet && strings.EqualFold(m.Name, id) {
			return true
		}
	}
	return false
}

// sees reports whether the request's user may see machine id (K1: tenants).
func (a *App) sees(r *http.Request, id string) bool {
	u, ok := auth.FromContext(r.Context())
	return ok && u.Sees(a.machineTenant(id))
}

// machineTenant is the tenant a machine id belongs to. Machines configured in
// HUB_MACHINES are always the default tenant's, whatever rows say; an id the
// hub has no record of belongs to no tenant ("": the operator only).
func (a *App) machineTenant(id string) string {
	for _, m := range a.Cfg.Machines {
		if strings.EqualFold(m.Name, id) {
			return store.DefaultTenant
		}
	}
	return a.Store.MachineTenant(id)
}

// forgetMachine drops a machine id's live link and every cache kept under it,
// so whoever registers the id next starts from nothing.
func (a *App) forgetMachine(id string) {
	a.Runtime.Forget(id)
	a.files.dropMachine(id)
	a.agentCache.Lock()
	a.agentCache.at, a.agentCache.val = time.Time{}, nil
	a.agentCache.Unlock()
}

// machineOr404 is the machine of the route, if it exists and is in the
// user's tenant; a machine of another tenant is "unknown", as if absent.
func (a *App) machineOr404(w http.ResponseWriter, r *http.Request) (string, bool) {
	id := r.PathValue("id")
	if !a.knownMachine(id) || !a.sees(r, id) {
		jsonErr(w, http.StatusNotFound, "unknown machine "+id)
		return "", false
	}
	return id, true
}

func rfc(t time.Time) *string {
	if t.IsZero() {
		return nil
	}
	s := t.UTC().Format(time.RFC3339)
	return &s
}

func (a *App) listMachines(w http.ResponseWriter, r *http.Request) {
	byID := map[string]*machineOut{}
	for _, m := range a.Tokens.Machines() {
		if !m.Fleet {
			byID[m.Name] = &machineOut{ID: m.Name, Name: m.Name}
		}
	}
	if rows, err := a.Store.Machines(); err == nil {
		for _, m := range rows {
			if m.Fleet {
				continue
			}
			o := byID[m.Name]
			if o == nil {
				o = &machineOut{ID: m.Name, Name: m.Name}
				byID[m.Name] = o
			}
			o.Version, o.Protocol = m.Version, m.Protocol
			if m.LastSeen > 0 {
				o.LastSeen = rfc(time.Unix(m.LastSeen, 0))
			}
		}
	}
	for _, in := range a.Runtime.Machines() {
		o := byID[in.ID]
		if o == nil {
			o = &machineOut{ID: in.ID}
			byID[in.ID] = o
		}
		o.Name = firstOf(in.Name, in.ID)
		o.Online, o.Version, o.Protocol = in.Online, in.Version, in.Protocol
		o.LastSeen = rfc(in.LastSeen)
	}
	out := make([]machineOut, 0, len(byID))
	for _, o := range byID {
		if a.sees(r, o.ID) {
			out = append(out, *o)
		}
	}
	sort.Slice(out, func(i, j int) bool { return strings.ToLower(out[i].ID) < strings.ToLower(out[j].ID) })
	jsonOut(w, http.StatusOK, out)
}

func firstOf(v ...string) string {
	for _, s := range v {
		if s != "" {
			return s
		}
	}
	return ""
}

// getTree answers with the machine's session.snapshot. The tree's revision is its ETag: a client that sends it
// back in If-None-Match gets 304, and the machine itself answers `tree_unchanged` to the hub when nothing moved.
func (a *App) getTree(w http.ResponseWriter, r *http.Request) {
	id, ok := a.machineOr404(w, r)
	if !ok {
		return
	}
	ctx, cancel := context.WithTimeout(r.Context(), 8*time.Second)
	defer cancel()
	t, err := a.Runtime.Tree(ctx, id)
	if err != nil {
		failure(w, err)
		return
	}
	etag := `"` + t.Rev + `"`
	w.Header().Set("ETag", etag)
	w.Header().Set("X-Tree-Rev", t.Rev)
	w.Header().Set("Cache-Control", "no-cache")
	if r.Header.Get("If-None-Match") == etag {
		w.WriteHeader(http.StatusNotModified)
		return
	}
	w.Header().Set("Content-Type", "application/json")
	_, _ = w.Write(t.Snapshot)
}

func (a *App) postCall(w http.ResponseWriter, r *http.Request) {
	id, ok := a.machineOr404(w, r)
	if !ok {
		return
	}
	var body struct {
		Method    string          `json:"method"`
		Params    json.RawMessage `json:"params"`
		TimeoutMs int             `json:"timeoutMs"`
	}
	if !decode(w, r, &body) {
		return
	}
	if !hub.AllowedCalls[body.Method] {
		jsonErr(w, http.StatusForbidden, "method not allowed: "+body.Method)
		return
	}
	timeout := 15 * time.Second
	if body.TimeoutMs > 0 && body.TimeoutMs <= 60000 {
		timeout = time.Duration(body.TimeoutMs) * time.Millisecond
	}
	var params any = map[string]any{}
	if len(body.Params) > 0 {
		params = body.Params
	}
	res, err := a.Runtime.Call(r.Context(), id, body.Method, params, timeout)
	if err != nil {
		failure(w, err)
		return
	}
	jsonOut(w, http.StatusOK, map[string]json.RawMessage{"result": res})
}

func (a *App) getChat(w http.ResponseWriter, r *http.Request) {
	id, ok := a.machineOr404(w, r)
	if !ok {
		return
	}
	feed := a.Runtime.Feed(id, r.PathValue("pane"))
	// Nobody is watching: one read now so a first paint is not empty (the poll itself runs only for watchers).
	if st := feed.State(); st.Rev == 0 && a.Runtime.Online(id) {
		ctx, cancel := context.WithTimeout(r.Context(), 12*time.Second)
		feed.PollOnce(ctx)
		cancel()
	}
	jsonOut(w, http.StatusOK, feed.State())
}

type inputBody struct {
	Text  string   `json:"text"`
	Keys  []string `json:"keys"`
	Enter bool     `json:"enter"`
}

const maxInput = 64 << 10

func (a *App) postInput(w http.ResponseWriter, r *http.Request) {
	id, ok := a.machineOr404(w, r)
	if !ok {
		return
	}
	var b inputBody
	if !decode(w, r, &b) {
		return
	}
	if b.Text == "" || len(b.Text) > maxInput {
		jsonErr(w, http.StatusBadRequest, "text must be 1 byte to 64 KB")
		return
	}
	pane := r.PathValue("pane")
	if err := a.Runtime.Input(id, pane, b.Text); err != nil {
		failure(w, err)
		return
	}
	if b.Enter {
		_ = a.Runtime.Keys(id, pane, []string{"enter"})
	}
	w.WriteHeader(http.StatusNoContent)
}

func (a *App) postKeys(w http.ResponseWriter, r *http.Request) {
	id, ok := a.machineOr404(w, r)
	if !ok {
		return
	}
	var b inputBody
	if !decode(w, r, &b) {
		return
	}
	if len(b.Keys) == 0 || len(b.Keys) > 32 {
		jsonErr(w, http.StatusBadRequest, "keys must hold 1 to 32 entries")
		return
	}
	if err := a.Runtime.Keys(id, r.PathValue("pane"), b.Keys); err != nil {
		failure(w, err)
		return
	}
	w.WriteHeader(http.StatusNoContent)
}

// postSend is what the chat's composer does: the text goes to the pane and is submitted. The hub's note is added
// (so agents name files by absolute path), except to slash commands. A slash command to an agent whose
// structured log never records local commands is chatted locally with the output read off the screen.
func (a *App) postSend(w http.ResponseWriter, r *http.Request) {
	id, ok := a.machineOr404(w, r)
	if !ok {
		return
	}
	var b inputBody
	if !decode(w, r, &b) {
		return
	}
	text := strings.TrimSpace(b.Text)
	if text == "" || len(text) > maxInput {
		jsonErr(w, http.StatusBadRequest, "text must be 1 byte to 64 KB")
		return
	}
	pane := r.PathValue("pane")
	feed := a.Runtime.Feed(id, pane)
	if hubchat.IsSlashCommand(text) && feed.State().Source != "jsonl" {
		if err := feed.SendLocalCommand(r.Context(), text); err != nil {
			failure(w, err)
			return
		}
		w.WriteHeader(http.StatusNoContent)
		return
	}
	if err := a.Runtime.Input(id, pane, hubchat.AppendNote(text, a.AgentNote)); err != nil {
		failure(w, err)
		return
	}
	_ = a.Runtime.Keys(id, pane, []string{"enter"})
	w.WriteHeader(http.StatusNoContent)
}
