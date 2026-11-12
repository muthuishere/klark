import { forwardRef, useCallback, useImperativeHandle, useLayoutEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { atBottom, computeWindow, offsets } from '../windowing';

export interface VirtualHandle { scrollToIndex: (i: number) => void; scrollToBottom: () => void }
interface Props<T> {
  items: T[];
  getKey: (t: T) => string;
  render: (t: T, index: number) => ReactNode;
  estimate?: number;
  overscan?: number;
  className?: string;
  onPinnedChange?: (pinned: boolean) => void;
  /** Shown inside the column above the first row. */
  header?: ReactNode;
}

/**
 * Windowed chat list: only rows near the viewport are in the DOM; the rest are two spacers of the exact height
 * they would take (measured once a row has been drawn, estimated before). Stays pinned to the bottom while the
 * reader is there, so new rows follow; scrolling up unpins.
 */
function VirtualListInner<T>({ items, getKey, render, estimate = 90, overscan = 900, className = '', onPinnedChange, header }: Props<T>, ref: React.Ref<VirtualHandle>) {
  const sc = useRef<HTMLDivElement>(null);
  const heights = useRef(new Map<string, number>());
  const nodes = useRef(new Map<string, HTMLElement>());
  const [version, setVersion] = useState(0);
  const [scrollTop, setScrollTop] = useState(0);
  const [viewport, setViewport] = useState(() => (typeof window === 'undefined' ? 800 : window.innerHeight));
  const pinned = useRef(true);
  const ro = useRef<ResizeObserver | null>(null);

  const offs = useMemo(
    () => offsets(items.length, heights.current, (i) => getKey(items[i]), estimate),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [items, version, estimate],
  );
  const total = offs[items.length];
  // While pinned, the window follows the bottom even before the scroll position has caught up.
  const effTop = pinned.current ? Math.max(0, total - viewport) : scrollTop;
  const w = computeWindow(offs, effTop, viewport, overscan);

  const measure = useCallback(() => {
    let changed = false;
    let above = 0;
    const el = sc.current;
    const top = el?.scrollTop ?? 0;
    const o = offs;
    items.forEach((it, i) => {
      const n = nodes.current.get(getKey(it));
      if (!n) return;
      const h = n.offsetHeight;
      if (h <= 0) return;
      const k = getKey(it);
      const prev = heights.current.get(k) ?? estimate;
      if (Math.abs(prev - h) > 0.5) {
        heights.current.set(k, h);
        changed = true;
        if (o[i + 1] <= top) above += h - prev; // a row above the viewport changed: keep what the reader sees still
      } else if (!heights.current.has(k)) heights.current.set(k, h);
    });
    if (changed) {
      if (above && el && !pinned.current) el.scrollTop = top + above;
      setVersion((v) => v + 1);
    }
  }, [items, getKey, estimate, offs]);

  useLayoutEffect(() => {
    measure();
    if (pinned.current && sc.current) sc.current.scrollTop = sc.current.scrollHeight;
  });

  useLayoutEffect(() => {
    const el = sc.current;
    if (!el) return;
    const size = () => setViewport(el.clientHeight || window.innerHeight);
    size();
    if (typeof ResizeObserver === 'undefined') return;
    const obs = new ResizeObserver(() => { size(); });
    obs.observe(el);
    ro.current = new ResizeObserver(() => measure());
    nodes.current.forEach((n) => ro.current?.observe(n));
    return () => { obs.disconnect(); ro.current?.disconnect(); };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const onScroll = () => {
    const el = sc.current;
    if (!el) return;
    setScrollTop(el.scrollTop);
    const p = atBottom(el.scrollTop, el.clientHeight || viewport, el.scrollHeight);
    if (p !== pinned.current) { pinned.current = p; onPinnedChange?.(p); }
  };

  useImperativeHandle(ref, () => ({
    scrollToBottom() {
      pinned.current = true;
      onPinnedChange?.(true);
      if (sc.current) sc.current.scrollTop = sc.current.scrollHeight;
      setVersion((v) => v + 1);
    },
    scrollToIndex(i: number) {
      pinned.current = false;
      onPinnedChange?.(false);
      const y = Math.max(0, (offs[i] ?? 0) - 60);
      if (sc.current) sc.current.scrollTop = y;
      setScrollTop(y);
    },
  }));

  return (
    <div className={'chat-scroll ' + className} ref={sc} onScroll={onScroll} data-testid="chat-scroll">
      <div className="chat-col">
        {header}
        <div className="chat-pad" style={{ height: w.padTop }} aria-hidden="true" />
        {items.slice(w.start, w.end).map((it, n) => {
          const k = getKey(it);
          return (
            <div
              className="vrow"
              key={k}
              data-key={k}
              ref={(el) => {
                const prev = nodes.current.get(k);
                if (prev && prev !== el) ro.current?.unobserve(prev);
                if (el) { nodes.current.set(k, el); ro.current?.observe(el); } else nodes.current.delete(k);
              }}
            >
              {render(it, w.start + n)}
            </div>
          );
        })}
        <div className="chat-pad" style={{ height: w.padBottom }} aria-hidden="true" />
      </div>
    </div>
  );
}

export const VirtualList = forwardRef(VirtualListInner) as <T>(p: Props<T> & { ref?: React.Ref<VirtualHandle> }) => ReturnType<typeof VirtualListInner>;
