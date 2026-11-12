import { useCallback, useEffect, useState, type RefObject } from 'react';
import { mergeItems } from './chat';
import { decodeBase64, type TerminalHandle } from './components/Terminal';
import type { SseState } from './sse';
import type { Transport } from './transport';
import type { Blocked, ChatItem, ChatPayload, Offer } from './types';

export interface ChatData {
  items: ChatItem[]; offers: Offer[]; loading: boolean; error: string | null; live: SseState;
  size: { cols: number; rows: number }; blocked: Blocked | null; reload: () => Promise<void>;
}

/**
 * One agent's chat: an initial getChat, then `chat` events (rows upserted by id; `replace` = the whole chat). A
 * reconnect refetches what was missed. With `frames`, terminal output goes to `term`.
 */
export function useChat(transport: Transport, machine: string, agent: string, frames = false, term?: RefObject<TerminalHandle | null>): ChatData {
  const [items, setItems] = useState<ChatItem[]>([]);
  const [offers, setOffers] = useState<Offer[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [live, setLive] = useState<SseState>('connecting');
  const [size, setSize] = useState({ cols: 120, rows: 40 });
  const [blocked, setBlocked] = useState<Blocked | null>(null);

  const load = useCallback(() =>
    transport.getChat(machine, agent)
      .then((p: ChatPayload) => {
        setBlocked(p.blocked ?? null);
        setItems((cur) => (cur.length ? mergeItems(cur, p.rows ?? []) : p.rows ?? []));
        setOffers(p.offers ?? []);
        setError(null);
      })
      .catch((e: Error) => setError(e.message))
      .finally(() => setLoading(false)), [transport, machine, agent]);

  useEffect(() => { setItems([]); setOffers([]); setBlocked(null); setLoading(true); void load(); }, [load]);
  useEffect(() => transport.events(machine, agent, { frames }, {
    state: setLive,
    resync: () => void load(),
    chat: (p) => {
      setBlocked(p.blocked ?? null);
      // Upsert by id. A replace keeps rows the event lacks (the klark app's behaviour; a later reload drops them).
      setItems((cur) => mergeItems(cur, p.rows ?? []));
    },
    frame: (f) => { const b = decodeBase64(f.data); if (f.reset) term?.current?.reset(b); else term?.current?.write(b); },
    size: (s) => { setSize(s); term?.current?.resize(s.cols, s.rows); },
    offer: (o) => setOffers((cur) => (cur.some((x) => x.id === o.id) ? cur : [...cur, o])),
  }), [transport, machine, agent, frames, load, term]);
  return { items, offers, loading, error, live, size, blocked, reload: load };
}
