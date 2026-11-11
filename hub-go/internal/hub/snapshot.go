package hub

import "encoding/json"

// PaneInfo is the part of a pane the hub itself needs (the UI reads the snapshot directly).
type PaneInfo struct {
	ID, Workspace, Tab, Title, Agent, AgentStatus, Cwd string
	Focused                                            bool
}

func firstNonEmpty(v ...string) string {
	for _, s := range v {
		if s != "" {
			return s
		}
	}
	return ""
}

// ParsePanes reads the panes of a `session.snapshot`.
func ParsePanes(snapshot json.RawMessage) []PaneInfo {
	var s struct {
		Panes []map[string]any `json:"panes"`
	}
	if json.Unmarshal(snapshot, &s) != nil {
		return nil
	}
	out := make([]PaneInfo, 0, len(s.Panes))
	for _, p := range s.Panes {
		f, _ := p["focused"].(bool)
		out = append(out, PaneInfo{
			ID: str(p["pane_id"]), Workspace: str(p["workspace_id"]), Tab: str(p["tab_id"]),
			Title:       firstNonEmpty(str(p["label"]), str(p["title"]), str(p["terminal_title_stripped"]), str(p["pane_id"])),
			Agent:       firstNonEmpty(str(p["display_agent"]), str(p["agent"])),
			AgentStatus: str(p["agent_status"]),
			Cwd:         firstNonEmpty(str(p["foreground_cwd"]), str(p["cwd"])),
			Focused:     f,
		})
	}
	return out
}

// Pane finds one pane in a machine's (cached) tree.
func (rt *Runtime) Pane(machineID, paneID string) (PaneInfo, bool) {
	m := rt.get(machineID)
	m.mu.Lock()
	snap := m.treeSnap
	m.mu.Unlock()
	for _, p := range ParsePanes(snap) {
		if p.ID == paneID {
			return p, true
		}
	}
	return PaneInfo{}, false
}
