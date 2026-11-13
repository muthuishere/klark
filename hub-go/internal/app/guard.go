package app

import (
	"net/http"
	"os"
	"strconv"
	"strings"
	"sync"
	"time"

	"github.com/muthuishere/klark/hub-go/internal/auth"
	"github.com/muthuishere/klark/hub-go/internal/store"
)

// Actions through the hub (typing, keys, send, upload, messages) and file
// reads: a read-only user may not act, each user and each machine has an
// action budget per minute, and every one leaves an audit row (no content).

const (
	defaultUserActionsPerMinute    = 120
	defaultMachineActionsPerMinute = 240
)

// window counts events per key in a sliding minute.
type window struct {
	mu   sync.Mutex
	seen map[string][]time.Time
}

func (w *window) allow(key string, max int, now time.Time) bool {
	w.mu.Lock()
	defer w.mu.Unlock()
	if w.seen == nil {
		w.seen = map[string][]time.Time{}
	}
	cut := now.Add(-time.Minute)
	kept := w.seen[key][:0]
	for _, t := range w.seen[key] {
		if t.After(cut) {
			kept = append(kept, t)
		}
	}
	if len(kept) >= max {
		w.seen[key] = kept
		return false
	}
	w.seen[key] = append(kept, now)
	return true
}

type statusWriter struct {
	http.ResponseWriter
	status int
	bytes  int64
}

func (s *statusWriter) WriteHeader(code int) { s.status = code; s.ResponseWriter.WriteHeader(code) }
func (s *statusWriter) Write(b []byte) (int, error) {
	if s.status == 0 {
		s.status = http.StatusOK
	}
	n, err := s.ResponseWriter.Write(b)
	s.bytes += int64(n)
	return n, err
}
func (s *statusWriter) Flush() {
	if f, ok := s.ResponseWriter.(http.Flusher); ok {
		f.Flush()
	}
}

func actorOf(u auth.User) string { return u.Name }

// acting wraps a route that changes something on a machine.
func (a *App) acting(action string, f http.HandlerFunc) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		u, _ := auth.FromContext(r.Context())
		machine := r.PathValue("id")
		if u.ReadOnly() {
			a.audit(u, machine, r.PathValue("pane"), action, http.StatusForbidden, 0)
			jsonErr(w, http.StatusForbidden, "read-only access: this account can watch but not type or send")
			return
		}
		// A machine the caller cannot see is "unknown" (404) before anything is
		// charged: nobody spends another tenant's machine budget.
		if machine != "" && (!a.knownMachine(machine) || !a.sees(r, machine)) {
			jsonErr(w, http.StatusNotFound, "unknown machine "+machine)
			return
		}
		now := time.Now()
		if !a.actions.allow("u:"+strings.ToLower(u.Tenant+"/"+u.Name), a.userActionLimit(), now) ||
			(machine != "" && !a.actions.allow("m:"+strings.ToLower(machine), a.machineActionLimit(), now)) {
			a.audit(u, machine, r.PathValue("pane"), action, http.StatusTooManyRequests, 0)
			w.Header().Set("Retry-After", "60")
			jsonErr(w, http.StatusTooManyRequests, "too many actions; try again in a minute")
			return
		}
		sw := &statusWriter{ResponseWriter: w}
		f(sw, r)
		a.audit(u, machine, r.PathValue("pane"), action, statusOr200(sw.status), max(r.ContentLength, 0))
	}
}

// reading wraps a route that hands out a file's content.
func (a *App) reading(action string, f http.HandlerFunc) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		u, _ := auth.FromContext(r.Context())
		sw := &statusWriter{ResponseWriter: w}
		f(sw, r)
		target := r.PathValue("id")
		if target == "" {
			target = r.PathValue("sha")
		}
		a.audit(u, r.URL.Query().Get("machine"), target, action, statusOr200(sw.status), sw.bytes)
	}
}

// audit records under the machine's tenant, so that tenant's admin sees every
// action on its machines (the operator's too), and also under the actor's
// tenant when that is a different one.
func (a *App) audit(u auth.User, machine, target, action string, status int, bytes int64) {
	row := store.AuditRow{Actor: actorOf(u), Machine: machine, Target: target, Action: action, Status: status, Bytes: bytes}
	tenants := []string{u.Tenant}
	if machine != "" {
		if mt := a.machineTenant(machine); mt != "" && !strings.EqualFold(mt, u.Tenant) {
			tenants = []string{mt, u.Tenant}
		}
	}
	for _, t := range tenants {
		row.Tenant = t
		_ = a.Store.Audit(row)
	}
}

func statusOr200(s int) int {
	if s == 0 {
		return http.StatusOK
	}
	return s
}

func (a *App) userActionLimit() int {
	return envInt("HUB_USER_ACTIONS_PER_MINUTE", defaultUserActionsPerMinute)
}
func (a *App) machineActionLimit() int {
	return envInt("HUB_MACHINE_ACTIONS_PER_MINUTE", defaultMachineActionsPerMinute)
}

// adminAudit lists the newest audit rows of the caller's tenant (all for the operator).
func (a *App) adminAudit(w http.ResponseWriter, r *http.Request) {
	me, _ := auth.FromContext(r.Context())
	limit, _ := strconv.Atoi(r.URL.Query().Get("limit"))
	if limit < 1 || limit > 1000 {
		limit = 200
	}
	tenant := me.Tenant
	if me.Operator() {
		tenant = ""
	}
	rows, err := a.Store.AuditRows(limit, tenant)
	if err != nil {
		jsonErr(w, 500, "could not read the audit log")
		return
	}
	jsonOut(w, http.StatusOK, rows)
}

func envInt(name string, def int) int {
	if n, err := strconv.Atoi(strings.TrimSpace(os.Getenv(name))); err == nil && n > 0 {
		return n
	}
	return def
}
