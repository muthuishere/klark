import { forwardRef, useEffect, useImperativeHandle, useRef } from 'react';

export interface TerminalHandle { write: (b: Uint8Array) => void; reset: (b: Uint8Array) => void; resize: (cols: number, rows: number) => void }
const MIN_SCALE = 0.55;

export function decodeBase64(b64: string): Uint8Array {
  const bin = atob(b64);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

/**
 * The pane's own geometry (never fit() to the window), scaled down with CSS to the container width and
 * scrolling sideways below a minimum scale, so TUIs keep their exact layout. xterm loads only when first shown.
 */
export const TerminalView = forwardRef<TerminalHandle, { cols: number; rows: number; visible: boolean }>(function TerminalView({ cols, rows, visible }, ref) {
  const wrap = useRef<HTMLDivElement>(null);
  const sizer = useRef<HTMLDivElement>(null);
  const inner = useRef<HTMLDivElement>(null);
  const term = useRef<import('@xterm/xterm').Terminal | null>(null);
  const queue = useRef<Array<() => void>>([]);
  const geom = useRef({ cols, rows });
  geom.current = { cols, rows };

  const scale = () => {
    const screen = inner.current?.querySelector<HTMLElement>('.xterm-screen');
    const w = wrap.current, sz = sizer.current, inn = inner.current;
    if (!screen || !w || !sz || !inn || !w.clientWidth) return;
    const sw = screen.offsetWidth, sh = screen.offsetHeight;
    if (!sw || !sh) return;
    const s = Math.max(MIN_SCALE, Math.min(1, (w.clientWidth - 2) / sw));
    inn.style.transform = `scale(${s})`;
    sz.style.width = `${Math.ceil(sw * s)}px`;
    sz.style.height = `${Math.ceil(sh * s)}px`;
  };

  const run = (f: () => void) => { if (term.current) f(); else queue.current.push(f); };
  useImperativeHandle(ref, () => ({
    write: (b) => run(() => term.current?.write(b)),
    reset: (b) => run(() => { term.current?.reset(); term.current?.write(b); }),
    resize: (c, r) => run(() => { if (term.current && (term.current.cols !== c || term.current.rows !== r)) { term.current.resize(c, r); requestAnimationFrame(scale); } }),
  }));

  useEffect(() => {
    if (!visible || term.current) return;
    let dead = false;
    (async () => {
      const [{ Terminal }, { Unicode11Addon }] = await Promise.all([import('@xterm/xterm'), import('@xterm/addon-unicode11'), import('@xterm/xterm/css/xterm.css')]);
      if (dead || !inner.current) return;
      const t = new Terminal({
        cols: geom.current.cols, rows: geom.current.rows, convertEol: false, disableStdin: true, allowProposedApi: true,
        fontFamily: 'ui-monospace, SFMono-Regular, Menlo, Consolas, monospace', fontSize: 13, scrollback: 5000,
        theme: { background: '#0d1117', foreground: '#e6edf3', cursor: '#e6edf3', selectionBackground: '#264f78' },
      });
      t.loadAddon(new Unicode11Addon());
      t.unicode.activeVersion = '11';
      t.open(inner.current);
      term.current = t;
      queue.current.splice(0).forEach((f) => f());
      requestAnimationFrame(scale);
    })();
    return () => { dead = true; };
  }, [visible]);

  useEffect(() => {
    const w = wrap.current;
    if (!w || typeof ResizeObserver === 'undefined') return;
    const ro = new ResizeObserver(scale);
    ro.observe(w);
    return () => ro.disconnect();
  }, []);
  useEffect(() => () => { term.current?.dispose(); term.current = null; }, []);
  useEffect(() => { if (visible) requestAnimationFrame(scale); }, [visible]);

  return (
    <div className="terminal-view" style={{ display: visible ? undefined : 'none' }} aria-label="Terminal">
      <div className="term-wrap" ref={wrap}><div className="term-sizer" ref={sizer}><div className="term-inner" ref={inner} /></div></div>
    </div>
  );
});
