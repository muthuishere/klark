import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { HubPane } from '../src/components/HubPane';
import { Sidebar } from '../src/components/Sidebar';
import type { Transport } from '../src/transport';
import type { PaneInfo } from '../src/types';

function fakeTransport() {
  const typed: string[] = [];
  const t: Transport = {
    machines: async () => [],
    tree: async () => ({ panes: [] }) as never,
    getChat: async () => ({ rows: [{ id: '1', kind: 'assistant', text: 'hello from the agent' }] }) as never,
    events: () => () => {},
    send: async () => {},
    input: async (_m, _a, text) => { typed.push(text); },
    keys: async (_m, _a, k) => { typed.push(k.join('+')); },
    upload: async (_m, _a, f) => ({ path: '/tmp/' + f.name }),
    fileUrl: (_m, _a, p) => p,
  };
  return { t, typed };
}

const info = { pane_id: 'p1', agent: 'claude', agent_status: 'idle', cwd: '/work/app' } as unknown as PaneInfo;

describe('HubPane', () => {
  it('types through the transport when the viewer may type', async () => {
    const { t, typed } = fakeTransport();
    render(<HubPane transport={t} machine="mac" pane="p1" info={info} online view="chat" onViewChange={() => {}} />);
    const box = await screen.findByRole('textbox');
    await userEvent.type(box, 'hi{enter}');
    await waitFor(() => expect(typed).toEqual(['hi', 'enter']));
  });
  it('shows the host text and no composer when the viewer may not type on this machine', () => {
    const { t } = fakeTransport();
    render(<HubPane transport={t} machine="mac" pane="p1" info={info} online canType={false} readOnlyText="Only the machine's owner can type" view="chat" onViewChange={() => {}} />);
    expect(screen.getByText("Only the machine's owner can type")).toBeInTheDocument();
    expect(screen.queryByRole('textbox')).toBeNull();
  });
  it('shows the host offline text when the machine is offline', () => {
    const { t } = fakeTransport();
    render(<HubPane transport={t} machine="mac" pane="p1" info={info} online={false} offlineText="run klark hub connect" view="chat" onViewChange={() => {}} />);
    expect(screen.getByText('run klark hub connect')).toBeInTheDocument();
    expect(screen.queryByRole('textbox')).toBeNull();
  });
  it('hands the host menu the search toggle', async () => {
    const { t } = fakeTransport();
    render(<HubPane transport={t} machine="mac" pane="p1" info={info} online view="chat" onViewChange={() => {}}
      renderActions={({ searchOpen, toggleSearch }) => <button type="button" onClick={toggleSearch}>{searchOpen ? 'close' : 'search'}</button>} />);
    await userEvent.click(screen.getByRole('button', { name: 'search' }));
    expect(screen.getByRole('button', { name: 'close' })).toBeInTheDocument();
  });
});

describe('Sidebar belowHeader', () => {
  it('renders below the header row, not inside it', () => {
    const { container } = render(<Sidebar items={[]} header={<span>brand</span>} belowHeader={<div>Disabled sessions</div>} />);
    const head = container.querySelector('.side-head')!;
    const below = container.querySelector('.side-below')!;
    expect(head.textContent).toBe('brand');
    expect(below.textContent).toBe('Disabled sessions');
    expect(head.contains(below)).toBe(false);
  });
});
