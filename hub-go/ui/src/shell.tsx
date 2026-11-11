import { createContext, useContext, useEffect } from 'react';
import { Outlet, useLocation } from 'react-router-dom';
import { Sidebar } from './components/Sidebar';
import { useMachineTrees, type MachineTree } from './hooks';
import { readStore, toggleSide } from './lib/theme';

const MachinesCtx = createContext<{ items: MachineTree[]; loaded: boolean }>({ items: [], loaded: false });
export const useMachines = () => useContext(MachinesCtx);

/** The signed-in frame: sidebar (a drawer on phones) plus the page. */
export function Shell() {
  const { items, loaded } = useMachineTrees();
  const loc = useLocation();
  const home = loc.pathname === '/';
  useEffect(() => {
    document.documentElement.classList.toggle('side-closed', readStore('side') === 'closed');
  }, []);
  useEffect(() => { document.documentElement.classList.remove('drawer-open'); }, [loc.pathname]);
  return (
    <MachinesCtx.Provider value={{ items, loaded }}>
      <div className={'app' + (home ? ' home' : '')}>
        <Sidebar />
        <Outlet />
      </div>
    </MachinesCtx.Provider>
  );
}

/** A top bar for the non-pane pages: drawer toggle and a title. */
export function PageBar({ title }: { title: string }) {
  return (
    <header className="pane-bar">
      <button type="button" className="icon side-open" aria-label="Sidebar" onClick={toggleSide}>☰</button>
      <div className="who"><span className="name">{title}</span></div>
    </header>
  );
}
