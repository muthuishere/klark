import { useState, type FormEvent, type ReactNode } from 'react';
import { NavLink } from 'react-router-dom';
import { api } from '../lib/api';
import { usePoll } from '../hooks';
import { PageBar } from '../shell';
import type { AdminMachine, AdminUser } from '../lib/types';

export function AdminFrame({ title, children }: { title: string; children: ReactNode }) {
  return (
    <main className="pane-app">
      <PageBar title={title} />
      <div className="page admin">
        <nav className="subnav" aria-label="Admin">
          <NavLink to="/admin/users">Users</NavLink><NavLink to="/admin/machines">Machines</NavLink><NavLink to="/admin/directory">Directory</NavLink>
        </nav>
        <h1>{title}</h1>
        {children}
      </div>
    </main>
  );
}

export function Notice({ ok, err }: { ok: string | null; err: string | null }) {
  return <>{ok && <p className="ok" role="status">{ok}</p>}{err && <p className="error" role="alert">{err}</p>}</>;
}

export function AdminUsers() {
  const { data, error, reload } = usePoll<AdminUser[]>('api/admin/users', 0);
  const [ok, setOk] = useState<string | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [f, setF] = useState({ username: '', displayName: '', password: '', admin: false, source: 'local', groups: '' });
  const act = async (p: Promise<unknown>, msg: string) => { setOk(null); setErr(null); try { await p; setOk(msg); reload(); } catch (e) { setErr((e as Error).message); } };
  const add = async (e: FormEvent) => { e.preventDefault(); await act(api('api/admin/users', { method: 'POST', json: { ...f, groups: f.source === 'mockldap' ? f.groups.split(',').map((g) => g.trim()).filter(Boolean) : undefined } }), `Saved ${f.username}.`); setF({ username: '', displayName: '', password: '', admin: false, source: 'local', groups: '' }); };
  return (
    <AdminFrame title="Users">
      <Notice ok={ok} err={err ?? error} />
      <ul className="rows">
        {(data ?? []).map((u) => (
          <li className="row" key={u.username}>
            <div><b>{u.username}</b> <span className="muted">{u.displayName} {u.admin ? '· admin' : ''} {u.source ? '· ' + u.source : ''}{u.groups?.length ? ' · groups: ' + u.groups.join(', ') : ''}</span></div>
            <button type="button" className="danger" aria-label={'Delete ' + u.username} onClick={() => { if (confirm(`Delete ${u.username}?`)) void act(api(`api/admin/users/${encodeURIComponent(u.username)}`, { method: 'DELETE' }), `Deleted ${u.username}.`); }}>Delete</button>
          </li>
        ))}
      </ul>
      <h2>Add or update a user</h2>
      <form className="stack" onSubmit={add}>
        <label>Name<input autoComplete="off" required value={f.username} onChange={(e) => setF({ ...f, username: e.target.value })} /></label>
        <label>Display name<input autoComplete="off" value={f.displayName} onChange={(e) => setF({ ...f, displayName: e.target.value })} /></label>
        <label>Source<select value={f.source} onChange={(e) => setF({ ...f, source: e.target.value })}><option value="local">local</option><option value="mockldap">mockldap (test directory user)</option></select></label>
        {f.source === 'mockldap' && <label>Groups (comma separated)<input autoComplete="off" value={f.groups} onChange={(e) => setF({ ...f, groups: e.target.value })} /></label>}
        <label>Password<input type="password" autoComplete="new-password" minLength={f.source === 'mockldap' ? undefined : 8} required value={f.password} onChange={(e) => setF({ ...f, password: e.target.value })} /></label>
        <label className="check"><input type="checkbox" checked={f.admin} onChange={(e) => setF({ ...f, admin: e.target.checked })} /> Administrator</label>
        <button type="submit">Save user</button>
      </form>
    </AdminFrame>
  );
}

export function AdminMachines() {
  const { data, error, reload } = usePoll<AdminMachine[]>('api/admin/machines', 0);
  const [err, setErr] = useState<string | null>(null);
  const revoke = async (name: string) => { try { await api(`api/admin/machines/${encodeURIComponent(name)}`, { method: 'DELETE' }); reload(); } catch (e) { setErr((e as Error).message); } };
  return (
    <AdminFrame title="Machines">
      <Notice ok={null} err={err ?? error} />
      <p className="muted">To add a machine, run <code>klark hub connect</code> on it and approve the code it shows. Revoking drops its live link, and its token stops working.</p>
      <ul className="rows">
        {(data ?? []).map((m) => (
          <li className="row" key={m.name}>
            <div><b>{m.name}</b> <span className="muted">{m.status}{m.approvedBy ? ` · approved by ${m.approvedBy}` : ''}{m.created ? ', ' + new Date(m.created).toLocaleString() : ''}</span></div>
            <button type="button" className="danger" aria-label={'Revoke ' + m.name} onClick={() => { if (confirm(`Revoke ${m.name}?`)) void revoke(m.name); }}>Revoke</button>
          </li>
        ))}
      </ul>
      {data && data.length === 0 && <p className="muted">No machines yet.</p>}
    </AdminFrame>
  );
}
