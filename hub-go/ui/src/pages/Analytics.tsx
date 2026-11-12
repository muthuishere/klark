import { useEffect, useState } from 'react';
import { usePoll } from '../hooks';
import { AdminFrame, Notice } from './Admin';
import { Sparkline } from '../components/Sparkline';
import { fmtBytes, fmtPct, fmtUptime } from '../lib/units';
import type { Analytics, MachineAnalytics } from '../lib/types';

const RANGES = ['1h', '6h', '24h', '7d'] as const;

function Tile({ label, value }: { label: string; value: string }) {
  return <div className="tile"><div className="muted">{label}</div><div className="tile-v">{value}</div></div>;
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
        <Sparkline label="klark memory" points={series((p) => p.klarkRssBytes)} format={fmtBytes} peak={pk?.klarkRssBytes} />
        <Sparkline label="klark CPU" points={series((p) => p.klarkCpuPercent)} format={fmtPct} peak={pk?.klarkCpuPercent} />
        <Sparkline label="host available memory" points={series((p) => p.hostMemAvailableBytes)} format={fmtBytes} />
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
  return (
    <AdminFrame title="Analytics">
      <Notice ok={null} err={error} />
      <div role="group" aria-label="Range" className="subnav">
        {RANGES.map((r) => <button key={r} type="button" className={r === range ? 'sel' : ''} aria-pressed={r === range} onClick={() => setRange(r)}>{r}</button>)}
      </div>
      {t && <div className="tiles">
        <Tile label="Machines online" value={`${t.online} / ${t.machines}`} />
        <Tile label="Panes" value={String(t.panes)} />
        <Tile label="Agents" value={String(t.agents)} />
        <Tile label="Klark memory" value={fmtBytes(t.klarkRssBytes)} />
      </div>}
      {data && data.machines.length === 0 && <p className="muted">No machines yet.</p>}
      {(data?.machines ?? []).map((m) => <MachineCard key={m.id} m={m} />)}
    </AdminFrame>
  );
}
