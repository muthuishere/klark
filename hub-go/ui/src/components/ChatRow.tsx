import { createContext, memo, useContext, type ReactNode } from 'react';
import { pairCalls, phrase, toolIcon, verb, type ChatRowModel, type ToolCall } from '../lib/chat';
import { viewerHref } from '../lib/files';
import { Link } from 'react-router-dom';

/** Which <details> are open, kept outside the rows so a row scrolled out of the window and back keeps its state. */
export const OpenCtx = createContext<{ has: (k: string) => boolean; set: (k: string, open: boolean) => void }>({ has: () => false, set: () => {} });
export const PaneCtx = createContext<{ machine: string; pane: string }>({ machine: '', pane: '' });

const fmt = (t?: string) => (t ? new Date(t).toLocaleString() : undefined);

function Details({ k, defaultOpen, className, children }: { k: string; defaultOpen?: boolean; className?: string; children: (open: boolean) => ReactNode }) {
  const o = useContext(OpenCtx);
  const open = o.has('x:' + k) ? o.has(k) : !!defaultOpen;
  return (
    <details className={className} open={open} onToggle={(e) => { const v = (e.currentTarget as HTMLDetailsElement).open; if (v !== open) { o.set('x:' + k, true); o.set(k, v); } }}>
      {children(open)}
    </details>
  );
}

function Html({ html, cls }: { html?: string; cls: string }) {
  // The hub renders markdown / ANSI to escaped HTML on the server (Markdig-equivalent pipeline); nothing here is user-supplied raw HTML.
  return <div className={cls} dangerouslySetInnerHTML={{ __html: html ?? '' }} />;
}

function Call({ c }: { c: ToolCall }) {
  const { machine, pane } = useContext(PaneCtx);
  if (c.kind === 'thinking') {
    return (
      <Details k={'t:' + c.key} className="tool">
        {() => (<><summary><span className="ico">✦</span><span className="t">Thought</span></summary><pre className="tool-body">{c.use?.text}</pre></>)}
      </Details>
    );
  }
  const name = c.use?.tool ?? c.result?.tool ?? 'Tool';
  const arg = c.use?.summary ?? '';
  const [v, noun] = verb(name);
  const isFile = noun === 'file' && arg.startsWith('/');
  const failed = c.result?.ok === false;
  return (
    <Details k={'t:' + c.key} className={'tool' + (failed ? ' failed' : '')}>
      {() => (
        <>
          <summary title={fmt(c.use?.time ?? c.result?.time)}>
            <span className="ico">{toolIcon(name)}</span><span className="t">{v}</span>
            {arg && (isFile
              ? <Link className="arg fpath" to={viewerHref(machine, pane, arg)} onClick={(e) => e.stopPropagation()}>{arg}</Link>
              : <span className="arg">{arg}</span>)}
            {!c.result ? <span className="state">…</span> : failed ? <span className="state bad" aria-label="failed">✗</span> : c.result.ok ? <span className="state ok" aria-label="done">✓</span> : null}
          </summary>
          {c.use?.input && <pre className="tool-body">{c.use.input}</pre>}
          {c.result?.text && <pre className="tool-body result">{c.result.text}</pre>}
        </>
      )}
    </Details>
  );
}

function Body({ item }: { item: { html?: string; text?: string; kind: string } }) {
  if (item.kind === 'output') return <pre className="out" dangerouslySetInnerHTML={{ __html: item.html ?? '' }} />;
  return item.html != null ? <Html html={item.html} cls="md" /> : <div className="md"><p>{item.text}</p></div>;
}

function ChatRowImpl({ row }: { row: ChatRowModel }) {
  const first = row.items[0];
  switch (row.kind) {
    case 'user':
      return <div className="msg user" data-id={row.key} title={fmt(first.time)}><div className="bubble">{first.text ?? stripTags(first.html)}</div></div>;
    case 'text':
      return <div className="msg asst" data-id={row.key} title={fmt(first.time)}><Body item={first} /></div>;
    case 'block': {
      const cmd = row.items.find((i) => i.kind === 'command');
      const out = row.items.find((i) => i.kind === 'output');
      const lines = out?.lines ?? (out?.html ? out.html.split('\n').length : 0);
      return (
        <Details k={'b:' + row.key} className="block" defaultOpen={lines <= 40}>
          {() => (
            <>
              <summary><span className="prompt">$</span><span className="cmd">{cmd?.text ?? 'output'}</span>{lines > 0 && <span className="state">{lines} {lines === 1 ? 'line' : 'lines'}</span>}</summary>
              {out && <Body item={out} />}
            </>
          )}
        </Details>
      );
    }
    default: {
      const calls = pairCalls(row.items);
      if (calls.length === 1) return <div data-id={row.key}><Call c={calls[0]} /></div>;
      const tools = calls.filter((c) => c.kind === 'tool');
      const label = phrase(tools.map((c) => c.use?.tool ?? c.result?.tool ?? 'Tool'), calls.length - tools.length);
      const bad = tools.some((c) => c.result?.ok === false);
      const pending = tools.some((c) => !c.result);
      const ok = tools.length > 0 && tools.every((c) => c.result?.ok === true);
      return (
        <Details k={'g:' + row.key} className="tools">
          {() => (
            <>
              <summary><span className="ico">⋯</span><span className="t">{label}</span>{bad ? <span className="state bad" aria-label="failed">✗</span> : pending ? <span className="state">…</span> : ok ? <span className="state ok" aria-label="done">✓</span> : null}</summary>
              <div className="tool-list">{calls.map((c) => <Call key={c.key} c={c} />)}</div>
            </>
          )}
        </Details>
      );
    }
  }
}

export function stripTags(html?: string): string {
  return (html ?? '').replace(/<[^>]*>/g, '').replace(/&lt;/g, '<').replace(/&gt;/g, '>').replace(/&quot;/g, '"').replace(/&#39;/g, "'").replace(/&amp;/g, '&');
}

/** A row re-renders only when its content (stamp) changes. */
export const ChatRow = memo(ChatRowImpl, (a, b) => a.row.stamp === b.row.stamp && a.row.kind === b.row.kind);
