import { url } from './base';
import type { Me, Providers } from './types';

export class ApiError extends Error {
  constructor(public status: number, message: string) {
    super(message);
  }
}

type Listener = () => void;
const unauthorized = new Set<Listener>();
/** Fired when any call answers 401 after sign-in, so the auth guard can send the user to /login. */
export function onUnauthorized(fn: Listener): () => void {
  unauthorized.add(fn);
  return () => unauthorized.delete(fn);
}

export interface Opts extends Omit<RequestInit, 'body'> { json?: unknown; body?: BodyInit; quiet401?: boolean }

/** Every request is relative to <base href>, carries the cookie, and speaks JSON. */
export async function api<T = unknown>(path: string, opts: Opts = {}): Promise<T> {
  const { json, quiet401, headers, ...rest } = opts;
  const h = new Headers(headers);
  h.set('Accept', 'application/json');
  let body = rest.body;
  if (json !== undefined) {
    h.set('Content-Type', 'application/json');
    body = JSON.stringify(json);
  }
  let res: Response;
  try {
    res = await fetch(url(path), { credentials: 'same-origin', ...rest, headers: h, body });
  } catch {
    throw new ApiError(0, 'The hub did not answer.');
  }
  if (res.status === 401 && !quiet401) unauthorized.forEach((f) => f());
  if (res.status === 204) return undefined as T;
  const ct = res.headers.get('content-type') ?? '';
  const data = ct.includes('json') ? await res.json().catch(() => ({})) : await res.text().catch(() => '');
  if (!res.ok) {
    const msg = typeof data === 'object' && data && 'error' in data ? String((data as { error: unknown }).error) : `Error ${res.status}`;
    throw new ApiError(res.status, msg);
  }
  return data as T;
}

const enc = encodeURIComponent;
export const paneBase = (m: string, p: string) => `api/machines/${enc(m)}/panes/${enc(p)}`;

export const getMe = () => api<Me>('api/auth/me', { quiet401: true });
export const getProviders = () => api<Providers>('api/auth/providers', { quiet401: true });
export const login = (username: string, password: string) =>
  api<Me>('api/auth/login', { method: 'POST', json: { username, password }, quiet401: true });
export const logout = () => api<void>('api/auth/logout', { method: 'POST', quiet401: true });
