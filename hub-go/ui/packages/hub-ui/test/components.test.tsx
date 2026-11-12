import { act, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { ChatView, PaneHeader, Sidebar, httpTransport, memoryStorage, useChat, webStorage, type EventHandlers, type MachineTree, type Transport } from '../src';

const json = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status, headers: { 'Content-Type': 'application/json' } });

describe('httpTransport', () => {
  it('speaks klark hub URLs under the base path', async () => {
    const calls: Array<[string, RequestInit]> = [];
    const f = vi.fn(async (u: RequestInfo | URL, init: RequestInit = {}) => { calls.push([String(u), init]); return String(u).endsWith('/chat') ? json({ rows: [] }) : new Response(null, { status: 204 }); });
    const t = httpTransport('/klark', { fetch: f as never });
    await t.getChat('m 1', 'p/1');
    await t.send('m', 'p', 'hi');
    expect(calls.map((c) => c[0])).toEqual(['/klark/api/machines/m%201/panes/p%2F1/chat', '/klark/api/machines/m/panes/p/input', '/klark/api/machines/m/panes/p/keys']);
    expect(JSON.parse(String(calls[2][1].body))).toEqual({ keys: ['enter'] });
    expect(t.fileUrl('m', 'p', '/a b.go', 12)).toBe('/klark/m/m/f?path=%2Fa%20b.go&pane=p#L12');
  });
  it('throws the server error and reports the status', async () => {
    const seen: number[] = [];
    const t = httpTransport('/', { fetch: (async () => json({ error: 'nope' }, 403)) as never, onStatus: (s) => seen.push(s) });
    await expect(t.keys('m', 'p', ['esc'])).rejects.toMatchObject({ status: 403, message: 'nope' });
    expect(seen).toEqual([403]);
  });
  it('parses SSE events into handlers', () => {
    const all: Array<{ url: string; l: Map<string, (e: MessageEvent) => void> }> = [];
    class ES { l = new Map(); onopen = null; onerror = null; constructor(public url: string) { all.push(this as never); } addEventListener(n: string, f: never) { this.l.set(n, f); } close() {} }
    const chat = vi.fn(), frame = vi.fn(), size = vi.fn();
    const close = httpTransport('/', { EventSource: ES as never }).events('m', 'p', { frames: true }, { chat, frame, size });
    expect(all[0].url).toBe('/api/machines/m/panes/p/events?frames=1');
    all[0].l.get('chat')!({ data: '{"rows":[{"id":"1","kind":"text"}],"replace":true}' } as MessageEvent);
    all[0].l.get('frame')!({ data: '{"data":"aGk=","reset":true}' } as MessageEvent);
    all[0].l.get('size')!({ data: '{"cols":80,"rows":24}' } as MessageEvent);
    expect(chat).toHaveBeenCalledWith({ rows: [{ id: '1', kind: 'text' }], replace: true });
    expect(frame).toHaveBeenCalledWith({ data: 'aGk=', reset: true });
    expect(size).toHaveBeenCalledWith({ cols: 80, rows: 24 });
    close();
  });
});

describe('storage', () => {
  it('prefixes keys and removes on null', () => {
    const s = webStorage(() => localStorage, 'x:');
    s.set('a', '1');
    expect(localStorage.getItem('x:a')).toBe('1');
    s.set('a', null);
    expect(s.get('a')).toBeNull();
  });
});

function fakeTransport(): Transport & { h: EventHandlers } {
  const t = {
    h: {} as EventHandlers,
    machines: async () => [], tree: async () => ({}),
    getChat: async () => ({ rows: [{ id: 'u1', role: 'user', kind: 'text', text: 'hello' }] }),
    events: (_m: string, _a: string, _o: unknown, h: EventHandlers) => { t.h = h; return () => {}; },
    send: async () => {}, input: async () => {}, keys: vi.fn(async () => {}), upload: async () => ({ path: '/tmp/x.png' }),
    fileUrl: (_m: string, _a: string, p: string) => '/f?path=' + p,
  };
  return t;
}

