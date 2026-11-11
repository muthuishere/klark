package app

import (
	"context"
	"encoding/json"
	"net/http"
	"sort"
	"strconv"
	"strings"
	"sync"
	"time"

	"github.com/google/uuid"

	"github.com/muthuishere/klark/hub-go/internal/auth"
	"github.com/muthuishere/klark/hub-go/internal/hub"
	"github.com/muthuishere/klark/hub-go/internal/store"
)

const maxMessageText = 16 * 1024

type agentEntry struct {
	Machine string `json:"machine"`
	Session string `json:"session"`
	Agent   string `json:"agent"`
	Name    string `json:"name"`
	PaneID  string `json:"pane_id"`
	Status  string `json:"status"`
	Address string `json:"address"`
}

var agentCache = struct {
	sync.Mutex
	at  time.Time
	val []agentEntry
}{}

// agents asks every online protocol>=2 machine for its agents, concurrently (answers shared for 2 s).
func (a *App) agents(ctx context.Context) []agentEntry {
	agentCache.Lock()
	defer agentCache.Unlock()
	if time.Since(agentCache.at) < 2*time.Second {
		return agentCache.val
	}
	var wg sync.WaitGroup
	var mu sync.Mutex
	var all []agentEntry
	for _, m := range a.Runtime.Machines() {
		if !m.Online || m.Protocol < 2 {
			continue
		}
		wg.Add(1)
		go func(id string) {
			defer wg.Done()
			raw, err := a.Runtime.Request(ctx, id, map[string]any{"type": "list_agents"}, 10*time.Second)
			if err != nil {
				return
			}
			var list []map[string]any
			_ = json.Unmarshal(raw, &list)
			for _, e := range list {
				pane, _ := e["pane_id"].(string)
				if pane == "" {
					continue
				}
				sess, _ := e["session"].(string)
				if sess == "" {
					sess = "default"
				}
				name, _ := e["name"].(string)
				ag, _ := e["agent"].(string)
				st, _ := e["status"].(string)
				who := name
				if who == "" {
					who = pane
				}
				mu.Lock()
				all = append(all, agentEntry{id, sess, ag, name, pane, st, id + "/" + sess + "/" + who})
				mu.Unlock()
			}
		}(m.ID)
	}
	wg.Wait()
	sort.Slice(all, func(i, j int) bool { return strings.ToLower(all[i].Address) < strings.ToLower(all[j].Address) })
	agentCache.at, agentCache.val = time.Now(), all
	return all
}

func (a *App) listAgents(w http.ResponseWriter, r *http.Request) {
	jsonOut(w, http.StatusOK, append([]agentEntry{}, a.agents(r.Context())...))
}

type messageOut struct {
	TS     string `json:"ts"`
	From   string `json:"from"`
	To     string `json:"to"`
	Text   string `json:"text"`
	OK     bool   `json:"ok"`
	Error  string `json:"error,omitempty"`
	Sender string `json:"sender"`
	ID     string `json:"id"`
}

func (a *App) listMessages(w http.ResponseWriter, r *http.Request) {
	limit, _ := strconv.Atoi(r.URL.Query().Get("limit"))
	if limit < 1 || limit > 500 {
		limit = 100
	}
	rows, err := a.Store.RecentMessages(limit)
	if err != nil {
		jsonErr(w, http.StatusInternalServerError, err.Error())
		return
	}
	out := make([]messageOut, 0, len(rows))
	for _, m := range rows {
		out = append(out, messageOut{time.Unix(m.Time, 0).UTC().Format(time.RFC3339), m.From, m.To, m.Text, m.OK, m.Error, m.Sender, m.ID})
	}
	jsonOut(w, http.StatusOK, out)
}

// postMessage delivers text to `machine/session/agent` and records it (ADR 0009).
func (a *App) postMessage(w http.ResponseWriter, r *http.Request) {
	u, _ := auth.FromContext(r.Context())
	var b struct{ To, Text, From string }
	if !decode(w, r, &b) {
		return
	}
	parts := strings.Split(b.To, "/")
	if len(parts) != 3 || strings.TrimSpace(parts[0]) == "" || strings.TrimSpace(parts[1]) == "" || strings.TrimSpace(parts[2]) == "" {
		jsonErr(w, http.StatusBadRequest, "to must be machine/session/agent")
		return
	}
	if b.Text == "" {
		jsonErr(w, http.StatusBadRequest, "text is empty")
		return
	}
	if len(b.Text) > maxMessageText {
		jsonErr(w, http.StatusRequestEntityTooLarge, "text is longer than 16384 bytes")
		return
	}
	machine, session, agent := strings.TrimSpace(parts[0]), strings.TrimSpace(parts[1]), strings.TrimSpace(parts[2])
	for _, in := range a.Runtime.Machines() {
		if strings.EqualFold(in.ID, machine) {
			machine = in.ID
		}
	}
	from := strings.TrimSpace(b.From)
	if from == "" {
		from = "user:" + u.Name
	}
	id := uuid.NewString()
	status, errText := http.StatusOK, ""
	info, known := a.Runtime.Info(machine)
	switch {
	case !known:
		status, errText = http.StatusNotFound, "machine "+machine+" is not linked to this hub"
	case !info.Online:
		status, errText = http.StatusBadGateway, "machine is offline"
	case info.Protocol < 2:
		status, errText = http.StatusConflict, "machine links with protocol "+strconv.Itoa(info.Protocol)+"; update klark there to receive messages"
	default:
		_, err := a.Runtime.Request(r.Context(), machine, map[string]any{"type": "deliver", "session": session, "agent": agent, "from": from, "text": b.Text}, 10*time.Second)
		switch err {
		case nil:
		case hub.ErrTimeout:
			status, errText = http.StatusGatewayTimeout, machine+" did not answer in time"
		default:
			status, errText = http.StatusBadGateway, err.Error()
		}
	}
	_ = a.Store.RecordMessage(store.Message{ID: id, Time: time.Now().Unix(), Sender: "user:" + u.Name, From: from,
		To: machine + "/" + session + "/" + agent, Text: b.Text, OK: status == http.StatusOK, Error: errText})
	if status != http.StatusOK {
		jsonOut(w, status, map[string]any{"error": errText, "id": id})
		return
	}
	jsonOut(w, http.StatusOK, map[string]any{"id": id, "ok": true})
}
