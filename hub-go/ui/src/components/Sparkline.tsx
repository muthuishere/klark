import { useId } from 'react';

export interface SparkPoint { t: number; v: number }

/** Tiny theme-aware line chart. The peak is marked with a ring; the text beside it carries the numbers. */
export function Sparkline({ label, points, format, peak }: { label: string; points: SparkPoint[]; format: (n: number) => string; peak?: number }) {
  const id = useId();
  const W = 240, H = 48, P = 4;
  const vals = points.map((p) => p.v);
  const last = vals.length ? vals[vals.length - 1] : undefined;
  const top = Math.max(1e-9, ...vals, peak ?? 0);
  const t0 = points.length ? points[0].t : 0, t1 = points.length ? points[points.length - 1].t : 1;
  const x = (t: number) => P + (t1 === t0 ? (W - 2 * P) / 2 : ((t - t0) / (t1 - t0)) * (W - 2 * P));
  const y = (v: number) => H - P - (v / top) * (H - 2 * P);
  const d = points.map((p, i) => `${i ? 'L' : 'M'}${x(p.t).toFixed(1)},${y(p.v).toFixed(1)}`).join(' ');
  let pk = -1;
  vals.forEach((v, i) => { if (pk < 0 || v >= vals[pk]) pk = i; });
  return (
    <div className="spark">
      <div className="spark-head"><span>{label}</span><span className="muted">now <b>{last == null ? '-' : format(last)}</b> · peak <b>{format(peak ?? (pk >= 0 ? vals[pk] : 0))}</b></span></div>
      <svg viewBox={`0 0 ${W} ${H}`} role="img" aria-labelledby={id} preserveAspectRatio="none">
        <title id={id}>{`${label}: ${last == null ? 'no data' : 'now ' + format(last)}, peak ${format(peak ?? (pk >= 0 ? vals[pk] : 0))}, ${points.length} samples`}</title>
        <line x1={P} x2={W - P} y1={H - P} y2={H - P} stroke="var(--line)" />
        {points.length > 1 && <path d={d} fill="none" stroke="var(--link)" strokeWidth="1.6" vectorEffect="non-scaling-stroke" />}
        {pk >= 0 && <circle data-testid="peak-mark" cx={x(points[pk].t)} cy={y(vals[pk])} r="3" fill="var(--card)" stroke="var(--blocked)" strokeWidth="1.5" />}
      </svg>
    </div>
  );
}
