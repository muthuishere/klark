import { useEffect, useMemo, useRef, useState } from 'react';
import { Link, NavLink, useParams } from 'react-router-dom';
import { useAuth } from '../auth';
import { agentIcon, agentKey, displayName, groupRows, paneAgent, paneCwd, paneStatus, workspaceLabel, type Row } from '../lib/panes';
import { readStore, setTheme, THEMES, writeStore, currentTheme } from '../lib/theme';
import { useMachines } from '../shell';
import { Menu } from './Menu';
import { IconSidebar } from './Icons';

export const paneHref = (m: string, p: string) => `/m/${encodeURIComponent(m)}/p/${encodeURIComponent(p)}`;

export function Sidebar() {
  const { items, loaded } = useMachines();
  const { me, signOut } = useAuth();
  const params = useParams();
  const [filter, setFilter] = useState(() => { try { return sessionStorage.getItem('klark:home:filter') ?? ''; } catch { return ''; } });
  const [showAll, setShowAll] = useState(() => readStore('showall') === '1');
  const [folded, setFolded] = useState<Set<string>>(() => new Set((readStore('folded') ?? '').split('\n').filter(Boolean)));
  const [theme, setThemeState] = useState(currentTheme);
  const search = useRef<HTMLInputElement>(null);

  useEffect(() => { try { sessionStorage.setItem('klark:home:filter', filter); } catch { /* ignore */ } }, [filter]);
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
    writeStore('folded', [...n].join('\n'));
  };
  const closeDrawer = () => document.documentElement.classList.remove('drawer-open');

  return (
    <>
      <aside className="side" aria-label="Agents">
        <div className="side-head">
          <Link className="brand" to="/" onClick={closeDrawer}><span className="brand-text">klark</span></Link>
          <button type="button" className="icon side-x" aria-label="Close sidebar" title="Close sidebar" onClick={() => { document.documentElement.classList.add('side-closed'); writeStore('side', 'closed'); }}><IconSidebar /></button>
        </div>
        <div className="side-search">
          <input ref={search} type="search" placeholder="Search agents" aria-label="Search agents" value={filter} onChange={(e) => setFilter(e.target.value)} />
          <kbd>⌘K</kbd>
        </div>
        <nav className="side-list" aria-label="Panes">
          {loaded && items.length === 0 && <p className="empty pad">No machines are linked yet. On a machine, run <code>klark hub connect</code>.</p>}
          {groups.map((g) => {
            const open = !folded.has(g.key);
            return (
              <section className="proj" key={g.key}>
                <button type="button" className="proj-head" title={g.path} aria-expanded={open} onClick={() => fold(g.key)}>
                  <span className="chev">{open ? '▾' : '▸'}</span><span className="proj-name">{g.name}</span><span className="proj-n">{g.rows.length}</span>
                </button>
                {open && g.rows.map((r) => {
                  const st = paneStatus(r.pane);
                  const agent = paneAgent(r.pane);
                  const cur = params.machine === r.machineId && params.pane === r.pane.pane_id;
                  return (
                    <NavLink key={r.machineId + '/' + r.pane.pane_id} to={paneHref(r.machineId, r.pane.pane_id)} className={'ent' + (cur ? ' cur' : '')} title={paneCwd(r.pane)} onClick={closeDrawer} aria-current={cur ? 'page' : undefined}>
                      <span className={'dot dot-' + (st || 'none')} aria-hidden="true" />
                      <span className="badge-a" data-agent={agentKey(agent)} aria-hidden="true">{agentIcon(agent)}</span>
                      <span className="ent-title">{displayName(r.pane)}</span>
                      <span className="ent-time">{st === 'blocked' ? '!' : ''}</span>
                      <span className="ent-sub">
                        {st === 'blocked' && <b className="needs">needs you · </b>}
                        {agent && <><b className="kind">{agent.toLowerCase()}</b> · </>}
                        {r.machineName} · {r.session}
                      </span>
                    </NavLink>
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
                <Link key={machine.id} to={`/m/${encodeURIComponent(machine.id)}`} className="ent off" onClick={closeDrawer}>
                  <span className="dot dot-none" /><span className="ent-title">{machine.name || machine.id}</span><span />
                </Link>
              ))}
            </section>
          )}
        </nav>
        <div className="side-foot">
          <label className="toggle"><input type="checkbox" checked={showAll} onChange={(e) => { setShowAll(e.target.checked); writeStore('showall', e.target.checked ? '1' : null); }} /> Show all panes</label>
          <button type="button" className="icon" title="Next theme" aria-label="Next theme" onClick={() => { const i = THEMES.findIndex((t) => t.id === theme); const n = THEMES[(i + 1) % THEMES.length].id; setTheme(n); setThemeState(n); }}>◐</button>
          <Menu label="Account" up>
            <div className="menu-info">{me?.displayName || me?.username}</div>
            <div className="menu-info">Theme</div>
            <div className="theme-picks">
              {THEMES.map((t) => (
                <button key={t.id} type="button" className="theme-pick" aria-pressed={theme === t.id} onClick={() => { setTheme(t.id); setThemeState(t.id); }}><span style={{ background: t.color }} />{t.label}</button>
              ))}
            </div>
            <Link to="/messages">Messages</Link>
            {me?.admin && <><Link to="/admin/users">Users</Link><Link to="/admin/directory">Sign-in directory</Link><Link to="/admin/machines">Machines</Link></>}
            <button type="button" className="menu-item" onClick={() => void signOut()}>Sign out</button>
          </Menu>
        </div>
      </aside>
      <div className="scrim" aria-hidden="true" onClick={closeDrawer} />
    </>
  );
}
