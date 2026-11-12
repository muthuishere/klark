/** Wire types of klark hub's chat API (hub-go/API.md "Chat rows", "Pane SSE"). */

export type ItemKind = 'text' | 'user' | 'tool_use' | 'tool_result' | 'thinking' | 'command' | 'output';
/** One transcript item, as the hub sends it. html is server-rendered (markdown/ANSI) and already escaped. */
export interface ChatItem {
  id: string; role?: string; kind: ItemKind | string; html?: string; text?: string; time?: string;
  tool?: string; summary?: string; input?: string; ok?: boolean; toolUseId?: string; lines?: number;
}
/** A file the agent offered for download. */
export interface Offer { id: string; name: string; size?: number; mime?: string; url: string }
export interface Choice { key: string; label: string }
/** A question the agent waits on: a numbered menu (choices, selected = the highlighted key) or quick replies. */
export interface Blocked { question: string; options?: { label: string; send: string; enter: boolean }[]; choices?: Choice[]; selected?: string }
/** GET .../chat and the `chat` SSE event (`replace` = the rows are the whole chat). */
export interface ChatPayload {
  rows: ChatItem[]; replace?: boolean; rev?: string | number; agent?: string; status?: string; source?: string; error?: string;
  offers?: Offer[]; blocked?: Blocked | null;
}

export interface MachineInfo { id: string; name: string; online: boolean; version?: string; protocol?: number; lastSeen?: string }
// session.snapshot (snake_case, straight from the machine)
export interface Workspace { workspace_id: string; label?: string; number?: number; agent_status?: string }
export interface Tab { tab_id: string; workspace_id: string; label?: string; number?: number }
export interface PaneInfo {
  pane_id: string; workspace_id?: string; tab_id?: string; label?: string; title?: string; terminal_title_stripped?: string;
  agent?: string; display_agent?: string; agent_status?: string; cwd?: string; foreground_cwd?: string; focused?: boolean;
}
export interface Tree { workspaces?: Workspace[]; tabs?: Tab[]; panes?: PaneInfo[] }
/** A machine and its session tree (what the sidebar lists). */
export interface MachineTree { machine: MachineInfo; tree?: Tree }
