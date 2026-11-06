# E-06: the real hub under load: CPU, RAM, sockets, wire, latency, sizing

- Date: 2026-11-06. Status: **done** (10 real machines measured; 100-machine
  curve with simulated machines; 500 is extrapolated).
- Owner requirement: "performance is a functional requirement".
- Raw data (CSV every 2 s, phase marks, latency samples, API results):
  [`data/e06/`](data/e06/). The hub samples for the 100-machine curve
  (`curve/`, `curve2/`) are in `run1/hub.csv` (same hub, same sampler); the
  `lt*.csv` files are the machine-side counters. Scripts: [`experiments/hubload/`](../../experiments/hubload/).

## Top of the report

1. **Lazy loading holds.** No viewers: 10 real Claude sessions + 100
   simulated machines cost the hub ~1 % of a core and **~5 B/s on the wire**
   (WebSocket keepalives only): no pane output, no tree pulls, no transcript
   polls. When the last viewer leaves, every observer on the machine stops
   within 15 s (graceful leave) and machine traffic drops to 0.
2. **One lazy-loading gap (a real cost, not a regression): a tab that dies
   without saying goodbye keeps polling for 3 minutes.** Killing the browser
   (`kill -9`, like a crashed tab, a phone dropping off Wi-Fi or a closed laptop) left
   10 transcript polls (10/s) and 4 machines-page pull loops (~33 `get_tree`/s
   over 100 machines) running **for ~185 s** after the sockets closed
   (`data/e06/afterkill.txt`: sockets 145 → 101 at once, polls stop between
   +178 s and +193 s). Cause: Blazor keeps a disconnected circuit for
   `DisconnectedCircuitRetentionPeriod` (default 3 min) and its components,
   with their timers, stay alive. See *Bugs and fixes* #2.
3. **The Docker image as committed does not work in a browser.** `hub/Dockerfile:6`
   publishes with `--no-restore`; the published `staticwebassets.endpoints.json`
   then has no `_framework/blazor.web.js`, so `/_framework/blazor.web.js` is
   **404** and no page becomes interactive (no terminal, no chat, no composer).
   Same build without `--no-restore`: present. Every number below was taken
   with that one flag removed (product code unchanged). See *Bugs and fixes* #1.

## Setup

- **Hub:** the real image (`docker build hub`, `--no-restore` dropped, see
  above), `KLARK_HUB__AUTH__MODE=MockLdap`, one fleet token and one machine
  token, port 5100. Host: an 18-core Apple Silicon Mac, Docker Desktop.
  Unconstrained, then `--memory 512m --cpus 1`.
- **Real machines:** 10 `klark server` sessions (`s01`..`s10`, temp
  `XDG_CONFIG_HOME`), each with 3 panes: a **real Claude Code on Haiku**
  (`claude --model haiku`, a small fizzbuzz-with-tests task in a throwaway
  dir, project settings allowing only python and edits in that dir), an
  output generator (s01's is the hot pane, ~90 KB/s of coloured log lines;
  the others ~1 KB/s) and an idle shell. All 10 sessions exposed through
  **one** real `klark expose` (hub machine `mac`, 30 panes, 10 agents).
- **Simulated machines:** `klark-loadtest` (extended for this run: protocol
  v3, binary frames, a 3-workspace / 6-tab / 12-pane tree with 8 agents whose
  status changes every ~20 s, incremental `get_transcript` with markdown /
  code / tool items, `list_agents`, `pane_size`, keystroke echo, 26 KB/s of
  diff/log/markdown output per subscribed pane with 4x bursts, per-kind byte
  counters to CSV, real reconnect backoff).
- **Viewers:** headless chrome-agent browsers with throwaway profiles, signed
  in as alice; each viewer is an iframe = its own Blazor circuit. 10 viewers:
  4 on the machines list, 3 on chat views (3 different Claude panes), 3 on
  terminals (2 on the hot pane, 1 on an idle shell for latency) + a deliberately
  slow viewer (second browser, main thread blocked 1.5 s of every 1.6 s) on the hot pane.
- **API:** 10 concurrent JWT clients looping `GET /api/agents` and
  `GET /api/messages?limit=50` (0.5 s think time).
