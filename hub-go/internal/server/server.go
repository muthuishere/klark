// Package server is the hub's HTTP API.
package server

import (
	"compress/gzip"
	"encoding/json"
	"io"
	"net/http"
	"strconv"
	"strings"

	"github.com/muthuishere/klark/hub-go/internal/link"
	"github.com/muthuishere/klark/hub-go/internal/tokens"
)

const maxSegment = 64 << 20

// Server wires the API routes.
type Server struct {
	Tokens   tokens.Source
	Registry *link.Registry
	// UI, when set, serves the built React app for every path the API does not handle.
	UI http.Handler
	// Forward, when set, is asked about a request for a session this pod does not hold (several pods): it may serve
	// the request itself (proxy it to the pod that holds the session) and report true.
	Forward func(w http.ResponseWriter, r *http.Request, session string) bool
}

// Mount registers the machine link routes (ADR 0013) and /healthz on mux.
func (s *Server) Mount(mux *http.ServeMux) {
	mux.HandleFunc("GET /healthz", func(w http.ResponseWriter, _ *http.Request) { _, _ = io.WriteString(w, "ok") })
	mux.HandleFunc("GET /agent", s.probe)
	mux.HandleFunc("POST /agent/session", s.openSession)
	mux.HandleFunc("GET /agent/events", s.events)
	mux.HandleFunc("POST /agent/push", s.push)
}

// Handler is the machine link alone (plus the UI when set), for tests and tools.
func (s *Server) Handler() http.Handler {
	mux := http.NewServeMux()
	s.Mount(mux)
	if s.UI != nil {
		mux.Handle("/", s.UI)
	}
	return mux
}

func writeJSON(w http.ResponseWriter, status int, v any) {
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(status)
	_ = json.NewEncoder(w).Encode(v)
}

func bearer(r *http.Request) string {
	h := r.Header.Get("Authorization")
	if len(h) > 7 && strings.EqualFold(h[:7], "Bearer ") {
		return strings.TrimSpace(h[7:])
	}
	return ""
}

// machine authenticates a machine token and resolves the id it acts as.
func (s *Server) machine(w http.ResponseWriter, r *http.Request) (id string, ok bool) {
	m := tokens.Match(s.Tokens.Machines(), bearer(r))
	if m == nil {
		writeJSON(w, http.StatusUnauthorized, map[string]string{"error": "unknown machine token"})
		return "", false
	}
	id, refused := tokens.ResolveMachine(m, r.Header.Get("X-Klark-Machine"))
	if id == "" {
		writeJSON(w, http.StatusBadRequest, map[string]string{"error": refused})
		return "", false
	}
	return id, true
}

// probe is the token check `klark hub connect` uses.
func (s *Server) probe(w http.ResponseWriter, r *http.Request) {
	if id, ok := s.machine(w, r); ok {
		writeJSON(w, http.StatusOK, map[string]string{"machine": id})
	}
}

func (s *Server) openSession(w http.ResponseWriter, r *http.Request) {
	id, ok := s.machine(w, r)
	if !ok {
		return
	}
	var hello map[string]any
	if err := json.NewDecoder(io.LimitReader(r.Body, 1<<20)).Decode(&hello); err != nil {
		writeJSON(w, http.StatusBadRequest, map[string]string{"error": "the body must be a hello message"})
		return
	}
	conn, err := s.Registry.Open(id, bearer(r), hello)
	if err != nil {
		writeJSON(w, http.StatusBadRequest, map[string]string{"error": err.Error()})
		return
	}
	writeJSON(w, http.StatusOK, map[string]any{
		"session_id": conn.SessionID, "heartbeat_seconds": int(s.Registry.Heartbeat.Seconds())})
}

func (s *Server) session(w http.ResponseWriter, r *http.Request) *link.Conn {
	conn, wrong := s.Registry.Session(r.URL.Query().Get("session"), bearer(r))
	if conn != nil {
		return conn
	}
	if wrong {
		writeJSON(w, http.StatusUnauthorized, map[string]string{"error": "token does not match the session"})
	} else if s.Forward != nil && s.Forward(w, r, r.URL.Query().Get("session")) {
		return nil
	} else {
		writeJSON(w, http.StatusNotFound, map[string]string{"error": "unknown session; open a new one"})
	}
	return nil
}

func (s *Server) events(w http.ResponseWriter, r *http.Request) {
	conn := s.session(w, r)
	if conn == nil {
		return
	}
	after, _ := strconv.ParseInt(r.URL.Query().Get("after"), 10, 64)
	conn.Stream(r.Context(), w, after)
}

func (s *Server) push(w http.ResponseWriter, r *http.Request) {
	conn := s.session(w, r)
	if conn == nil {
		return
	}
	seq, _ := strconv.ParseInt(r.URL.Query().Get("seq"), 10, 64)
	if seq <= 0 {
		writeJSON(w, http.StatusBadRequest, map[string]string{"error": "seq is required"})
		return
	}
	var body io.Reader = r.Body
	if strings.Contains(strings.ToLower(r.Header.Get("Content-Encoding")), "gzip") {
		zr, err := gzip.NewReader(r.Body)
		if err != nil {
			writeJSON(w, http.StatusBadRequest, map[string]string{"error": "bad gzip body"})
			return
		}
		defer zr.Close()
		body = zr
	}
	data, err := io.ReadAll(io.LimitReader(body, maxSegment+1))
	if err != nil {
		writeJSON(w, http.StatusBadRequest, map[string]string{"error": "bad gzip body"})
		return
	}
	if len(data) > maxSegment {
		writeJSON(w, http.StatusRequestEntityTooLarge, map[string]string{"error": "segment too large"})
		return
	}
	if conn.Push(seq, data) {
		w.WriteHeader(http.StatusNoContent)
		return
	}
	writeJSON(w, http.StatusOK, map[string]bool{"duplicate": true})
}
