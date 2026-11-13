package app

import (
	"crypto/rand"
	"encoding/hex"
	"errors"
	"net/http"
	"strings"
	"time"

	"github.com/muthuishere/klark/hub-go/internal/auth"
	"github.com/muthuishere/klark/hub-go/internal/store"
	"github.com/muthuishere/klark/hub-go/internal/tokens"
)

func (a *App) adminUsers(w http.ResponseWriter, r *http.Request) {
	me, _ := auth.FromContext(r.Context())
	all, err := a.Store.Users()
	if err != nil {
		jsonErr(w, 500, err.Error())
		return
	}
	rows := []store.User{}
	for _, u := range all {
		if me.Sees(u.Tenant) {
			rows = append(rows, u)
		}
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
		Tenant                          string   // the operator only: which tenant; others create in their own
		Role                            string   // member (default) or viewer (read-only); absent keeps it
	}
	if !decode(w, r, &b) {
		return
	}
	name := strings.TrimSpace(b.Username)
	if name == "" || len(name) > 64 || strings.ContainsAny(name, " \t:,=") {
		jsonErr(w, 400, "username must be 1-64 characters without spaces, ':', ',' or '='")
		return
	}
	me, _ := auth.FromContext(r.Context())
	cur, err := a.Store.User(name)
	exists := err == nil
	if exists && !me.Sees(cur.Tenant) {
		jsonErr(w, 409, "this username is not available")
		return
	}
	tenant, ok := a.adminTenant(w, me, b.Tenant)
	if !ok {
		return
	}
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
	if b.Role != "" && b.Role != "member" && b.Role != "viewer" {
		jsonErr(w, 400, "role must be member or viewer")
		return
	}
	u := store.User{Name: name, DisplayName: b.DisplayName, Admin: b.Admin, Source: "local", Groups: b.Groups, Tenant: tenant, Role: b.Role}
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
		if b.Tenant == "" {
			u.Tenant = cur.Tenant
		}
		if b.Role == "" {
			u.Role = cur.Role
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
	me, _ := auth.FromContext(r.Context())
	if strings.EqualFold(me.Name, name) {
		jsonErr(w, 409, "you cannot delete yourself")
		return
	}
	if u, err := a.Store.User(name); err == nil && !me.Sees(u.Tenant) {
		jsonErr(w, 404, "no such user")
		return
	} else if err == nil && u.Source != "local" && u.Source != auth.SourceMock {
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

func (a *App) adminMachines(w http.ResponseWriter, r *http.Request) {
	me, _ := auth.FromContext(r.Context())
	out := []adminMachine{}
	seen := map[string]bool{}
	status := func(name string) string {
		if a.Runtime.Online(name) {
			return "online"
		}
		return "offline"
	}
	for _, m := range a.Cfg.Machines {
		seen[strings.ToLower(m.Name)] = true
		if !me.Sees(store.DefaultTenant) {
			continue
		}
		out = append(out, adminMachine{Name: m.Name, Status: status(m.Name), ApprovedBy: "HUB_MACHINES", Source: "env"})
	}
	rows, _ := a.Store.Machines()
	for _, m := range rows {
		if seen[strings.ToLower(m.Name)] || !me.Sees(m.Tenant) {
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
		Name   string
		Fleet  bool
		Tenant string // the operator only
	}
	if !decode(w, r, &b) {
		return
	}
	name := strings.TrimSpace(b.Name)
	if name == "" || len(name) > 64 || strings.ContainsAny(name, " /:*,=") {
		jsonErr(w, 400, "name must be 1-64 characters without spaces, '/', ':', '*', ',' or '='")
		return
	}
	for _, m := range a.Cfg.Machines {
		if strings.EqualFold(m.Name, name) {
			jsonErr(w, 409, "a machine with this name exists")
			return
		}
	}
	me, _ := auth.FromContext(r.Context())
	tenant, ok := a.adminTenant(w, me, b.Tenant)
	if !ok {
		return
	}
	raw := make([]byte, 24)
	_, _ = rand.Read(raw)
	token := hex.EncodeToString(raw)
	ok, err := a.Store.AddMachine(store.Machine{Name: name, SHA256: tokens.Sha256Hex(token), Fleet: b.Fleet, CreatedBy: me.Name, Tenant: tenant})
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
	if me, _ := auth.FromContext(r.Context()); !me.Sees(a.machineTenant(name)) {
		jsonErr(w, 404, "no such machine")
		return
	}
	err := a.Store.DeleteMachine(name)
	if err != nil && !errors.Is(err, store.ErrNotFound) {
		jsonErr(w, 500, err.Error())
		return
	}
	a.Tokens.Invalidate()
	revoked := err == nil
	for _, in := range a.Runtime.Machines() {
		if strings.EqualFold(in.ID, name) || strings.HasPrefix(strings.ToLower(in.ID), strings.ToLower(name)+":") {
			revoked = true
		}
	}
	a.forgetMachine(name)
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

// adminTenant is the tenant an admin request acts in: the admin's own, or for
// the operator the one it names (which must exist).
func (a *App) adminTenant(w http.ResponseWriter, me auth.User, asked string) (string, bool) {
	asked = strings.TrimSpace(asked)
	if asked == "" || strings.EqualFold(asked, me.Tenant) {
		return me.Tenant, true
	}
	if !me.Operator() {
		jsonErr(w, http.StatusForbidden, "you can only manage your own tenant")
		return "", false
	}
	if !a.Store.TenantExists(asked) {
		jsonErr(w, http.StatusBadRequest, "no such tenant "+asked)
		return "", false
	}
	return asked, true
}
