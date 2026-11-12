export interface WindowResult { start: number; end: number; padTop: number; padBottom: number; total: number }

/** Offsets of each row's top edge, plus the total as the last element. */
export function offsets(count: number, heights: ReadonlyMap<string, number> | ((i: number) => number), key: (i: number) => string, estimate: number): number[] {
  const out = new Array<number>(count + 1);
  out[0] = 0;
  for (let i = 0; i < count; i++) {
    const h = typeof heights === 'function' ? heights(i) : heights.get(key(i)) ?? estimate;
    out[i + 1] = out[i] + h;
  }
  return out;
}

/**
 * Which rows to draw: those that intersect [scrollTop - overscan, scrollTop + viewport + overscan].
 * Everything else is replaced by two spacers of the exact height they would have taken.
 */
export function computeWindow(offs: number[], scrollTop: number, viewport: number, overscan: number): WindowResult {
  const count = offs.length - 1;
  const total = offs[count] ?? 0;
  if (count <= 0) return { start: 0, end: 0, padTop: 0, padBottom: 0, total };
  const lo = Math.max(0, scrollTop - overscan);
  const hi = scrollTop + viewport + overscan;
  const find = (y: number) => {
    let a = 0, b = count; // first row whose bottom edge is > y
    while (a < b) {
      const m = (a + b) >> 1;
      if (offs[m + 1] > y) b = m; else a = m + 1;
    }
    return a;
  };
  const start = Math.min(find(lo), count - 1);
  let end = find(hi) + 1; // exclusive
  end = Math.min(count, Math.max(end, start + 1));
  return { start, end, padTop: offs[start], padBottom: total - offs[end], total };
}

/** True when the viewport is within `slack` px of the bottom (so new rows should keep it pinned there). */
export function atBottom(scrollTop: number, viewport: number, total: number, slack = 80): boolean {
  return total - (scrollTop + viewport) <= slack;
}
