import { describe, expect, it } from 'vitest';
import { agentIcon, agentKey } from '../packages/hub-ui/src/panes';
import { readerClass } from '../packages/hub-ui/src/components/HubPane';

describe('agentKey', () => {
  it('knows every agent the hub reads from its own log', () => {
    expect(agentKey('claude')).toBe('claude');
    expect(agentKey('devin')).toBe('devin');
    expect(agentKey('codex')).toBe('codex');
    expect(agentKey('opencode')).toBe('opencode');
    expect(agentIcon('opencode')).toBe('◇');
  });
  it('knows Antigravity (agy) by either name, with its own icon and a proportional, non-generic reader', () => {
    expect(agentKey('agy')).toBe('agy');
    expect(agentKey('Antigravity')).toBe('agy');
    expect(agentKey('antigravity')).toBe('agy');
    expect(agentIcon('agy')).toBe('✦');
    expect(readerClass('agy')).toBe('agy');
    expect(readerClass('antigravity')).toBe('agy');
    expect(readerClass('agy')).not.toBe('generic');
    expect(readerClass('grok')).toBe('generic');
    expect(readerClass('')).toBe('shell');
  });
  it('falls back for agents read from the screen, and a plain shell', () => {
    expect(agentKey('grok')).toBe('other');
    expect(agentKey('strategy-bot')).toBe('other'); // "agy" only as a word
    expect(agentKey('')).toBe('shell');
  });
});
