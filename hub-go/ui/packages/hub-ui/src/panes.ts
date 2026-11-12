import type { PaneInfo, Tree } from './types';

export const paneId = (p: PaneInfo) => p.pane_id;
export const paneCwd = (p: PaneInfo) => p.foreground_cwd || p.cwd || '';
export const paneAgent = (p: PaneInfo) => p.display_agent || p.agent || '';
export const paneStatus = (p: PaneInfo) => (p.agent_status && p.agent_status !== 'unknown' ? p.agent_status : '');
export const isAgent = (p: PaneInfo) => paneAgent(p).length > 0 || paneStatus(p) !== '';

export function agentKey(agent: string): 'claude' | 'devin' | 'codex' | 'shell' | 'other' {
  const a = agent.toLowerCase();
  return a.includes('claude') ? 'claude' : a.includes('devin') ? 'devin' : a.includes('codex') ? 'codex' : a === '' ? 'shell' : 'other';
}
export function agentIcon(agent: string): string {
  switch (agentKey(agent)) {
    case 'claude': return '✻';
    case 'devin': return '◆';
    case 'codex': return '◎';
    case 'shell': return '$';
    default: return agent ? agent[0].toUpperCase() : '•';
  }
}

export function folderName(cwd?: string): string {
  const s = (cwd ?? '').trim().replace(/\/+$/, '');
  if (s === '' || s === '~' || /^\/(Users|home)\/[^/]+$/.test(s)) return '~';
  return s.slice(s.lastIndexOf('/') + 1);
}

/** The pane's own name; else "agent · folder"; never a raw pane id when anything better exists. */
export function displayName(p: PaneInfo): string {
  const title = (p.label || p.title || p.terminal_title_stripped || p.pane_id || '').replace(/^[✳✻✽✶·*⠂⠐ ]+/, '');
  const agent = paneAgent(p);
  const cwd = paneCwd(p);
  const raw = title === '' || title === p.pane_id || /^w?\d+[:/]p?\d+$/.test(title) || title.toLowerCase() === agent.toLowerCase() || title === cwd ||
    ['zsh', 'bash', 'sh', 'fish'].includes(title) || /^(?:[\w.-]+@[\w.-]+:)?(?:~|\.{0,2})?\//.test(title);
  if (!raw) return title;
  const folder = cwd.replace(/\/+$/, '') ? cwd.replace(/\/+$/, '').split('/').pop() ?? '' : '';
  if (agent) return folder ? `${agent} · ${folder}` : agent;
  return folder || title || p.pane_id;
}

export function workspaceLabel(t: Tree | undefined, id?: string): string {
  const w = t?.workspaces?.find((x) => x.workspace_id === id);
  return w ? w.label || `workspace ${w.number ?? ''}`.trim() : id ?? '';
}

export const statusRank = (s: string) => (s === 'blocked' ? 3 : s === 'working' ? 2 : s === 'idle' ? 1 : 0);

export interface Row { machineId: string; machineName: string; pane: PaneInfo; session: string }
export interface Group { key: string; name: string; path: string; rows: Row[] }

/** Every word (AND) must appear in the pane's name, agent, status, folder, full path, session or machine. */
export function matches(r: Row, words: string[]): boolean {
  if (!words.length) return true;
  const p = r.pane;
  const hay = `${r.machineName} ${r.session} ${displayName(p)} ${p.title ?? ''} ${paneAgent(p)} ${paneStatus(p)} ${paneCwd(p)} ${folderName(paneCwd(p))} ${p.pane_id}`.toLowerCase();
  return words.every((w) => hay.includes(w.toLowerCase()));
}

/** Sidebar grouping: by project folder, needs-you first. */
export function groupRows(rows: Row[], filter: string, showAll: boolean): Group[] {
  const words = filter.split(/\s+/).filter(Boolean);
  const kept = rows.filter((r) => (showAll || isAgent(r.pane)) && matches(r, words));
  const by = new Map<string, Row[]>();
  for (const r of kept) {
    const k = folderName(paneCwd(r.pane));
    by.set(k, [...(by.get(k) ?? []), r]);
  }
  const rank = (rs: Row[]) => Math.max(...rs.map((r) => statusRank(paneStatus(r.pane))));
  return [...by.entries()]
    .map(([key, rs]) => ({
      key, name: key, path: paneCwd(rs[0].pane),
      rows: rs.slice().sort((a, b) => statusRank(paneStatus(b.pane)) - statusRank(paneStatus(a.pane)) || displayName(a.pane).localeCompare(displayName(b.pane))),
    }))
    .sort((a, b) => rank(b.rows) - rank(a.rows) || a.name.localeCompare(b.name, undefined, { sensitivity: 'base' }));
}
