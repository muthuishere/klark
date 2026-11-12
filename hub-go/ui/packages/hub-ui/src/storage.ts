/** Where components keep small UI state (folded groups, drafts). Synchronous; failures are swallowed. */
export interface Storage {
  get(key: string): string | null;
  /** null or '' removes the key. */
  set(key: string, value: string | null): void;
}

/** A Storage over a Web Storage area (localStorage by default), with a key prefix. Private mode just forgets. */
export function webStorage(area?: () => globalThis.Storage, prefix = ''): Storage {
  const a = area ?? (() => localStorage);
  return {
    get(key) { try { return a().getItem(prefix + key); } catch { return null; } },
    set(key, value) {
      try { if (value === null || value === '') a().removeItem(prefix + key); else a().setItem(prefix + key, value); } catch { /* unavailable */ }
    },
  };
}

/** In-memory Storage (tests, SSR). */
export function memoryStorage(): Storage {
  const m = new Map<string, string>();
  return { get: (k) => m.get(k) ?? null, set: (k, v) => { if (v === null || v === '') m.delete(k); else m.set(k, v); } };
}

/** localStorage, no prefix. */
export const defaultStorage: Storage = webStorage();
