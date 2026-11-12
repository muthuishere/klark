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
export declare function backoff(attempt: number, min?: number, max?: number): number;
/** EventSource that reconnects with exponential backoff (the browser's own retry gives up on some errors). `href` is
 * used as given (resolve it against your base first). */
export declare function connectSSE(href: string, o: SseOptions): () => void;
