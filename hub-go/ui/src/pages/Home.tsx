import { Navigate } from 'react-router-dom';
import { PageBar, useMachines } from '../shell';
import { paneHref } from '../components/Sidebar';
import { readStore } from '../lib/theme';

export function Home() {
  const { items, loaded } = useMachines();
  const wide = typeof matchMedia === 'function' && !matchMedia('(max-width: 860px)').matches;
  const last = readStore('lastpane');
  if (wide && last) {
    const [m, p] = last.split('\n');
    const ok = items.some((i) => i.machine.id === m && i.tree?.panes?.some((x) => x.pane_id === p));
    if (ok) return <Navigate to={paneHref(m, p)} replace />;
  }
  return (
    <main className="pane-app home-main">
      <PageBar title="klark" />
      <div className="pane-body home-main">
        <div className="home-empty">
          <div className="mark" aria-hidden="true">✻</div>
          <h1>{loaded && items.length === 0 ? 'No machines yet' : 'Pick an agent'}</h1>
          <p className="muted">{loaded && items.length === 0 ? <>Run <code>klark hub connect</code> on a machine.</> : 'Choose a pane from the list to read its chat.'}</p>
        </div>
      </div>
    </main>
  );
}
