import { useEffect, useState, type FormEvent } from 'react';
import { Navigate, useLocation, useNavigate, useSearchParams } from 'react-router-dom';
import { useAuth } from '../auth';
import { getProviders, login } from '../lib/api';
import { url } from '../lib/base';
import type { Providers } from '../lib/types';

const ERRORS: Record<string, string> = {
  adfs_failed: 'Sign-in with ADFS failed. Try again, or ask an admin.',
  adfs_unavailable: 'ADFS is not reachable right now.',
  oidc_failed: 'Sign-in with the identity provider failed. Try again, or ask an admin.',
  oidc_unavailable: 'The identity provider is not reachable right now.',
  access_denied: 'Your account is not allowed to use this hub.',
};
export function loginErrorText(code: string | null): string | null {
  if (!code) return null;
  return ERRORS[code] ?? (/_failed$/.test(code) ? 'Sign-in failed. Try again, or ask an admin.' : /_unavailable$/.test(code) ? 'The sign-in provider is not reachable right now.' : `Sign-in error: ${code.replace(/_/g, ' ')}.`);
}

export function Login() {
  const [params] = useSearchParams();
  const urlErr = loginErrorText(params.get('error'));
  const { me, setMe, loading } = useAuth();
  const nav = useNavigate();
  const from = (useLocation().state as { from?: string } | null)?.from || '/';
  const [prov, setProv] = useState<Providers | null>(null);
  const [user, setUser] = useState('');
  const [pw, setPw] = useState('');
  const [err, setErr] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => { getProviders().then(setProv).catch(() => setProv({ static: true })); }, []);
  if (!loading && me) return <Navigate to={from} replace />;

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    setBusy(true); setErr(null);
    try {
      setMe(await login(user, pw));
      nav(from, { replace: true });
    } catch (x) {
      setErr((x as Error).message === 'Error 401' || (x as { status?: number }).status === 401 ? 'Wrong user name or password.' : (x as Error).message);
    } finally { setBusy(false); }
  };
  const adfs = prov?.adfs?.enabled ? prov.adfs : null;
  const oidc = prov?.oidc ?? [];
  const dir = prov?.directory;
  const mock = dir?.mode === 'mockldap';
  const showStatic = prov ? prov.static !== false : true;

  return (
    <main className="login">
      <h1>Sign in to klark</h1>
      {urlErr && <p className="error" role="alert">{urlErr}</p>}
      {showStatic && (
        <form onSubmit={submit}>
          <label>User name<input name="username" autoComplete="username" autoCapitalize="none" required value={user} onChange={(e) => setUser(e.target.value)} /></label>
          <label>Password<input name="password" type="password" autoComplete="current-password" required value={pw} onChange={(e) => setPw(e.target.value)} /></label>
          {dir?.name && <p className="muted">Signs in admin users and {dir.name} users.</p>}
          {mock && <p className="muted">{dir?.name || 'Mock LDAP'}: try user1 / user1pass</p>}
          {err && <p className="error" role="alert">{err}</p>}
          <button type="submit" disabled={busy}>{busy ? 'Signing in…' : 'Sign in'}</button>
        </form>
      )}
      {adfs && <a className="btn-link" href={url(adfs.loginUrl || 'auth/adfs/login')}>Sign in with {adfs.name || 'ADFS'}</a>}
      {oidc.map((o) => <a key={o.id} className="btn-link" href={url(o.loginUrl || `auth/oidc/${o.id}/login`)}>Sign in with {o.name || o.id}</a>)}
      {prov && !showStatic && !adfs && oidc.length === 0 && <p className="empty">No sign-in method is configured on this hub.</p>}
    </main>
  );
}
