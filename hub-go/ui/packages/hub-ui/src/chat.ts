import type { ChatItem } from './types';

export interface ChatRowModel { key: string; kind: 'user' | 'text' | 'tools' | 'block'; items: ChatItem[]; stamp: string }

const isRun = (k: string) => k === 'tool_use' || k === 'tool_result' || k === 'thinking';
const make = (kind: ChatRowModel['kind'], items: ChatItem[]): ChatRowModel => ({
  key: items[0].id,
  kind,
  items,
  // Changes only when the row's content does, so memoised rows skip re-rendering.
  stamp: `${items.length}:${items[items.length - 1].id}:${items.reduce((n, i) => n + (i.html?.length ?? 0) + (i.text?.length ?? 0), 0)}`,
});

/** Fold the flat transcript into display rows (port of ChatRowModel.Build). */
export function buildRows(items: ChatItem[]): ChatRowModel[] {
  const rows: ChatRowModel[] = [];
  let run: ChatItem[] = [];
  const flush = () => { if (run.length) rows.push(make('tools', run)); run = []; };
  for (let n = 0; n < items.length; n++) {
    const i = items[n];
    if (isRun(i.kind)) { run.push(i); continue; }
    flush();
    if (i.kind === 'command') {
      const next = items[n + 1];
      rows.push(make('block', next && next.kind === 'output' ? [i, items[++n]] : [i]));
      continue;
    }
    if (i.kind === 'output') { rows.push(make('block', [i])); continue; }
    if (!i.text && !i.html) continue;
    rows.push(make(i.role === 'user' || i.kind === 'user' ? 'user' : 'text', [i]));
  }
  flush();
  return rows;
}

/** Add or replace rows by id, keeping arrival order. New ids append. */
export function mergeItems(cur: ChatItem[], incoming: ChatItem[]): ChatItem[] {
  if (!incoming.length) return cur;
  const at = new Map(cur.map((r, i) => [r.id, i] as const));
  const next = cur.slice();
  for (const r of incoming) {
    const i = at.get(r.id);
    if (i === undefined) { at.set(r.id, next.length); next.push(r); } else next[i] = r;
  }
  return next;
}

export interface ToolCall { key: string; kind: 'tool' | 'thinking'; use?: ChatItem; result?: ChatItem }

/** Pair each tool_use with the result that follows it; thinking stands alone. */
export function pairCalls(items: ChatItem[]): ToolCall[] {
  const list: ToolCall[] = [];
  for (const i of items) {
    if (i.kind === 'tool_result') {
      let open = i.toolUseId ? list.findLastIndex((c) => c.kind === 'tool' && c.use?.toolUseId === i.toolUseId && !c.result) : -1;
      if (open < 0) open = list.findLastIndex((c) => c.kind === 'tool' && c.use && !c.result);
      if (open >= 0) list[open] = { ...list[open], result: i };
      else list.push({ key: i.id, kind: 'tool', result: i });
    } else if (i.kind === 'thinking') list.push({ key: i.id, kind: 'thinking', use: i });
    else list.push({ key: i.id, kind: 'tool', use: i });
  }
  return list;
}

export function verb(tool: string): [string, string] {
  switch (tool) {
    case 'Read': case 'NotebookRead': return ['Read', 'file'];
    case 'Edit': case 'MultiEdit': case 'str_replace': case 'NotebookEdit': return ['Edited', 'file'];
    case 'Write': return ['Wrote', 'file'];
    case 'Bash': case 'BashOutput': case 'shell': case 'exec': return ['Ran', 'command'];
    case 'Grep': case 'Glob': case 'search': return ['Searched', 'pattern'];
    case 'WebSearch': return ['Searched the web', 'query'];
    case 'WebFetch': return ['Fetched', 'page'];
    case 'Task': case 'Agent': return ['Ran', 'agent'];
    case 'TodoWrite': return ['Updated', 'todo list'];
    default: return [tool, 'call'];
  }
}

export function toolIcon(tool: string): string {
  switch (verb(tool)[0]) {
    case 'Ran': return tool === 'Task' || tool === 'Agent' ? '◆' : '$';
    case 'Read': return '◉';
    case 'Edited': case 'Wrote': return '✎';
    case 'Searched': case 'Searched the web': return '⌕';
    case 'Fetched': return '↗';
    case 'Updated': return '☰';
    default: return '•';
  }
}

/** "Read 3 files · Ran 2 commands · thought" */
export function phrase(tools: string[], thinking: number): string {
  const groups = new Map<string, { v: string; n: string; count: number }>();
  for (const t of tools) {
    const [v, n] = verb(t);
    const k = v + '|' + n;
    const g = groups.get(k) ?? { v, n, count: 0 };
    g.count++;
    groups.set(k, g);
  }
  const parts = [...groups.values()].map(({ v, n, count }) => {
    if (v === n || n === 'call') return count === 1 ? v : `${v} ×${count}`;
    const plural = n.endsWith('y') ? n.slice(0, -1) + 'ies' : n + 's';
    return `${v} ${count} ${count === 1 ? n : plural}`;
  });
  if (thinking > 0) parts.push('thought');
  return parts.join(' · ');
}
