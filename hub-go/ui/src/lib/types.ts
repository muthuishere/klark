export interface Me { username: string; displayName?: string; admin?: boolean }
export interface Providers {
  static?: boolean;
  directory?: { mode?: string; name?: string };
  adfs?: { enabled: boolean; name?: string; loginUrl?: string };
  oidc?: Array<{ id: string; name?: string; loginUrl?: string }>;
}
export interface DirectorySettings {
  mode?: string; host?: string; port?: number; ssl?: boolean; startTls?: boolean; skipCertCheck?: boolean; timeoutSeconds?: number;
  bindDn?: string; bindPasswordSet?: boolean; bindPassword?: string; userDnTemplate?: string; userSearchBase?: string; userFilter?: string;
  groupSearchBase?: string; groupFilter?: string; useMemberOf?: boolean; displayNameAttr?: string; adminGroup?: string; allowedGroups?: string[] | string;
  oidc?: Array<{ id: string; name?: string; redirectUri?: string; loginUrl?: string }>;
}
/** GET /api/admin/directory: the effective settings nested under `settings`. */
export interface DirectoryInfo {
  mode?: string; name?: string; source?: 'env' | 'admin'; settings?: DirectorySettings;
  adfs?: { enabled: boolean; name?: string; loginUrl?: string; redirectUri?: string };
  oidc?: Array<{ id: string; name?: string; redirectUri?: string; loginUrl?: string; issuer?: string; clientId?: string }>;
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
export interface AdminUser { username: string; displayName?: string; admin?: boolean; source?: string; groups?: string[] }
export interface AdminMachine { name: string; status?: string; approvedBy?: string; created?: string }

export interface AgentStat { paneId: string; name: string; state: string; rssBytes: number; cpuPercent: number }
export interface LatestStats {
  ts: number; klarkRssBytes: number; klarkPeakRssBytes: number; klarkCpuPercent: number; uptimeSeconds: number; version: string;
  hostMemTotalBytes: number; hostMemAvailableBytes: number; load1: number; cpus: number; panes: number; agents: number; agentList: AgentStat[];
}
export interface SeriesPoint { t: number; klarkRssBytes: number; klarkCpuPercent: number; hostMemAvailableBytes: number; panes: number; agents: number }
export interface MachineAnalytics {
  id: string; name: string; online: boolean; hasStats: boolean; latest: LatestStats | null;
  peak?: { klarkRssBytes: number; klarkCpuPercent: number; hostMemUsedPercent: number; panes: number; agents: number } | null;
  series?: SeriesPoint[] | null;
}
export interface Analytics {
  range: string; intervalSeconds: number; bucketSeconds?: number;
  totals: { machines: number; online: number; panes: number; agents: number; klarkRssBytes: number };
  machines: MachineAnalytics[];
}
export interface StorageInfo {
  kind: 'none' | 'folder' | 's3'; source?: 'env' | 'admin'; path?: string;
  s3?: { bucket?: string; region?: string; endpoint?: string; accessKeyId?: string; secretAccessKeySet?: boolean; pathStyle?: boolean };
  sqlite?: { enabled?: boolean; every?: string; keep?: number; owner?: string; backups?: number };
  lastSqliteBackupMs?: number; lastAnalyticsWriteMs?: number; lastError?: string; filesWritten?: number;
}
