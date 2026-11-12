import { useCallback, useEffect, useState } from 'react';
import { api } from './lib/api';
import type { MachineTree } from '@muthuishere/klark-hub-ui';
import { transport } from './lib/transport';

/** Poll a JSON endpoint; keeps the last good value while a refresh is in flight or failing. */
export function usePoll<T>(path: string | null, every = 5000): { data: T | undefined; error: string | null; reload: () => void } {
  const [data, setData] = useState<T>();
  const [error, setError] = useState<string | null>(null);
  const [n, setN] = useState(0);
  useEffect(() => {
    if (!path) return;
    let live = true;
    const run = () =>
      api<T>(path)
        .then((d) => { if (live) { setData(d); setError(null); } })
        .catch((e) => { if (live) setError(e.message); });
    run();
    const t = every > 0 ? setInterval(run, every) : undefined;
    return () => { live = false; if (t) clearInterval(t); };
  }, [path, every, n]);
  return { data, error, reload: useCallback(() => setN((x) => x + 1), []) };
}

export type { MachineTree };

/** Machines and their session trees, for the sidebar and the home page. */
export function useMachineTrees(every = 4000): { items: MachineTree[]; loaded: boolean; error: string | null } {
  const [items, setItems] = useState<MachineTree[]>([]);
  const [loaded, setLoaded] = useState(false);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    let live = true;
    const run = async () => {
      try {
        const ms = await transport.machines();
        const out = await Promise.all(ms.map(async (machine) => {
          try { return { machine, tree: await transport.tree(machine.id) }; } catch { return { machine } as MachineTree; }
        }));
        if (live) { setItems(out); setLoaded(true); setError(null); }
      } catch (e) { if (live) setError((e as Error).message); }
    };
    run();
    const t = setInterval(run, every);
    return () => { live = false; clearInterval(t); };
  }, [every]);
  return { items, loaded, error };
}
