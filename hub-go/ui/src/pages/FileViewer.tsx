import { useEffect, useMemo, useState } from 'react';
import { Link, useNavigate, useParams, useSearchParams } from 'react-router-dom';
import { ApiError, api } from '../lib/api';
import { url } from '../lib/base';
import { filesApi, imageExt } from '../lib/files';
import type { FileMeta } from '../lib/types';
import { IconBack } from '../components/Icons';

type Json = null | boolean | number | string | Json[] | { [k: string]: Json };

export function parseDelimited(text: string, sep: string): string[][] {
  const rows: string[][] = [];
  let row: string[] = [], cell = '', quoted = false;
  for (let i = 0; i < text.length; i++) {
    const c = text[i];
    if (quoted) {
      if (c === '"' && text[i + 1] === '"') { cell += '"'; i++; } else if (c === '"') quoted = false; else cell += c;
    } else if (c === '"') quoted = true;
    else if (c === sep) { row.push(cell); cell = ''; }
    else if (c === '\n') { row.push(cell); rows.push(row); row = []; cell = ''; }
    else if (c !== '\r') cell += c;
  }
  if (cell || row.length) { row.push(cell); rows.push(row); }
  return rows;
}

function JsonNode({ k, v, depth }: { k?: string; v: Json; depth: number }) {
  const label = k !== undefined ? <span className="jk">{k}: </span> : null;
  if (v === null || typeof v !== 'object') return <div className="jl">{label}<span className={'jv j-' + typeof v}>{JSON.stringify(v)}</span></div>;
  const entries = Array.isArray(v) ? v.map((x, i) => [String(i), x] as const) : Object.entries(v);
  return (
    <details open={depth < 2} className="jl">
      <summary>{label}<span className="muted">{Array.isArray(v) ? `[${entries.length}]` : `{${entries.length}}`}</span></summary>
      <div className="jin">{entries.map(([kk, vv]) => <JsonNode key={kk} k={kk} v={vv as Json} depth={depth + 1} />)}</div>
    </details>
  );
}

