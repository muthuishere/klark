---
title: Performance
description: Measured numbers for the hub, the link and the web app.
---

Performance is treated as a requirement. These numbers come from the
experiments in `docs/experiments/` (E-06, E-11, E-12). Where a number is
extrapolated, it says so.

## Idle costs almost nothing

| what | measured |
|---|---|
| idle machine, network | 1.3 B/s hub → machine (a 22-byte ping every 15 s), 0 B/s machine → hub, 0 POSTs (E-11) |
| idle machine, CPU of the link | 0.00 s over 120 s: the loop waits on the stream (E-11) |
| hub with 100 idle machines | +16.8 MiB RSS (~170 KB each), ≈0.3 % of a core (E-11) |
| 10 real machines with Claude Code, no viewers | ~1 % of a core (E-06) |
| everyone leaves | machine traffic drops to 0 (E-06) |

## While watching

| what | measured |
|---|---|
| a busy pane, 26 KB/s of output | 11.2 KB/s on the wire (0.33× raw), 15.1 KB/s with HTTP headers (E-11) |
| keystroke → echo in the browser, idle shell | p50 15 ms, p90 19 ms (E-11) |
| same, with a busy pane streaming on that machine | p50 65 ms, max 67 ms (E-11) |
| hub, 10 machines + 10 viewers + 10 API clients | 130–155 MiB, under 10 % of a core on average (E-06) |
| 101 machines, 11 viewers, `--memory 512m --cpus 1` | 224 MiB peak, ≤ 38 % CPU (E-06) |

## The web app

| what | measured (E-12) |
|---|---|
| JavaScript on first load (besides Blazor) | 26.3 KB raw / 9.3 KB gzip, down from 443 KB |
| Claude chat, fresh load on localhost | DOMContentLoaded 79 ms, first paint 124 ms |
| loaded only when needed | xterm (65.7 KB gz) on first terminal open; highlight.js core (8 KB gz) + one grammar per language; the file viewer (7.2 KB gz) |

## Sizing

See the table on [Run the hub](/hub/run/#sizing). The 100- and 500-machine rows
are extrapolated from per-unit costs, not measured.

## Full reports

- [E-06: the real hub under load](https://github.com/muthuishere/klark/blob/master/docs/experiments/E-06-real-hub-resources.md)
- [E-11: HTTP-only transport](https://github.com/muthuishere/klark/blob/master/docs/experiments/E-11-http-transport.md)
- [E-12: UI performance](https://github.com/muthuishere/klark/blob/master/docs/experiments/E-12-ui-performance.md)
