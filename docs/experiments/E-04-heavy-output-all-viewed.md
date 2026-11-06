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
