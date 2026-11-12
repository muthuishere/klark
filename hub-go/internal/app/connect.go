package app

import (
	"crypto/rand"
	"encoding/hex"
	"errors"
	"log"
	"net/http"
	"regexp"
	"strconv"
	"strings"
	"time"

	"github.com/muthuishere/klark/hub-go/internal/auth"
	"github.com/muthuishere/klark/hub-go/internal/store"
	"github.com/muthuishere/klark/hub-go/internal/tokens"
)

// Device authorization for `klark hub connect` (ADR 0012), the same HTTP contract as the Blazor hub: the machine
// asks for a code, a signed-in person approves it on the /connect page, and the machine's next poll receives a
// fresh token exactly once. Requests live in the database (so any pod answers); only the SHA-256 of the poll id
// is stored, and the token is minted when the approved request is collected, so only its SHA-256 is ever kept.

const (
	connectLifetime   = 10 * time.Minute
	connectInterval   = 2 // seconds between polls
	connectMaxPending = 1000
	connectAlphabet   = "ABCDEFGHJKLMNPQRSTUVWXYZ23456789" // no 0/O, 1/I
)

var machineNameRE = regexp.MustCompile(`^[A-Za-z0-9._-]{1,64}$`)

func (a *App) connectClock() time.Time {
	if a.Now != nil {
		return a.Now()
	}
	return time.Now()
}

func newConnectCode() string {
	b := make([]byte, 8)
	_, _ = rand.Read(b)
	out := make([]byte, 0, 9)
	for i, x := range b {
		if i == 4 {
			out = append(out, '-')
		}
		out = append(out, connectAlphabet[int(x)%len(connectAlphabet)])
	}
	return string(out)
}

// normalizeCode accepts "abcd efgh", "ABCDEFGH" or "ABCD-EFGH"; "" when it is not 8 letters/digits.
func normalizeCode(code string) string {
	var b strings.Builder
	for _, c := range strings.ToUpper(code) {
		if c >= 'A' && c <= 'Z' || c >= '0' && c <= '9' {
			b.WriteRune(c)
		}
	}
	s := b.String()
	if len(s) != 8 {
		return ""
	}
	return s[:4] + "-" + s[4:]
}

// rateLimited records one request for key and answers 429 when the limiter is over its budget.
func rateLimited(w http.ResponseWriter, l *auth.Limiter, key string) bool {
	if blocked, wait := l.Blocked(key); blocked {
		w.Header().Set("Retry-After", strconv.Itoa(int(wait.Seconds())+1))
		jsonErr(w, http.StatusTooManyRequests, "too many requests; try again later")
		return true
	}
	l.Fail(key)
	return false
}

// connectStart: POST /api/connect/start {"name"} -> {code, poll_id, url, expires_in, interval}. No sign-in.
func (a *App) connectStart(w http.ResponseWriter, r *http.Request) {
	if rateLimited(w, a.connectStartLimit, auth.ClientIP(r)) {
		return
	}
	var b struct {
		Name string `json:"name"`
	}
	if !decode(w, r, &b) {
		return
	}
	name := strings.TrimSpace(b.Name)
	if !machineNameRE.MatchString(name) {
		jsonErr(w, http.StatusBadRequest, "name: use letters, digits, '-', '_' or '.', at most 64 characters")
		return
	}
	now := a.connectClock()
	left, err := a.Store.SweepConnectRequests(now.UnixMilli())
	if err != nil {
		jsonErr(w, 500, "could not store the request")
		return
	}
	if left >= connectMaxPending {
		jsonErr(w, http.StatusServiceUnavailable, "too many pending requests; try again later")
		return
	}
	raw := make([]byte, 32)
	_, _ = rand.Read(raw)
	pollID := hex.EncodeToString(raw)
	req := store.ConnectRequest{PollSHA: tokens.Sha256Hex(pollID), Machine: name, Status: "pending", IP: auth.ClientIP(r),
		Created: now.UnixMilli(), Expires: now.Add(connectLifetime).UnixMilli()}
	for i := 0; ; i++ {
		req.Code = newConnectCode()
		ok, err := a.Store.AddConnectRequest(req)
		if err != nil || (!ok && i == 4) {
			jsonErr(w, 500, "could not store the request")
			return
		}
		if ok {
			break
		}
	}
	log.Printf("connect request %s for machine %s from %s", req.Code, name, req.IP)
	jsonOut(w, http.StatusOK, map[string]any{
		"code": req.Code, "poll_id": pollID, "url": a.Auth.ExternalBase(r) + "/connect?code=" + req.Code,
		"expires_in": int(connectLifetime.Seconds()), "interval": connectInterval,
	})
}

