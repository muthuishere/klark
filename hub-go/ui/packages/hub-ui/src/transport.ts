import { connectSSE, type SseState } from './sse';
import type { ChatPayload, MachineInfo, Offer, Tree, FeedInfo } from './types';

/** What a live pane/agent stream delivers. Frames come only when `frames` was asked for. */
export interface EventHandlers {
  chat?: (p: ChatPayload) => void;
  frame?: (f: { data: string; reset?: boolean }) => void;
  size?: (s: { cols: number; rows: number }) => void;
  offer?: (o: Offer) => void;
  state?: (s: SseState) => void;
  /** The stream reconnected: refetch what may have been missed. */
  resync?: () => void;
}

/**
 * How the components reach a hub. `m` is a machine id, `a` an agent (pane) id. Implement it over any protocol;
 * httpTransport speaks klark hub's HTTP + SSE contract.
 */
export interface Transport {
  machines(): Promise<MachineInfo[]>;
  tree(m: string): Promise<Tree>;
  getChat(m: string, a: string): Promise<ChatPayload>;
  /** Opens the live stream; returns close. */
  events(m: string, a: string, opts: { frames?: boolean }, h: EventHandlers): () => void;
  /** Types text and presses enter. */
  send(m: string, a: string, text: string): Promise<void>;
  input(m: string, a: string, text: string): Promise<void>;
  /** Key names: enter, esc, up, down, ctrl+c ... */
  keys(m: string, a: string, keys: string[]): Promise<void>;
  /** Stores a file on the machine; the agent gets `path`. */
  upload(m: string, a: string, file: File): Promise<{ path: string; name?: string }>;
  /** The link for a file path an agent mentioned (line 0/undefined = none). */
  fileUrl(m: string, a: string, path: string, line?: number): string;
  /** The machine's feeds, read-only. Optional: a transport without feeds shows none. */
  feeds?(m: string): Promise<FeedInfo[]>;
}

/** An HTTP error: status 0 when the hub did not answer. */
export class TransportError extends Error {
  constructor(public status: number, message: string) {
    super(message);
  }
}

export interface HttpTransportOptions {
  fetch?: typeof fetch;
  EventSource?: typeof EventSource;
  /** Called with every response status (e.g. send the user to sign in on 401). */
  onStatus?: (status: number) => void;
}

const enc = encodeURIComponent;
const parse = <T,>(s: string): T | null => { try { return JSON.parse(s) as T; } catch { return null; } };

/**
 * klark hub's contract: `{base}api/machines`, `{base}api/machines/{m}/tree`, `{base}api/machines/{m}/panes/{a}/
 * chat|events|input|keys|upload`, with the cookie. `basePath` is where the hub is mounted ("/" or "/klark/");
 * fileUrl is the hub's viewer route `{base}m/{m}/f?path=&pane=#L{line}`.
 */
export function httpTransport(basePath = '/', o: HttpTransportOptions = {}): Transport {
  const base = basePath.endsWith('/') ? basePath : basePath + '/';
  const pane = (m: string, a: string) => `${base}api/machines/${enc(m)}/panes/${enc(a)}`;
  async function req<T>(path: string, init: RequestInit & { json?: unknown } = {}): Promise<T> {
    const { json, ...rest } = init;
    const h = new Headers(rest.headers);
    h.set('Accept', 'application/json');
    let body = rest.body;
    if (json !== undefined) { h.set('Content-Type', 'application/json'); body = JSON.stringify(json); }
    let res: Response;
    try {
      res = await (o.fetch ?? fetch)(path, { credentials: 'same-origin', ...rest, headers: h, body });
    } catch {
      throw new TransportError(0, 'The hub did not answer.');
    }
    o.onStatus?.(res.status);
    if (res.status === 204) return undefined as T;
    const ct = res.headers.get('content-type') ?? '';
    const data = ct.includes('json') ? await res.json().catch(() => ({})) : await res.text().catch(() => '');
    if (!res.ok) {
      const msg = typeof data === 'object' && data && 'error' in data ? String((data as { error: unknown }).error) : `Error ${res.status}`;
      throw new TransportError(res.status, msg);
    }
    return data as T;
  }
  const t: Transport = {
    machines: () => req<MachineInfo[]>(`${base}api/machines`),
    tree: (m) => req<Tree>(`${base}api/machines/${enc(m)}/tree`),
    feeds: (m) => req<{ feeds: FeedInfo[] }>(`${base}api/machines/${enc(m)}/feeds`).then((r) => r.feeds ?? []),
    getChat: (m, a) => req<ChatPayload>(`${pane(m, a)}/chat`),
    events: (m, a, { frames }, h) => connectSSE(`${pane(m, a)}/events${frames ? '?frames=1' : ''}`, {
      events: ['chat', 'frame', 'size', 'offer'],
      EventSourceCtor: o.EventSource,
      onState: h.state,
      onResync: h.resync,
      onEvent: (name, data) => {
        if (name === 'chat') {
          const p = parse<ChatPayload>(data);
          if (p) h.chat?.(Array.isArray(p) ? { rows: p } : 'rows' in p ? p : { rows: [p as never] });
        } else if (name === 'frame') {
          const f = parse<{ data: string; reset?: boolean }>(data);
          h.frame?.(f && typeof f === 'object' ? f : { data });
        } else if (name === 'size') {
          const s = parse<{ cols: number; rows: number }>(data);
          if (s) h.size?.(s);
        } else if (name === 'offer') {
          const of = parse<Offer>(data);
          if (of) h.offer?.(of);
        }
      },
    }),
    input: (m, a, text) => req<void>(`${pane(m, a)}/input`, { method: 'POST', json: { text } }),
    keys: (m, a, keys) => req<void>(`${pane(m, a)}/keys`, { method: 'POST', json: { keys } }),
    send: async (m, a, text) => { await t.input(m, a, text); await t.keys(m, a, ['enter']); },
    upload: (m, a, file) => {
      const fd = new FormData();
      fd.append('file', file, file.name);
      return req<{ path: string; name?: string }>(`${pane(m, a)}/upload`, { method: 'POST', body: fd });
    },
    fileUrl: (m, a, path, line) => `${base}m/${enc(m)}/f?path=${enc(path)}&pane=${enc(a)}${line ? '#L' + line : ''}`,
  };
  return t;
}
