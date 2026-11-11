import { vi } from 'vitest';

export function setBase(href: string) {
  let b = document.head.querySelector('base');
  if (!b) { b = document.createElement('base'); document.head.prepend(b); }
  b.setAttribute('href', href);
}

type Handler = (url: string, init: RequestInit) => { status?: number; body?: unknown; type?: string } | Promise<{ status?: number; body?: unknown; type?: string }>;
/** Replace fetch with a router over relative-to-origin URLs; records each call. */
export function mockFetch(h: Handler) {
  const calls: Array<{ url: string; init: RequestInit }> = [];
  const f = vi.fn(async (input: RequestInfo | URL, init: RequestInit = {}) => {
    const u = String(input);
    calls.push({ url: u, init });
    const r = await h(u, init);
    const status = r.status ?? 200;
    const isText = typeof r.body === 'string';
    return new Response(status === 204 ? null : isText ? (r.body as string) : JSON.stringify(r.body ?? {}), {
      status, headers: { 'Content-Type': r.type ?? (isText ? 'text/plain' : 'application/json') },
    });
  });
  vi.stubGlobal('fetch', f);
  return calls;
}