function Pane({ t }: { t: Transport }) {
  const c = useChat(t, 'm', 'p');
  return <ChatView items={c.items} loading={c.loading} blocked={c.blocked} onKeys={(k) => t.keys('m', 'p', k)} fileHref={(p) => t.fileUrl('m', 'p', p)} />;
}

describe('useChat + ChatView', () => {
  it('loads, upserts events by id and answers a choice menu', async () => {
    const t = fakeTransport();
    render(<Pane t={t} />);
    expect(await screen.findByText('hello')).toBeInTheDocument();
    act(() => t.h.chat!({ rows: [{ id: 'a1', role: 'assistant', kind: 'text', html: '<p>hi there</p>' }], blocked: { question: 'Proceed?', choices: [{ key: '1', label: 'Yes' }, { key: '2', label: 'No' }], selected: '1' } }));
    expect(await screen.findByText('hi there')).toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: '2. No' }));
    expect(t.keys).toHaveBeenCalledWith('m', 'p', ['down', 'enter']);
  });
  it('hands file links to onNavigate', async () => {
    const nav = vi.fn(() => true);
    render(<ChatView items={[{ id: '1', role: 'assistant', kind: 'text', html: '<p><a class="fpath" href="/f?path=/a.go">/a.go</a></p>' }]} onNavigate={nav} />);
    await userEvent.click(screen.getByRole('link', { name: '/a.go' }));
    expect(nav).toHaveBeenCalledWith(expect.stringContaining('/f?path=/a.go'), expect.any(HTMLAnchorElement));
  });
});

describe('PaneHeader', () => {
  it('shows the agent, status and a terminal toggle', async () => {
    const toggle = vi.fn();
    render(<PaneHeader title="api" agent="claude" status="working" statusText="working…" onToggleView={toggle} actions={<button type="button">More</button>} />);
    expect(screen.getByRole('heading', { name: 'api' })).toBeInTheDocument();
    expect(screen.getByRole('status')).toHaveTextContent('working…');
    await userEvent.click(screen.getByRole('button', { name: 'Terminal' }));
    expect(toggle).toHaveBeenCalled();
  });
});

describe('Sidebar', () => {
  const items: MachineTree[] = [
    { machine: { id: 'mac', name: 'mac', online: true }, tree: { workspaces: [{ workspace_id: 'w1', label: 'main' }], panes: [
      { pane_id: 'p1', workspace_id: 'w1', agent: 'claude', agent_status: 'blocked', cwd: '/Users/x/api' },
      { pane_id: 'p2', workspace_id: 'w1', agent: 'codex', agent_status: 'idle', cwd: '/Users/x/web' },
    ] } },
    { machine: { id: 'old', name: 'old box', online: false } },
  ];
  it('groups by folder, marks current, opens on click and remembers folds', async () => {
    const store = memoryStorage();
    const open = vi.fn();
    render(<Sidebar items={items} store={store} current={{ machine: 'mac', pane: 'p2' }} hrefFor={(m, p) => `/m/${m}/p/${p}`} onOpen={open} renderGroupExtra={(g) => <i>{'x-' + g.name}</i>} />);
    expect(screen.getByText(/needs you/)).toBeInTheDocument();
    expect(screen.getByText('x-api')).toBeInTheDocument();
    expect(screen.getByText('old box')).toBeInTheDocument();
    const cur = document.querySelector('.ent.cur');
    expect(cur).toHaveAttribute('href', '/m/mac/p/p2');
    await userEvent.click(cur!);
    expect(open).toHaveBeenCalledWith('mac', 'p2', expect.anything());
    await userEvent.click(screen.getByRole('button', { name: /api/ }));
    expect(store.get('folded')).toBe('api');
    await userEvent.type(screen.getByLabelText('Search agents'), 'codex');
    await waitFor(() => expect(screen.queryByText('x-api')).toBeNull());
  });
});
