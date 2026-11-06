# E-03: how often to flush pane output (merge interval)

- Date: 2026-11-06. Status: done. **50 ms adopted** (`READ_TICK` in `crates/klark-link/src/lib.rs`).
- Setup: 100 machines, 25 panes viewed, 2 KB/s each, 10 s.

| interval | frames | wire / raw | load-test CPU (100 sims) |
|---|---|---|---|
| 25 ms | 5288 | 1.17 | 9 % |
| **50 ms** | **2680** | **0.84** | 11 % |
| 100 ms | 1483 | 0.65 | 9 % |

**Conclusion:**
- Each doubling of the interval halves the message count.
- At 50 ms, gzip starts paying off (0.84×).
- 100 ms is cheaper still, but 100 ms of delay is noticeable when typing.
- 50 ms is the balance point. It only delays how output is grouped: a reply
  from the hub is handled as soon as it arrives.

**Follow-up:** E-05, adaptive interval.
