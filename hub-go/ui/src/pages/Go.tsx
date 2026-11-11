import { Link, Navigate, useSearchParams } from 'react-router-dom';
import { PageBar, useMachines } from '../shell';
import { paneHref } from '../components/Sidebar';

/** A shared link from `klark expose url`: find the machine that reported that host name, then open the pane. */
export function Go() {
  const [q] = useSearchParams();
  const machine = q.get('machine') ?? '';
  const pane = q.get('pane') ?? '';
  const { items, loaded } = useMachines();
  if (!loaded) return <main className="pane-app"><p className="empty pad" role="status">Opening…</p></main>;
  const lc = machine.toLowerCase();
  const found = items.filter(({ machine: m }) => m.id.toLowerCase() === lc || m.name.toLowerCase() === lc || m.id.toLowerCase().endsWith(':' + lc));
  if (machine && pane && found.length === 1) return <Navigate to={paneHref(found[0].machine.id, pane)} replace />;
  return (
    <main className="pane-app">
      <PageBar title="Opening pane" />
      <div className="page">
        {found.length === 0 ? <p className="empty">No machine called <b>{machine}</b> that you can see is linked to this hub.</p> : (
          <>
            <h1>Which machine?</h1>
            <ul className="panes">{found.map(({ machine: m }) => <li key={m.id}><Link className="ent" to={paneHref(m.id, pane)}>{m.id}</Link></li>)}</ul>
          </>
        )}
      </div>
    </main>
  );
}
