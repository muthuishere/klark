import { useEffect, useState } from 'react';
import { Link, useParams } from 'react-router-dom';
import { PageBar, useMachines } from '../shell';
import { paneHref } from '../components/AppSidebar';
import { transport } from '../lib/transport';
import { type FeedInfo, agentIcon, agentKey, displayName, paneAgent, paneCwd, paneStatus } from '@muthuishere/klark-hub-ui';

/** One machine's session tree: workspaces, their tabs, their panes. */
export function MachinePage() {
  const { machine = '' } = useParams();
  const { items, loaded } = useMachines();
  const m = items.find((i) => i.machine.id === machine);
  const tree = m?.tree;
  const online = !!m?.machine.online;
  const [feeds, setFeeds] = useState<FeedInfo[]>([]);
  useEffect(() => {
    if (!online || !transport.feeds) { setFeeds([]); return; }
    let live = true;
    const load = () => transport.feeds!(machine).then((f) => { if (live) setFeeds(f); }, () => { /* an older machine has no feed.list: show none */ });
    load();
    const t = setInterval(load, 15000);
    return () => { live = false; clearInterval(t); };
  }, [machine, online]);
  return (
    <main className="pane-app">
      <PageBar title={m?.machine.name || machine} />
      <div className="page">
        {!loaded && <p className="empty" role="status">Loading…</p>}
        {loaded && !m && <p className="empty">No machine called <b>{machine}</b>.</p>}
        {m && !m.machine.online && <p className="muted">Offline{m.machine.lastSeen ? `, last seen ${new Date(m.machine.lastSeen).toLocaleString()}` : ''}.</p>}
        {(tree?.workspaces ?? []).map((w) => (
          <section key={w.workspace_id} aria-label={w.label || `workspace ${w.number}`}>
            <h2>{w.label || `workspace ${w.number ?? ''}`}</h2>
            {(tree?.tabs ?? []).filter((t) => t.workspace_id === w.workspace_id).map((t) => (
              <div key={t.tab_id}>
                <h3 className="muted">{t.label || `tab ${t.number ?? ''}`}</h3>
                <ul className="panes">
                  {(tree?.panes ?? []).filter((p) => p.tab_id === t.tab_id).map((p) => (
                    <li key={p.pane_id}>
                      <Link className="ent" to={paneHref(machine, p.pane_id)}>
                        <span className={'dot dot-' + (paneStatus(p) || 'none')} />
                        <span className="badge-a" data-agent={agentKey(paneAgent(p))}>{agentIcon(paneAgent(p))}</span>
                        <span className="ent-title">{displayName(p)}</span><span className="ent-time">{paneStatus(p)}</span>
                        <span className="ent-sub">{paneCwd(p)}</span>
                      </Link>
                    </li>
                  ))}
                </ul>
              </div>
            ))}
          </section>
        ))}
        {feeds.length > 0 && (
          <section aria-label="feeds">
            <h2>Feeds</h2>
            <p className="muted">Read-only: feeds are added, stopped and started on the machine.</p>
            <ul className="panes">
              {feeds.map((f) => (
                <li key={f.id} className="ent">
                  <span className={'dot dot-' + (f.running ? 'working' : 'none')} />
                  <span className="ent-title">{f.label || f.id}</span>
                  <span className="ent-time">{f.state}</span>
                  <span className="ent-sub">{f.kind} → {f.agents.join(', ')}{f.last_delivery ? ` · last sent ${new Date(f.last_delivery * 1000).toLocaleString()}` : ''}</span>
                </li>
              ))}
            </ul>
          </section>
        )}
      </div>
    </main>
  );
}
