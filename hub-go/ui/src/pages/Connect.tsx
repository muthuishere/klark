import { useEffect, useState } from 'react';
import { Link, useSearchParams } from 'react-router-dom';
import { api, ApiError } from '../lib/api';
import type { ConnectRequest } from '../lib/types';

/** `klark hub connect` (ADR 0012): a machine shows a code and this page's URL; a signed-in person approves it. */
export function Connect() {
  const [params] = useSearchParams();
  const code = params.get('code') ?? '';
  const [req, setReq] = useState<ConnectRequest | null>(null);
  const [missing, setMissing] = useState<string | null>(null);
  const [done, setDone] = useState<string | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    let live = true;
    setReq(null); setMissing(null); setDone(null); setErr(null);
    if (!code) { setMissing('No code given. Open the link that klark hub connect printed.'); return; }
    api<ConnectRequest>(`api/connect/requests/${encodeURIComponent(code)}`)
      .then((r) => live && setReq(r))
      .catch((e) => live && setMissing(e instanceof ApiError && e.status === 404 ? 'This code is unknown, already used, or has expired. Run klark hub connect again.' : (e as Error).message));
    return () => { live = false; };
  }, [code]);

  const decide = async (approve: boolean) => {
    if (!req) return;
    setBusy(true); setErr(null);
    try {
      await api(`api/connect/requests/${encodeURIComponent(req.code)}/${approve ? 'approve' : 'deny'}`, { method: 'POST' });
      setDone(approve ? `Approved. ${req.machine} is connecting now.` : `Denied. ${req.machine} was not connected.`);
    } catch (e) { setErr((e as Error).message); } finally { setBusy(false); }
  };

  return (
    <main className="login connect">
      <h1>Connect a machine</h1>
      {done && <p className="ok" role="status">{done}</p>}
      {err && <p className="error" role="alert">{err}</p>}
      {missing && <p className="muted" role="alert">{missing}</p>}
      {!req && !missing && <p className="muted" role="status">Loading…</p>}
      {req && !done && req.status === 'pending' && (
        <>
          <p>Machine <b>{req.machine}</b> asks to join this hub with code</p>
          <p className="code" aria-label="Code">{req.code}</p>
          {req.existing && <p className="muted">A machine named {req.machine} already exists{req.owner ? ` (approved by ${req.owner})` : ''}; approving gives it a new token and the old one stops working.</p>}
          <p className="error">Only approve if you just ran <code>klark hub connect</code> yourself and it shows this same code. You will be recorded as the one who approved it.</p>
          <div className="stack">
            <button type="button" disabled={busy} onClick={() => void decide(true)}>Approve</button>
            <button type="button" className="danger" disabled={busy} onClick={() => void decide(false)}>Deny</button>
          </div>
        </>
      )}
      {req && !done && req.status !== 'pending' && <p className="muted">This request was already decided.</p>}
      <p><Link to="/">Back to the hub</Link></p>
    </main>
  );
}