- **Sampled every 2 s** inside the container: `docker stats` (memory, CPU),
  VmRSS and threads of PID 1, established TCP sockets, `/data` SQLite and
  asset bytes, eth0 rx/tx.

## Results: 10 real machines, 10 viewers, 10 API clients (unconstrained)

`data/e06/run1/`. rx = bytes into the hub, tx = bytes out (machines + browsers).

| phase | s | docker mem MiB (mean/max) | CPU % of 1 core (mean/max) | RSS MiB max | threads | TCP est | rx KB/s | tx KB/s |
|---|---|---|---|---|---|---|---|---|
| empty hub | 20 | 53 / 54 | 0.6 / 1.2 | 125 | 40 | 2 | 0.0 | 0.0 |
| 1. 10 machines linked, no viewers | 80 | 46 / 54 | 0.6 / 5.8 | 120 | 37 | 3 | **0.2** (≈5 B/s after connect) | 0.1 |
| 2. + 4 list viewers | 65 | 81 / 88 | 3.2 / 45 (page loads) | 131 | 42 | 15 | 1.3 | 2.9 |
| 3. + 3 chat viewers | 82 | 97 / 99 | 3.3 / 49 | 142 | 42 | 18 | 2.4 | 1.3 |
| 4. + 3 terminals (hot pane) + 10 API clients | 37 | 130 / 138 | 9.5 / 18 | 192 | 47 | 24 | 52 | 56 |
| 4b. + slow viewer | 38 | 152 / 155 | 6.8 / 11 | 192 | 46 | 26 | 47 | 59 |
| 5. everyone leaves | 92 | 147 / 160 | 0.7 / 12 | 183 | 51 | 9 | **0.0** | **0.0** |

Same phases on **`--memory 512m --cpus 1`, with 100 idle simulated machines
linked as well** (101 machines; `data/e06/run512/`):

| phase | docker mem MiB (mean/max) | CPU % (mean/max) | RSS MiB | TCP est | rx KB/s | tx KB/s |
|---|---|---|---|---|---|---|
| 1. 101 machines, no viewers | 78 / 82 | 1.0 / 4.9 | 151 | 103 | 0.5 | 0.5 |
| 2. + 4 list viewers (101 machines, 1,230 panes each) | 156 / 182 | 3.9 / 29 | 217 | 110 | 11.2 | 16.1 |
| 3. + 3 chat viewers | 185 / 198 | 4.7 / 37 | 229 | 113 | 13.7 | 21.3 |
| 4. + terminals + 10 API clients | 195 / 207 | 14.9 / 38 | 251 | 116 | 339 | 1,218 |
| 4b. + slow viewer | 209 / **224** | 8.7 / 14 | 252 | 119 | 251 | 874 |
| 5. everyone leaves | 194 / 205 | 0.6 / 6.5 | 234 | 109 | 0.5 | 0.3 |

