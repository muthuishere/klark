import { useEffect, useState } from 'react';
import { usePoll } from '../hooks';
import { AdminFrame, Notice } from './Admin';
import { Sparkline } from '../components/Sparkline';
import { fmtBytes, fmtPct, fmtUptime } from '../lib/units';
import type { Analytics, HubAnalytics, MachineAnalytics } from '../lib/types';

const RANGES = ['1h', '6h', '24h', '7d'] as const;

function Tile({ label, value, hint }: { label: string; value: string; hint?: string }) {
  return <div className="tile"><div className="muted">{label}</div><div className="tile-v">{value}</div>{hint && <div className="muted hint">{hint}</div>}</div>;
}

const ago = (ms: number | undefined) => {
  if (!ms) return 'never';
  const s = Math.max(0, Math.round((Date.now() - ms) / 1000));
  return s < 60 ? `${s}s ago` : `${fmtUptime(s)} ago`;
};

/** The hub process itself: memory, Go runtime, CPU, links and streams, storage. */
export function HubCard({ h }: { h: HubAnalytics | null | undefined }) {
  const l = h?.latest, s = h?.series ?? [], pk = h?.peak;
  const series = (f: (p: (typeof s)[number]) => number) => s.map((p) => ({ t: p.t, v: f(p) }));
  return (
    <section className="card wide" aria-label="Hub">
      <h3>Hub {l && <span className="muted">· v{l.version} · {l.os} · {l.goVersion} · up {fmtUptime(l.uptimeSeconds)}</span>}</h3>
      {!l ? <p className="muted">no sample of the hub yet</p> : <>
        <div className="tiles">
          <Tile label="Memory (RSS)" value={l.rssBytes ? fmtBytes(l.rssBytes) : '—'} hint={l.rssBytes ? `peak ${fmtBytes(Math.max(l.peakRssBytes, pk?.rssBytes ?? 0))}` : 'not measured on this OS'} />
          <Tile label="Go heap" value={fmtBytes(l.heapInuseBytes)} hint={`of ${fmtBytes(l.heapSysBytes)} reserved`} />
          <Tile label="CPU" value={fmtPct(l.cpuPercent)} hint={`peak ${fmtPct(pk?.cpuPercent)}`} />
          <Tile label="Goroutines" value={String(l.goroutines)} hint={`peak ${pk?.goroutines ?? l.goroutines}`} />
          <Tile label="GC" value={String(l.gcCount)} hint={`last pause ${l.gcLastPauseMs.toFixed(2)} ms`} />
          <Tile label="Machine links" value={String(l.machineLinks)} />
          <Tile label="Live streams" value={String(l.sseStreams)} hint={`${l.paneViewers} terminal viewers`} />
          <Tile label="Requests / min" value={l.requestsPerMinute.toFixed(1)} hint={`${l.requests} since start`} />
          <Tile label="hub.db" value={fmtBytes(l.dbBytes + l.dbWalBytes)} hint={`wal ${fmtBytes(l.dbWalBytes)}`} />
          <Tile label="Assets" value={fmtBytes(l.assetsBytes)} />
          <Tile label="Last backup" value={ago(l.lastBackupMs)} hint={`analytics write ${ago(l.lastAnalyticsWriteMs)}`} />
        </div>
        <div className="sparks">
          {s.some((p) => p.rssBytes > 0) && <Sparkline label="hub memory" points={series((p) => p.rssBytes)} format={fmtBytes} peak={pk?.rssBytes} />}
          <Sparkline label="hub CPU" points={series((p) => p.cpuPercent)} format={fmtPct} peak={pk?.cpuPercent} />
          <Sparkline label="Go heap" points={series((p) => p.heapBytes)} format={fmtBytes} peak={pk?.heapBytes} />
          <Sparkline label="goroutines" points={series((p) => p.goroutines)} format={(n) => String(Math.round(n))} peak={pk?.goroutines} />
          <Sparkline label="live streams" points={series((p) => p.streams)} format={(n) => String(Math.round(n))} />
        </div>
      </>}
    </section>
  );
}

