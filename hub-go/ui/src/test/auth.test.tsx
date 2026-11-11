import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MemoryRouter, Route, Routes, useLocation } from 'react-router-dom';
import { AuthProvider, RequireAuth } from '../auth';
import { Login } from '../pages/Login';
import { mockFetch, setBase } from './helpers';

const Where = () => <p data-testid="loc">{useLocation().pathname}</p>;
const app = (start: string) => (
  <MemoryRouter initialEntries={[start]}>
    <AuthProvider>
      <Where />
      <Routes>
        <Route path="/login" element={<Login />} />
        <Route path="/secret" element={<RequireAuth><p>top secret</p></RequireAuth>} />
        <Route path="/admin" element={<RequireAuth admin><p>admin area</p></RequireAuth>} />
      </Routes>
    </AuthProvider>
  </MemoryRouter>
);

describe('auth guard', () => {
  beforeEach(() => setBase('/'));
  it('redirects to /login on 401', async () => {
    mockFetch((u) => (u.endsWith('api/auth/me') ? { status: 401, body: { error: 'no' } } : { body: { static: true } }));
    render(app('/secret'));
    await waitFor(() => expect(screen.getByTestId('loc')).toHaveTextContent('/login'));
    expect(screen.queryByText('top secret')).toBeNull();
  });
  it('renders the page for a signed-in user', async () => {
    mockFetch(() => ({ body: { username: 'alice', admin: false } }));
    render(app('/secret'));
    expect(await screen.findByText('top secret')).toBeInTheDocument();
  });
  it('keeps non-admins out of admin pages', async () => {
    mockFetch(() => ({ body: { username: 'bob', admin: false } }));
    render(app('/admin'));
    expect(await screen.findByRole('alert')).toHaveTextContent('Admins only');
  });
  it('shows an error when the hub is unreachable instead of bouncing to login', async () => {
    vi.stubGlobal('fetch', vi.fn(async () => { throw new TypeError('down'); }));
    render(app('/secret'));
    expect(await screen.findByRole('alert')).toHaveTextContent('did not answer');
  });
});

describe('login page', () => {
  beforeEach(() => setBase('/klark/'));
  it('offers ADFS as a plain navigation relative to the base', async () => {
    mockFetch((u) => (u.endsWith('api/auth/me') ? { status: 401, body: {} } : { body: { static: true, adfs: { enabled: true, name: 'ADFS', loginUrl: 'auth/adfs/login' } } }));
    render(app('/login'));
    const a = await screen.findByRole('link', { name: /sign in with adfs/i });
    expect(a).toHaveAttribute('href', '/klark/auth/adfs/login');
  });
  it('hides the ADFS button when it is not enabled', async () => {
    mockFetch((u) => (u.endsWith('api/auth/me') ? { status: 401, body: {} } : { body: { static: true, adfs: { enabled: false } } }));
    render(app('/login'));
    await screen.findByLabelText('User name');
    expect(screen.queryByRole('link', { name: /adfs/i })).toBeNull();
  });
  it('signs in and goes back to where the user was headed', async () => {
    let signed = false;
    mockFetch((u, init) => {
      if (u.endsWith('api/auth/me')) return signed ? { body: { username: 'alice' } } : { status: 401, body: {} };
      if (u.endsWith('api/auth/login') && init.method === 'POST') { signed = true; return { body: { username: 'alice' } }; }
      return { body: { static: true } };
    });
    render(app('/secret'));
    await userEvent.type(await screen.findByLabelText('User name'), 'alice');
    await userEvent.type(screen.getByLabelText('Password'), 'pw');
    await userEvent.click(screen.getByRole('button', { name: 'Sign in' }));
    expect(await screen.findByText('top secret')).toBeInTheDocument();
  });
  it('says so when the password is wrong', async () => {
    mockFetch((u) => (u.endsWith('api/auth/login') ? { status: 401, body: { error: 'bad' } } : u.endsWith('api/auth/me') ? { status: 401, body: {} } : { body: { static: true } }));
    render(app('/login'));
    await userEvent.type(await screen.findByLabelText('User name'), 'a');
    await userEvent.type(screen.getByLabelText('Password'), 'x');
    await userEvent.click(screen.getByRole('button', { name: 'Sign in' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('Wrong user name or password');
  });
});
