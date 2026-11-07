# E-11: HTTP-only transport (ADR 0013) vs the WebSocket link

- Date: 2026-11-07. Status: **done**; the change passes (idle cost not above the
  WebSocket version's, latency within 2×).
- Setup: macOS laptop, everything on 127.0.0.1. Hub = `hub/` Debug build
  (`dotnet bin/Debug/net10.0/KlarkHub.dll`), Production environment, data in `/tmp/kv/h`.
  Machine = release `klark server` + `klark hub connect --url http://127.0.0.1:18080 --token …`.
  Simulated machines = release `klark-loadtest` (fleet token). Browser = chrome-agent
  (signed in as admin). Wire bytes counted by a byte-counting TCP proxy in front of the
  hub (HTTP headers included, TCP/IP headers not) and by `nettop`.
- WebSocket numbers are E-06's (real hub image in Docker, same load-test rates).
  Not the same box or build, so compare orders of magnitude and ratios, not the last %.

## Results

| measure | WebSocket (E-06) | HTTP: NDJSON down, POST segments up |
|---|---|---|
| idle machine, wire | ~5 B/s (WS ping every 20 s + pong) | **1.3 B/s payload** hub → machine (a 22-byte ping line every 15 s), **0 B/s** machine → hub, **0 POSTs** (nettop, 120 s) |
| idle machine, CPU of the link process | loop woke every 50 ms (read timeout) | **0.00 s over 120 s**: the loop blocks on the event stream while nothing is subscribed |
| idle machine, connections | 1 TCP | 1 TCP (the held `/agent/events`); the POST connection is opened on first push and kept alive |
| hub with 10 idle machines | 0.2–0.6 MiB each | +0.8 MiB RSS for 10; hub CPU +0.15 s per 60 s (≈0.25 % of a core, connects included) |
| hub with 100 idle machines | 101 machines: +~30 MiB, ~1 % of a core | +16.8 MiB RSS (~170 KB each); hub CPU +0.19 s per 60 s (≈0.3 % of a core) |
| active pane, 26 KB/s simulated output (bursts; 34 KB/s measured raw) | ~9 KB/s machine → hub (0.35 × raw) | **11.2 KB/s segment bodies (0.33 × raw), 15.1 KB/s with HTTP headers (0.44 ×)**; 1.5 KB/s hub → machine (204 responses); 18.5 segments/s |
| keystroke → echo in the browser, idle shell, a hot pane streaming on the same machine | p50 **54 ms**, p95 55 ms | p50 **65 ms**, p90 66, max 67 (n=60) — 1.2 × |
| same, nothing else streaming | — | p50 15 ms, p90 19 ms (n=60) |
| WebSockets opened by the browser | 1 per tab | **0** (`window.WebSocket` wrapped before any page script: 0 constructions; `/_blazor/negotiate` offers only ServerSentEvents and LongPolling) |

Per segment the HTTP framing costs ~210 B up (request line, Host, Authorization,
Content-Type/Encoding/Length) and ~80 B down (`204 No Content`). At 18.5 segments/s
that is the whole difference between 11.2 and 15.1 KB/s. It is paid only while a pane
is watched.

## What made the difference

1. **Frames as JSON text, not base64.** The first cut sent `frame.bytes` as base64
   inside the gzip'd segment: 0.61 × raw for the bodies, 0.70 × with headers (2 × the
   WebSocket). Sending UTF-8 output as `frame.text` (base64 only for non-UTF-8 bytes)
   lets gzip see the repetition again: 0.33 × raw. Same finding as E-02.
2. **Gzip only ≥ 1 KB.** A keystroke echo segment is ~80 B; it goes plain.
3. **204, and the machine header only on the session request**, trimmed ~100 B per
   segment.
4. **Idle = blocked.** With no subscribed pane and no pending answer, the link loop
   blocks on the event stream instead of ticking every 50 ms. The hub's streams own no
   timers: one hub-wide 5 s timer drops offline sessions and rings quiet streams for
   their ping; a stream is one awaiting writer on a one-slot channel.

## Browser transport: SSE in config, long polling in practice

The server offers `ServerSentEvents | LongPolling` and the page asks for the same.
The browser ends up on **long polling**: Blazor's circuit uses the binary
`blazorpack` hub protocol and SignalR's SSE transport is text-only, so the client
skips SSE. Zero WebSockets either way, and the latency above is measured on long
polling. Getting real SSE would need a text hub protocol for circuits, which Blazor
does not offer. `idle.js` (`Blazor.pauseCircuit`) works on long polling: a tab left
alone 60 s shows the pause pill and stops polling the machine.

## How to rerun

```bash
cargo build --release --bin klark --bin klark-loadtest
dotnet build hub
# hub: Hub:Machines with a token and a Fleet entry, then
KLARK_HUB__DATADIR=/tmp/kv/h ASPNETCORE_URLS=http://127.0.0.1:18080 dotnet hub/bin/Debug/net10.0/KlarkHub.dll
# a real machine
klark server & klark hub connect --url http://127.0.0.1:18080 --token <token>
# idle fleet / active pane (open /m/load:act-000/p/p0 in a browser for the active run)
klark-loadtest --hub http://127.0.0.1:18080 --token <fleet> --machines 100 --rate 0 --seconds 80
klark-loadtest --hub http://127.0.0.1:18081 --token <fleet> --machines 1 --rate 26000 --seconds 60 --csv a.csv
```

Latency: on a pane page, set the composer to a unique token, click Send, and time
until the token appears in `.xterm-rows` (30 samples per run, 50 ms apart; a second
iframe viewer keeps a `printf` loop pane streaming for the "hot" runs).
