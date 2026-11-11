import { useState, type FormEvent } from 'react';
import { api } from '../lib/api';
import type { HubMessage } from '../lib/types';
import { usePoll } from '../hooks';
import { PageBar } from '../shell';

export function Messages() {
  const { data, error, reload } = usePoll<HubMessage[]>('api/messages', 5000);
  const [to, setTo] = useState('');
  const [text, setText] = useState('');
  const [result, setResult] = useState<{ ok: boolean; text: string } | null>(null);
  const [busy, setBusy] = useState(false);

  const send = async (e: FormEvent) => {
    e.preventDefault();
    setBusy(true);
    try {
      await api('api/messages', { method: 'POST', json: { to, text } });
      setResult({ ok: true, text: `Delivered to ${to}.` });
      setText('');
      reload();
    } catch (x) { setResult({ ok: false, text: (x as Error).message }); } finally { setBusy(false); }
  };

  return (
    <main className="pane-app">
      <PageBar title="Messages" />
      <div className="page">
        <h1>Messages</h1>
        <form className="reply" onSubmit={send}>
          <input placeholder="machine/session/agent" aria-label="To" required value={to} onChange={(e) => setTo(e.target.value)} />
          <input placeholder="Message" aria-label="Message" required value={text} onChange={(e) => setText(e.target.value)} />
          <button type="submit" disabled={busy}>Send</button>
        </form>
        {result && <p className={result.ok ? 'muted' : 'error'} role="status">{result.text}</p>}
        {error && <p className="error" role="alert">{error}</p>}
        {data && data.length === 0 && <p className="empty">No messages yet.</p>}
        {data && data.length > 0 && (
          <div className="tbl">
            <table className="messages">
              <thead><tr><th>Time</th><th>From</th><th>To</th><th>Text</th><th>Status</th></tr></thead>
              <tbody>{data.map((m, i) => (
                <tr key={i}><td>{new Date(m.ts).toLocaleString()}</td><td>{m.from}{m.sender && m.sender !== m.from && <span className="muted"> ({m.sender})</span>}</td><td>{m.to}</td><td className="text">{m.text}</td>
                  <td>{m.ok ? <span className="ok">ok</span> : <span className="error">{m.error}</span>}</td></tr>))}</tbody>
            </table>
          </div>
        )}
      </div>
    </main>
  );
}