// connectPoll: POST /api/connect/poll {"poll_id"} -> pending | denied | expired, or approved with the token (once).
func (a *App) connectPoll(w http.ResponseWriter, r *http.Request) {
	if rateLimited(w, a.connectPollLimit, auth.ClientIP(r)) {
		return
	}
	var b struct {
		PollID string `json:"poll_id"`
	}
	if !decode(w, r, &b) {
		return
	}
	if b.PollID == "" {
		jsonOut(w, http.StatusNotFound, map[string]string{"status": "unknown"})
		return
	}
	pollSHA := tokens.Sha256Hex(b.PollID)
	req, err := a.Store.ConnectRequestByPoll(pollSHA)
	if errors.Is(err, store.ErrNotFound) {
		jsonOut(w, http.StatusNotFound, map[string]string{"status": "unknown"})
		return
	} else if err != nil {
		jsonErr(w, 500, "could not read the request")
		return
	}
	switch {
	case req.Status == "approved":
		took, err := a.Store.TakeConnectRequest(pollSHA, "approved")
		if err != nil || !took { // another poll collected it first
			jsonOut(w, http.StatusNotFound, map[string]string{"status": "unknown"})
			return
		}
		raw := make([]byte, 32)
		_, _ = rand.Read(raw)
		token := "km_" + hex.EncodeToString(raw)
		if err := a.Store.UpsertMachine(req.Machine, tokens.Sha256Hex(token), req.DecidedBy); err != nil {
			jsonErr(w, 500, "could not store the machine")
			return
		}
		a.Tokens.Invalidate()
		log.Printf("machine %s linked by connect request %s (approved by %s)", req.Machine, req.Code, req.DecidedBy)
		jsonOut(w, http.StatusOK, map[string]string{"status": "approved", "token": token, "machine": req.Machine})
	case req.Expires <= a.connectClock().UnixMilli():
		_ = a.Store.DeleteConnectRequest(pollSHA)
		jsonOut(w, http.StatusOK, map[string]string{"status": "expired"})
	case req.Status == "denied":
		_ = a.Store.DeleteConnectRequest(pollSHA)
		jsonOut(w, http.StatusOK, map[string]string{"status": "denied"})
	default:
		jsonOut(w, http.StatusOK, map[string]string{"status": "pending"})
	}
}

type connectView struct {
	Code      string `json:"code"`
	Machine   string `json:"machine"`
	Status    string `json:"status"` // pending | approved | denied
	ExpiresAt string `json:"expiresAt"`
	// Existing: a machine with this name is already stored (approving gives it a new token); Owner approved it.
	Existing bool   `json:"existing"`
	Owner    string `json:"owner,omitempty"`
}

// pendingRequest finds an unexpired request by code, or answers 404.
func (a *App) pendingRequest(w http.ResponseWriter, r *http.Request) (store.ConnectRequest, bool) {
	code := normalizeCode(r.PathValue("code"))
	req, err := a.Store.ConnectRequestByCode(code)
	if code == "" || errors.Is(err, store.ErrNotFound) || (err == nil && req.Expires <= a.connectClock().UnixMilli()) {
		jsonErr(w, http.StatusNotFound, "This code is unknown or has expired. Run klark hub connect again.")
		return req, false
	} else if err != nil {
		jsonErr(w, 500, "could not read the request")
		return req, false
	}
	return req, true
}

func (a *App) storedMachine(name string) *store.Machine {
	rows, _ := a.Store.Machines()
	for i := range rows {
		if strings.EqualFold(rows[i].Name, name) {
			return &rows[i]
		}
	}
	return nil
}

// connectGet: GET /api/connect/requests/{code} (signed in): what the approval page shows.
func (a *App) connectGet(w http.ResponseWriter, r *http.Request) {
	req, ok := a.pendingRequest(w, r)
	if !ok {
		return
	}
	v := connectView{Code: req.Code, Machine: req.Machine, Status: req.Status, ExpiresAt: time.UnixMilli(req.Expires).UTC().Format(time.RFC3339)}
	if m := a.storedMachine(req.Machine); m != nil {
		v.Existing, v.Owner = true, m.CreatedBy
	}
	jsonOut(w, http.StatusOK, v)
}

// connectDecide: POST /api/connect/requests/{code}/approve|deny (signed in). Anyone signed in may approve a new
// machine and is recorded as the one who approved it; giving an existing machine a new token needs an admin or the
// person who approved it before. Machines configured in HUB_MACHINES cannot be taken over.
func (a *App) connectDecide(approve bool) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		req, ok := a.pendingRequest(w, r)
		if !ok {
			return
		}
		me, _ := auth.FromContext(r.Context())
		status := "denied"
		if approve {
			status = "approved"
			for _, m := range a.Cfg.Machines {
				if strings.EqualFold(m.Name, req.Machine) {
					jsonErr(w, http.StatusConflict, req.Machine+" is configured in HUB_MACHINES; pick another name (KLARK_MACHINE_NAME)")
					return
				}
			}
			if m := a.storedMachine(req.Machine); m != nil {
				if m.Fleet {
					jsonErr(w, http.StatusConflict, req.Machine+" is a fleet token; pick another name (KLARK_MACHINE_NAME)")
					return
				}
				if !me.Admin && !strings.EqualFold(m.CreatedBy, me.Name) {
					jsonErr(w, http.StatusForbidden, "A machine named "+req.Machine+" already exists and belongs to someone else.")
					return
				}
			}
		}
		done, err := a.Store.DecideConnectRequest(req.Code, status, me.Name, a.connectClock().UnixMilli())
		if err != nil {
			jsonErr(w, 500, "could not store the decision")
			return
		}
		if !done {
			jsonErr(w, http.StatusConflict, "This request was already decided.")
			return
		}
		log.Printf("connect request %s for %s %s by %s", req.Code, req.Machine, status, me.Name)
		jsonOut(w, http.StatusOK, map[string]string{"status": status, "machine": req.Machine})
	}
}
