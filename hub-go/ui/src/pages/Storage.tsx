import { useEffect, useState, type FormEvent } from 'react';
import { api } from '../lib/api';
import { usePoll } from '../hooks';
import { AdminFrame, Notice } from './Admin';
import type { StorageInfo } from '../lib/types';

type S3 = NonNullable<StorageInfo['s3']>;
const when = (ms?: number) => (ms ? new Date(ms).toLocaleString() : 'never');

export function duckdbExample(f: Pick<StorageInfo, 'kind' | 'path' | 's3'>): string {
  const p = (f.path ?? '').replace(/\/+$/, '');
  const root = f.kind === 's3' ? `s3://${f.s3?.bucket || 'BUCKET'}${p ? '/' + p.replace(/^\/+/, '') : ''}` : p || '/PATH';
  return `SELECT * FROM read_json_auto('${root}/analytics/**/*.ndjson.gz');`;
}

/** One storage target for the SQLite backups and the analytics files (API.md, "Storage"). */
export function AdminStorage() {
  const { data, error, reload } = usePoll<StorageInfo>('api/admin/storage', 0);
  const [f, setF] = useState<StorageInfo>({ kind: 'none' });
  const [secret, setSecret] = useState('');
  const [clear, setClear] = useState(false);
  const [ok, setOk] = useState<string | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [res, setRes] = useState<{ ok: boolean; message: string } | null>(null);
  useEffect(() => { if (data) setF(data); }, [data]);
  const s3: S3 = f.s3 ?? {};
  const setS3 = (p: Partial<S3>) => setF({ ...f, s3: { ...s3, ...p } });
  const body = () => {
    const { secretAccessKeySet: _k, ...rest } = s3;
    return { kind: f.kind, path: f.path ?? '', s3: { ...rest, ...(secret ? { secretAccessKey: secret } : {}) }, ...(clear ? { clearSecretAccessKey: true } : {}) };
  };
  const run = async (p: Promise<unknown>, msg: string) => {
    setOk(null); setErr(null);
    try { await p; setSecret(''); setClear(false); setOk(msg); reload(); } catch (x) { setErr((x as Error).message); }
  };
  const save = (e: FormEvent) => { e.preventDefault(); void run(api('api/admin/storage', { method: 'PUT', json: body() }), 'Saved.'); };
  const reset = () => { if (confirm('Drop the saved storage settings and use the environment again?')) void run(api('api/admin/storage', { method: 'DELETE' }), 'Using the environment settings.'); };
  const test = async () => {
    setRes(null);
    try { setRes(await api<{ ok: boolean; message: string }>('api/admin/storage/test', { method: 'POST', json: body() })); }
    catch (x) { setRes({ ok: false, message: (x as Error).message }); }
  };
  const sq = data?.sqlite;
  return (
    <AdminFrame title="Storage">
      <Notice ok={ok} err={err ?? error} />
      <p className="muted">One place for the SQLite backups (<code>&lt;path&gt;/sqlite/</code>) and the hourly analytics files (<code>&lt;path&gt;/analytics/</code>, gzipped NDJSON).
        {data?.source && <> Settings from the {data.source === 'admin' ? 'admin page' : 'environment'}.</>}</p>
      <form className="stack" onSubmit={save} aria-label="Storage settings">
        <label>Store to
          <select value={f.kind} onChange={(e) => setF({ ...f, kind: e.target.value as StorageInfo['kind'] })}>
            <option value="none">none</option><option value="folder">folder</option><option value="s3">S3 (or compatible)</option>
          </select>
        </label>
        {f.kind !== 'none' && <label>{f.kind === 's3' ? 'Key prefix (parent)' : 'Parent folder'}<input autoComplete="off" value={f.path ?? ''} onChange={(e) => setF({ ...f, path: e.target.value })} /></label>}
        {f.kind === 's3' && <>
          <label>Bucket<input autoComplete="off" value={s3.bucket ?? ''} onChange={(e) => setS3({ bucket: e.target.value })} /></label>
          <label>Region<input autoComplete="off" value={s3.region ?? ''} onChange={(e) => setS3({ region: e.target.value })} /></label>
          <label>Endpoint (optional)<input autoComplete="off" value={s3.endpoint ?? ''} onChange={(e) => setS3({ endpoint: e.target.value })} /></label>
          <label>Access key ID<input autoComplete="off" value={s3.accessKeyId ?? ''} onChange={(e) => setS3({ accessKeyId: e.target.value })} /></label>
          <label>Secret access key {s3.secretAccessKeySet && <span className="muted" data-testid="secret-set">(set; leave empty to keep)</span>}
            <input type="password" autoComplete="new-password" value={secret} placeholder={s3.secretAccessKeySet ? 'unchanged' : ''} onChange={(e) => setSecret(e.target.value)} />
          </label>
          {s3.secretAccessKeySet && <label className="check"><input type="checkbox" checked={clear} onChange={(e) => setClear(e.target.checked)} /> Remove the stored secret</label>}
          <label className="check"><input type="checkbox" checked={!!s3.pathStyle} onChange={(e) => setS3({ pathStyle: e.target.checked })} /> Path-style addressing</label>
        </>}
        <div className="row-actions"><button type="submit">Save</button><button type="button" onClick={() => void test()}>Test</button>
          {data?.source === 'admin' && <button type="button" className="danger" onClick={reset}>Use environment</button>}</div>
      </form>
      {res && <p className={res.ok ? 'ok' : 'error'} role="status">{res.ok ? 'Test passed: ' : 'Test failed: '}{res.message}</p>}

      <h2>Status</h2>
      <ul className="rows" aria-label="Storage status">
        <li className="row"><span>SQLite backups</span><span className="muted">{sq?.enabled ? `every ${sq.every ?? '?'}, keep ${sq.keep ?? '?'}${sq.backups != null ? ` · ${sq.backups} since start` : ''}${sq.owner ? ` · owner ${sq.owner}` : ''}` : 'off (needs a target and sqlite mode)'}</span></li>
        <li className="row"><span>Last SQLite backup</span><span className="muted" data-testid="last-sqlite">{when(data?.lastSqliteBackupMs)}</span></li>
        <li className="row"><span>Last analytics write</span><span className="muted" data-testid="last-analytics">{when(data?.lastAnalyticsWriteMs)} · {data?.filesWritten ?? 0} files since start</span></li>
      </ul>
      <p className="muted">Backup interval and keep count come from <code>HUB_SQLITE_BACKUP_EVERY</code> and <code>HUB_SQLITE_BACKUP_KEEP</code>.</p>
      {data?.lastError && <p className="error" role="alert">Last error: {data.lastError}</p>}
      <details><summary>Query it with DuckDB</summary><pre className="code"><code>{duckdbExample(f)}</code></pre></details>
    </AdminFrame>
  );
}
