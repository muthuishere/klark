export const THEMES = [
  { id: 'paper', label: 'Paper', color: '#2563EB' },
  { id: 'teal', label: 'Teal', color: '#2DD4BF' },
  { id: 'indigo', label: 'Indigo', color: '#8B9CFF' },
  { id: 'forest', label: 'Forest', color: '#7FD8A6' },
  { id: 'dusk', label: 'Dusk', color: '#F2A65A' },
  { id: 'light', label: 'Term', color: '#1a7f37' },
  { id: 'dark', label: 'Term dark', color: '#3fb950' },
] as const;

export function readStore(key: string): string | null {
  try { return localStorage.getItem('klark:' + key); } catch { return null; }
}
export function writeStore(key: string, value: string | null): void {
  try { if (value === null || value === '') localStorage.removeItem('klark:' + key); else localStorage.setItem('klark:' + key, value); } catch { /* private mode */ }
}

export function currentTheme(): string {
  const t = readStore('theme');
  return THEMES.some((x) => x.id === t) ? (t as string) : 'paper';
}
export function setTheme(id: string): void {
  document.documentElement.dataset.theme = id;
  writeStore('theme', id);
}
export function nextTheme(): string {
  const i = THEMES.findIndex((t) => t.id === currentTheme());
  const n = THEMES[(i + 1) % THEMES.length].id;
  setTheme(n);
  return n;
}

/** The sidebar button: a drawer on phones, collapse/expand on desktop (remembered). */
export function toggleSide(): void {
  const h = document.documentElement;
  const narrow = typeof matchMedia === 'function' && matchMedia('(max-width: 860px)').matches;
  if (narrow) { h.classList.toggle('drawer-open'); return; }
  const closed = h.classList.toggle('side-closed');
  writeStore('side', closed ? 'closed' : null);
}
