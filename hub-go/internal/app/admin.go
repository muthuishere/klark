package app

import (
	"crypto/rand"
	"encoding/hex"
	"errors"
	"net/http"
	"net/url"
	"strings"
	"time"

	"github.com/muthuishere/klark/hub-go/internal/auth"
	"github.com/muthuishere/klark/hub-go/internal/store"
	"github.com/muthuishere/klark/hub-go/internal/tokens"
)

func (a *App) adminUsers(w http.ResponseWriter, _ *http.Request) {
	rows, err := a.Store.Users()
	if err != nil {
		jsonErr(w, 500, err.Error())
		return
	}
	if rows == nil {
		rows = []store.User{}
	}
	jsonOut(w, 200, rows)
}

// adminSaveUser creates a user or updates it (password, display name, role). Users from HUB_USERS are reset to
// the environment on every start; the built-in admin cannot lose its admin role.
func (a *App) adminSaveUser(w http.ResponseWriter, r *http.Request) {
	var b struct {
		Username, DisplayName, Password string
		Admin                           bool
		Source                          string   // new users only: local (default) or mockldap
		Groups                          []string // mockldap users: directory groups; absent keeps them
	}
	if !decode(w, r, &b) {
		return
	}
	name := strings.TrimSpace(b.Username)
	if name == "" || len(name) > 64 || strings.ContainsAny(name, " \t:,=") {
		jsonErr(w, 400, "username must be 1-64 characters without spaces, ':', ',' or '='")
		return
	}
	cur, err := a.Store.User(name)
	exists := err == nil
	if !exists && len(b.Password) < 8 {
		jsonErr(w, 400, "password must be at least 8 characters")
		return
	}
	if exists && b.Password != "" && len(b.Password) < 8 {
		jsonErr(w, 400, "password must be at least 8 characters")
		return
	}
	if b.Source != "" && b.Source != "local" && b.Source != auth.SourceMock {
		jsonErr(w, 400, "source must be local or mockldap")
		return
	}
	for _, g := range b.Groups {
		if strings.TrimSpace(g) == "" || strings.Contains(g, "|") || len(g) > 128 {
			jsonErr(w, 400, "group names must be 1-128 characters without '|'")
			return
		}
	}
	u := store.User{Name: name, DisplayName: b.DisplayName, Admin: b.Admin, Source: "local", Groups: b.Groups}
	if b.Source != "" {
		u.Source = b.Source
	}
	if exists {
		u.Hash, u.Source = cur.Hash, cur.Source
		if b.Groups == nil {
			u.Groups = cur.Groups
		}
		if u.DisplayName == "" {
			u.DisplayName = cur.DisplayName
		}
		if cur.Source == "builtin" {
			u.Admin = true
		}
	}
	if b.Password != "" {
		if u.Hash, err = auth.HashPassword(b.Password); err != nil {
			jsonErr(w, 500, err.Error())
			return
		}
	}
	if err := a.Store.UpsertUser(u); err != nil {
		jsonErr(w, 500, err.Error())
		return
	}
	saved, _ := a.Store.User(name)
	jsonOut(w, http.StatusOK, saved)
}

func (a *App) adminDeleteUser(w http.ResponseWriter, r *http.Request) {
	name := r.PathValue("name")
	if me, _ := auth.FromContext(r.Context()); strings.EqualFold(me.Name, name) {
		jsonErr(w, 409, "you cannot delete yourself")
		return
	}
	if u, err := a.Store.User(name); err == nil && u.Source != "local" && u.Source != auth.SourceMock {
		jsonErr(w, 409, "this account comes from "+u.Source+" configuration and is recreated on start; change the environment instead")
		return
	}
	if err := a.Store.DeleteUser(name); errors.Is(err, store.ErrNotFound) {
		jsonErr(w, 404, "no such user")
		return
	} else if err != nil {
		jsonErr(w, 500, err.Error())
		return
	}
	w.WriteHeader(http.StatusNoContent)
}

type adminMachine struct {
	Name       string `json:"name"`
	Status     string `json:"status"` // online | offline | configured
	ApprovedBy string `json:"approvedBy"`
	Created    string `json:"created,omitempty"`
	Source     string `json:"source"` // env | admin
}

func (a *App) adminMachines(w http.ResponseWriter, _ *http.Request) {
	out := []adminMachine{}
	seen := map[string]bool{}
	status := func(name string) string {
		if a.machineOnline(name) {
			return "online"
		}
		return "offline"
	}
	for _, m := range a.Cfg.Machines {
		seen[strings.ToLower(m.Name)] = true
		out = append(out, adminMachine{Name: m.Name, Status: status(m.Name), ApprovedBy: "HUB_MACHINES", Source: "env"})
	}
	rows, _ := a.Store.Machines()
	for _, m := range rows {
		if seen[strings.ToLower(m.Name)] {
			continue
		}
		out = append(out, adminMachine{Name: m.Name, Status: status(m.Name), ApprovedBy: m.CreatedBy,
			Created: time.Unix(m.Created, 0).UTC().Format(time.RFC3339), Source: "admin"})
	}
	jsonOut(w, 200, out)
}

