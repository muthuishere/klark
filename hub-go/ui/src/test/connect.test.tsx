import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MemoryRouter } from 'react-router-dom';
import { AuthProvider } from '../auth';
import { AppRoutes } from '../App';
import { Composer } from '../components/Composer';
import { chatFor, machines, trees } from '../../mock/fixtures';
import { mockFetch, setBase } from './helpers';

class NoES { addEventListener() {} close() {} onopen = null; onerror = null; constructor(public url: string) {} }
const mount = (path: string) => render(<MemoryRouter initialEntries={[path]} future={{ v7_startTransition: true, v7_relativeSplatPath: true }}><AuthProvider><AppRoutes /></AuthProvider></MemoryRouter>);
const path = (u: string) => new URL(u, 'http://x').pathname.replace(/^\/klark/, '');
const pending = { code: 'ABCD-EFGH', machine: 'box-1', status: 'pending', expiresAt: '2026-11-12T10:00:00Z', existing: false };

describe('connect approval page', () => {
  beforeEach(() => { setBase('/klark/'); sessionStorage.clear(); });

  it('shows the machine and code, and approves', async () => {
    const calls = mockFetch((u, init) => {
      const p = path(u);
      if (p === '/api/auth/me') return { body: { username: 'alice', admin: false } };
      if (p === '/api/connect/requests/ABCD-EFGH' && !init.method) return { body: pending };
      if (p === '/api/connect/requests/ABCD-EFGH/approve') return { body: { status: 'approved', machine: 'box-1' } };
      return { status: 404, body: { error: 'nope' } };
    });
    mount('/connect?code=ABCD-EFGH');
    expect(await screen.findByLabelText('Code')).toHaveTextContent('ABCD-EFGH');
    expect(screen.getByText('box-1')).toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: 'Approve' }));
    expect(await screen.findByRole('status')).toHaveTextContent('Approved. box-1 is connecting now.');
    expect(screen.queryByRole('button', { name: 'Approve' })).toBeNull();
    const post = calls.find((c) => c.init.method === 'POST');
    expect(post?.url).toBe('/klark/api/connect/requests/ABCD-EFGH/approve');
  });

  it('denies', async () => {
    mockFetch((u, init) => {
      const p = path(u);
      if (p === '/api/auth/me') return { body: { username: 'alice', admin: true } };
      if (p.endsWith('/deny') && init.method === 'POST') return { body: { status: 'denied' } };
      if (p === '/api/connect/requests/ABCD-EFGH') return { body: { ...pending, existing: true, owner: 'bob' } };
      return { status: 404, body: { error: 'nope' } };
    });
    mount('/connect?code=ABCD-EFGH');
    expect(await screen.findByText(/already exists \(approved by bob\)/)).toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: 'Deny' }));
    expect(await screen.findByRole('status')).toHaveTextContent('Denied.');
  });

  it('says when the code expired, and shows refusals', async () => {
    mockFetch((u) => {
      const p = path(u);
      if (p === '/api/auth/me') return { body: { username: 'alice', admin: false } };
      return { status: 404, body: { error: 'This code is unknown or has expired.' } };
    });
    mount('/connect?code=ZZZZ-ZZZZ');
    expect(await screen.findByText(/unknown, already used, or has expired/)).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Approve' })).toBeNull();
  });

  it('shows a refused approval', async () => {
    mockFetch((u, init) => {
      const p = path(u);
      if (p === '/api/auth/me') return { body: { username: 'alice', admin: false } };
      if (init.method === 'POST') return { status: 403, body: { error: 'A machine named box-1 already exists and belongs to someone else.' } };
      return { body: pending };
    });
    mount('/connect?code=ABCD-EFGH');
    await userEvent.click(await screen.findByRole('button', { name: 'Approve' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('belongs to someone else');
  });

  it('sends a signed-out person to the login page', async () => {
    mockFetch((u) => (path(u) === '/api/auth/me' ? { status: 401 } : { body: { static: true } }));
    mount('/connect?code=ABCD-EFGH');
    expect(await screen.findByRole('button', { name: /sign in/i })).toBeInTheDocument();
  });
});

describe('image upload in the chat', () => {
  beforeEach(() => { setBase('/klark/'); vi.stubGlobal('EventSource', NoES); sessionStorage.clear(); localStorage.clear(); });

  it('picks an image, uploads it, previews it, and sends its path with the text', async () => {
    URL.createObjectURL = vi.fn(() => 'blob:preview-1');
    URL.revokeObjectURL = vi.fn();
    const calls = mockFetch((u) => {
      const p = path(u);
      if (p === '/api/auth/me') return { body: { username: 'alice', admin: true } };
      if (p === '/api/machines') return { body: machines };
      let m = /^\/api\/machines\/([^/]+)\/tree$/.exec(p);
      if (m) return { body: trees[m[1]] };
      m = /^\/api\/machines\/([^/]+)\/panes\/([^/]+)\/chat$/.exec(p);
      if (m) return { body: chatFor(m[1], m[2]) };
      if (p.endsWith('/upload')) return { body: { name: 'shot.png', path: '/home/me/.klark/uploads/abc.png', sha256: 'abc', size: 3, mime: 'image/png' } };
      if (/\/(input|keys)$/.test(p)) return { status: 204 };
      return { status: 404, body: { error: 'nope' } };
    });
    mount('/m/macbook/p/p1');
    await screen.findByLabelText('Message');
    const file = new File([new Uint8Array([0x89, 0x50, 0x4e, 0x47])], 'shot.png', { type: 'image/png' });
    await userEvent.upload(screen.getByLabelText('Attach an image', { selector: 'input' }), file);
    const chip = await screen.findByTitle('/home/me/.klark/uploads/abc.png');
    expect(chip).toHaveTextContent('shot.png');
    expect(chip.querySelector('img')).toHaveAttribute('src', 'blob:preview-1');
    const up = calls.find((c) => c.url.endsWith('/upload'))!;
    expect(up.url).toBe('/klark/api/machines/macbook/panes/p1/upload');
    expect((up.init.body as FormData).get('file')).toBeInstanceOf(File);

    await userEvent.type(screen.getByLabelText('Message'), 'what is this?{Enter}');
    const input = calls.find((c) => c.url.endsWith('/input'))!;
    expect(JSON.parse(String(input.init.body))).toEqual({ text: '[image from hub] /home/me/.klark/uploads/abc.png what is this?' });
    expect(URL.revokeObjectURL).toHaveBeenCalledWith('blob:preview-1');
    expect(screen.queryByTitle('/home/me/.klark/uploads/abc.png')).toBeNull();
  });

  it('shows why an upload was refused', async () => {
    mockFetch((u) => {
      const p = path(u);
      if (p === '/api/auth/me') return { body: { username: 'alice', admin: true } };
      if (p === '/api/machines') return { body: machines };
      let m = /^\/api\/machines\/([^/]+)\/tree$/.exec(p);
      if (m) return { body: trees[m[1]] };
      m = /^\/api\/machines\/([^/]+)\/panes\/([^/]+)\/chat$/.exec(p);
      if (m) return { body: chatFor(m[1], m[2]) };
      if (p.endsWith('/upload')) return { status: 415, body: { error: 'not an image; send png, jpeg, gif, webp, heic or avif' } };
      return { status: 404, body: { error: 'nope' } };
    });
    mount('/m/macbook/p/p1');
    await screen.findByLabelText('Message');
    await userEvent.upload(screen.getByLabelText('Attach an image', { selector: 'input' }), new File(['x'], 'x.png', { type: 'image/png' }));
    expect(await screen.findByText(/Not uploaded: not an image/)).toBeInTheDocument();
  });

  it('pasting an image hands it to the upload', async () => {
    const onFiles = vi.fn();
    render(<Composer draftKey="t" commands={[]} terminal={false} attachments={[]} uploading={false} onRemoveAttachment={() => {}} onFiles={onFiles} onSend={() => {}} onKey={() => {}} />);
    const img = new File(['x'], 'p.png', { type: 'image/png' });
    screen.getByLabelText('Message').focus();
    await userEvent.paste({ files: [img], getData: () => '', types: ['Files'], items: [] } as unknown as DataTransfer);
    expect(onFiles).toHaveBeenCalledWith([img]);
  });
});
