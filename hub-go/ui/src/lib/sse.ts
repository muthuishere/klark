import { url } from './base';

export type SseState = 'connecting' | 'open' | 'retrying' | 'closed';
export interface SseOptions {
  events: string[];
  onEvent: (name: string, data: string) => void;
  onState?: (s: SseState) => void;
  /** Called on every (re)open after the first, so the caller can refetch what it missed. */
  onResync?: () => void;
  EventSourceCtor?: typeof EventSource;
  minDelay?: number;
  maxDelay?: number;
}

/** backoff(n) = min(max, min * 2^n), with the first retry after `min`. */
export function backoff(attempt: number, min = 1000, max = 30000): number {
  return Math.min(max, min * 2 ** attempt);
}

/** EventSource that reconnects with exponential backoff (the browser's own retry gives up on some errors). */
export function connectSSE(path: string, o: SseOptions): () => void {
  const Ctor = o.EventSourceCtor ?? EventSource;
  let es: EventSource | null = null;
  let timer: ReturnType<typeof setTimeout> | undefined;
  let attempt = 0;
  let opened = 0;
  let stopped = false;

  const open = () => {
    if (stopped) return;
    o.onState?.('connecting');
    es = new Ctor(url(path), { withCredentials: true });
    es.onopen = () => {
      attempt = 0;
      o.onState?.('open');
      if (++opened > 1) o.onResync?.();
    };
    for (const name of o.events) es.addEventListener(name, (e) => o.onEvent(name, (e as MessageEvent).data));
    es.onerror = () => {
      es?.close();
      es = null;
      if (stopped) return;
      o.onState?.('retrying');
      timer = setTimeout(open, backoff(attempt++, o.minDelay, o.maxDelay));
    };
  };
  open();
  return () => {
    stopped = true;
    clearTimeout(timer);
    es?.close();
    o.onState?.('closed');
  };
}
