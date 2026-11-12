import { useEffect, useState, type FormEvent } from 'react';
import { api } from '../lib/api';
import { usePoll } from '../hooks';
import { AdminFrame, Notice } from './Admin';
import type { DirectoryInfo, DirectorySettings } from '../lib/types';

const toText = (v: string[] | string | undefined) => (Array.isArray(v) ? v.join(', ') : v ?? '');
const toList = (s: string) => s.split(/[,\n]/).map((x) => x.trim()).filter(Boolean);

export function AdminDirectory() {
  const { data, error, reload } = usePoll<DirectoryInfo>('api/admin/directory', 0);
  const [f, setF] = useState<DirectorySettings>({});
  const [allowed, setAllowed] = useState('');
  const [pw, setPw] = useState('');
  const [ok, setOk] = useState<string | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [t, setT] = useState({ username: '', password: '' });
  const [res, setRes] = useState<{ ok: boolean; message: string } | null>(null);
  const [copied, setCopied] = useState<string | null>(null);
  useEffect(() => { if (data) { const st = data.settings ?? {}; setF(st); setAllowed(toText(st.allowedGroups)); } }, [data]);

  const set = <K extends keyof DirectorySettings>(k: K, v: DirectorySettings[K]) => setF((p) => ({ ...p, [k]: v }));
  const body = () => {
    const { bindPasswordSet: _s, oidc: _o, bindPassword: _p, ...rest } = f;
    return { ...rest, allowedGroups: toList(allowed), ...(pw ? { bindPassword: pw } : {}) };
  };
  const save = async (e: FormEvent) => {
    e.preventDefault(); setOk(null); setErr(null);
    try { await api('api/admin/directory', { method: 'PUT', json: body() }); setPw(''); setOk('Saved.'); reload(); } catch (x) { setErr((x as Error).message); }
  };
  const test = async (e: FormEvent) => {
    e.preventDefault(); setRes(null);
    try { setRes(await api<{ ok: boolean; message: string }>('api/admin/directory/test', { method: 'POST', json: { settings: body(), ...t } })); }
    catch (x) { setRes({ ok: false, message: (x as Error).message }); }
  };
  const copy = async (id: string, text: string) => {
    try { await navigator.clipboard.writeText(text); setCopied(id); } catch { setCopied(null); }
  };

  const text = (k: keyof DirectorySettings, label: string, extra: { type?: string } = {}) => (
    <label>{label}<input autoComplete="off" type={extra.type} value={String(f[k] ?? '')} onChange={(e) => set(k, (extra.type === 'number' ? Number(e.target.value) : e.target.value) as never)} /></label>
  );
  const check = (k: keyof DirectorySettings, label: string) => (
    <label className="check"><input type="checkbox" checked={!!f[k]} onChange={(e) => set(k, e.target.checked as never)} /> {label}</label>
  );
  const mode = f.mode ?? data?.mode;
  const live = data?.mode;

  return (
    <AdminFrame title="Directory">
      <Notice ok={ok} err={err ?? error} />
      <p>Mode: <b data-testid="dir-mode">{live === 'mockldap' ? 'Mock LDAP' : live === 'ldap' ? 'LDAP' : 'local'}</b>{data?.source && <span className="muted"> · from {data.source === 'admin' ? 'saved settings' : 'environment'}</span>}</p>
      <form className="stack" onSubmit={save} aria-label="LDAP settings">
        <label>Mode<select value={mode ?? 'local'} onChange={(e) => set('mode', e.target.value)}><option value="local">local (no directory)</option><option value="mockldap">mockldap (test users)</option><option value="ldap">ldap</option></select></label>
        {text('host', 'Host')}
        {text('port', 'Port', { type: 'number' })}
        {check('ssl', 'Use SSL (LDAPS)')}
        {check('startTls', 'Use StartTLS')}
        {check('skipCertCheck', 'Skip certificate check (not for production)')}
        {text('timeoutSeconds', 'Timeout (seconds)', { type: 'number' })}
        {text('bindDn', 'Bind DN')}
        <label>Bind password {f.bindPasswordSet && <span className="muted" data-testid="pw-set">(set; leave empty to keep)</span>}
          <input type="password" autoComplete="new-password" value={pw} placeholder={f.bindPasswordSet ? 'unchanged' : ''} onChange={(e) => setPw(e.target.value)} /></label>
        {text('userDnTemplate', 'User DN template')}
        {text('userSearchBase', 'User search base')}
        {text('userFilter', 'User filter')}
        {text('groupSearchBase', 'Group search base')}
        {text('groupFilter', 'Group filter')}
        {check('useMemberOf', 'Read groups from memberOf')}
        {text('displayNameAttr', 'Display name attribute')}
        {text('adminGroup', 'Admin group')}
        <label>Allowed groups (comma separated)<input autoComplete="off" value={allowed} onChange={(e) => setAllowed(e.target.value)} /></label>
        <button type="submit">Save</button>
      </form>

      <h2>Test connection</h2>
      <form className="stack" onSubmit={test} aria-label="Test connection">
        <label>Test user<input autoComplete="off" required value={t.username} onChange={(e) => setT({ ...t, username: e.target.value })} /></label>
        <label>Test password<input type="password" autoComplete="new-password" required value={t.password} onChange={(e) => setT({ ...t, password: e.target.value })} /></label>
        <button type="submit">Test connection</button>
      </form>
      {res && <p className={res.ok ? 'ok' : 'error'} role="status">{res.ok ? 'OK: ' : 'Failed: '}{res.message}</p>}

      <h2>OIDC providers</h2>
      {(data?.oidc ?? []).length === 0 ? <p className="muted">No extra OIDC providers are configured.</p> : (
        <table className="tbl"><thead><tr><th>Provider</th><th>Redirect URI to register</th><th /></tr></thead><tbody>
          {(data?.oidc ?? []).map((o) => (
            <tr key={o.id}><td>{o.name || o.id}</td><td><code>{o.redirectUri}</code></td>
              <td><button type="button" aria-label={'Copy redirect URI of ' + (o.name || o.id)} onClick={() => void copy(o.id, o.redirectUri ?? '')}>{copied === o.id ? 'Copied' : 'Copy'}</button></td></tr>
          ))}
        </tbody></table>
      )}
    </AdminFrame>
  );
}
