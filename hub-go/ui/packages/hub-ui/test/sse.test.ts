import { backoff, connectSSE } from '../src/sse';

class FakeES {
  static all: FakeES[] = [];
  listeners = new Map<string, (e: MessageEvent) => void>();
  onopen: (() => void) | null = null;
  onerror: (() => void) | null = null;
  closed = false;
  constructor(public url: string, public init?: EventSourceInit) { FakeES.all.push(this); }
  addEventListener(n: string, f: (e: MessageEvent) => void) { this.listeners.set(n, f); }
  close() { this.closed = true; }
  emit(n: string, data: string) { this.listeners.get(n)?.({ data } as MessageEvent); }
}

describe('sse', () => {
  beforeEach(() => { FakeES.all = []; vi.useFakeTimers(); });
  afterEach(() => vi.useRealTimers());

  it('doubles the delay up to a cap', () => {
    expect([0, 1, 2, 3, 4, 5, 6].map((n) => backoff(n))).toEqual([1000, 2000, 4000, 8000, 16000, 30000, 30000]);
  });
  it('connects to the href as given and delivers events', () => {
    const got: string[] = [];
    connectSSE('/klark/api/machines/m/panes/p/events', { events: ['chat'], onEvent: (n, d) => got.push(n + ':' + d), EventSourceCtor: FakeES as never });
    expect(FakeES.all[0].url).toBe('/klark/api/machines/m/panes/p/events');
    FakeES.all[0].emit('chat', '{"rows":[]}');
    expect(got).toEqual(['chat:{"rows":[]}']);
  });
  it('reconnects with backoff, resets it on open, and asks the caller to resync', () => {
    const states: string[] = [];
    const resync = vi.fn();
    const stop = connectSSE('x', { events: [], onEvent: () => {}, onState: (s) => states.push(s), onResync: resync, EventSourceCtor: FakeES as never });
    FakeES.all[0].onopen!();
    expect(resync).not.toHaveBeenCalled(); // first open: nothing missed
    FakeES.all[0].onerror!();
    expect(FakeES.all[0].closed).toBe(true);
    vi.advanceTimersByTime(999); expect(FakeES.all).toHaveLength(1);
    vi.advanceTimersByTime(1); expect(FakeES.all).toHaveLength(2);
    FakeES.all[1].onerror!(); // still down: wait 2 s now
    vi.advanceTimersByTime(1999); expect(FakeES.all).toHaveLength(2);
    vi.advanceTimersByTime(1); expect(FakeES.all).toHaveLength(3);
    FakeES.all[2].onopen!();
    expect(resync).toHaveBeenCalledTimes(1);
    FakeES.all[2].onerror!(); // after a good open the delay starts over at 1 s
    vi.advanceTimersByTime(1000); expect(FakeES.all).toHaveLength(4);
    expect(states).toContain('retrying');
    stop();
    vi.advanceTimersByTime(60000);
    expect(FakeES.all).toHaveLength(4);
    expect(FakeES.all[3].closed).toBe(true);
  });
});
