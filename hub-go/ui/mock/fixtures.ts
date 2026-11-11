// Realistic fixtures for the dev mock API (shapes follow hub-go/API.md and the machine's session.snapshot).
export const machines = [
  { id: 'macbook', name: 'macbook', online: true, version: '0.9.5.6', protocol: 5, lastSeen: new Date().toISOString() },
  { id: 'build-box', name: 'build-box', online: true, version: '0.9.5.6', protocol: 5, lastSeen: new Date().toISOString() },
  { id: 'old-laptop', name: 'old-laptop', online: false, version: '0.9.4', protocol: 4, lastSeen: '2026-11-01T09:12:00Z' },
];

export const trees: Record<string, unknown> = {
  macbook: {
    workspaces: [{ workspace_id: 'w1', label: 'klark', number: 1 }, { workspace_id: 'w2', label: 'reqsume', number: 2 }],
    tabs: [{ tab_id: 't1', workspace_id: 'w1', label: 'agents', number: 1 }, { tab_id: 't2', workspace_id: 'w2', label: 'main', number: 1 }],
    panes: [
      { pane_id: 'p1', workspace_id: 'w1', tab_id: 't1', label: 'hub rewrite', agent: 'claude', agent_status: 'working', cwd: '/Users/me/code/klark', focused: true },
      { pane_id: 'p2', workspace_id: 'w1', tab_id: 't1', title: 'zsh', agent: 'devin', agent_status: 'blocked', cwd: '/Users/me/code/klark' },
      { pane_id: 'p3', workspace_id: 'w2', tab_id: 't2', agent: 'codex', agent_status: 'idle', cwd: '/Users/me/code/reqsume' },
      { pane_id: 'p4', workspace_id: 'w2', tab_id: 't2', title: 'zsh', cwd: '/Users/me/code/reqsume' },
      { pane_id: 'p5', workspace_id: 'w1', tab_id: 't1', label: 'long transcript', agent: 'claude', agent_status: 'idle', cwd: '/Users/me/code/klark/docs' },
    ],
  },
  'build-box': {
    workspaces: [{ workspace_id: 'w1', label: 'ci', number: 1 }],
    tabs: [{ tab_id: 't1', workspace_id: 'w1', label: 'jobs', number: 1 }],
    panes: [{ pane_id: 'p1', workspace_id: 'w1', tab_id: 't1', label: 'release', agent: 'claude', agent_status: 'idle', cwd: '/srv/build/klark' }],
  },
  'old-laptop': { workspaces: [], tabs: [], panes: [] },
};

let n = 0;
const id = () => 'i' + ++n;
const t = (min: number) => new Date(Date.now() - min * 60000).toISOString();
const code = (lang: string, body: string) => `<div class="codeblk"><div class="code-head"><span>${lang}</span><button type="button" class="copy">Copy</button></div><pre><code data-lang="${lang}">${body}</code></pre></div>`;
const path = (m: string, p: string, file: string) => `<a class="fpath" href="m/${m}/f?path=${encodeURIComponent(file)}&amp;pane=${p}">${file}</a>`;

