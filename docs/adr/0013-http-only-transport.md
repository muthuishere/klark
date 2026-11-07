# ADR 0013: HTTP-only transport — NDJSON stream down, POST segments up; browser SSE

- Status: accepted (owner decision, 2026-11-07), implemented
  (`crates/klark-link/src/transport.rs`, `hub/Program.cs`, `hub/Services/MachineRegistry.cs`,
  `hub/Components/App.razor`). Measured in [E-11](../experiments/E-11-http-transport.md).
- Supersedes: the WebSocket transport of ADR 0002 (the message set stays), ADR 0010
  (`--hub ws://…/agent`) and ADR 0012 (`ws(s)://host/agent` derived from the URL).

## Context
WebSockets need every proxy, ingress, NodePort front and corporate firewall on the
way to allow the upgrade and keep an idle connection open. Plain HTTP passes all of
them. Performance is a functional requirement: lazy loading must stay, an idle
machine must cost no more than it did over a WebSocket, and keystroke → echo must
stay within ~2× of the WebSocket's 54 ms (E-06).

## Decision
No WebSocket anywhere. Link protocol **v4** = the v3 messages over HTTP.

**Machine ↔ hub** (all requests carry `Authorization: Bearer <machine token>`):

| request | what |
|---|---|
| `POST /agent/session` | body = `hello` (JSON); `X-Klark-Machine` for fleet tokens. → `{session_id, heartbeat_seconds}`. 401 unknown token, 400 wrong protocol. A new session for a linked machine replaces the old one (= reconnect). |
| `GET /agent/events?session=…&after=<seq>` | held open, `Content-Type: application/x-ndjson`, chunked, `Cache-Control: no-cache`, `X-Accel-Buffering: no`, flushed after every line. One hub message per line with a `seq`; `{"type":"ping"}` (no seq) after 15 s without a line. After a drop the machine resumes with `after=<last seq>`; the hub keeps unread messages for that. 404 = session gone → new session. |
| `POST /agent/push?session=…&seq=<n>` | one segment: NDJSON, one machine message per line; `Content-Encoding: gzip` only when the segment is ≥ 1 KB. Never a streamed request body (proxies buffer uploads). 204 when taken; a `seq` already taken is ignored (`200 {"duplicate":true}`), so a retried push is harmless; a gap is logged. |
| `GET /agent` | token probe for `klark hub connect` (200 / 401). |

- **Lazy rule unchanged.** Nothing is pushed for a pane unless it is subscribed; output
  is merged per 50 ms tick into one segment; empty segments are never sent.
- **Idle = one held request.** With no subscribed pane and no answer pending, the
  machine's loop blocks on the event stream (no tick, no timer) and makes no POSTs.
  The hub writes a ping line every 15 s.
- **One connection per direction.** The events request has its own connection; all
  POSTs reuse one kept-alive connection (retried once on a fresh one).
- **Liveness.** A machine is online while its events request is open; it goes
  offline 45 s after the stream ends. One hub-wide timer (every 5 s) drops offline
  sessions and rings quiet streams for their ping; each stream is a single awaiting
  writer on a one-slot channel (no per-machine thread or timer).
- **Reconnect.** Any error ends the session: jittered backoff (1 s doubling to 30 s,
  ±50 %, as before), then a new session. Revoking a machine drops its session at once;
  the token then gets 401.
- **TLS.** `https://` works as before (rustls, `KLARK_HUB_CA_FILE`). `--hub` / `--url`
  take `http(s)://host:port`; old `ws(s)://host/agent` URLs map to `http(s)://host`.
- **Wire format.** Pane output is a `frame` line: `{"type":"frame","pane_id","text"}`
  when the bytes are UTF-8, `bytes` (base64) otherwise. Inside a gzip'd segment that is
  0.33 × raw at 26 KB/s (E-11; base64 was 0.61 ×), replacing the binary WebSocket
  frames of ADR 0001 D13.

**Browser ↔ hub.** Blazor Server's SignalR is restricted to `ServerSentEvents |
LongPolling`: the server strips WebSockets from `/_blazor/negotiate` (the circuit
endpoints' `HttpConnectionDispatcherOptions` metadata) and the page starts Blazor
with `autostart="false"` and `withUrl("_blazor", { transport: SSE | LongPolling })`.
Idle pause (`idle.js`, `Blazor.pauseCircuit`) works unchanged. Without sticky
sessions this needs one replica, which the deployment already is.

## Consequences
- Any HTTP path works: k8s NodePort, nginx/ingress with default settings (60 s read
  timeout > 15 s ping), corporate proxies.
- Per pushed segment the machine pays HTTP headers (~200 B up, ~100 B down) that a
  WebSocket frame did not; E-11 has the numbers.
- **Blazor's circuit protocol is binary (blazorpack), and SignalR's SSE transport is
  text-only**, so browsers fall through to long polling in practice. Measured latency
  is still within target (E-11). Getting real SSE would need a text hub protocol for
  the circuit, which Blazor does not offer.
- Old (v1–v3) machines cannot link to this hub; the hub accepts protocol 4 only.
