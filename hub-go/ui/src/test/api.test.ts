import { ApiError, api, login, onUnauthorized, paneBase } from '../lib/api';
import { mockFetch, setBase } from './helpers';

describe('api client', () => {
  it('requests relative to the base, with the cookie, as JSON', async () => {
    setBase('/klark/');
    const calls = mockFetch(() => ({ body: { ok: 1 } }));
    await expect(api('api/machines')).resolves.toEqual({ ok: 1 });
    expect(calls[0].url).toBe('/klark/api/machines');
    expect(calls[0].init.credentials).toBe('same-origin');
    expect((calls[0].init.headers as Headers).get('Accept')).toBe('application/json');
  });
  it('posts JSON bodies', async () => {
    mockFetch(() => ({ body: { username: 'a' } }));
    const calls = mockFetch(() => ({ body: { username: 'a' } }));
    await login('a', 'pw');
    expect(calls[0].init.method).toBe('POST');
    expect(calls[0].init.body).toBe('{"username":"a","password":"pw"}');
    expect((calls[0].init.headers as Headers).get('Content-Type')).toBe('application/json');
  });
  it('turns {"error"} into an ApiError and 401 into an unauthorized event', async () => {
    mockFetch(() => ({ status: 401, body: { error: 'sign in' } }));
    const fn = vi.fn();
    const off = onUnauthorized(fn);
    await expect(api('api/machines')).rejects.toMatchObject({ status: 401, message: 'sign in' });
    expect(fn).toHaveBeenCalledTimes(1);
    await expect(api('api/auth/me', { quiet401: true })).rejects.toBeInstanceOf(ApiError);
    expect(fn).toHaveBeenCalledTimes(1);
    off();
  });
  it('handles 204 and network failure', async () => {
    mockFetch(() => ({ status: 204 }));
    await expect(api('api/auth/logout', { method: 'POST' })).resolves.toBeUndefined();
    vi.stubGlobal('fetch', vi.fn(async () => { throw new TypeError('down'); }));
    await expect(api('api/x')).rejects.toMatchObject({ status: 0 });
  });
  it('encodes machine and pane ids', () => {
    expect(paneBase('a b', 'p/1')).toBe('api/machines/a%20b/panes/p%2F1');
  });
});
