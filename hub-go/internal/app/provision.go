package app

import (
	"context"
	"crypto/rand"
	"encoding/base32"
	"encoding/hex"
	"errors"
	"net/http"
	"regexp"
	"strings"
	"time"

	"github.com/google/uuid"

	"github.com/muthuishere/klark/hub-go/internal/auth"
	"github.com/muthuishere/klark/hub-go/internal/store"
	"github.com/muthuishere/klark/hub-go/internal/tokens"
)

// Provisioning API (K3): a trusted platform holding a service token creates
// tenants, issues one-time pairing codes for them, lists and revokes their
// machines, and deletes them. A token acts only on the tenants it created;
// any other tenant answers 404.

var tenantID = regexp.MustCompile(`^[a-z0-9][a-z0-9-]{1,39}$`)

const (
	pairingDefault = 15 * time.Minute
	pairingMax     = 24 * time.Hour
)

type svcKey struct{}

func (a *App) mountProvisioning(mux *http.ServeMux) {
	svc := func(f http.HandlerFunc) http.Handler { return a.requireService(f) }
	mux.Handle("POST /api/provision/tenants", svc(a.provCreateTenant))
	mux.Handle("DELETE /api/provision/tenants/{tenant}", svc(a.provDeleteTenant))
	mux.Handle("POST /api/provision/tenants/{tenant}/pairing-codes", svc(a.provPairingCode))
	mux.Handle("GET /api/provision/tenants/{tenant}/machines", svc(a.provMachines))
	mux.Handle("DELETE /api/provision/tenants/{tenant}/machines/{name}", svc(a.provRevokeMachine))
	// Redeemed by `klark hub connect --code`: no sign-in, the code is the credential.
	mux.HandleFunc("POST /api/connect/redeem", a.connectRedeem)

	admin := func(f http.HandlerFunc) http.Handler {
		return auth.CSRF(a.Auth.Require(true, http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			if u, _ := auth.FromContext(r.Context()); !u.Operator() {
				jsonErr(w, http.StatusForbidden, "hub operator only")
				return
			}
			f(w, r)
		})))
	}
	mux.Handle("GET /api/admin/service-tokens", admin(a.adminServiceTokens))
	mux.Handle("POST /api/admin/service-tokens", admin(a.adminAddServiceToken))
	mux.Handle("DELETE /api/admin/service-tokens/{id}", admin(a.adminRevokeServiceToken))
}

func (a *App) requireService(next http.HandlerFunc) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		h := r.Header.Get("Authorization")
		if len(h) < 8 || !strings.EqualFold(h[:7], "Bearer ") {
			jsonErr(w, http.StatusUnauthorized, "service token required (Authorization: Bearer)")
			return
		}
		t, err := a.Store.ServiceTokenBySHA(tokens.Sha256Hex(strings.TrimSpace(h[7:])))
		if err != nil {
			jsonErr(w, http.StatusUnauthorized, "unknown or revoked service token")
			return
		}
		next(w, r.WithContext(context.WithValue(r.Context(), svcKey{}, t)))
	})
}

func serviceOf(r *http.Request) store.ServiceToken {
	t, _ := r.Context().Value(svcKey{}).(store.ServiceToken)
	return t
}

// ownedTenant is the route's tenant when this service token created it.
func (a *App) ownedTenant(w http.ResponseWriter, r *http.Request) (string, bool) {
	id := strings.ToLower(r.PathValue("tenant"))
	if !a.Store.TenantOwnedBy(id, serviceOf(r).ID) {
		jsonErr(w, http.StatusNotFound, "no such tenant")
		return "", false
	}
	return id, true
}

func (a *App) provCreateTenant(w http.ResponseWriter, r *http.Request) {
	var b struct{ ID, Name string }
	if !decode(w, r, &b) {
		return
	}
	id := strings.ToLower(strings.TrimSpace(b.ID))
	if !tenantID.MatchString(id) || id == store.DefaultTenant {
		jsonErr(w, http.StatusBadRequest, "id must be 2-40 characters: a-z, 0-9 and '-', starting with a letter or digit (not 'default')")
		return
	}
	ok, err := a.Store.CreateOwnedTenant(store.Tenant{ID: id, Name: strings.TrimSpace(b.Name)}, serviceOf(r).ID)
	if err != nil {
		jsonErr(w, 500, "could not create the tenant")
		return
	}
	if !ok {
		jsonErr(w, http.StatusConflict, "a tenant with this id exists")
		return
	}
	jsonOut(w, http.StatusCreated, map[string]string{"id": id, "name": b.Name})
}