No OOM kill, no restart. Peak 224 MiB of 512; one core never above 38 %.
(Phase 4's tx is the API: 101-machine `/api/agents` answers are 122 KB each.)

### Latency

| measure | unconstrained | 512m / 1 cpu |
|---|---|---|
| keystroke → echo back in the viewer, idle shell, while the hot pane streams | p50 **54 ms**, p95 55 ms (n=22) | p50 54 ms, p95 57 ms, max 76 (n=26) |
| same, with the slow viewer on the hot pane | p50 54, p95 56 ms (n=17) | p50 54, p95 66 ms (n=20) |
| chat: send in the composer → the person's turn visible in the chat view (real Claude, n=3) | 267 / 679 / 695 ms | 323 / 689 / 629 ms |
| `GET /api/agents`, 10 clients, 8.7–9.3 req/s | 1 machine: p50 60, p95 72 ms | **101 machines: p50 68, p95 213, max 1,207 ms** |
| `GET /api/messages`, same clients | p50 3.4, p95 6.1 ms | p50 2.6, p95 4.7 ms |

The ~54 ms is almost all the machine's 50 ms output merge tick (E-03): the hub
adds a few ms. Samples under 30 ms were dropped as artefacts (the harness times
"first frame after send", and an idle zsh sometimes redraws on its own).
The chat number is end to end: input → klark → Claude Code writes its JSONL →
the next 1 s `get_transcript` poll → the circuit pushes the row.

The slow viewer got 3.7 MB in the phase without a single reset and without
delaying anyone (latency unchanged): the per-viewer queue (S-13) isolates it.
Blocking 1.5 s is not enough to overflow the 50-frame queue; a longer stall
falls back to the repaint path.

## Per-unit costs (100-machine curve, simulated machines)

`data/e06/curve/`, `data/e06/curve2/`; costs are deltas between phases.

| unit | memory | CPU (% of one core) | sockets / fds | wire | measured / derived |
|---|---|---|---|---|---|
| empty hub process | 50 MiB (docker) / 120 MiB RSS | 0.5 % | — | — | measured |
| linked machine, nobody looking | 0.2–0.6 MiB | ~0 | 1 TCP + 3 SQLite fds (db, wal, shm) | ~5 B/s (WS ping every 20 s) | measured (0 → 100) |
| known machine on disk | — | — | — | ~120 KB SQLite each, kept forever (also for fleet names) | measured (401 names) |
| machines-list viewer (circuit) | 9 MiB at 30 panes; 13 MiB at 1,200 panes | page load spike 30–45 %, then ~0.5 % | 1 WebSocket | pulls each visible machine every ~3 s (shared by all list viewers): 0.33 `get_tree`/s per machine; ~40 B if unchanged, ~600 B gzip for a 12-pane tree that changed | measured |
| chat poll (first viewer of a pane) | ~5 MiB (feed + circuit) | ~0.2 % | 1 WebSocket | **1 `get_transcript`/s per pane**, ~130 B/s steady | measured: 10 panes → 10.0 polls/s |
| extra chat viewer on an already-polled pane | ~1.2 MiB | ~0 | 1 WebSocket | **0 more machine traffic** (40 viewers on 10 panes: still 10.0 polls/s) | measured |
| streaming pane (26 KB/s raw, bursts) | ~3 MiB | **~1.4 %** (gunzip, SQLite, base64, circuit) | — | machine → hub ~9 KB/s (gzip 0.35); disk ~3x raw until the 10 MB cap | measured (10 panes) |
| extra viewer on a streaming pane (fan-out) | ~0 | ~0.1 % | 1 WebSocket | hub → browser ~7 KB/s (Blazor WebSocket compression); **machine stream unchanged** (11 viewers, 1 stream) | measured |
| real hot pane (coloured log lines with timestamps, ~90 KB/s raw) | | phase 4 total 9.5 % incl. 10 API clients | | ~50 KB/s machine → hub (gzip ~0.55) | measured (s01) |
| API client polling `/api/agents` every ~1 s | ~0 | ~1 % per client at 101 machines | 1 HTTP keep-alive | **101 machine round trips per call** (not shared, see bottlenecks) | measured |

Memory does not go back to the pre-viewer level after everyone leaves
(46 → 147 MiB unconstrained, 78 → 194 MiB at 512m). Six open/close cycles
of 10 viewers ([`data/e06/cycles/`](data/e06/cycles/)) show whether this is a leak or GC high-water
mark — see *Leak check* below.

## Leak check

Six cycles on the 512m / 1 cpu hub, 10 real machines: open 10 viewers
(4 list, 3 chat, 3 terminal on the hot pane) for 30 s, close them for 45 s.

| cycle | 1 | 2 | 3 | 4 | 5 | 6 |
|---|---|---|---|---|---|---|
| docker mem while closed (MiB) | 142 | 160 | 144 | 167 | 148 | 172 |
| RSS while closed, max (MiB) | 181 | 189 | 167 | 180 | 168 | 189 |
| CPU while closed (%) | 0.6 | 0.7 | 0.5 | 0.7 | 0.8 | 0.3 |

RSS moves around a 170–190 MiB plateau and does not climb cycle after cycle:
**no leak**. The level after leaving is the GC's high-water mark, not
retained circuits. Within 4 s of a close the sockets drop from 17 to 7 and
machine traffic goes back to keepalives (66 B per 20 s), every cycle.

## Bottlenecks (file:line)

1. **`/api/agents` fans out to every machine on every call, unshared**
   (`hub/Services/Messaging.cs:56-73`). 10 clients at ~1 req/s over 101
   machines = 880 `list_agents` round trips/s; p95 213 ms, max 1.2 s, 122 KB
   per answer. On a real machine each `list_agents` spawns a `klark msg ls`
   process (`crates/klark-link/src/lib.rs:441`, `upstream.rs` `run_msg`).
   Fix: share it like trees: cache each machine's agent list for ~2 s and
   join in-flight requests (same pattern as `EnsureTreeAsync`,
   `MachineRegistry.cs:116`).
2. **Every pane viewer re-renders on every tree that arrives from any machine**
   (`hub/Components/Pages/PaneView.razor:142`, `:176`): `Registry.Changed` is
   global and `OnChanged` is not throttled (the machines page throttles to
   4/s, `Home.razor:152`). With 100 machines changing state, each open pane
   or chat re-renders ~5x/s for nothing. Fix: filter on the viewer's own
   machine (pass the machine id in the event) and throttle like Home.
3. **The machines page renders every pane of every machine in every list
   circuit** (`Home.razor`): 13 MiB per list viewer at 1,200 panes; at 500
   machines (~6,000 panes) expect ~50 MiB and a heavy diff per change
   (extrapolated). Fix before 300+ machines: collapse machines by default or
   virtualize the list.
4. **Agents' animated titles defeat `tree_unchanged`.** A working Claude
   updates its terminal title (spinner), so a real machine answers almost every
   pull with a full tree (`crates/klark-link/src/lib.rs:586`, `tree_rev` over
   the whole snapshot). 30 panes: ~1.3 KB/s per list-viewed machine instead of
   ~15 B/s. Fix: leave `terminal_title` spinner glyphs out of the rev (or
   send titles only on the pane page).
5. **SQLite writes ~3x the raw output rate** (output rows + WAL), until the
   10 MB per-machine cap; each machine's WAL then stays at ~4 MB. 150 MB of
   `/data` after this test (401 machine files, mostly fleet names that never
   come back). That is E-08 territory; a retention sweep for unknown fleet
   members would bound the file count.

## Bugs and fixes (not applied here, product code untouched)

1. **Image not interactive** — `hub/Dockerfile:6`
   `RUN dotnet publish -c Release -o /app --no-restore`: the framework's
   `_framework/blazor.web.js` is missing from the published static-asset
   manifest, so it 404s. Evidence: `curl /_framework/blazor.web.js` → 404 and
   `grep -c _framework/blazor.web.js /app/KlarkHub.staticwebassets.endpoints.json` → 0;
   the same Dockerfile without `--no-restore` → present, page interactive.
   Fix: drop `--no-restore` (or `dotnet restore` after `COPY . .`). The commit
   message of 19900f77 ("Published builds (Docker) were not affected") is wrong.
2. **Dead tabs keep polling for 3 minutes** — `hub/Program.cs:60`
   `AddInteractiveServerComponents()` with the default 3 min disconnected-circuit
   retention; `Home.razor:140` (pull loop) and `ChatView.razor:125`
   (`Feeds.Watch`) keep running while the circuit is disconnected. Evidence in
   *Top of the report* #2. Fix: a `CircuitHandler` that pauses pulls and
   leaves transcript feeds on `OnConnectionDownAsync` and resumes on
   `OnConnectionUpAsync` (keeps phone reconnects instant), or at least
   `o.DisconnectedCircuitRetentionPeriod = TimeSpan.FromSeconds(30)`.
3. **A machine that drops without a close handshake logs a full error stack
   twice** (Kestrel "unhandled exception" + the exception handler) —
   `MachineRegistry.cs:367` (`ReceiveAsync` throws `WebSocketException`)
   through `RunAsync` (`:64`) to `Program.cs:139`. Laptops sleeping or Wi-Fi
   dropping do this all the time; 100 machines → 1,200 log lines. Fix: catch
   `WebSocketException` / `OperationCanceledException` in `RunAsync` and log
   one Information line.
4. **`klark expose` logs a snapshot error on every pull when the default
   session is not running** (`crates/klark-link/src/lib.rs:622`): "snapshot
   default: connect …/klark.sock: No such file or directory" every ~3 s while
   anyone looks. Fix: skip the default session when its socket does not exist.
5. **The hub ignores SIGTERM while machines are linked** (E-10): machine
   sockets run on `ctx.RequestAborted` (`hub/Program.cs:139`), which shutdown
   does not cancel, so `docker restart` / `stop` always ends in a SIGKILL after
   the timeout. Fix: link with `IHostApplicationLifetime.ApplicationStopping`.

## Sizing (EXTRAPOLATED from the per-unit costs above, not measured)

Assumptions: a viewer mix like this test (40 % list, 30 % chat, 30 %
terminal); 1 in 10 machines has a pane being watched and streaming at a
build-log rate (~26 KB/s raw); `DOTNET_gcServer` left at the default. Memory
is the steady state plus ~50 % GC headroom; CPU is one modern x86/ARM core.

| machines | viewers | hub RAM (steady → set `--memory`) | vCPU | sockets / fds | machine → hub wire |
|---|---|---|---|---|---|
| 10 | 10 | ~150 MiB → **512m** (measured: 224 MiB peak with 101 machines) | 0.25 (measured peak 38 % of 1) | ~30 / ~80 | ~50 KB/s while 1 hot pane is watched |
| 50 | 25 | ~250 MiB → **512m** | 0.5 | ~100 / ~300 | ~50–100 KB/s |
| 100 | 50 | ~400 MiB → **1g** | 1 | ~170 / ~500 | ~100–150 KB/s |
| 500 | 100 | ~1.2 GiB → **2g** (list viewers dominate unless bottleneck 3 is fixed) | 2 (streaming 50 panes ≈ 0.7 core; list pulls of 500 machines ≈ 165 `get_tree`/s) | ~650 / ~2,300 | ~0.5–1 MB/s |

Recommendations:
- **Docker:** `--memory 512m --cpus 1` is enough up to ~50 machines / 25
  viewers (measured at 101 machines / 11 viewers / 10 API clients: 224 MiB,
  ≤ 38 % CPU). 1 GiB / 1 vCPU for 100 machines; 2 GiB / 2 vCPU for 500.
- **ulimit:** each machine holds 1 socket + 3 SQLite fds, each viewer 1 socket.
  Docker's default `nofile` (1,048,576 on Docker Desktop; often 1,024 soft on
  plain Linux hosts) — set `--ulimit nofile=65536:65536` explicitly anywhere
  above ~200 machines.
