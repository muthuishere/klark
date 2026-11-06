# E-04: worst case, every pane viewed

- Date: 2026-11-06. Status: done.
- Setup: 100 machines, 1 pane viewed on **each** machine.

| output / pane | interval | wire KB/s (all 100) | wire / raw | load-test CPU | load-test RSS |
|---|---|---|---|---|---|
| 2 KB/s | 50 ms | 115 | 0.71 | 18 % | 29 MB |
| 20 KB/s | 50 ms | 226 | 0.16 | 32 % | 29 MB |
| 20 KB/s | 100 ms | 183 | 0.13 | 26 % | 29 MB |

**Conclusion:**
- Even with 100 panes each producing 20 KB/s of build-log output (~1.4 MB/s
  raw), the wire carries only ~230 KB/s, because repetitive terminal output
  compresses well in 50 ms batches.
- Bandwidth will not limit a 100-machine hub. The hub's CPU for decompressing
  and fanning out (E-06) is what is left to measure.
- The output is synthetic and repetitive. Real agent output (markdown, diffs)
  will compress less; expect 0.3–0.5×.

**Update (E-06, 2026-11-06, real hub):** with the new load-test output
(diffs, build logs and markdown with changing numbers and hashes, 4x bursts)
10 watched panes at ~26 KB/s raw each put **~9 KB/s per pane** on the wire
(gzip ratio 0.35) and cost the real hub **~1.4 % of a core per streaming
pane**. A real pane printing coloured log lines with timestamps compressed to
~0.55. Extra viewers on the same pane add no machine traffic (one stream,
fanned out) and ~0.1 % CPU each. By that measure, 100 panes × 26 KB/s ≈
0.9 MB/s in and ~1.4 cores, which is not yet measured.
