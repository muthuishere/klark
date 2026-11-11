import type { IncomingMessage, ServerResponse } from 'node:http';
import type { Plugin } from 'vite';
import { chatFor, files, machines, messages, trees } from './fixtures';

// A tiny in-memory hub for `npm run dev:mock`: same URLs and shapes as hub-go/API.md, no backend needed.
// Sign in with alice / pw (admin) or bob / pw. A new chat row arrives over SSE every few seconds on p1.
export function mockApi(): Plugin {
  const users = [{ username: 'alice', displayName: 'Alice', admin: true }, { username: 'bob', displayName: 'Bob', admin: false }];
  const chats = new Map<string, ReturnType<typeof chatFor>>();
  const streams = new Map<string, Set<ServerResponse>>();
  let directory: Record<string, unknown> = { mode: 'static', ldap: { host: '', port: 389 } };
  let seq = 1000;

  const chat = (m: string, p: string) => { const k = m + '/' + p; if (!chats.has(k)) chats.set(k, chatFor(m, p)); return chats.get(k)!; };
  const json = (res: ServerResponse, status: number, body?: unknown) => {
    if (status === 204) { res.statusCode = 204; res.end(); return; }
    res.statusCode = status;
    res.setHeader('Content-Type', 'application/json');
    res.end(JSON.stringify(body));
  };
  const body = (req: IncomingMessage) => new Promise<string>((ok) => { let s = ''; req.on('data', (c) => (s += c)); req.on('end', () => ok(s)); });
  const emit = (key: string, ev: string, data: unknown) => streams.get(key)?.forEach((r) => r.write(`event: ${ev}\ndata: ${JSON.stringify(data)}\n\n`));
  const push = (m: string, p: string, row: Record<string, unknown>) => { chat(m, p).rows.push(row as never); emit(m + '/' + p, 'chat', { rows: [row] }); };

  return {
    name: 'klark-mock-api',
    configureServer(server) {
      setInterval(() => {
        const row = { id: 'live' + ++seq, role: 'assistant', kind: 'text', html: `<p>Still working. Tick ${seq - 1000}.</p>`, time: new Date().toISOString() };
        push('macbook', 'p1', row);
      }, 8000);

      server.middlewares.use(async (req, res, next) => {
        const u = new URL(req.url ?? '/', 'http://x');
        const p = u.pathname;
        if (!p.startsWith('/api/')) return next();
        const authed = /klark_session=ok:(\w+)/.exec(req.headers.cookie ?? '');
        const me = users.find((x) => x.username === (authed?.[1] ?? (process.env.MOCK_AUTH === '1' ? 'alice' : '')));
        if (p === '/api/auth/providers') return json(res, 200, { static: true, adfs: { enabled: true, name: 'ADFS', loginUrl: 'auth/adfs/login' } });
        if (p === '/api/auth/login' && req.method === 'POST') {
          const b = JSON.parse((await body(req)) || '{}');
          const user = users.find((x) => x.username === b.username);
          if (!user || b.password !== 'pw') return json(res, 401, { error: 'bad credentials' });
          res.setHeader('Set-Cookie', `klark_session=ok:${user.username}; Path=/; HttpOnly; SameSite=Lax`);
          return json(res, 200, user);
        }
        if (p === '/api/auth/logout') { res.setHeader('Set-Cookie', 'klark_session=; Path=/; Max-Age=0'); return json(res, 204); }
        if (!me) return json(res, 401, { error: 'sign in' });
        if (p === '/api/auth/me') return json(res, 200, me);

        let m: RegExpExecArray | null;
        if (p === '/api/machines') return json(res, 200, machines);
        if ((m = /^\/api\/machines\/([^/]+)\/tree$/.exec(p))) return json(res, 200, trees[decodeURIComponent(m[1])] ?? {});
        if ((m = /^\/api\/machines\/([^/]+)\/panes\/([^/]+)\/(chat|events|input|keys|upload)$/.exec(p))) {
          const [mid, pid] = [decodeURIComponent(m[1]), decodeURIComponent(m[2])];
          const key = mid + '/' + pid;
          if (m[3] === 'chat') return json(res, 200, chat(mid, pid));
          if (m[3] === 'events') {
            res.writeHead(200, { 'Content-Type': 'text/event-stream', 'Cache-Control': 'no-cache', Connection: 'keep-alive' });
            res.write(`event: size\ndata: ${JSON.stringify({ cols: 100, rows: 30 })}\n\n`);
            if (u.searchParams.get('frames')) res.write(`event: frame\ndata: ${JSON.stringify({ reset: true, data: Buffer.from('\x1b[1;32mklark\x1b[0m mock terminal\r\n$ ').toString('base64') })}\n\n`);
            const set = streams.get(key) ?? new Set();
            set.add(res); streams.set(key, set);
            const ping = setInterval(() => res.write(': ping\n\n'), 15000);
            req.on('close', () => { clearInterval(ping); set.delete(res); });
            return;
          }
          if (m[3] === 'input') {
            const b = JSON.parse((await body(req)) || '{}');
            push(mid, pid, { id: 'u' + ++seq, role: 'user', kind: 'user', text: b.text, time: new Date().toISOString() });
            setTimeout(() => push(mid, pid, { id: 'a' + ++seq, role: 'assistant', kind: 'text', html: '<p>Got it. Working on that now.</p>', time: new Date().toISOString() }), 1200);
            return json(res, 204);
          }
          if (m[3] === 'keys') { await body(req); return json(res, 204); }
          if (m[3] === 'upload') { await body(req); return json(res, 200, { name: 'photo.png', path: '/tmp/klark-assets/photo.png' }); }
        }
        if ((m = /^\/api\/files\/([^/]+)(\/markdown|\/raw)?$/.exec(p))) {
          const f = files[u.searchParams.get('path') ?? ''];
          if (!f) return json(res, 404, { error: 'file not found' });
          if (!m[2]) return json(res, 200, f.meta);
          if (m[2] === '/markdown') return json(res, 200, { html: f.html ?? '' });
          res.setHeader('Content-Type', 'text/plain'); return void res.end(f.body);
        }
        if (p === '/api/messages') {
          if (req.method === 'POST') { const b = JSON.parse(await body(req)); messages.unshift({ ts: new Date().toISOString(), from: me.username, to: b.to, text: b.text, ok: true } as never); return json(res, 200, {}); }
          return json(res, 200, messages);
        }
        if (!me.admin && p.startsWith('/api/admin/')) return json(res, 403, { error: 'admins only' });
        if (p === '/api/admin/users') {
          if (req.method === 'POST') { const b = JSON.parse(await body(req)); const i = users.findIndex((x) => x.username === b.username); const row = { username: b.username, displayName: b.displayName, admin: !!b.admin }; if (i >= 0) users[i] = row; else users.push(row); return json(res, 200, row); }
          return json(res, 200, users);
        }
        if ((m = /^\/api\/admin\/users\/([^/]+)$/.exec(p)) && req.method === 'DELETE') { const i = users.findIndex((x) => x.username === decodeURIComponent(m![1])); if (i >= 0) users.splice(i, 1); return json(res, 204); }
        if (p === '/api/admin/machines') return json(res, 200, machines.map((x) => ({ name: x.name, status: x.online ? 'online' : 'offline', approvedBy: 'alice', created: '2026-10-20T10:00:00Z' })));
        if (p === '/api/admin/directory') { if (req.method === 'PUT') { directory = JSON.parse(await body(req)); return json(res, 204); } return json(res, 200, directory); }
        return json(res, 404, { error: 'not found' });
      });
    },
  };
}