// adminAddMachine registers a machine and returns its token once (only the SHA-256 is stored).
func (a *App) adminAddMachine(w http.ResponseWriter, r *http.Request) {
	var b struct {
		Name  string
		Fleet bool
	}
	if !decode(w, r, &b) {
		return
	}
	name := strings.TrimSpace(b.Name)
	if name == "" || len(name) > 64 || strings.ContainsAny(name, " /:*,=") {
		jsonErr(w, 400, "name must be 1-64 characters without spaces, '/', ':', '*', ',' or '='")
		return
	}
	raw := make([]byte, 24)
	_, _ = rand.Read(raw)
	token := hex.EncodeToString(raw)
	me, _ := auth.FromContext(r.Context())
	ok, err := a.Store.AddMachine(store.Machine{Name: name, SHA256: tokens.Sha256Hex(token), Fleet: b.Fleet, CreatedBy: me.Name})
	if err != nil {
		jsonErr(w, 500, err.Error())
		return
	}
	if !ok {
		jsonErr(w, 409, "a machine with this name exists")
		return
	}
	a.Tokens.Invalidate()
	jsonOut(w, http.StatusCreated, map[string]string{"name": name, "token": token})
}

// adminRevokeMachine removes a stored machine's token and drops its session at once. Machines in HUB_MACHINES
// can only have their session dropped (the token comes from the environment).
func (a *App) adminRevokeMachine(w http.ResponseWriter, r *http.Request) {
	name := r.PathValue("id")
	err := a.Store.DeleteMachine(name)
	if err != nil && !errors.Is(err, store.ErrNotFound) {
		jsonErr(w, 500, err.Error())
		return
	}
	a.Tokens.Invalidate()
	revoked := err == nil
	for _, in := range a.Runtime.Machines() {
		if strings.EqualFold(in.ID, name) || strings.HasPrefix(strings.ToLower(in.ID), strings.ToLower(name)+":") {
			a.Runtime.Revoke(in.ID)
			revoked = true
		}
	}
	if a.Cluster != nil { // a machine linked to another pod is dropped there
		for _, pod := range a.Cluster.otherPodURLs() {
			_, _ = a.Cluster.podCall(r.Context(), pod, "POST", "/internal/revoke?machine="+url.QueryEscape(name))
		}
	}
	for _, m := range a.Cfg.Machines {
		if strings.EqualFold(m.Name, name) {
			revoked = true
		}
	}
	if !revoked {
		jsonErr(w, 404, "no such machine")
		return
	}
	w.WriteHeader(http.StatusNoContent)
}

// adminDirectory is the sign-in directory in force plus the redirect URIs of every OAuth/OIDC provider.
func (a *App) adminDirectory(w http.ResponseWriter, r *http.Request) {
	jsonOut(w, http.StatusOK, a.Auth.AdminView(r))
}

// adminPutDirectory stores the administrator's directory settings (they replace the environment's as a whole).
func (a *App) adminPutDirectory(w http.ResponseWriter, r *http.Request) {
	var in auth.DirectoryInput
	if !decode(w, r, &in) {
		return
	}
	if err := a.Auth.SaveDirectory(in); err != nil {
		jsonErr(w, 400, err.Error())
		return
	}
	a.afterDirectoryChange()
	a.adminDirectory(w, r)
}

// adminResetDirectory drops the stored settings so the environment applies again.
func (a *App) adminResetDirectory(w http.ResponseWriter, r *http.Request) {
	if err := a.Auth.ResetDirectory(); err != nil {
		jsonErr(w, 500, err.Error())
		return
	}
	a.afterDirectoryChange()
	a.adminDirectory(w, r)
}

func (a *App) afterDirectoryChange() {
	if a.Auth.DirectoryMode() == "mockldap" {
		_ = a.Auth.SeedMock()
	}
}

// adminTestDirectory signs a test user in against the given (or current) settings; nothing is stored.
func (a *App) adminTestDirectory(w http.ResponseWriter, r *http.Request) {
	var b struct {
		Settings *auth.DirectoryInput `json:"settings"`
		Username string               `json:"username"`
		Password string               `json:"password"`
	}
	if !decode(w, r, &b) {
		return
	}
	if b.Settings == nil && a.Auth.DirectoryMode() != "ldap" {
		jsonOut(w, 200, map[string]any{"ok": false, "message": "The directory is " + a.Auth.DirectoryMode() + ", not ldap; send settings to test an LDAP server."})
		return
	}
	ok, msg := a.Auth.TestDirectory(b.Settings, b.Username, b.Password)
	jsonOut(w, 200, map[string]any{"ok": ok, "message": msg})
}