export function FileViewer() {
  const { machine = '' } = useParams();
  const [q] = useSearchParams();
  const path = q.get('path') ?? '';
  const pane = q.get('pane') ?? '';
  const nav = useNavigate();
  const [meta, setMeta] = useState<FileMeta | null>(null);
  const [text, setText] = useState('');
  const [html, setHtml] = useState<string | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [tab, setTab] = useState(0);
  const [wrap, setWrap] = useState(false);
  const [find, setFind] = useState('');

  useEffect(() => {
    let live = true;
    setMeta(null); setErr(null); setText(''); setHtml(null); setTab(0);
    (async () => {
      try {
        const m = await api<FileMeta>(filesApi(machine, '', path, pane));
        if (!live) return;
        setMeta(m);
        if (['binary', 'image', 'pdf'].includes(m.view)) return;
        const raw = await api<string>(m.raw ? m.raw : filesApi(machine, '/raw', path, pane));
        if (!live) return;
        setText(typeof raw === 'string' ? raw : JSON.stringify(raw, null, 2));
        if (m.view === 'markdown') {
          const h = await api<string | { html: string }>(filesApi(machine, '/markdown', path, pane)).catch(() => null);
          if (live && h) setHtml(typeof h === 'string' ? h : h.html);
        }
      } catch (e) { if (live) setErr(e instanceof ApiError ? e.message : 'The hub did not answer.'); }
    })();
    return () => { live = false; };
  }, [machine, path, pane]);

  const lines = useMemo(() => { const l = text.split('\n'); if (l.length > 1 && l[l.length - 1] === '') l.pop(); return l; }, [text]);
  const rawUrl = url(meta?.raw ?? filesApi(machine, '/raw', path, pane));
  const parts = path.split('/').filter(Boolean);
  const tabs = meta ? ({ markdown: ['Preview', 'Code'], csv: ['Table', 'Code'], tsv: ['Table', 'Code'], json: ['Tree', 'Code'] } as Record<string, string[]>)[meta.view] : undefined;
  const special = !!tabs && tab === 0;
  const hits = find ? lines.reduce((n, l) => n + (l.toLowerCase().includes(find.toLowerCase()) ? 1 : 0), 0) : 0;
  const back = `/m/${encodeURIComponent(machine)}/p/${encodeURIComponent(pane)}`;
  const parsedJson = useMemo<Json | undefined>(() => { if (meta?.view !== 'json') return undefined; try { return JSON.parse(text) as Json; } catch { return undefined; } }, [meta, text]);

  return (
    <main className="viewer">
      <header className="vbar">
        <Link className="icon" to={back} aria-label="Back to chat" onClick={(e) => { if (window.history.length > 1 && document.referrer) { e.preventDefault(); nav(-1); } }}><IconBack /></Link>
        <nav className="crumbs" aria-label="Path">{parts.map((s, i) => i === parts.length - 1 ? <b key={i}>{s}</b> : <span key={i}>{s} / </span>)}</nav>
      </header>
      {err && <div className="vmsg bad" role="alert">{err}</div>}
      {!meta && !err && <div className="vmsg" role="status">Loading…</div>}
      {meta && (
        <>
          <div className="vhead">
            <div className="meta">{[meta.size_label, meta.lines ? `${meta.lines.toLocaleString()} lines` : '', meta.lang].filter(Boolean).map((p) => <span key={p}>{p}</span>)}</div>
            {tabs && <div className="tabs" role="tablist">{tabs.map((t, i) => <button key={t} role="tab" aria-selected={tab === i} className={tab === i ? 'on' : ''} onClick={() => setTab(i)}>{t}</button>)}</div>}
            <div className="acts">
              {!special && !['binary', 'image', 'pdf'].includes(meta.view) && <><input type="search" aria-label="Find in file" placeholder="Find in file" value={find} onChange={(e) => setFind(e.target.value)} />{find && <span className="muted">{hits} lines</span>}
                <label className="chk"><input type="checkbox" checked={wrap} onChange={(e) => setWrap(e.target.checked)} /> Wrap</label></>}
              <a className="btn" href={rawUrl} target="_blank" rel="noopener">Raw</a>
              <button className="btn" type="button" onClick={() => void navigator.clipboard?.writeText(text)}>Copy</button>
              <a className="btn primary" href={meta.download ? url(meta.download) : rawUrl} download={meta.name}>Download</a>
            </div>
          </div>
          <div className="vbody">
            {meta.view === 'binary' && <div className="vmsg">Binary file, {meta.size_label}.</div>}
            {(meta.view === 'image' || (imageExt.test(meta.name) && meta.view !== 'code')) && <div className="img"><img alt={meta.name} src={rawUrl} /></div>}
            {meta.view === 'pdf' && <iframe className="pdf" title={meta.name} src={rawUrl} />}
            {special && meta.view === 'markdown' && (html != null ? <div className="md pad" dangerouslySetInnerHTML={{ __html: html }} /> : <pre className="vcode">{text}</pre>)}
            {special && (meta.view === 'csv' || meta.view === 'tsv') && (() => {
              const r = parseDelimited(text, meta.view === 'csv' ? ',' : '\t');
              return <div className="tbl"><table><thead><tr>{(r[0] ?? []).map((c, i) => <th key={i}>{c}</th>)}</tr></thead><tbody>{r.slice(1).map((row, i) => <tr key={i}>{row.map((c, j) => <td key={j}>{c}</td>)}</tr>)}</tbody></table></div>;
            })()}
            {special && meta.view === 'json' && (parsedJson !== undefined ? <div className="jtree pad"><JsonNode v={parsedJson} depth={0} /></div> : <pre className="vcode">{text}</pre>)}
            {!special && !['binary', 'image', 'pdf'].includes(meta.view) && (
              <div className={'vcode' + (wrap ? ' wrap' : '')} role="region" aria-label="File contents">
                {lines.map((l, i) => {
                  const hit = find && l.toLowerCase().includes(find.toLowerCase());
                  return <div className={'ln' + (hit ? ' hit' : '')} key={i} id={'L' + (i + 1)}><a className="no" href={'#L' + (i + 1)} aria-label={'Line ' + (i + 1)}>{i + 1}</a><span className="tx">{l || ' '}</span></div>;
                })}
              </div>
            )}
          </div>
        </>
      )}
    </main>
  );
}