- **GC:** on a many-core host the hub uses Server GC (one heap per core) and
  keeps its high-water mark (147 MiB after viewers left). With `--cpus` set it
  stays lower (78 → 194 MiB on 1 cpu). For small boxes set
  `DOTNET_GCHeapHardLimit` or `--memory`; .NET honours the cgroup limit.
- **Before 300+ machines:** fix bottlenecks 1–3 (API fan-out, global
  `Changed`, full-list rendering), and bug 2 (dead-tab polling), whose cost
  scales with machines × dead tabs.
- **Bandwidth is never the limit:** even 100 machines × 1 watched pane each
  is ~1 MB/s into the hub (E-04).

## Limits of this experiment

- One Mac runs everything: the hub (Docker Desktop VM), 10 klark sessions,
  10 Claude Code agents, the load test and two headless browsers. CPU numbers
  are Docker's per-container figure on that VM.
- **44 iframes in one headless tab** was too many for chrome-agent (it moved
  to a new tab; the old one kept its 44 circuits): stay at ~20 circuits per
  browser tab. The 100-concurrent-viewer target was therefore not run as 100
  live circuits; the per-circuit costs above (1.2–13 MiB, ~0 idle CPU) make
  100 viewers ≈ +150–500 MiB, which is **extrapolated**.
- Simulated machines answer instantly; a real machine answers `get_tree` by
  snapshotting every session over its socket, and `list_agents` by spawning a process.
- Rendering cost in the browser is out of scope (E-09); offscreen iframes do
  not paint, the harness counts bytes handed to `klarkTerm.write*`.
- Anthropic usage of the 10 Haiku agents for the whole test (from their
  JSONL logs, `experiments/hubload/cost.py`): 34 API responses, 20.6k output
  tokens, 273k cache-write and 1.58M cache-read tokens; **≈ $0.60** at Haiku
  list prices (an estimate, not a bill).
