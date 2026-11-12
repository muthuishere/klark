/** Where components keep small UI state (folded groups, drafts). Synchronous; failures are swallowed. */
export interface Storage {
    get(key: string): string | null;
    /** null or '' removes the key. */
    set(key: string, value: string | null): void;
}
/** A Storage over a Web Storage area (localStorage by default), with a key prefix. Private mode just forgets. */
export declare function webStorage(area?: () => globalThis.Storage, prefix?: string): Storage;
/** In-memory Storage (tests, SSR). */
export declare function memoryStorage(): Storage;
/** localStorage, no prefix. */
export declare const defaultStorage: Storage;
