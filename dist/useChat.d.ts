import { type RefObject } from 'react';
import { type TerminalHandle } from './components/Terminal';
import type { SseState } from './sse';
import type { Transport } from './transport';
import type { Blocked, ChatItem, Offer } from './types';
export interface ChatData {
    items: ChatItem[];
    offers: Offer[];
    loading: boolean;
    error: string | null;
    live: SseState;
    size: {
        cols: number;
        rows: number;
    };
    blocked: Blocked | null;
    reload: () => Promise<void>;
}
/**
 * One agent's chat: an initial getChat, then `chat` events (rows upserted by id; `replace` = the whole chat). A
 * reconnect refetches what was missed. With `frames`, terminal output goes to `term`.
 */
export declare function useChat(transport: Transport, machine: string, agent: string, frames?: boolean, term?: RefObject<TerminalHandle | null>): ChatData;
