import { useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import type { Transport } from '../transport';
import type { Offer, PaneInfo } from '../types';
import { agentKey, displayName, isAgent, paneAgent, paneCwd, paneStatus } from '../panes';
import { webStorage, type Storage } from '../storage';
import { useChat } from '../useChat';
import { ChatView, type ChatViewHandle } from './ChatView';
import { Composer, type Attachment, type SlashCommand } from './Composer';
import { IconFile } from './Icons';
import { PaneHeader } from './PaneHeader';
import { Terminal, type TerminalHandle } from './Terminal';

/** What the host's header menu gets: the search toggle lives in the pane, the menu in the host. */
export interface HubPaneActions {
  view: 'chat' | 'terminal';
  searchOpen: boolean;
  toggleSearch: () => void;
  agent: string;
  status: string;
  offline: boolean;
}

export interface HubPaneProps {
  transport: Transport;
  machine: string;
  pane: string;
  /** The pane from the machine's tree; undefined while it loads (the title falls back to the pane id). */
  info?: PaneInfo;
  /** The machine is connected. false: the composer is replaced by offlineText. */
  online: boolean;
  /** The pane's session (workspace) label. */
  session?: string;
  view: 'chat' | 'terminal';
  onViewChange: (v: 'chat' | 'terminal') => void;
  /** This viewer may type, press keys and upload on THIS machine (the host decides per machine). Default true. */
  canType?: boolean;
  /** Shown instead of the composer when canType is false. */
  readOnlyText?: ReactNode;
  /** Shown instead of the composer when the machine is offline. */
  offlineText?: ReactNode;
  /** Header: before the title (sidebar toggle, back link). */
  leading?: ReactNode;
  /** Header: the host's menu, given the pane's state. */
  renderActions?: (a: HubPaneActions) => ReactNode;
  /** Slash commands for the composer; default none. */
  commands?: SlashCommand[];
  /** A link in the chat: return true when the host handled it (opened it in-app). */
  onNavigate?: (href: string, a: HTMLAnchorElement) => boolean;
  /** Where an offered file downloads from; default its url as given. */
  offerHref?: (o: Offer) => string;
  /** Composer drafts; default sessionStorage under "klark:". */
  drafts?: Storage;
  /** The text a sent image becomes in front of the message; default "[image from hub] <path>". */
  imageLine?: (path: string) => string;
}

const sessionDrafts = webStorage(() => sessionStorage, 'klark:');

// The page's reader class picks the chat's look. OpenCode, like Claude and Devin, comes from its own log as markdown,
// so it gets proportional text; "generic" (monospace) is for agents the hub still reads off the screen.
function readerClass(agent: string): string {
  if (!agent) return 'shell';
  const k = agentKey(agent);
  return k === 'claude' ? 'claude' : k === 'opencode' ? 'opencode' : k === 'other' ? 'generic' : 'devin';
}

function sizeLabel(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(n < 10 * 1024 ? 1 : 0)} KB`;
  return `${(n / 1024 / 1024).toFixed(1)} MB`;
}

/**
 * One agent's page: header, chat or terminal, offered files, and the composer (with image upload and drop). Router
 * free: the host gives machine, pane, view and the menu. klark's own hub and hosts like the MCP Store console use it
 * over their own Transport, so the page looks and behaves the same everywhere.
 */
export function HubPane({
  transport, machine, pane, info, online, session = '', view, onViewChange, canType = true,
  readOnlyText = 'Read-only', offlineText = 'Machine offline', leading, renderActions, commands = [],
  onNavigate, offerHref = (o) => o.url, drafts = sessionDrafts, imageLine = (p) => `[image from hub] ${p}`,
}: HubPaneProps) {
  const term = useRef<TerminalHandle>(null);
  const list = useRef<ChatViewHandle>(null);
  const chat = useChat(transport, machine, pane, view === 'terminal', term);
  const offline = !online;
  const [searchOpen, setSearchOpen] = useState(false);
  const [attachments, setAttachments] = useState<Attachment[]>([]);
  const [uploading, setUploading] = useState(false);
  const [toast, setToast] = useState<{ text: string; bad: boolean } | null>(null);
  const [dropping, setDropping] = useState(false);

  useEffect(() => { setAttachments([]); setToast(null); }, [machine, pane]);
  const agent = info ? paneAgent(info) : '';
  // A choice menu on screen means the agent waits, even when the machine reports it idle.
  const status = offline ? '' : chat.blocked?.choices?.length ? 'blocked' : info ? paneStatus(info) : '';
  const title = info ? displayName(info) : pane;
  const statusText = offline ? 'offline' : chat.live === 'retrying' ? 'reconnecting…' : status === 'working' ? 'working…' : status === 'blocked' ? 'needs you' : status === 'idle' ? 'idle' : '';

  const notSent = (e: Error) => setToast({ text: `Not sent: ${e.message}`, bad: true });
  const keys = (k: string[]) => transport.keys(machine, pane, k).catch(notSent);
  const dropPreview = (a: Attachment) => { if (a.preview) try { URL.revokeObjectURL(a.preview); } catch { /* ignore */ } };
  const send = async (text: string) => {
    const lines = attachments.map((a) => imageLine(a.path));
    const full = [...lines, ...(text ? [text] : [])].join(' ');
    attachments.forEach(dropPreview);
    setAttachments([]);
    if (!full) return;
    await transport.input(machine, pane, full).catch(notSent);
    await keys(['enter']);
    list.current?.scrollToBottom();
  };
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
  const fileHref = useMemo(() => (path: string, line?: number) => transport.fileUrl(machine, pane, path, line), [transport, machine, pane]);
  const typing = online && canType;

  return (
    <main className={'pane-app reader-' + readerClass(agent) + (dropping ? ' dropping' : '')}
      onDragOver={(e) => { if (typing && e.dataTransfer.types.includes('Files')) { e.preventDefault(); setDropping(true); } }}
      onDragLeave={() => setDropping(false)}
      onDrop={(e) => {
        e.preventDefault(); setDropping(false);
        if (!typing) return;
        const f = [...e.dataTransfer.files].filter((x) => x.type.startsWith('image/'));
        if (f.length) void upload(f);
      }}>
      <PaneHeader
        title={title} agent={agent} session={session} status={status} statusText={statusText} cwd={info ? paneCwd(info) : undefined}
        view={view} onToggleView={() => onViewChange(view === 'terminal' ? 'chat' : 'terminal')}
        leading={leading}
        actions={renderActions?.({ view, searchOpen, toggleSearch: () => setSearchOpen((o) => !o), agent, status, offline })}
      />

      <div className="pane-body">
        {view === 'chat' && (
          <ChatView
            ref={list} items={chat.items} loading={chat.loading} error={chat.error} blocked={chat.blocked} status={status} offline={offline}
            fileHref={fileHref} onNavigate={onNavigate} searchOpen={searchOpen} onSearchClose={() => setSearchOpen(false)}
            onKeys={typing ? async (k) => { await keys(k); setTimeout(() => void chat.reload(), 400); } : undefined}
            footer={chat.offers.length > 0 && (
              <div className="files chat-col" aria-label="Files offered by the agent">
                {chat.offers.map((o) => (
                  <a key={o.id} className="fcard" href={offerHref(o)} download={o.name}>
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
      {offline
        ? <div className="composer-wrap"><div className="composer ro muted">{offlineText}</div></div>
        : !canType ? <div className="composer-wrap"><div className="composer ro muted">{readOnlyText}</div></div>
        : <Composer key={machine + '/' + pane} draftKey={machine + '/' + pane} storage={drafts} commands={info && isAgent(info) ? commands : []} terminal={view === 'terminal'}
            attachments={attachments} uploading={uploading} onRemoveAttachment={(a) => { dropPreview(a); setAttachments((c) => c.filter((x) => x !== a)); }} onFiles={(f) => void upload(f)} onSend={send}
            onKey={(k) => void keys([k])} />}
    </main>
  );
}
