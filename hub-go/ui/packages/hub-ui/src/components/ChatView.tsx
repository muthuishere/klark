import { forwardRef, useImperativeHandle, useMemo, useRef, useState, type MouseEvent, type ReactNode } from 'react';
import { buildRows, type ChatRowModel } from '../chat';
import type { Blocked, ChatItem } from '../types';
import { ChatRow, FileHrefCtx, OpenCtx, stripTags } from './ChatRow';
import { ChoiceBar } from './ChoiceBar';
import { VirtualList, type VirtualHandle } from './VirtualList';

export interface ChatViewProps {
  items: ChatItem[];
  loading?: boolean;
  error?: string | null;
  blocked?: Blocked | null;
  /** working shows a "working…" line. */
  status?: string;
  /** Hides the choice bar and the working line. */
  offline?: boolean;
  /** Answer a choice menu (key names). */
  onKeys?: (keys: string[]) => Promise<unknown> | void;
  /** Links of file paths in tool rows (server-rendered paths come linked already). */
  fileHref?: (path: string, line?: number) => string;
  /**
   * A plain left click on a link inside the chat (file paths, `a.fpath`). Return true to say it was handled
   * (the browser's navigation is then prevented).
   */
  onNavigate?: (href: string, a: HTMLAnchorElement) => boolean | void;
  /** Search bar above the chat. */
  searchOpen?: boolean;
  onSearchClose?: () => void;
  /** Below the list, above the choice bar (e.g. files the agent offered). */
  footer?: ReactNode;
  className?: string;
}

export interface ChatViewHandle { scrollToBottom: () => void }

const keyOf = (r: ChatRowModel) => r.key;

/** A windowed chat: rows, tool groups, a choice bar, search, "jump to latest". */
export const ChatView = forwardRef<ChatViewHandle, ChatViewProps>(function ChatView(
  { items, loading, error, blocked, status, offline, onKeys, fileHref, onNavigate, searchOpen, onSearchClose, footer, className = '' }, ref) {
  const list = useRef<VirtualHandle>(null);
  const rows = useMemo(() => buildRows(items), [items]);
  const [pinned, setPinned] = useState(true);
  const [needle, setNeedle] = useState('');
  const [hit, setHit] = useState(0);
  const open = useRef(new Set<string>());
  const openCtx = useMemo(() => ({ has: (k: string) => open.current.has(k), set: (k: string, v: boolean) => { if (v) open.current.add(k); else open.current.delete(k); } }), []);
  useImperativeHandle(ref, () => ({ scrollToBottom: () => list.current?.scrollToBottom() }));

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

  // Copy buttons of code blocks, and links (file paths) handed to onNavigate.
  const onClick = (e: MouseEvent) => {
    const t = e.target as HTMLElement;
    const copy = t.closest<HTMLElement>('.codeblk .copy');
    if (copy) {
      const code = copy.closest('.codeblk')?.querySelector('code')?.textContent ?? '';
      void navigator.clipboard?.writeText(code).then(() => { copy.textContent = 'Copied'; setTimeout(() => { copy.textContent = 'Copy'; }, 1500); });
      return;
    }
    const a = t.closest<HTMLAnchorElement>('a[href]');
    if (!a || !onNavigate || a.target === '_blank' || e.metaKey || e.ctrlKey || e.shiftKey || e.button !== 0) return;
    if (onNavigate(a.href, a)) e.preventDefault();
  };

  const header = loading ? <div className="skel-chat" aria-hidden="true"><div className="skel bubble-r" /><div className="skel line w90" /><div className="skel line w80" /><div className="skel tool" /></div>
    : rows.length === 0 ? <p className="empty chat-empty">{error ?? 'Nothing here yet.'}</p> : null;

  return (
    <div className={'chat ' + className} onClick={onClick}>
      {searchOpen && (
        <div className="chat-search" role="search">
          <input type="search" autoFocus aria-label="Search in chat" placeholder="Search in chat" value={needle}
            onChange={(e) => { setNeedle(e.target.value); setHit(0); }} onKeyDown={(e) => { if (e.key === 'Enter') jump(e.shiftKey ? -1 : 1); if (e.key === 'Escape') onSearchClose?.(); }} />
          <span className="muted" aria-live="polite">{needle ? `${matches.length ? hit + 1 : 0}/${matches.length}` : ''}</span>
          <button type="button" className="icon" aria-label="Previous match" onClick={() => jump(-1)}>↑</button>
          <button type="button" className="icon" aria-label="Next match" onClick={() => jump(1)}>↓</button>
        </div>
      )}
      <FileHrefCtx.Provider value={fileHref ?? null}>
        <OpenCtx.Provider value={openCtx}>
          <VirtualList ref={list} items={rows} getKey={keyOf} onPinnedChange={setPinned} header={header} render={(r) => <ChatRow row={r} />} />
        </OpenCtx.Provider>
      </FileHrefCtx.Provider>
      {footer}
      {blocked && !offline && onKeys && <ChoiceBar blocked={blocked} onKeys={async (keys) => { await onKeys(keys); list.current?.scrollToBottom(); }} />}
      {!pinned && <button type="button" className="jump show" onClick={() => list.current?.scrollToBottom()}>Jump to latest</button>}
      {status === 'working' && !offline && <div className="live live-working" role="status"><span className="dot" aria-hidden="true" />working…</div>}
    </div>
  );
});