func (a *App) provDeleteTenant(w http.ResponseWriter, r *http.Request) {
	id, ok := a.ownedTenant(w, r)
	if !ok {
		return
	}
	machines, _ := a.Store.MachinesOf(id)
	if err := a.Store.DeleteTenant(id); err != nil {
		jsonErr(w, 500, "could not delete the tenant")
		return
	}
	a.Tokens.Invalidate()
	for _, m := range machines {
		a.dropMachine(m.Name)
	}
	w.WriteHeader(http.StatusNoContent)
}

// provPairingCode issues a one-time code a machine redeems with
// `klark hub connect --url <hub> --code <code>`; the machine joins this tenant.
func (a *App) provPairingCode(w http.ResponseWriter, r *http.Request) {
	id, ok := a.ownedTenant(w, r)
	if !ok {
		return
	}
	var b struct {
		TTLSeconds int `json:"ttl_seconds"`
	}
	if r.ContentLength > 0 && !decode(w, r, &b) {
		return
	}
	ttl := pairingDefault
	if b.TTLSeconds > 0 {
		ttl = min(time.Duration(b.TTLSeconds)*time.Second, pairingMax)
	}
	code := newPairingCode()
	expires := time.Now().Add(ttl)
	if err := a.Store.AddPairingCode(tokens.Sha256Hex(code), id, expires.UnixMilli()); err != nil {
		jsonErr(w, 500, "could not store the code")
		return
	}
	out := map[string]any{"code": code, "tenant": id, "expires": expires.UTC().Format(time.RFC3339)}
	if base := a.Cfg.PublicURL; base != "" {
		out["connect"] = "klark hub connect --url " + base + " --code " + code
	}
	jsonOut(w, http.StatusCreated, out)
}

func (a *App) provMachines(w http.ResponseWriter, r *http.Request) {
	id, ok := a.ownedTenant(w, r)
	if !ok {
		return
	}
	rows, err := a.Store.MachinesOf(id)
	if err != nil {
		jsonErr(w, 500, "could not list machines")
		return
	}
	type out struct {
		Name     string  `json:"name"`
		Online   bool    `json:"online"`
		Version  string  `json:"version"`
		LastSeen *string `json:"lastSeen"`
	}
	list := []out{}
	for _, m := range rows {
		o := out{Name: m.Name, Online: a.Runtime.Online(m.Name), Version: m.Version}
		if m.LastSeen > 0 {
			o.LastSeen = rfc(time.Unix(m.LastSeen, 0))
		}
		list = append(list, o)
	}
	jsonOut(w, http.StatusOK, list)
}

func (a *App) provRevokeMachine(w http.ResponseWriter, r *http.Request) {
	id, ok := a.ownedTenant(w, r)
	if !ok {
		return
	}
	name := r.PathValue("name")
	if !strings.EqualFold(a.Store.MachineTenant(name), id) {
		jsonErr(w, http.StatusNotFound, "no such machine")
		return
	}
	if err := a.Store.DeleteMachine(name); err != nil && !errors.Is(err, store.ErrNotFound) {
		jsonErr(w, 500, "could not revoke the machine")
		return
	}
	a.Tokens.Invalidate()
	a.dropMachine(name)
	w.WriteHeader(http.StatusNoContent)
}

// dropMachine ends a machine's live link (and its fleet members') at once.
func (a *App) dropMachine(name string) {
	for _, in := range a.Runtime.Machines() {
		if strings.EqualFold(in.ID, name) || strings.HasPrefix(strings.ToLower(in.ID), strings.ToLower(name)+":") {
			a.Runtime.Revoke(in.ID)
		}
	}
}

