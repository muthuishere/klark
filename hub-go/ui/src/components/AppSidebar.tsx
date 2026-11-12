import { useEffect, useState } from 'react';
import { Link, useNavigate, useParams } from 'react-router-dom';
import { IconSidebar, Sidebar, webStorage } from '@muthuishere/klark-hub-ui';
import { useAuth } from '../auth';
import { url } from '../lib/base';
import { currentTheme, setTheme, THEMES, writeStore } from '../lib/theme';
import { useMachines } from '../shell';
import { Menu } from './Menu';

export const paneHref = (m: string, p: string) => `/m/${encodeURIComponent(m)}/p/${encodeURIComponent(p)}`;
const machineHref = (m: string) => `/m/${encodeURIComponent(m)}`;
const store = webStorage(() => localStorage, 'klark:');
const closeDrawer = () => document.documentElement.classList.remove('drawer-open');

/** The package Sidebar with klark's brand, routes, account menu and theme picker. */
export function AppSidebar() {
  const { items, loaded } = useMachines();
  const { me, signOut } = useAuth();
  const params = useParams();
  const nav = useNavigate();
  const [filter, setFilter] = useState(() => { try { return sessionStorage.getItem('klark:home:filter') ?? ''; } catch { return ''; } });
  const [theme, setThemeState] = useState(currentTheme);
  useEffect(() => { try { sessionStorage.setItem('klark:home:filter', filter); } catch { /* ignore */ } }, [filter]);
  const route = (m: string, p?: string) => (p ? paneHref(m, p) : machineHref(m));

  return (
    <>
      <Sidebar
        items={items} loaded={loaded} store={store} filter={filter} onFilterChange={setFilter}
        current={params.machine && params.pane ? { machine: params.machine, pane: params.pane } : undefined}
        hrefFor={(m, p) => url(route(m, p).slice(1))}
        onOpen={(m, p) => { closeDrawer(); nav(route(m, p)); }}
        empty={<p className="empty pad">No machines are linked yet. On a machine, run <code>klark hub connect</code>.</p>}
        header={<>
          <Link className="brand" to="/" onClick={closeDrawer}><span className="brand-text">klark</span></Link>
          <button type="button" className="icon side-x" aria-label="Close sidebar" title="Close sidebar" onClick={() => { document.documentElement.classList.add('side-closed'); writeStore('side', 'closed'); }}><IconSidebar /></button>
        </>}
        footer={<>
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
        </>}
      />
      <div className="scrim" aria-hidden="true" onClick={closeDrawer} />
    </>
  );
}
