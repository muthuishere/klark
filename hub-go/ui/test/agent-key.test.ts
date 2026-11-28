import { describe, expect, it } from 'vitest';
import { agentIcon, agentKey } from '../packages/hub-ui/src/panes';

describe('agentKey', () => {
  it('knows every agent the hub reads from its own log', () => {
    expect(agentKey('claude')).toBe('claude');
    expect(agentKey('devin')).toBe('devin');
    expect(agentKey('codex')).toBe('codex');
    expect(agentKey('opencode')).toBe('opencode');
    expect(agentIcon('opencode')).toBe('◇');
  });
  it('falls back for agents read from the screen, and a plain shell', () => {
    expect(agentKey('grok')).toBe('other');
    expect(agentKey('')).toBe('shell');
  });
});
