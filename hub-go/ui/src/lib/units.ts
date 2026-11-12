/** Human byte size: 12.3 MiB, 1.5 GiB. */
export function fmtBytes(n: number | undefined | null): string {
  if (n == null || !isFinite(n)) return '-';
  const u = ['B', 'KiB', 'MiB', 'GiB', 'TiB'];
  let i = 0; let v = n;
  while (Math.abs(v) >= 1024 && i < u.length - 1) { v /= 1024; i++; }
  return `${i === 0 ? v.toFixed(0) : v.toFixed(v >= 100 ? 0 : 1)} ${u[i]}`;
}
export const fmtPct = (n: number | undefined | null) => (n == null || !isFinite(n) ? '-' : `${n.toFixed(n >= 100 ? 0 : 1)}%`);
export function fmtUptime(s: number | undefined): string {
  if (s == null) return '-';
  const d = Math.floor(s / 86400), h = Math.floor((s % 86400) / 3600), m = Math.floor((s % 3600) / 60);
  return d ? `${d}d ${h}h` : h ? `${h}h ${m}m` : `${m}m`;
}
