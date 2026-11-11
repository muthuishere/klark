import { render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MemoryRouter } from 'react-router-dom';
import { AuthProvider } from '../auth';
import { AppRoutes } from '../App';
import { chatFor, files, machines, trees } from '../../mock/fixtures';
import { mockFetch, setBase } from './helpers';

class NoES { addEventListener() {} close() {} onopen = null; onerror = null; constructor(public url: string) { (globalThis as { __es?: string[] }).__es?.push(url); } }

function hub() {
  return mockFetch((u, init) => {
    const p = new URL(u, 'http://x').pathname.replace(/^\/klark/, '');
    if (p === '/api/auth/me') return { body: { username: 'alice', displayName: 'Alice', admin: true } };
    if (p === '/api/machines') return { body: machines };
    let m = /^\/api\/machines\/([^/]+)\/tree$/.exec(p);
    if (m) return { body: trees[m[1]] };
    m = /^\/api\/machines\/([^/]+)\/panes\/([^/]+)\/chat$/.exec(p);
    if (m) return { body: chatFor(m[1], m[2]) };
    if (/\/(input|keys)$/.test(p)) return { status: 204 };
    m = /^\/api\/files\/([^/]+)$/.exec(p);
    if (m) return { body: files[new URL(u, 'http://x').searchParams.get('path')!].meta };
    if (p.endsWith('/raw')) return { body: files['/Users/me/code/klark/hub-go/cmd/main.go'].body };
    void init;
    return { status: 404, body: { error: 'nope' } };
  });
}
const mount = (path: string) => render(<MemoryRouter initialEntries={[path]} future={{ v7_startTransition: true, v7_relativeSplatPath: true }}><AuthProvider><AppRoutes /></AuthProvider></MemoryRouter>);

describe('the app against the mock hub', () => {
  beforeEach(() => { setBase('/klark/'); vi.stubGlobal('EventSource', NoES); sessionStorage.clear(); localStorage.clear(); });

  it('shows the sidebar grouped by folder and a pane with its chat and composer', async () => {
    const calls = hub();
    mount('/m/macbook/p/p1');
    expect(await screen.findByRole('heading', { name: 'hub rewrite' })).toBeInTheDocument();
    const nav = await screen.findByRole('navigation', { name: 'Panes' });
    expect(await within(nav).findByText('klark')).toBeInTheDocument(); // folder group
    expect(await screen.findByText('Fix the failing test, then run it again.')).toBeInTheDocument();
    expect(screen.getByText(/Where this stands/)).toBeInTheDocument();
    expect(screen.getByLabelText('Message')).toBeInTheDocument();
    expect(calls.every((c) => c.url.startsWith('/klark/'))).toBe(true); // nothing escapes the base path
  });

  it('sends a message as input then Enter', async () => {
    const calls = hub();
    mount('/m/macbook/p/p1');
    const box = await screen.findByLabelText('Message');
    await userEvent.type(box, 'hello{Enter}');
    const posts = calls.filter((c) => c.init.method === 'POST').map((c) => [c.url, c.init.body]);
    expect(posts).toEqual([
      ['/klark/api/machines/macbook/panes/p1/input', '{"text":"hello"}'],
      ['/klark/api/machines/macbook/panes/p1/keys', '{"keys":["enter"]}'],
    ]);
  });

  it('opens the events stream under the base path', async () => {
    const seen: string[] = []; (globalThis as { __es?: string[] }).__es = seen;
    hub();
    mount('/m/macbook/p/p1');
    await screen.findByLabelText('Message');
    expect(seen[0]).toBe('/klark/api/machines/macbook/panes/p1/events');
  });

  it('disables the composer for an offline machine', async () => {
    hub();
    mount('/m/old-laptop/p/p1');
    expect(await screen.findByText(/Machine offline/)).toBeInTheDocument();
    expect(screen.queryByLabelText('Message')).toBeNull();
  });

  it('shows a file in the viewer with line numbers', async () => {
    hub();
    mount('/m/macbook/f?path=%2FUsers%2Fme%2Fcode%2Fklark%2Fhub-go%2Fcmd%2Fmain.go&pane=p1');
    expect(await screen.findByText('println("klarkhub")')).toBeInTheDocument();
    expect(screen.getByLabelText('Line 3')).toBeInTheDocument();
  });

  it('has a 404 page', async () => {
    hub();
    mount('/nowhere/at/all');
    expect(await screen.findByRole('heading', { name: 'Not found' })).toBeInTheDocument();
  });
});
