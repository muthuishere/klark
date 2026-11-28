import { readFileSync } from 'node:fs';
import { render } from '@testing-library/react';
import { HubPane } from '../src/components/HubPane';
import { PaneHeader } from '../src/components/PaneHeader';
import { agentKey } from '../src/panes';
import type { Transport } from '../src/transport';
import type { PaneInfo } from '../src/types';

const css = readFileSync('style.css', 'utf8');
const t = {
  machines: async () => [], tree: async () => ({ panes: [] }), getChat: async () => ({ rows: [] }), events: () => () => {},
  send: async () => {}, input: async () => {}, keys: async () => {}, upload: async () => ({ path: '/tmp/x' }), fileUrl: (_m: string, _a: string, p: string) => p,
} as unknown as Transport;
const info = { pane_id: 'p1', agent: 'opencode', agent_status: 'idle', cwd: '/work/app' } as unknown as PaneInfo;

describe('opencode styling', () => {
  it('keys the agent and styles its badge and user bubble like claude and devin', () => {
    expect(agentKey('opencode')).toBe('opencode');
    expect(css).toMatch(/\.badge-a\[data-agent="opencode"\]\s*\{[^}]*color:/);
    expect(css).toMatch(/\.reader-opencode \.msg\.user \.bubble\s*\{[^}]*background:/);
  });
  it('puts the opencode hooks on the pane and its header badge', () => {
    const { container } = render(<HubPane transport={t} machine="mac" pane="p1" info={info} online view="chat" onViewChange={() => {}} />);
    expect(container.querySelector('main.reader-opencode')).not.toBeNull();
    render(<PaneHeader title="devinfinder" agent="opencode" />);
    expect(document.querySelector('.badge-a[data-agent="opencode"]')).not.toBeNull();
  });
});