export function MachineCard({ m }: { m: MachineAnalytics }) {
  const l = m.latest, s = m.series ?? [], pk = m.peak;
  const series = (f: (p: (typeof s)[number]) => number) => s.map((p) => ({ t: p.t, v: f(p) }));
  const agents = [...(l?.agentList ?? [])].sort((a, b) => b.rssBytes - a.rssBytes);
  return (
    <section className="card" aria-label={m.name}>
      <h3><span className={'dot ' + (m.online ? 'on' : 'off')} role="img" aria-label={m.online ? 'online' : 'offline'} /> {m.name}</h3>
      {!m.hasStats || !l ? <p className="muted">no stats from this machine (older klark)</p> : <>
        <p className="muted">
          klark {fmtBytes(l.klarkRssBytes)} (peak {fmtBytes(pk?.klarkRssBytes ?? l.klarkPeakRssBytes)}) · CPU {fmtPct(l.klarkCpuPercent)} (peak {fmtPct(pk?.klarkCpuPercent)}) ·
          host free {fmtBytes(l.hostMemAvailableBytes)} of {fmtBytes(l.hostMemTotalBytes)} · load {l.load1?.toFixed(2)} on {l.cpus} cpus · {l.panes} panes, {l.agents} agents · up {fmtUptime(l.uptimeSeconds)} · v{l.version}
        </p>
        <div className="sparks">
        <Sparkline label="klark memory" points={series((p) => p.klarkRssBytes)} format={fmtBytes} peak={pk?.klarkRssBytes} />
        <Sparkline label="klark CPU" points={series((p) => p.klarkCpuPercent)} format={fmtPct} peak={pk?.klarkCpuPercent} />
        <Sparkline label="host available memory" points={series((p) => p.hostMemAvailableBytes)} format={fmtBytes} />
        </div>
        {agents.length > 0 && (
          <table className="agents">
            <caption className="muted">Agents by memory</caption>
            <thead><tr><th>Name</th><th>State</th><th>Memory</th><th>CPU</th></tr></thead>
            <tbody>{agents.map((a) => <tr key={a.paneId}><td>{a.name}</td><td>{a.state}</td><td>{fmtBytes(a.rssBytes)}</td><td>{fmtPct(a.cpuPercent)}</td></tr>)}</tbody>
          </table>
        )}
      </>}
    </section>
  );
}

export function AdminAnalytics() {
  const [range, setRange] = useState<(typeof RANGES)[number]>('24h');
  const { data, error, reload } = usePoll<Analytics>(`api/admin/analytics?range=${range}`, 0);
  useEffect(() => {
    const t = setInterval(() => { if (document.visibilityState !== 'hidden') reload(); }, 30000);
    return () => clearInterval(t);
  }, [reload]);
  const t = data?.totals;
  const reporting = (data?.machines ?? []).some((m) => m.online && m.latest);
  return (
    <AdminFrame title="Analytics">
      <Notice ok={null} err={error} />
      <div role="group" aria-label="Range" className="subnav">
        {RANGES.map((r) => <button key={r} type="button" className={r === range ? 'sel' : ''} aria-pressed={r === range} onClick={() => setRange(r)}>{r}</button>)}
      </div>
      {data && <HubCard h={data.hub} />}
      {t && <div className="tiles">
        <Tile label="Machines online" value={`${t.online} / ${t.machines}`} />
        <Tile label="Panes" value={String(t.panes)} />
        <Tile label="Agents" value={String(t.agents)} />
        <Tile label="Klark memory" value={reporting ? fmtBytes(t.klarkRssBytes) : '—'} hint={reporting ? 'online machines' : 'no machine reports stats'} />
      </div>}
      <div className="cards">
        {(data?.machines ?? []).map((m) => <MachineCard key={m.id} m={m} />)}
      </div>
      {data && data.machines.length === 0 && <p className="muted">No machines yet.</p>}
    </AdminFrame>
  );
}
