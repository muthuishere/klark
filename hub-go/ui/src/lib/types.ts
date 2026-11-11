export interface Me { username: string; displayName?: string; admin?: boolean }
export interface Providers { static?: boolean; adfs?: { enabled: boolean; name?: string; loginUrl?: string } }
export interface MachineInfo { id: string; name: string; online: boolean; version?: string; protocol?: number; lastSeen?: string }

// session.snapshot (snake_case, straight from the machine)
export interface Workspace { workspace_id: string; label?: string; number?: number; agent_status?: string }
export interface Tab { tab_id: string; workspace_id: string; label?: string; number?: number }
export interface PaneInfo {
  pane_id: string; workspace_id?: string; tab_id?: string; label?: string; title?: string; terminal_title_stripped?: string;
  agent?: string; display_agent?: string; agent_status?: string; cwd?: string; foreground_cwd?: string; focused?: boolean;
}
export interface Tree { workspaces?: Workspace[]; tabs?: Tab[]; panes?: PaneInfo[] }

export type ItemKind = 'text' | 'user' | 'tool_use' | 'tool_result' | 'thinking' | 'command' | 'output';
/** One transcript item, as the hub sends it. html is server-rendered (markdown/ANSI) and already escaped. */
export interface ChatItem {
  id: string; role?: string; kind: ItemKind | string; html?: string; text?: string; time?: string;
  tool?: string; summary?: string; input?: string; ok?: boolean; toolUseId?: string; lines?: number;
}
export interface Offer { id: string; name: string; size?: number; mime?: string; url: string }
export interface ChatPayload { rows: ChatItem[]; rev?: string | number; offers?: Offer[] }

export interface FileMeta {
  path: string; name: string; view: string; size_label?: string; lines?: number; lang?: string; raw?: string; download?: string;
}
export interface HubMessage { ts: string; from: string; to: string; text: string; ok: boolean; error?: string; sender?: string }
export interface AdminUser { username: string; displayName?: string; admin?: boolean; source?: string }
export interface AdminMachine { name: string; status?: string; approvedBy?: string; created?: string }
