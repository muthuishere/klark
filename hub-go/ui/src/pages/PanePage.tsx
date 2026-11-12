import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { Link, useNavigate, useParams, useSearchParams } from 'react-router-dom';
import { Composer, type Attachment } from '../components/Composer';
import { ChatRow, OpenCtx, PaneCtx, stripTags } from '../components/ChatRow';
import { IconBack, IconChat, IconSidebar, IconTerminal, IconFile } from '../components/Icons';
import { Menu } from '../components/Menu';
import { TerminalView, decodeBase64, type TerminalHandle } from '../components/Terminal';
import { VirtualList, type VirtualHandle } from '../components/VirtualList';
import { api, paneBase } from '../lib/api';
import { appPath, url } from '../lib/base';
import { buildRows, mergeItems, type ChatRowModel } from '../lib/chat';
import { agentIcon, agentKey, displayName, isAgent, paneAgent, paneCwd, paneStatus, workspaceLabel } from '../lib/panes';
import { connectSSE, type SseState } from '../lib/sse';
import { slashCommandsFor } from '../lib/slash';
import { nextTheme, toggleSide, writeStore } from '../lib/theme';
import type { Blocked, ChatItem, ChatPayload, Offer } from '../lib/types';
import { ChoiceBar } from '../components/ChoiceBar';
import { useMachines } from '../shell';
import { sizeLabel } from '../lib/files';

const parse = <T,>(s: string): T | null => { try { return JSON.parse(s) as T; } catch { return null; } };

