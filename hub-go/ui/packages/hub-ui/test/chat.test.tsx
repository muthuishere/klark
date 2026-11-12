import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { ChatRow, FileHrefCtx } from '../src/components/ChatRow';
import { VirtualList } from '../src/components/VirtualList';
import { buildRows, mergeItems, pairCalls, phrase } from '../src/chat';
import { httpTransport } from '../src/transport';
import type { ChatItem } from '../src/types';

const it_ = (id: string, kind: string, extra: Partial<ChatItem> = {}): ChatItem => ({ id, kind, role: kind === 'user' ? 'user' : 'assistant', ...extra });

describe('chat rows', () => {
  const items = [
    it_('1', 'user', { text: 'hi' }),
    it_('2', 'tool_use', { tool: 'Read', summary: '/a/b.go', toolUseId: 'x' }),
    it_('3', 'tool_result', { tool: 'Read', ok: true, toolUseId: 'x', text: 'file body' }),
    it_('4', 'tool_use', { tool: 'Bash', summary: 'ls', toolUseId: 'y' }),
    it_('5', 'thinking', { text: 'hmm' }),
    it_('6', 'text', { html: '<p>done</p>' }),
    it_('7', 'command', { text: 'make' }),
    it_('8', 'output', { html: 'ok' }),
    it_('9', 'text', {}),
  ];
  it('folds runs of tools, pairs commands with output, drops empty items', () => {
    const rows = buildRows(items);
    expect(rows.map((r) => r.kind)).toEqual(['user', 'tools', 'text', 'block']);
    expect(rows[1].items).toHaveLength(4);
    expect(rows[3].items.map((i) => i.id)).toEqual(['7', '8']);
  });
  it('stamp changes only when content does', () => {
    const a = buildRows(items)[1].stamp;
    expect(buildRows(items)[1].stamp).toBe(a);
    expect(buildRows([...items.slice(0, 4), it_('5', 'thinking', { text: 'hmm!' })])[1].stamp).not.toBe(a);
  });
  it('pairs results by tool_use_id, falling back to the last open call', () => {
    const calls = pairCalls([it_('a', 'tool_use', { toolUseId: '1' }), it_('b', 'tool_use', { toolUseId: '2' }), it_('c', 'tool_result', { toolUseId: '1' }), it_('d', 'tool_result')]);
    expect(calls[0].result?.id).toBe('c');
    expect(calls[1].result?.id).toBe('d');
  });
  it('phrases a group like Claude does', () => {
    expect(phrase(['Read', 'Read', 'Read', 'Bash', 'Bash'], 1)).toBe('Read 3 files · Ran 2 commands · thought');
    expect(phrase(['Bash'], 0)).toBe('Ran 1 command');
    expect(phrase(['Mystery'], 0)).toBe('Mystery');
  });
  it('merges updates by id and appends new ids', () => {
    const m = mergeItems([it_('1', 'text', { html: 'a' })], [it_('1', 'text', { html: 'b' }), it_('2', 'text', { html: 'c' })]);
    expect(m.map((x) => x.html)).toEqual(['b', 'c']);
  });
});

describe('ChatRow rendering', () => {
  const wrap = (el: React.ReactNode) => render(<FileHrefCtx.Provider value={(p) => httpTransport('/').fileUrl('mac', 'p1', p)}>{el}</FileHrefCtx.Provider>);
  it('shows a user turn as plain text (never as HTML)', () => {
    wrap(<ChatRow row={buildRows([it_('1', 'user', { text: '<b>hi</b>' })])[0]} />);
    expect(screen.getByText('<b>hi</b>')).toBeInTheDocument();
  });
  it("renders the hub's markdown HTML, with file links", () => {
    const { container } = wrap(<ChatRow row={buildRows([it_('1', 'text', { html: '<p>see <a class="fpath" href="m/mac/f?path=%2Fa">/a</a></p>' })])[0]} />);
    expect(container.querySelector('.msg.asst .md a.fpath')).toHaveAttribute('href', 'm/mac/f?path=%2Fa');
  });
  it('turns a single file tool call into a viewer link and a check mark', () => {
    wrap(<ChatRow row={buildRows([it_('2', 'tool_use', { tool: 'Read', summary: '/a/b.go', toolUseId: 'x' }), it_('3', 'tool_result', { tool: 'Read', ok: true, toolUseId: 'x' })])[0]} />);
    expect(screen.getByRole('link', { name: '/a/b.go' })).toHaveAttribute('href', '/m/mac/f?path=%2Fa%2Fb.go&pane=p1');
    expect(screen.getByLabelText('done')).toBeInTheDocument();
  });
  it('groups several tools under one summary and expands on click', async () => {
    const rows = buildRows([it_('1', 'tool_use', { tool: 'Bash', summary: 'ls', input: 'ls -la' }), it_('2', 'tool_use', { tool: 'Bash', summary: 'pwd' })]);
    const { container } = wrap(<ChatRow row={rows[0]} />);
    expect(screen.getByText('Ran 2 commands')).toBeInTheDocument();
    const d = container.querySelector('details.tools') as HTMLDetailsElement;
    expect(d.open).toBe(false);
    await userEvent.click(d.querySelector('summary')!);
    expect(screen.getByText('ls')).toBeInTheDocument();
  });
  it('marks a failed call', () => {
    wrap(<ChatRow row={buildRows([it_('1', 'tool_use', { tool: 'Bash', summary: 'false', toolUseId: 'z' }), it_('2', 'tool_result', { ok: false, toolUseId: 'z' })])[0]} />);
    expect(screen.getByLabelText('failed')).toBeInTheDocument();
  });
  it('shows a shell block with its line count', () => {
    wrap(<ChatRow row={buildRows([it_('1', 'command', { text: 'make' }), it_('2', 'output', { html: 'a\nb\nc', lines: 3 })])[0]} />);
    expect(screen.getByText('make')).toBeInTheDocument();
    expect(screen.getByText('3 lines')).toBeInTheDocument();
  });
});

describe('VirtualList (windowed chat)', () => {
  it('keeps a long transcript out of the DOM', () => {
    const items = Array.from({ length: 2000 }, (_, i) => ({ id: 'r' + i }));
    const { container } = render(<VirtualList items={items} getKey={(r) => r.id} render={(r) => <p>{r.id}</p>} estimate={80} />);
    const drawn = container.querySelectorAll('.vrow').length;
    expect(drawn).toBeGreaterThan(0);
    expect(drawn).toBeLessThan(60);
    // pinned to the bottom: the last row is drawn, the first is not
    expect(screen.getByText('r1999')).toBeInTheDocument();
    expect(screen.queryByText('r0')).toBeNull();
    const pads = container.querySelectorAll<HTMLElement>('.chat-pad');
    expect(parseFloat(pads[0].style.height)).toBeGreaterThan(100_000);
  });
  it('draws everything of a short chat', () => {
    const items = Array.from({ length: 5 }, (_, i) => ({ id: 'r' + i }));
    const { container } = render(<VirtualList items={items} getKey={(r) => r.id} render={(r) => <p>{r.id}</p>} />);
    expect(container.querySelectorAll('.vrow')).toHaveLength(5);
  });
});
