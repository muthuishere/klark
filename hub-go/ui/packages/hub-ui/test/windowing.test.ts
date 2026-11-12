import { atBottom, computeWindow, offsets } from '../src/windowing';

const uniform = (n: number, h: number) => offsets(n, () => h, (i) => String(i), h);

describe('windowing', () => {
  it('offsets are prefix sums, measured heights win over the estimate', () => {
    const m = new Map([['1', 50]]);
    expect(offsets(3, m, (i) => String(i), 10)).toEqual([0, 10, 60, 70]);
  });
  it('draws only rows near the viewport', () => {
    const o = uniform(1000, 100); // 100 000 px
    const w = computeWindow(o, 50_000, 800, 200);
    expect(w.start).toBe(498);
    expect(w.end).toBe(511);
    expect(w.end - w.start).toBeLessThan(20);
  });
  it('spacers add back exactly the height of the rows left out', () => {
    const o = uniform(1000, 100);
    const w = computeWindow(o, 12_345, 700, 300);
    expect(w.padTop + (w.end - w.start) * 100 + w.padBottom).toBe(w.total);
    expect(w.padTop).toBe(w.start * 100);
  });
  it('clamps at both ends and handles empty lists', () => {
    const o = uniform(50, 40);
    expect(computeWindow(o, 0, 400, 0).start).toBe(0);
    const end = computeWindow(o, 5000, 400, 100);
    expect(end.end).toBe(50);
    expect(end.padBottom).toBe(0);
    expect(computeWindow([0], 0, 400, 100)).toEqual({ start: 0, end: 0, padTop: 0, padBottom: 0, total: 0 });
  });
  it('always draws at least one row', () => {
    const w = computeWindow(uniform(10, 100), 100_000, 600, 0);
    expect(w.end - w.start).toBeGreaterThanOrEqual(1);
  });
  it('knows when the reader is at the bottom', () => {
    expect(atBottom(900, 100, 1000)).toBe(true);
    expect(atBottom(500, 100, 1000)).toBe(false);
  });
});
