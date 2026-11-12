package app

import (
	"context"
	"crypto/subtle"
	"encoding/json"
	"errors"
	"io"
	"log"
	"net"
	"net/http"
	"net/http/httputil"
	"net/url"
	"sync"
	"time"

	"github.com/muthuishere/klark/hub-go/internal/store"
)

// Several pods (HUB_DB=postgres + HUB_POD_URL): every machine link lives on the pod it connected to. The pods share
// the database; machine_presence says which pod holds which machine (heartbeat every 5 s, expires after 15 s). A
// request for a machine held by another pod is proxied to that pod's own address, authenticated by HUB_POD_SECRET
// (header X-Klark-Pod-Secret, compared in constant time). Without the secret the header is refused and /internal is
// closed.
const podHeader = "X-Klark-Pod-Secret"

type forwardedKey struct{}

// Cluster is the pod-to-pod part of the app; nil unless HUB_DB=postgres and HUB_POD_URL are set.
type Cluster struct {
	a      *App
	mu     sync.Mutex
	url    string
	client *http.Client
}

func (a *App) initCluster() {
	if !a.Store.IsPostgres() {
		return
	}
	if a.Cfg.PodURL == "" {
		log.Printf("postgres mode without HUB_POD_URL: single pod (set HUB_POD_URL and HUB_POD_SECRET to run several)")
		return
	}
	a.Cluster = &Cluster{a: a, url: a.Cfg.PodURL, client: &http.Client{Timeout: 10 * time.Second,
		Transport: &http.Transport{DialContext: (&net.Dialer{Timeout: 3 * time.Second}).DialContext, MaxIdleConnsPerHost: 8}}}
	prev := a.Runtime.OnSeen
	a.Runtime.OnSeen = func(id, version string, protocol int) {
		if prev != nil {
			prev(id, version, protocol)
		}
		a.Cluster.announce(id, true)
	}
	a.Server.Forward = a.Cluster.forwardSession
}

// SetURL changes the address other pods use to reach this one (tests; normally HUB_POD_URL).
func (c *Cluster) SetURL(u string) { c.mu.Lock(); c.url = u; c.mu.Unlock() }

func (c *Cluster) selfURL() string { c.mu.Lock(); defer c.mu.Unlock(); return c.url }

func nowMs() int64 { return time.Now().UnixMilli() }

func (c *Cluster) presence(id string) store.Presence {
	info, _ := c.a.Runtime.Info(id)
	p := store.Presence{MachineID: id, PodID: c.a.Cfg.PodID, PodURL: c.selfURL(), Name: info.Name, Version: info.Version, Protocol: info.Protocol}
	if conn := c.a.Registry.Conn(id); conn != nil {
		p.SessionID = conn.SessionID
	}
	return p
}

// announce records this pod as the holder of id (force: a session was just opened).
func (c *Cluster) announce(id string, force bool) {
	if err := c.a.Store.UpsertPresence(c.presence(id), nowMs(), force); err != nil {
		log.Printf("cluster: presence of %s: %v", id, err)
	}
}

// Beat refreshes the presence of every machine linked here and drops the rows of machines that left.
func (c *Cluster) Beat() {
	var keep []string
	for _, in := range c.a.Runtime.Machines() {
		if in.Online {
			keep = append(keep, in.ID)
			c.announce(in.ID, false)
		}
	}
	if err := c.a.Store.DeletePresenceExcept(c.a.Cfg.PodID, keep); err != nil {
		log.Printf("cluster: presence cleanup: %v", err)
	}
}

// Run heartbeats every 5 s until ctx ends, then withdraws this pod's presence.
func (c *Cluster) Run(ctx context.Context) {
	c.Beat()
	t := time.NewTicker(5 * time.Second)
	defer t.Stop()
	for {
		select {
		case <-ctx.Done():
			_ = c.a.Store.DeletePresenceExcept(c.a.Cfg.PodID, nil)
			return
		case <-t.C:
			c.Beat()
		}
	}
}

func isForwarded(r *http.Request) bool { v, _ := r.Context().Value(forwardedKey{}).(bool); return v }

// remoteOwner is the other pod holding id, when there is one.
func (c *Cluster) remoteOwner(id string) (store.Presence, bool) {
	p, ok, err := c.a.Store.PresenceOf(id, nowMs()-store.PresenceExpiryMs)
	if err != nil || !ok || p.PodID == c.a.Cfg.PodID {
		return p, false
	}
	return p, true
}

// others are the live presence rows of machines held by other pods.
func (c *Cluster) others() []store.Presence {
	rows, _ := c.a.Store.ListPresence(nowMs() - store.PresenceExpiryMs)
	var out []store.Presence
	for _, p := range rows {
		if p.PodID != c.a.Cfg.PodID {
			out = append(out, p)
		}
	}
	return out
}

func (c *Cluster) otherPodURLs() []string {
	seen := map[string]bool{}
	var out []string
	for _, p := range c.others() {
		if !seen[p.PodURL] {
			seen[p.PodURL] = true
			out = append(out, p.PodURL)
		}
	}
	return out
}

