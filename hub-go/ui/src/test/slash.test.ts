import { slashCommandsFor } from '../lib/slash';

describe('klark slash commands', () => {
  it('lists the per-agent commands', () => {
    expect(slashCommandsFor('claude-code').some((c) => c.name === '/compact')).toBe(true);
    expect(slashCommandsFor('devin').some((c) => c.name === '/plan')).toBe(true);
    expect(slashCommandsFor('amp')).toEqual([]);
    expect(slashCommandsFor(undefined)).toEqual([]);
  });
});
