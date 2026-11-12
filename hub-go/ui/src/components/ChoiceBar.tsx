import { useState } from 'react';
import type { Blocked } from '../lib/types';

/** Keys that pick `key` in an arrow-selected menu: from the highlighted option, up/down then enter. Without a
 * known highlight, the digit itself (claude, codex and agy menus all take it). */
export function choiceKeys(b: Blocked, key: string): string[] {
  const keys = (b.choices ?? []).map((c) => c.key);
  const to = keys.indexOf(key);
  const from = b.selected ? keys.indexOf(b.selected) : -1;
  if (to < 0) return [];
  if (from < 0) return [key];
  const d = to - from;
  return [...Array(Math.abs(d)).fill(d > 0 ? 'down' : 'up'), 'enter'];
}

/** The agent's choice menu as buttons under the last message. */
export function ChoiceBar({ blocked, onKeys }: { blocked: Blocked; onKeys: (keys: string[]) => Promise<unknown> | void }) {
  const [busy, setBusy] = useState(false);
  const choices = blocked.choices ?? [];
  if (!choices.length) return null;
  const pick = async (keys: string[]) => {
    if (!keys.length || busy) return;
    setBusy(true);
    try { await onKeys(keys); } finally { setBusy(false); }
  };
  return (
    <div className="blocked chat-col" role="group" aria-label="Agent is waiting for your answer">
      <div className="blocked-label">Waiting for you</div>
      {blocked.question && <pre className="blocked-q">{blocked.question}</pre>}
      <div className="quick">
        {choices.map((c) => (
          <button type="button" key={c.key} disabled={busy} className={c.key === blocked.selected ? 'chosen' : undefined}
            onClick={() => void pick(choiceKeys(blocked, c.key))}>{c.key}. {c.label}</button>
        ))}
        <button type="button" className="ghost" disabled={busy} onClick={() => void pick(['esc'])}>Esc</button>
      </div>
    </div>
  );
}