// proxy serves r from the pod at target (streams included).
func (c *Cluster) proxy(w http.ResponseWriter, r *http.Request, target string) {
	u, err := url.Parse(target)
	if err != nil || u.Host == "" {
		jsonErr(w, http.StatusBadGateway, "the pod holding this machine has no usable address")
		return
	}
	base, secret := c.a.Cfg.BasePath, c.a.Cfg.PodSecret
	rp := &httputil.ReverseProxy{
		Director: func(req *http.Request) {
			req.URL.Scheme, req.URL.Host, req.Host = u.Scheme, u.Host, u.Host
			req.URL.Path, req.URL.RawPath = base+r.URL.Path, ""
			req.Header.Set(podHeader, secret)
			req.Header.Del("Accept-Encoding") // this pod compresses for the browser
		},
		FlushInterval: -1,
		Transport:     c.client.Transport,
		ModifyResponse: func(resp *http.Response) error {
			for _, h := range []string{"X-Content-Type-Options", "X-Frame-Options", "Referrer-Policy", "Content-Security-Policy"} {
				resp.Header.Del(h) // this pod sets its own
			}
			return nil
		},
		ErrorHandler: func(w http.ResponseWriter, r *http.Request, err error) {
			if errors.Is(err, context.Canceled) {
				return
			}
			jsonErr(w, http.StatusBadGateway, "the pod holding this machine did not answer")
		},
	}
	rp.ServeHTTP(w, r)
}

// forwardMachine proxies the request when the machine is held by another pod; true when it did.
func (a *App) forwardMachine(w http.ResponseWriter, r *http.Request, id string) bool {
	if a.Cluster == nil || isForwarded(r) {
		return false
	}
	p, ok := a.Cluster.remoteOwner(id)
	if !ok {
		return false
	}
	a.Cluster.proxy(w, r, p.PodURL)
	return true
}

// owned wraps a machine route ({id} in the path): served here, or proxied to the pod that holds the machine.
func (a *App) owned(f http.HandlerFunc) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		if a.forwardMachine(w, r, r.PathValue("id")) {
			return
		}
		f(w, r)
	}
}

func (c *Cluster) forwardSession(w http.ResponseWriter, r *http.Request, session string) bool {
	if session == "" || isForwarded(r) {
		return false
	}
	p, ok, err := c.a.Store.PresenceBySession(session, nowMs()-store.PresenceExpiryMs)
	if err != nil || !ok || p.PodID == c.a.Cfg.PodID {
		return false
	}
	c.proxy(w, r, p.PodURL)
	return true
}

// machineOnline: linked here or held by another pod.
func (a *App) machineOnline(id string) bool {
	if a.Runtime.Online(id) {
		return true
	}
	if a.Cluster != nil {
		_, ok := a.Cluster.remoteOwner(id)
		return ok
	}
	return false
}

// podCall asks another pod's internal endpoint.
func (c *Cluster) podCall(ctx context.Context, pod, method, path string) ([]byte, error) {
	req, err := http.NewRequestWithContext(ctx, method, pod+c.a.Cfg.BasePath+path, nil)
	if err != nil {
		return nil, err
	}
	req.Header.Set(podHeader, c.a.Cfg.PodSecret)
	resp, err := c.client.Do(req)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()
	b, _ := io.ReadAll(io.LimitReader(resp.Body, 8<<20))
	if resp.StatusCode/100 != 2 {
		return nil, errors.New(resp.Status)
	}
	return b, nil
}

// podLayer is in front of the routes: it admits pod-to-pod requests (valid secret) and serves /internal/*.
func (a *App) podLayer(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		h := r.Header.Get(podHeader)
		trusted := false
		if h != "" {
			if a.Cfg.PodSecret == "" || subtle.ConstantTimeCompare([]byte(h), []byte(a.Cfg.PodSecret)) != 1 {
				jsonErr(w, http.StatusForbidden, "pod secret refused")
				return
			}
			trusted = true
			r = r.Clone(context.WithValue(r.Context(), forwardedKey{}, true))
			r.Header.Del(podHeader)
			// the pod that received the browser's request already did the cross-origin check
			r.Header.Del("Origin")
			r.Header.Del("Sec-Fetch-Site")
		}
		if len(r.URL.Path) >= 10 && r.URL.Path[:10] == "/internal/" {
			if !trusted {
				jsonErr(w, http.StatusForbidden, "internal route")
				return
			}
			a.internal(w, r)
			return
		}
		next.ServeHTTP(w, r)
	})
}

func (a *App) internal(w http.ResponseWriter, r *http.Request) {
	switch r.URL.Path {
	case "/internal/agents":
		jsonOut(w, 200, append([]agentEntry{}, a.agents(r.Context())...))
	case "/internal/stats/refresh":
		a.Stats.Refresh(r.Context(), 2*time.Second)
		w.WriteHeader(http.StatusNoContent)
	case "/internal/revoke":
		a.Runtime.Revoke(r.URL.Query().Get("machine"))
		w.WriteHeader(http.StatusNoContent)
	default:
		jsonErr(w, 404, "no such internal route")
	}
}

// remoteAgents are the agents of the machines held by other pods.
func (c *Cluster) remoteAgents(ctx context.Context) []agentEntry {
	var wg sync.WaitGroup
	var mu sync.Mutex
	var all []agentEntry
	for _, pod := range c.otherPodURLs() {
		wg.Add(1)
		go func(pod string) {
			defer wg.Done()
			cctx, cancel := context.WithTimeout(ctx, 8*time.Second)
			defer cancel()
			b, err := c.podCall(cctx, pod, "GET", "/internal/agents")
			if err != nil {
				return
			}
			var list []agentEntry
			if json.Unmarshal(b, &list) == nil {
				mu.Lock()
				all = append(all, list...)
				mu.Unlock()
			}
		}(pod)
	}
	wg.Wait()
	return all
}

// refreshRemote asks the other pods to sample their machines (each pod calls only its own machines).
func (c *Cluster) refreshRemote(ctx context.Context) {
	var wg sync.WaitGroup
	for _, pod := range c.otherPodURLs() {
		wg.Add(1)
		go func(pod string) {
			defer wg.Done()
			cctx, cancel := context.WithTimeout(ctx, 2500*time.Millisecond)
			defer cancel()
			_, _ = c.podCall(cctx, pod, "POST", "/internal/stats/refresh")
		}(pod)
	}
	wg.Wait()
}