export function chatFor(machine: string, pane: string) {
  if (pane === 'p5') {
    const rows = [];
    for (let i = 0; i < 600; i++) {
      rows.push({ id: id(), role: 'user', kind: 'user', text: `Question ${i + 1}: what does step ${i + 1} do?`, time: t(1200 - i) });
      rows.push({ id: id(), role: 'assistant', kind: 'text', html: `<p>Step ${i + 1} reads the queue, then ${i % 3 === 0 ? 'writes a row' : 'acks it'}. ${'More detail. '.repeat(1 + (i % 6))}</p>`, time: t(1200 - i) });
    }
    return { rows, rev: 'long' };
  }
  if (pane === 'p4') {
    return { rows: [
      { id: id(), role: 'user', kind: 'command', text: 'go test ./...', time: t(8) },
      { id: id(), role: 'assistant', kind: 'output', html: 'ok  \tklark/hub-go/internal/server\t0.412s\n<span class="af2">PASS</span>', lines: 2, time: t(8) },
    ], rev: 1 };
  }
  return { rows: [
    { id: id(), role: 'user', kind: 'user', text: 'Port the Blazor hub to Go and React. Keep the Paper theme.', time: t(42) },
    { id: id(), role: 'assistant', kind: 'thinking', text: 'Plan: contract first, then the API, then the UI.', time: t(41) },
    { id: id(), role: 'assistant', kind: 'tool_use', tool: 'Read', summary: '/Users/me/code/klark/hub-go/API.md', input: '{"file_path":"/Users/me/code/klark/hub-go/API.md"}', toolUseId: 'a', time: t(41) },
    { id: id(), role: 'assistant', kind: 'tool_result', tool: 'Read', text: '# klarkhub (Go + React) contract\n...', ok: true, toolUseId: 'a', time: t(41) },
    { id: id(), role: 'assistant', kind: 'tool_use', tool: 'Grep', summary: 'SlashCommands', toolUseId: 'b', time: t(40) },
    { id: id(), role: 'assistant', kind: 'tool_result', tool: 'Grep', text: 'hub/Services/SlashCommands.cs:12', ok: true, toolUseId: 'b', time: t(40) },
    { id: id(), role: 'assistant', kind: 'text', time: t(39), html:
      `<h2>Where this stands</h2><p>The contract is in ${path(machine, pane, '/Users/me/code/klark/hub-go/API.md')}. Next, the shape of the <code>chat</code> endpoint:</p>` +
      code('json', '{ "rows": [ { "id": "i1", "role": "user", "kind": "user", "text": "hi" } ], "rev": 3 }') +
      '<ul><li>Windowed chat rows</li><li>Composer with <code>/</code> autocomplete</li></ul><table><thead><tr><th>Area</th><th>Owner</th></tr></thead><tbody><tr><td>API</td><td>backend</td></tr><tr><td>UI</td><td>frontend</td></tr></tbody></table>' },
    { id: id(), role: 'assistant', kind: 'tool_use', tool: 'Bash', summary: 'go test ./...', toolUseId: 'c', time: t(5) },
    { id: id(), role: 'assistant', kind: 'tool_result', tool: 'Bash', text: 'FAIL klark/hub-go/internal/server', ok: false, toolUseId: 'c', time: t(5) },
    { id: id(), role: 'user', kind: 'user', text: 'Fix the failing test, then run it again.', time: t(4) },
  ], rev: 1, offers: pane === 'p1' ? [{ id: 'o1', name: 'design-notes.pdf', size: 482113, mime: 'application/pdf', url: 'api/offers/o1' }] : [] };
}

export const files: Record<string, { meta: Record<string, unknown>; body: string; html?: string }> = {
  '/Users/me/code/klark/hub-go/API.md': {
    meta: { path: '/Users/me/code/klark/hub-go/API.md', name: 'API.md', view: 'markdown', size_label: '4.1 KB', lines: 6, lang: 'markdown' },
    body: '# klarkhub contract\n\nOne binary, one container.\n\n- `GET /api/machines`\n- `GET /api/auth/me`\n',
    html: '<h1>klarkhub contract</h1><p>One binary, one container.</p><ul><li><code>GET /api/machines</code></li><li><code>GET /api/auth/me</code></li></ul>',
  },
  '/Users/me/code/klark/hub-go/cmd/main.go': {
    meta: { path: '/Users/me/code/klark/hub-go/cmd/main.go', name: 'main.go', view: 'code', size_label: '120 B', lines: 5, lang: 'go' },
    body: 'package main\n\nfunc main() {\n\tprintln("klarkhub")\n}\n',
  },
};

export const messages = [
  { ts: new Date(Date.now() - 600000).toISOString(), from: 'macbook/klark/claude', to: 'build-box/ci/claude', text: 'Release 0.9.5.7 is ready for the build.', ok: true, sender: 'macbook/klark/claude' },
  { ts: new Date(Date.now() - 300000).toISOString(), from: 'alice', to: 'macbook/klark/devin', text: 'Please look at the failing test.', ok: false, error: 'agent not found' },
];
