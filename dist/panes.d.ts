import type { PaneInfo, Tree } from './types';
export declare const paneId: (p: PaneInfo) => string;
export declare const paneCwd: (p: PaneInfo) => string;
export declare const paneAgent: (p: PaneInfo) => string;
export declare const paneStatus: (p: PaneInfo) => string;
export declare const isAgent: (p: PaneInfo) => boolean;
export declare function agentKey(agent: string): 'claude' | 'devin' | 'codex' | 'shell' | 'other';
export declare function agentIcon(agent: string): string;
export declare function folderName(cwd?: string): string;
/** The pane's own name; else "agent · folder"; never a raw pane id when anything better exists. */
export declare function displayName(p: PaneInfo): string;
export declare function workspaceLabel(t: Tree | undefined, id?: string): string;
export declare const statusRank: (s: string) => 0 | 1 | 2 | 3;
export interface Row {
    machineId: string;
    machineName: string;
    pane: PaneInfo;
    session: string;
}
export interface Group {
    key: string;
    name: string;
    path: string;
    rows: Row[];
}
/** Every word (AND) must appear in the pane's name, agent, status, folder, full path, session or machine. */
export declare function matches(r: Row, words: string[]): boolean;
/** Sidebar grouping: by project folder, needs-you first. */
export declare function groupRows(rows: Row[], filter: string, showAll: boolean): Group[];
