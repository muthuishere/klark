import { useEffect, useMemo, useRef, useState } from 'react';
import { Link, useNavigate, useParams, useSearchParams } from 'react-router-dom';
import {
  ChatView, Composer, IconBack, IconFile, IconSidebar, PaneHeader, Terminal, useChat, webStorage,
  displayName, isAgent, paneAgent, paneCwd, paneStatus, agentKey, workspaceLabel,
  type Attachment, type ChatViewHandle, type TerminalHandle,
} from '@muthuishere/klark-hub-ui';
import { Menu } from '../components/Menu';
import { appPath, url } from '../lib/base';
import { slashCommandsFor } from '../lib/slash';
import { nextTheme, toggleSide, writeStore } from '../lib/theme';
import { transport } from '../lib/transport';
import { useMachines } from '../shell';
import { sizeLabel } from '../lib/files';

const drafts = webStorage(() => sessionStorage, 'klark:');

export function PanePage() {
  const { machine = '', pane = '' } = useParams();
  const [q, setQ] = useSearchParams();
  const view = q.get('view') === 'terminal' ? 'terminal' : 'chat';
  const { items: machines } = useMachines();
  const nav = useNavigate();
  const term = useRef<TerminalHandle>(null);
  const list = useRef<ChatViewHandle>(null);
  const chat = useChat(transport, machine, pane, view === 'terminal', term);
  const m = machines.find((x) => x.machine.id === machine);
  const info = m?.tree?.panes?.find((p) => p.pane_id === pane);
  const offline = m ? !m.machine.online : false;
  const [searchOpen, setSearchOpen] = useState(false);
  const [attachments, setAttachments] = useState<Attachment[]>([]);
  const [uploading, setUploading] = useState(false);
  const [toast, setToast] = useState<{ text: string; bad: boolean } | null>(null);
  const [dropping, setDropping] = useState(false);

  useEffect(() => { writeStore('lastpane', `${machine}\n${pane}`); }, [machine, pane]);
  useEffect(() => { setAttachments([]); setToast(null); }, [machine, pane]);
  const agent = info ? paneAgent(info) : '';
  // A choice menu on screen means the agent waits, even when the machine reports it idle.
  const status = offline ? '' : chat.blocked?.choices?.length ? 'blocked' : info ? paneStatus(info) : '';
  const title = info ? displayName(info) : pane;
  const session = info && m ? workspaceLabel(m.tree, info.workspace_id) : '';
  const statusText = offline ? 'offline' : chat.live === 'retrying' ? 'reconnecting…' : status === 'working' ? 'working…' : status === 'blocked' ? 'needs you' : status === 'idle' ? 'idle' : '';

  const notSent = (e: Error) => setToast({ text: `Not sent: ${e.message}`, bad: true });
  const keys = (k: string[]) => transport.keys(machine, pane, k).catch(notSent);
  const send = async (text: string) => {
    const lines = attachments.map((a) => `[image from hub] ${a.path}`);
    const full = [...lines, ...(text ? [text] : [])].join(' ');
    attachments.forEach(dropPreview);
    setAttachments([]);
    if (!full) return;
    await transport.input(machine, pane, full).catch(notSent);
    await keys(['enter']);
    list.current?.scrollToBottom();
  };
  const dropPreview = (a: Attachment) => { if (a.preview) try { URL.revokeObjectURL(a.preview); } catch { /* ignore */ } };
  const upload = async (files: File[]) => {
    setUploading(true);
    for (const f of files) {
      try {
        const r = await transport.upload(machine, pane, f);
        const preview = typeof URL.createObjectURL === 'function' ? URL.createObjectURL(f) : undefined;
        setAttachments((cur) => [...cur, { name: r.name || f.name, path: r.path, preview }]);
      } catch (e) { setToast({ text: `Not uploaded: ${(e as Error).message}`, bad: true }); }
    }
    setUploading(false);
  };

  // File links in the chat open the viewer without leaving the app.
  const onNavigate = (href: string, a: HTMLAnchorElement) => {
    const p = appPath(href);
    if (p && (a.classList.contains('fpath') || /^\/m\/[^/]+\/f(\?|$)/.test(p))) { nav(p); return true; }
    return false;
  };
  const fileHref = useMemo(() => (path: string, line?: number) => transport.fileUrl(machine, pane, path, line), [machine, pane]);

  const readOnly = false;
  return (
    <main className={'pane-app reader-' + (agent ? (agentKey(agent) === 'claude' ? 'claude' : agentKey(agent) === 'other' ? 'generic' : 'devin') : 'shell') + (dropping ? ' dropping' : '')}
      onDragOver={(e) => { if (e.dataTransfer.types.includes('Files')) { e.preventDefault(); setDropping(true); } }}
      onDragLeave={() => setDropping(false)}
      onDrop={(e) => { e.preventDefault(); setDropping(false); const f = [...e.dataTransfer.files].filter((x) => x.type.startsWith('image/')); if (f.length) void upload(f); }}>
      <PaneHeader
        title={title} agent={agent} session={session} status={status} statusText={statusText} cwd={info ? paneCwd(info) : undefined}
        view={view} onToggleView={() => { const n = new URLSearchParams(q); if (view === 'terminal') n.delete('view'); else n.set('view', 'terminal'); setQ(n, { replace: true }); }}
        leading={<>
          <button type="button" className="icon side-open" aria-label="Sidebar" title="Sidebar" onClick={toggleSide}><IconSidebar /></button>
          <Link className="icon back" to="/" aria-label="Back"><IconBack /></Link>
        </>}
        actions={
          <Menu label="More">
            {view === 'chat' && <button type="button" className="menu-item" onClick={() => setSearchOpen((o) => !o)}>{searchOpen ? 'Close search' : 'Search in chat'}</button>}
            <button type="button" className="menu-item" onClick={() => nextTheme()}>Next theme</button>
            <div className="menu-info">
              <div>{agent || 'shell'} · {offline ? 'offline' : status || 'ready'}</div>
              <div>{machine}{session ? ' · ' + session : ''}</div>
              {info && paneCwd(info) && <div className="mono">{paneCwd(info)}</div>}
            </div>
          </Menu>
        }
      />

      <div className="pane-body">
        {view === 'chat' && (
          <ChatView
            ref={list} items={chat.items} loading={chat.loading} error={chat.error} blocked={chat.blocked} status={status} offline={offline}
            fileHref={fileHref} onNavigate={onNavigate} searchOpen={searchOpen} onSearchClose={() => setSearchOpen(false)}
            onKeys={async (k) => { await keys(k); setTimeout(() => void chat.reload(), 400); }}
            footer={chat.offers.length > 0 && (
              <div className="files chat-col" aria-label="Files offered by the agent">
                {chat.offers.map((o) => (
                  <a key={o.id} className="fcard" href={url(o.url)} download={o.name}>
                    <span className="ficon"><IconFile /></span><span className="fmeta"><span className="fname">{o.name}</span>{o.size != null && <span className="muted">{sizeLabel(o.size)}</span>}</span>
                  </a>
                ))}
              </div>
            )}
          />
        )}
        <Terminal ref={term} cols={chat.size.cols} rows={chat.size.rows} visible={view === 'terminal'} />
      </div>

      {toast && <div className={'toast' + (toast.bad ? ' bad' : '')} role="alert" onClick={() => setToast(null)}>{toast.text}</div>}
      {offline || (m && !m.machine.online)
        ? <div className="composer-wrap"><div className="composer ro muted">Machine offline: run <code>klark hub connect</code> on it to send messages</div></div>
        : readOnly ? <div className="composer-wrap"><div className="composer ro muted">Read-only</div></div>
        : <Composer key={machine + '/' + pane} draftKey={machine + '/' + pane} storage={drafts} commands={info && isAgent(info) ? slashCommandsFor(agent) : []} terminal={view === 'terminal'}
            attachments={attachments} uploading={uploading} onRemoveAttachment={(a) => { dropPreview(a); setAttachments((c) => c.filter((x) => x !== a)); }} onFiles={(f) => void upload(f)} onSend={send}
            onKey={(k) => void keys([k])} />}
    </main>
  );
}
