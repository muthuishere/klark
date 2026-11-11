import { useEffect, useState, type FormEvent } from 'react';
import { Navigate, useLocation, useNavigate } from 'react-router-dom';
import { useAuth } from '../auth';
import { getProviders, login } from '../lib/api';
import { url } from '../lib/base';
import type { Providers } from '../lib/types';

export function Login() {
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
  const showStatic = prov ? prov.static !== false : true;

  return (
    <main className="login">
      <h1>Sign in to klark</h1>
      {showStatic && (
        <form onSubmit={submit}>
          <label>User name<input name="username" autoComplete="username" autoCapitalize="none" required value={user} onChange={(e) => setUser(e.target.value)} /></label>
          <label>Password<input name="password" type="password" autoComplete="current-password" required value={pw} onChange={(e) => setPw(e.target.value)} /></label>
          {err && <p className="error" role="alert">{err}</p>}
          <button type="submit" disabled={busy}>{busy ? 'Signing in…' : 'Sign in'}</button>
        </form>
      )}
      {adfs && <a className="btn-link" href={url(adfs.loginUrl || 'auth/adfs/login')}>Sign in with {adfs.name || 'ADFS'}</a>}
      {prov && !showStatic && !adfs && <p className="empty">No sign-in method is configured on this hub.</p>}
    </main>
  );
}