/** Chat rows for one pane: an initial GET, then `chat` events; a reconnect refetches what was missed. */
export function useChat(machine: string, pane: string, wantFrames: boolean, term: React.RefObject<TerminalHandle | null>) {
  const [items, setItems] = useState<ChatItem[]>([]);
  const [offers, setOffers] = useState<Offer[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [live, setLive] = useState<SseState>('connecting');
  const [size, setSize] = useState({ cols: 120, rows: 40 });
  const [blocked, setBlocked] = useState<Blocked | null>(null);
  const meta = (p: ChatPayload) => { setBlocked(p.blocked ?? null); };

  const load = useCallback(() =>
    api<ChatPayload>(`${paneBase(machine, pane)}/chat`)
      .then((p) => { meta(p); setItems((cur) => (cur.length ? mergeItems(cur, p.rows ?? []) : p.rows ?? [])); setOffers(p.offers ?? []); setError(null); })
      .catch((e) => setError(e.message))
      .finally(() => setLoading(false)), [machine, pane]);

  useEffect(() => { setItems([]); setOffers([]); setBlocked(null); setLoading(true); void load(); }, [load]);
  useEffect(() => connectSSE(`${paneBase(machine, pane)}/events${wantFrames ? '?frames=1' : ''}`, {
    events: ['chat', 'frame', 'size', 'offer'],
    onState: setLive,
    onResync: () => void load(),
    onEvent: (name, data) => {
      if (name === 'chat') {
        const p = parse<ChatPayload | ChatItem[] | ChatItem>(data);
        const rows = Array.isArray(p) ? p : p && 'rows' in p ? p.rows : p ? [p as ChatItem] : [];
        if (p && !Array.isArray(p) && 'rows' in p) meta(p);
        setItems((cur) => mergeItems(cur, rows));
      } else if (name === 'frame') {
        const f = parse<{ data: string; reset?: boolean }>(data);
        const bytes = decodeBase64(f ? f.data : data);
        if (f?.reset) term.current?.reset(bytes); else term.current?.write(bytes);
      } else if (name === 'size') {
        const s = parse<{ cols: number; rows: number }>(data);
        if (s) { setSize(s); term.current?.resize(s.cols, s.rows); }
      } else if (name === 'offer') {
        const o = parse<Offer>(data);
        if (o) setOffers((cur) => (cur.some((x) => x.id === o.id) ? cur : [...cur, o]));
      }
    },
  }), [machine, pane, wantFrames, load, term]);
  return { items, offers, loading, error, live, size, blocked, reload: load };
}

const keyOf = (r: ChatRowModel) => r.key;

export function PanePage() {
  const { machine = '', pane = '' } = useParams();
  const [q, setQ] = useSearchParams();
  const view = q.get('view') === 'terminal' ? 'terminal' : 'chat';
  const { items: machines } = useMachines();
  const nav = useNavigate();
  const term = useRef<TerminalHandle>(null);
  const list = useRef<VirtualHandle>(null);
  const chat = useChat(machine, pane, view === 'terminal', term);
  const m = machines.find((x) => x.machine.id === machine);
  const info = m?.tree?.panes?.find((p) => p.pane_id === pane);
  const offline = m ? !m.machine.online : false;
  const rows = useMemo(() => buildRows(chat.items), [chat.items]);
  const [pinned, setPinned] = useState(true);
  const [searchOpen, setSearchOpen] = useState(false);
  const [needle, setNeedle] = useState('');
  const [hit, setHit] = useState(0);
  const [attachments, setAttachments] = useState<Attachment[]>([]);
  const [uploading, setUploading] = useState(false);
  const [toast, setToast] = useState<{ text: string; bad: boolean } | null>(null);
  const [dropping, setDropping] = useState(false);
  const open = useRef(new Set<string>());
  const openCtx = useMemo(() => ({ has: (k: string) => open.current.has(k), set: (k: string, v: boolean) => { if (v) open.current.add(k); else open.current.delete(k); } }), []);

  useEffect(() => { writeStore('lastpane', `${machine}\n${pane}`); }, [machine, pane]);
  useEffect(() => { setAttachments([]); setToast(null); }, [machine, pane]);
  const agent = info ? paneAgent(info) : '';
  // A choice menu on screen means the agent waits, even when the machine reports it idle.
  const status = offline ? '' : chat.blocked?.choices?.length ? 'blocked' : info ? paneStatus(info) : '';
  const title = info ? displayName(info) : pane;
  const session = info && m ? workspaceLabel(m.tree, info.workspace_id) : '';
  const statusText = offline ? 'offline' : chat.live === 'retrying' ? 'reconnecting…' : status === 'working' ? 'working…' : status === 'blocked' ? 'needs you' : status === 'idle' ? 'idle' : '';

  const matches = useMemo(() => {
    const n = needle.trim().toLowerCase();
    if (!n) return [] as number[];
    return rows.flatMap((r, i) => (r.items.some((it) => (it.text ?? stripTags(it.html) + ' ' + (it.summary ?? '')).toLowerCase().includes(n)) ? [i] : []));
  }, [rows, needle]);
  const jump = (d: number) => {
    if (!matches.length) return;
    const n = (hit + d + matches.length) % matches.length;
    setHit(n);
    list.current?.scrollToIndex(matches[n]);
  };

  const post = (path: string, json: unknown) => api<void>(`${paneBase(machine, pane)}/${path}`, { method: 'POST', json }).catch((e) => setToast({ text: `Not sent: ${e.message}`, bad: true }));
  const send = async (text: string) => {
    const lines = attachments.map((a) => `[image from hub] ${a.path}`);
    const full = [...lines, ...(text ? [text] : [])].join(' ');
    attachments.forEach(dropPreview);
    setAttachments([]);
    if (!full) return;
    await post('input', { text: full });
    await post('keys', { keys: ['enter'] });
    list.current?.scrollToBottom();
  };
  const dropPreview = (a: Attachment) => { if (a.preview) try { URL.revokeObjectURL(a.preview); } catch { /* ignore */ } };
  const upload = async (files: File[]) => {
    setUploading(true);
    for (const f of files) {
      try {
        const fd = new FormData();
        fd.append('file', f, f.name);
        const r = await api<{ name?: string; path: string }>(`${paneBase(machine, pane)}/upload`, { method: 'POST', body: fd });
        const preview = typeof URL.createObjectURL === 'function' ? URL.createObjectURL(f) : undefined;
        setAttachments((cur) => [...cur, { name: r.name || f.name, path: r.path, preview }]);
      } catch (e) { setToast({ text: `Not uploaded: ${(e as Error).message}`, bad: true }); }
    }
    setUploading(false);
  };

  // Links the server rendered inside chat HTML: copy buttons, and file paths that open the viewer without leaving the app.
  const onChatClick = (e: React.MouseEvent) => {
    const t = e.target as HTMLElement;
    const copy = t.closest<HTMLElement>('.codeblk .copy');
    if (copy) {
      const code = copy.closest('.codeblk')?.querySelector('code')?.textContent ?? '';
      void navigator.clipboard?.writeText(code).then(() => { copy.textContent = 'Copied'; setTimeout(() => { copy.textContent = 'Copy'; }, 1500); });
      return;
    }
    const a = t.closest<HTMLAnchorElement>('a[href]');
    if (!a || a.target === '_blank' || e.metaKey || e.ctrlKey || e.shiftKey || e.button !== 0) return;
    const p = appPath(a.href);
    if (p && (a.classList.contains('fpath') || /^\/m\/[^/]+\/f(\?|$)/.test(p))) { e.preventDefault(); nav(p); }
  };

  const readOnly = false;
  const header = chat.loading ? <div className="skel-chat" aria-hidden="true"><div className="skel bubble-r" /><div className="skel line w90" /><div className="skel line w80" /><div className="skel tool" /></div>
    : rows.length === 0 ? <p className="empty chat-empty">{chat.error ?? 'Nothing here yet.'}</p> : null;

  return (
    <main className={'pane-app reader-' + (agent ? (agentKey(agent) === 'claude' ? 'claude' : agentKey(agent) === 'other' ? 'generic' : 'devin') : 'shell') + (dropping ? ' dropping' : '')}
      onDragOver={(e) => { if (e.dataTransfer.types.includes('Files')) { e.preventDefault(); setDropping(true); } }}
      onDragLeave={() => setDropping(false)}
      onDrop={(e) => { e.preventDefault(); setDropping(false); const f = [...e.dataTransfer.files].filter((x) => x.type.startsWith('image/')); if (f.length) void upload(f); }}>
      <header className="pane-bar">
        <button type="button" className="icon side-open" aria-label="Sidebar" title="Sidebar" onClick={toggleSide}><IconSidebar /></button>
        <Link className="icon back" to="/" aria-label="Back"><IconBack /></Link>
        <div className="who" title={info ? paneCwd(info) : undefined}>
          <span className="badge-a" data-agent={agentKey(agent)} aria-hidden="true">{agentIcon(agent)}</span>
          <h1 className="name">{title}</h1>
          {session && <span className="pill" title={'Session on ' + machine}>{session}</span>}
          <span className={'state state-' + (status || 'none')} role="status">{statusText}</span>
        </div>
        <button type="button" className={'icon tgl' + (view === 'terminal' ? ' on' : '')} aria-pressed={view === 'terminal'} aria-label="Terminal" title={view === 'terminal' ? 'Back to chat' : 'Terminal'}
          onClick={() => { const n = new URLSearchParams(q); if (view === 'terminal') n.delete('view'); else n.set('view', 'terminal'); setQ(n, { replace: true }); }}>
          {view === 'terminal' ? <IconChat /> : <IconTerminal />}
        </button>
        <Menu label="More">
          {view === 'chat' && <button type="button" className="menu-item" onClick={() => setSearchOpen((o) => !o)}>{searchOpen ? 'Close search' : 'Search in chat'}</button>}
          <button type="button" className="menu-item" onClick={() => nextTheme()}>Next theme</button>
          <div className="menu-info">
            <div>{agent || 'shell'} · {offline ? 'offline' : status || 'ready'}</div>
            <div>{machine}{session ? ' · ' + session : ''}</div>
            {info && paneCwd(info) && <div className="mono">{paneCwd(info)}</div>}
          </div>
        </Menu>
      </header>

      <div className="pane-body">
        {view === 'chat' && (
          <div className="chat" onClick={onChatClick}>
            {searchOpen && (
              <div className="chat-search" role="search">
                <input type="search" autoFocus aria-label="Search in chat" placeholder="Search in chat" value={needle}
                  onChange={(e) => { setNeedle(e.target.value); setHit(0); }} onKeyDown={(e) => { if (e.key === 'Enter') jump(e.shiftKey ? -1 : 1); if (e.key === 'Escape') setSearchOpen(false); }} />
                <span className="muted" aria-live="polite">{needle ? `${matches.length ? hit + 1 : 0}/${matches.length}` : ''}</span>
                <button type="button" className="icon" aria-label="Previous match" onClick={() => jump(-1)}>↑</button>
                <button type="button" className="icon" aria-label="Next match" onClick={() => jump(1)}>↓</button>
              </div>
            )}
            <PaneCtx.Provider value={{ machine, pane }}>
              <OpenCtx.Provider value={openCtx}>
                <VirtualList ref={list} items={rows} getKey={keyOf} onPinnedChange={setPinned} header={header}
                  render={(r) => <ChatRow row={r} />} />
              </OpenCtx.Provider>
            </PaneCtx.Provider>
            {chat.offers.length > 0 && (
              <div className="files chat-col" aria-label="Files offered by the agent">
                {chat.offers.map((o) => (
                  <a key={o.id} className="fcard" href={url(o.url)} download={o.name}>
                    <span className="ficon"><IconFile /></span><span className="fmeta"><span className="fname">{o.name}</span>{o.size != null && <span className="muted">{sizeLabel(o.size)}</span>}</span>
                  </a>
                ))}
              </div>
            )}
            {chat.blocked && !offline && <ChoiceBar blocked={chat.blocked} onKeys={async (keys) => { await post('keys', { keys }); list.current?.scrollToBottom(); setTimeout(() => void chat.reload(), 400); }} />}
            {!pinned && <button type="button" className="jump show" onClick={() => list.current?.scrollToBottom()}>Jump to latest</button>}
            {status === 'working' && !offline && <div className="live live-working" role="status"><span className="dot" aria-hidden="true" />working…</div>}
          </div>
        )}
        <TerminalView ref={term} cols={chat.size.cols} rows={chat.size.rows} visible={view === 'terminal'} />
      </div>

      {toast && <div className={'toast' + (toast.bad ? ' bad' : '')} role="alert" onClick={() => setToast(null)}>{toast.text}</div>}
      {offline || (m && !m.machine.online)
        ? <div className="composer-wrap"><div className="composer ro muted">Machine offline: run <code>klark hub connect</code> on it to send messages</div></div>
        : readOnly ? <div className="composer-wrap"><div className="composer ro muted">Read-only</div></div>
        : <Composer key={machine + '/' + pane} draftKey={machine + '/' + pane} commands={info && isAgent(info) ? slashCommandsFor(agent) : []} terminal={view === 'terminal'}
            attachments={attachments} uploading={uploading} onRemoveAttachment={(a) => { dropPreview(a); setAttachments((c) => c.filter((x) => x !== a)); }} onFiles={(f) => void upload(f)} onSend={send}
            onKey={(k) => void post('keys', { keys: [k] })} />}
    </main>
  );
}
