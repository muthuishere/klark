import { useEffect, useMemo } from 'react';
import { Link, useNavigate, useParams, useSearchParams } from 'react-router-dom';
import { HubPane, IconBack, IconSidebar, paneAgent, paneCwd, workspaceLabel } from '@muthuishere/klark-hub-ui';
import { Menu } from '../components/Menu';
import { appPath, url } from '../lib/base';
import { slashCommandsFor } from '../lib/slash';
import { nextTheme, toggleSide, writeStore } from '../lib/theme';
import { transport } from '../lib/transport';
import { useMachines } from '../shell';

/** klark's own pane page: the shared HubPane, with klark's routes, menu and slash commands. */
export function PanePage() {
  const { machine = '', pane = '' } = useParams();
  const [q, setQ] = useSearchParams();
  const view = q.get('view') === 'terminal' ? 'terminal' : 'chat';
  const { items: machines } = useMachines();
  const nav = useNavigate();
  const m = machines.find((x) => x.machine.id === machine);
  const info = m?.tree?.panes?.find((p) => p.pane_id === pane);
  const session = info && m ? workspaceLabel(m.tree, info.workspace_id) : '';
  const agent = info ? paneAgent(info) : '';
  const commands = useMemo(() => slashCommandsFor(agent), [agent]);

  useEffect(() => { writeStore('lastpane', `${machine}\n${pane}`); }, [machine, pane]);

  // File links in the chat open the viewer without leaving the app.
  const onNavigate = (href: string, a: HTMLAnchorElement) => {
    const p = appPath(href);
    if (p && (a.classList.contains('fpath') || /^\/m\/[^/]+\/f(\?|$)/.test(p))) { nav(p); return true; }
    return false;
  };

  return (
    <HubPane
      transport={transport} machine={machine} pane={pane} info={info} online={m ? m.machine.online : true} session={session}
      view={view}
      onViewChange={(v) => { const n = new URLSearchParams(q); if (v === 'chat') n.delete('view'); else n.set('view', 'terminal'); setQ(n, { replace: true }); }}
      offlineText={<>Machine offline: run <code>klark hub connect</code> on it to send messages</>}
      commands={commands} onNavigate={onNavigate} offerHref={(o) => url(o.url)}
      leading={<>
        <button type="button" className="icon side-open" aria-label="Sidebar" title="Sidebar" onClick={toggleSide}><IconSidebar /></button>
        <Link className="icon back" to="/" aria-label="Back"><IconBack /></Link>
      </>}
      renderActions={({ view: v, searchOpen, toggleSearch, agent: ag, status, offline }) => (
        <Menu label="More">
          {v === 'chat' && <button type="button" className="menu-item" onClick={toggleSearch}>{searchOpen ? 'Close search' : 'Search in chat'}</button>}
          <button type="button" className="menu-item" onClick={() => nextTheme()}>Next theme</button>
          <div className="menu-info">
            <div>{ag || 'shell'} · {offline ? 'offline' : status || 'ready'}</div>
            <div>{machine}{session ? ' · ' + session : ''}</div>
            {info && paneCwd(info) && <div className="mono">{paneCwd(info)}</div>}
          </div>
        </Menu>
      )}
    />
  );
}
