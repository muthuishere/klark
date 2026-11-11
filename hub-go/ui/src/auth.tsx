import { createContext, useCallback, useContext, useEffect, useMemo, useState, type ReactNode } from 'react';
import { Navigate, useLocation } from 'react-router-dom';
import { ApiError, getMe, logout as apiLogout, onUnauthorized } from './lib/api';
import type { Me } from './lib/types';

interface AuthState { me: Me | null; loading: boolean; error: string | null; setMe: (m: Me | null) => void; signOut: () => Promise<void> }
const Ctx = createContext<AuthState | null>(null);

export function AuthProvider({ children }: { children: ReactNode }) {
  const [me, setMe] = useState<Me | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let live = true;
    getMe()
      .then((m) => live && setMe(m))
      .catch((e) => { if (live && !(e instanceof ApiError && e.status === 401)) setError(e.message); })
      .finally(() => live && setLoading(false));
    return () => { live = false; };
  }, []);
  // Any later 401 (cookie expired) drops the user back to the login page.
  useEffect(() => onUnauthorized(() => setMe(null)), []);

  const signOut = useCallback(async () => { try { await apiLogout(); } finally { setMe(null); } }, []);
  const value = useMemo(() => ({ me, loading, error, setMe, signOut }), [me, loading, error, signOut]);
  return <Ctx.Provider value={value}>{children}</Ctx.Provider>;
}

export function useAuth(): AuthState {
  const c = useContext(Ctx);
  if (!c) throw new Error('useAuth outside AuthProvider');
  return c;
}

/** Renders children only for a signed-in user; 401 goes to /login and comes back afterwards. */
export function RequireAuth({ children, admin }: { children: ReactNode; admin?: boolean }) {
  const { me, loading, error } = useAuth();
  const loc = useLocation();
  if (loading) return <p className="empty pad" role="status">Loading…</p>;
  if (error) return <p className="error pad" role="alert">{error}</p>;
  if (!me) return <Navigate to="/login" replace state={{ from: loc.pathname + loc.search }} />;
  if (admin && !me.admin) return <p className="empty pad" role="alert">Admins only.</p>;
  return <>{children}</>;
}
