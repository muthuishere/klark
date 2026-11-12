import { useEffect, useMemo, useRef, useState, type MouseEvent, type ReactNode } from 'react';
import { agentIcon, agentKey, displayName, groupRows, paneAgent, paneCwd, paneStatus, workspaceLabel, type Group, type Row } from '../panes';
import { defaultStorage, type Storage } from '../storage';
import type { MachineTree } from '../types';

export interface SidebarProps {
  /** Machines and their trees (Transport.machines + tree). Offline machines are listed apart. */
  items: MachineTree[];
  loaded?: boolean;
  /** The open agent, marked current. */
  current?: { machine: string; pane: string };
  /** The link of an agent (or of a machine, when pane is undefined); used as the anchor's href. */
  hrefFor?: (machine: string, pane?: string) => string;
  /** A plain left click on an entry (the default navigation is prevented when given). */
  onOpen?: (machine: string, pane: string | undefined, e: MouseEvent) => void;
  /** Controlled search text; omit to keep it inside. */
  filter?: string;
  onFilterChange?: (f: string) => void;
  /** Where folded groups (`folded`) and the show-all toggle (`showall`) are remembered. Default localStorage. */
  store?: Storage;
  /** Extra content in a group's header (after its count). */
  renderGroupExtra?: (g: Group) => ReactNode;
  /** A row's second line. Default: "needs you · kind · machine · session". The "needs you" mark stays in front either way. */
  renderSubtitle?: (r: Row) => ReactNode;
  header?: ReactNode;
  /** Right side of the footer (account menu, theme...). */
  footer?: ReactNode;
  /** Shown when there are no machines. */
  empty?: ReactNode;
}

const plainClick = (e: MouseEvent) => e.button === 0 && !e.metaKey && !e.ctrlKey && !e.shiftKey && !e.altKey;

/** Agents grouped by project folder, needs-you first; search with ⌘K. */
export function Sidebar({ items, loaded = true, current, hrefFor, onOpen, filter: filterProp, onFilterChange, store = defaultStorage, renderGroupExtra, renderSubtitle, header, footer, empty }: SidebarProps) {
  const [own, setOwn] = useState('');
  const filter = filterProp ?? own;
  const setFilter = (f: string) => { if (filterProp === undefined) setOwn(f); onFilterChange?.(f); };
  const [showAll, setShowAll] = useState(() => store.get('showall') === '1');
  const [folded, setFolded] = useState<Set<string>>(() => new Set((store.get('folded') ?? '').split('\n').filter(Boolean)));
  const search = useRef<HTMLInputElement>(null);

  useEffect(() => {
    const k = (e: KeyboardEvent) => { if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 'k') { e.preventDefault(); search.current?.focus(); } };
    window.addEventListener('keydown', k);
    return () => window.removeEventListener('keydown', k);
  }, []);

  const rows: Row[] = useMemo(() => items.filter((i) => i.machine.online).flatMap(({ machine, tree }) =>
    (tree?.panes ?? []).map((pane) => ({ machineId: machine.id, machineName: machine.name, pane, session: workspaceLabel(tree, pane.workspace_id) }))), [items]);
  const groups = useMemo(() => groupRows(rows, filter, showAll), [rows, filter, showAll]);
  const offline = items.filter((i) => !i.machine.online);

  const fold = (k: string) => {
    const n = new Set(folded);
    if (!n.delete(k)) n.add(k);
    setFolded(n);
    store.set('folded', [...n].join('\n'));
  };
  const click = (m: string, p?: string) => (e: MouseEvent) => {
    if (!onOpen || !plainClick(e)) return;
    e.preventDefault();
    onOpen(m, p, e);
  };

  return (
    <aside className="side" aria-label="Agents">
      {header && <div className="side-head">{header}</div>}
      <div className="side-search">
        <input ref={search} type="search" placeholder="Search agents" aria-label="Search agents" value={filter} onChange={(e) => setFilter(e.target.value)} />
        <kbd>⌘K</kbd>
      </div>
      <nav className="side-list" aria-label="Panes">
        {loaded && items.length === 0 && (empty ?? <p className="empty pad">No machines are linked yet.</p>)}
        {groups.map((g) => {
          const open = !folded.has(g.key);
          return (
            <section className="proj" key={g.key}>
              <button type="button" className="proj-head" title={g.path} aria-expanded={open} onClick={() => fold(g.key)}>
                <span className="chev">{open ? '▾' : '▸'}</span><span className="proj-name">{g.name}</span><span className="proj-n">{g.rows.length}</span>
                {renderGroupExtra?.(g)}
              </button>
              {open && g.rows.map((r) => {
                const st = paneStatus(r.pane);
                const agent = paneAgent(r.pane);
                const cur = current?.machine === r.machineId && current?.pane === r.pane.pane_id;
                return (
                  <a key={r.machineId + '/' + r.pane.pane_id} href={hrefFor?.(r.machineId, r.pane.pane_id)} className={'ent' + (cur ? ' cur' : '')} title={paneCwd(r.pane)}
                    onClick={click(r.machineId, r.pane.pane_id)} aria-current={cur ? 'page' : undefined}>
                    <span className={'dot dot-' + (st || 'none')} aria-hidden="true" />
                    <span className="badge-a" data-agent={agentKey(agent)} aria-hidden="true">{agentIcon(agent)}</span>
                    <span className="ent-title">{displayName(r.pane)}</span>
                    <span className="ent-time">{st === 'blocked' ? '!' : ''}</span>
                    <span className="ent-sub">
                      {st === 'blocked' && <b className="needs">needs you · </b>}
                      {renderSubtitle ? renderSubtitle(r) : <>
                        {agent && <><b className="kind">{agent.toLowerCase()}</b> · </>}
                        {r.machineName} · {r.session}
                      </>}
                    </span>
                  </a>
                );
              })}
            </section>
          );
        })}
        {filter && groups.length === 0 && <p className="empty pad">No match.</p>}
        {offline.length > 0 && !filter && (
          <section className="proj">
            <div className="proj-head static"><span className="proj-name">Offline machines</span></div>
            {offline.map(({ machine }) => (
              <a key={machine.id} href={hrefFor?.(machine.id)} className="ent off" onClick={click(machine.id)}>
                <span className="dot dot-none" /><span className="ent-title">{machine.name || machine.id}</span><span />
              </a>
            ))}
          </section>
        )}
      </nav>
      <div className="side-foot">
        <label className="toggle"><input type="checkbox" checked={showAll} onChange={(e) => { setShowAll(e.target.checked); store.set('showall', e.target.checked ? '1' : null); }} /> Show all panes</label>
        {footer}
      </div>
    </aside>
  );
}