// connectRedeem trades a pairing code for a machine token, once. The machine
// keeps its name unless another tenant has it.
func (a *App) connectRedeem(w http.ResponseWriter, r *http.Request) {
	var b struct{ Code, Name string }
	if !decode(w, r, &b) {
		return
	}
	name := strings.TrimSpace(b.Name)
	if name == "" || len(name) > 64 || strings.ContainsAny(name, " /:*,=") {
		jsonErr(w, http.StatusBadRequest, "name must be 1-64 characters without spaces, '/', ':', '*', ',' or '='")
		return
	}
	for _, m := range a.Cfg.Machines {
		if strings.EqualFold(m.Name, name) {
			jsonErr(w, http.StatusConflict, name+" is taken on this hub; pick another name (KLARK_MACHINE_NAME)")
			return
		}
	}
	codeSHA := tokens.Sha256Hex(strings.ToUpper(strings.TrimSpace(b.Code)))
	if m := a.storedMachine(name); m != nil {
		// Same name in another tenant (or a fleet): refuse before using up the code.
		if m.Fleet || !a.Store.PairingCodeTenantIs(codeSHA, m.Tenant) {
			jsonErr(w, http.StatusConflict, name+" is taken on this hub; pick another name (KLARK_MACHINE_NAME)")
			return
		}
	}
	tenant, err := a.Store.TakePairingCode(codeSHA, time.Now().UnixMilli())
	if err != nil {
		jsonErr(w, http.StatusForbidden, "unknown, used or expired pairing code")
		return
	}
	raw := make([]byte, 24)
	_, _ = rand.Read(raw)
	token := hex.EncodeToString(raw)
	if err := a.Store.UpsertMachine(name, tokens.Sha256Hex(token), "provisioning", tenant); err != nil {
		jsonErr(w, 500, "could not store the machine")
		return
	}
	a.Tokens.Invalidate()
	jsonOut(w, http.StatusOK, map[string]string{"token": token, "machine": name, "tenant": tenant})
}

// newPairingCode: 80 random bits as XXXX-XXXX-XXXX-XXXX (base32, no padding).
func newPairingCode() string {
	raw := make([]byte, 10)
	_, _ = rand.Read(raw)
	s := base32.StdEncoding.WithPadding(base32.NoPadding).EncodeToString(raw)
	return s[0:4] + "-" + s[4:8] + "-" + s[8:12] + "-" + s[12:16]
}

// ---- operator: service tokens ----------------------------------------------

func (a *App) adminServiceTokens(w http.ResponseWriter, _ *http.Request) {
	rows, err := a.Store.ServiceTokens()
	if err != nil {
		jsonErr(w, 500, err.Error())
		return
	}
	jsonOut(w, http.StatusOK, rows)
}

// adminAddServiceToken mints a service token and returns it once.
func (a *App) adminAddServiceToken(w http.ResponseWriter, r *http.Request) {
	var b struct{ Name string }
	if !decode(w, r, &b) {
		return
	}
	name := strings.TrimSpace(b.Name)
	if name == "" || len(name) > 64 {
		jsonErr(w, http.StatusBadRequest, "name must be 1-64 characters")
		return
	}
	raw := make([]byte, 32)
	_, _ = rand.Read(raw)
	token := "khs_" + hex.EncodeToString(raw)
	me, _ := auth.FromContext(r.Context())
	t := store.ServiceToken{ID: uuid.NewString(), Name: name, SHA256: tokens.Sha256Hex(token), CreatedBy: me.Name}
	if err := a.Store.AddServiceToken(t); err != nil {
		jsonErr(w, 500, "could not store the token")
		return
	}
	jsonOut(w, http.StatusCreated, map[string]string{"id": t.ID, "name": name, "token": token})
}

func (a *App) adminRevokeServiceToken(w http.ResponseWriter, r *http.Request) {
	if err := a.Store.RevokeServiceToken(r.PathValue("id")); errors.Is(err, store.ErrNotFound) {
		jsonErr(w, http.StatusNotFound, "no such service token")
		return
	} else if err != nil {
		jsonErr(w, 500, err.Error())
		return
	}
	w.WriteHeader(http.StatusNoContent)
}
