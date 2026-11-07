---
title: Link protocol v5
description: How a machine and the hub talk, over plain HTTP, with the hub always pulling.
---

The link is the connection between a klark machine and the hub. The current
version is **5**. The hub accepts versions **4 and 5**; older machines are
refused with `unsupported protocol N; hub speaks 4-5`.

Code: `crates/klark-link/src/protocol.rs` (messages),
`crates/klark-link/src/transport.rs` (HTTP), `hub/Services/MachineRegistry.cs`.

## Two rules

1. **The machine always dials out.** Nothing listens on the machine. No tunnel.
2. **The hub always pulls.** A machine sends only `hello` unasked. Trees are
   pulled while someone has the list open. Pane output flows only while at
   least one person has that pane open.

## Transport: HTTP only

No WebSocket anywhere. Every request carries `Authorization: Bearer <machine token>`.

| request | what |
|---|---|
| `POST /agent/session` | body is `hello`. Returns `{session_id, heartbeat_seconds}`. A new session replaces the old one |
| `GET /agent/events?session=…&after=<seq>` | held open, NDJSON, one hub message per line with a `seq`. A `{"type":"ping"}` line after 15 s of quiet. After a drop the machine resumes with `after=<last seq>` |
| `POST /agent/push?session=…&seq=<n>` | one segment of machine messages, NDJSON, gzip only when ≥ 1 KB. `204`. A repeated `seq` is ignored, so retries are safe |
| `GET /agent` | token check, 200 or 401 |

- **Idle is one held request.** No subscribed pane means no timer, no ticks, no POSTs.
- **Output** is merged per 50 ms tick into one segment. Empty segments are never sent.
- **Liveness.** A machine is online while its events request is open, and
  offline 45 s after it ends.
- **Reconnect.** Backoff from 1 s doubling to 30 s, ±50 % jitter.
- **Pane output** is a `frame` line: `{"type":"frame","pane_id","text"}` for
  UTF-8, base64 `bytes` otherwise.

## Messages

| hub → machine | machine answers |
|---|---|
| `get_tree` | `tree`, or `tree_unchanged` |
| `subscribe` / `unsubscribe` | output `frame`s, `pane_size {pane_id, cols, rows}` while subscribed |
| `input`, `keys` | text and keys into a pane |
| `call` | an allow-listed read-only API call |
| `list_agents` (v2) | `[{session, name, agent, pane_id, status}]` |
| `deliver` (v2) | delivers a `klark msg` to a local agent |
| `fetch_asset` (v3) | downloads an image and prompts the agent with its path |
| `get_transcript {pane_id, after?}` (v3) | chat items since a cursor, `source: "jsonl"` or `"screen"` |
| `get_file {pane_id, path, max_bytes}` (v5) | a file for the viewer, after the machine's own checks |

| version | added |
|---|---|
| 1 | trees, subscribe, input, keys, calls |
| 2 | `list_agents`, `deliver` (messaging) |
| 3 | `pane_size`, `fetch_asset`, `get_transcript` |
| 4 | the same messages over plain HTTP (no WebSocket) |
| 5 | `get_file` (file viewer) |

Background: ADR 0002 (lazy pull), 0009 (messaging), 0011 (assets, chat, files),
0013 (HTTP-only). See [Design records](/reference/design-records/).
