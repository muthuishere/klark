import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MemoryRouter } from 'react-router-dom';
import { AuthProvider } from '../auth';
import { AdminDirectory } from '../pages/Directory';
import { Login, loginErrorText } from '../pages/Login';
import { mockFetch, setBase } from './helpers';

const wrap = (el: React.ReactNode, at = '/') => render(<MemoryRouter initialEntries={[at]}><AuthProvider>{el}</AuthProvider></MemoryRouter>);
const me401 = (u: string) => u.endsWith('api/auth/me');

describe('login with a directory', () => {
  beforeEach(() => setBase('/'));
  it('shows the mock LDAP hint, OIDC buttons and the error code', async () => {
    mockFetch((u) => (me401(u) ? { status: 401 } : { body: { static: true, directory: { mode: 'mockldap', name: 'Mock LDAP' }, adfs: { enabled: true, name: 'ADFS', loginUrl: 'auth/adfs/login' }, oidc: [{ id: 'g', name: 'Google', loginUrl: 'auth/oidc/g/login' }] } }));
    wrap(<Login />, '/login?error=adfs_failed');
    expect(await screen.findByText(/try user1 \/ user1pass/)).toBeInTheDocument();
    expect(screen.getByRole('link', { name: 'Sign in with Google' })).toHaveAttribute('href', '/auth/oidc/g/login');
    expect(screen.getByRole('link', { name: 'Sign in with ADFS' })).toBeInTheDocument();
    expect(screen.getByRole('alert')).toHaveTextContent(/ADFS failed/);
  });
  it('has no mock hint for real LDAP', async () => {
    mockFetch((u) => (me401(u) ? { status: 401 } : { body: { static: true, directory: { mode: 'ldap', name: 'LDAP' } } }));
    wrap(<Login />, '/login');
    await screen.findByText(/LDAP users/);
    expect(screen.queryByText(/user1pass/)).toBeNull();
  });
  it('maps unknown error codes readably', () => {
    expect(loginErrorText('weird_thing')).toBe('Sign-in error: weird thing.');
    expect(loginErrorText(null)).toBeNull();
  });
});

describe('admin directory page', () => {
  beforeEach(() => setBase('/'));
  const settings = { mode: 'ldap', host: 'ldap.x', port: 389, bindDn: 'cn=svc', bindPasswordSet: true, allowedGroups: ['a', 'b'], oidc: [{ id: 'g', name: 'Google', redirectUri: 'https://h/auth/oidc/g/callback' }] };
  it('never prefills the bind password, keeps it when empty, tests and lists providers', async () => {
    const calls = mockFetch((u, init) => {
      if (me401(u)) return { body: { username: 'a', admin: true } };
      if (u.endsWith('directory/test')) return { body: { ok: true, message: 'bound as user1' } };
      if (init.method === 'PUT') return { status: 204 };
      return { body: settings };
    });
    wrap(<AdminDirectory />);
    expect(await screen.findByTestId('pw-set')).toBeInTheDocument();
    expect(screen.getByLabelText(/Bind password/)).toHaveValue('');
    expect(screen.getByText('https://h/auth/oidc/g/callback')).toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: 'Save' }));
    await waitFor(() => expect(calls.some((c) => c.init.method === 'PUT')).toBe(true));
    const put = JSON.parse(String(calls.find((c) => c.init.method === 'PUT')!.init.body));
    expect(put).not.toHaveProperty('bindPassword');
    expect(put).not.toHaveProperty('bindPasswordSet');
    expect(put.allowedGroups).toEqual(['a', 'b']);
    await userEvent.type(screen.getByLabelText('Test user'), 'user1');
    await userEvent.type(screen.getByLabelText('Test password'), 'pw');
    await userEvent.click(screen.getByRole('button', { name: 'Test connection' }));
    expect(await screen.findByText(/bound as user1/)).toBeInTheDocument();
  });
  it('sends a new bind password when typed', async () => {
    const calls = mockFetch((u, init) => (me401(u) ? { body: { username: 'a', admin: true } } : init.method === 'PUT' ? { status: 204 } : { body: settings }));
    wrap(<AdminDirectory />);
    await screen.findByTestId('pw-set');
    await userEvent.type(screen.getByLabelText(/Bind password/), 'newpw');
    await userEvent.click(screen.getByRole('button', { name: 'Save' }));
    await waitFor(() => expect(calls.some((c) => c.init.method === 'PUT')).toBe(true));
    expect(JSON.parse(String(calls.find((c) => c.init.method === 'PUT')!.init.body)).bindPassword).toBe('newpw');
  });
});
