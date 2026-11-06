# ADR 0001: klark, the built-in link, and the klark hub

- Status: accepted (implementation in progress)
- Date: 2026-11-06
- Branch: `ccr-8d742f2f-3wz6mn`

See also [docs/STATUS.md](../STATUS.md) for the live checklist and ADRs 0002–0007 for details.

This is the place we track what klark is, what is decided, what is
built, and what is still open. Update the checklists at the bottom as work lands.

## Context

klark is a fork of herdr 0.9.5, a terminal workspace for AI coding agents.
herdr-expose was a separate Go plugin that put one machine's panes on a web UI
over the LAN or a tunnel. What we want instead:

1. **klark is its own product.** No self-update, no herdr.dev website calls, no
   hosted announcements.
2. **Expose is built into klark**, in Rust, and is not a plugin.
3. **A central hub** (C# Blazor Server) that **many klark machines (target 100+)**
   connect to, and that **many people** sign in to so they can see and answer
   every machine's panes from one UI, including from a phone.
4. **Lazy data.** The hub asks for data and klark sends it. Nothing streams unless
   someone is looking.
5. **Small and fast on the wire.** Compressed, batched, cheap enough to run 100
   machines on one small server.
6. **Self-contained storage.** The hub uses SQLite, with one database file per
   klark machine capped at **10 MB**, plus a shared hub database. Deployable
   with one Docker image.

## Decisions

### D1. Rebrand and disable phoning home
- Package and binary are `klark`.
- Self-update (`klark update`), background version checks, product
  announcements and the remote agent-detection catalog are disabled.
- The catalog can be re-enabled with `KLARK_AGENT_DETECTION_MANIFEST_CATALOG_URL`.
- Superseded in part: env names were renamed to `KLARK_*`, and config dirs to `klark`,
  with a one-time move of the old herdr config dir (STATUS D-01). The original text said
  config dirs, socket paths and `HERDR_*` env names are kept for compatibility
  (see open item O1).

### D2. Expose ported to Rust as `crates/klark-link`
- The crate is separate from the main crate, so it builds and tests without the
  Zig/ghostty toolchain.
- It talks to the local klark server over the existing socket API (NDJSON), and
  streams pane output with `klark terminal session observe <pane>`. This is the
  same contract herdr-expose used, ported from Go.
- Exposed as the `klark link` subcommand.

### D3. Machines dial out to the hub (no inbound ports)
- One long-lived WebSocket per machine: `wss://<hub>/agent`, with
  `Authorization: Bearer <machine token>`.
- It works behind NAT and firewalls and needs no tunnel. Reconnects use
  exponential backoff (1s up to 30s).
- WebSocket was chosen over HTTP polling because it is the fastest option:
  - one TCP+TLS connection per machine;
  - the server can push to the machine (subscribe/unsubscribe) with no polling latency;
  - no per-request headers.

### D4. Lazy protocol
- On connect, the machine sends `hello`, then `tree` (the pane list). It sends
  `tree` again only when it changes.
- The hub sends `subscribe pane` when the first person opens a pane and
  `unsubscribe` when the last person leaves. Terminal bytes flow only in between.
- Reads (`pane.read`, `agent.read`, …) are request/response through `call`, and
  only for allow-listed read-only methods.
- Writes are limited to `input` (send text) and `keys`. The hub cannot close
  panes or run arbitrary commands.

### D5. Wire efficiency
- Every message is JSON, gzip-compressed and sent as a **binary** WebSocket frame.
  Plain text JSON frames are still accepted, for debugging and compatibility.
- The machine **coalesces terminal output**: all bytes for a pane within one tick
  (~25 ms) go out as one `frame` message, instead of one message per write.
- The pane tree is diffed on the machine, so an unchanged tree is never resent.

### D6. Hub = ASP.NET Core Blazor Server (.NET 9)
- The machine registry is in memory: one `MachineConnection` per machine and a
  ref-counted set of viewers per pane.
- Browsers use Blazor Server's own SignalR circuit. Pane bytes go to xterm.js
  through JS interop.
- Auth:
  - People: cookie sign-in, with passwords stored as ASP.NET `PasswordHasher` hashes.
  - Machines: tokens stored as SHA-256 hashes and compared in constant time.
- Every signed-in user sees every machine. This is a team hub; see O3.

### D7. Storage = SQLite
- `data/hub.db` (shared): machines that have ever connected (id, host, version,
  first/last seen, connected flag).
- `data/machines/<machine>.db` (one per klark machine):
  - the latest tree;
  - a rolling log of recent pane output;
  - the machine's events (connect, disconnect).
- **Each per-machine file is capped at 10 MB.** The oldest pane-output rows are
  pruned when the file passes the cap, with incremental vacuum.
- Writes are batched on a background channel, so slow disk never blocks the
  WebSocket loop. WAL mode is on.
- Why one file per machine: deleting a machine is deleting a file, a noisy
  machine cannot bloat the others, and there is no write-lock contention between
  machines.

### D8. Deployment
- One Docker image (multi-stage build on the .NET SDK, with the aspnet runtime).
- The `/data` volume holds the SQLite files. The hub listens on 8080; put TLS in
  front of it with a reverse proxy.

### D9. Plain HTTP and WebSocket, not HLS
- The hub runs on plain HTTP (port 8080, no HSTS). Machines connect with
  `ws://host:8080/agent`. `wss://` still works if a TLS proxy is ever put in
  front.
- **We chose not to use HLS.** HLS is built for video: it cuts the stream into
  segment files that players poll, which adds 2–10 s of delay. It is one-way,
  so there is no way to send input. Terminal output is small text deltas, and
  re-encoding it as video would cost far more bandwidth and CPU.
- WebSocket gives instant two-way delivery over one connection. Messages are
  gzip-compressed JSON (D5), and a pane's output is merged into one message per
  25 ms. Browsers get Blazor Server's SignalR (also WebSocket), and pages use
  HTTP response compression.
- If a proxy ever blocks WebSockets, SignalR falls back to Server-Sent Events or
  long polling on its own for browsers. A machine link fallback would be HTTP
  long polling (tracked as T9).

### D10. Full rename and cleanup (answers O1 and O2)
- Every `herdr`/`Herdr`/`HERDR` in code, tests, scripts and skills is now
  `klark`/`Klark`/`KLARK`, including env vars (`KLARK_SOCKET_PATH`, …), socket
  and config dirs, plugin manifest keys and agent hook file names.
- On first run, klark moves an existing `~/.config/herdr` directory to
  `~/.config/klark`.
- Deleted:
  - the website and versioned docs;
  - the install scripts and preview/agent-detection manifests;
  - nix and the flake;
  - the release, preview, distribution, website and contributor-gate workflows,
    with their scripts.
- Kept: `ci.yml`, and `distribution/install.ps1` and `latest.json`, because the
  code still compiles them in.
- The API schema moved to `api/klark-api.schema.json`.
- Agent integrations installed under the old name (hook files named for herdr
  inside agents' configs) have to be reinstalled with
  `klark integration install <agent>`.

### D11. Always pull, never push (supersedes the tree part of D4)
- A machine sends nothing on its own except `hello`. In particular it never
  pushes its pane tree.
- The hub pulls the tree with `get_tree {known_rev}` only while a page needs it:
  - the machines page asks every 3 s for trees older than 2 s;
  - a pane page asks once on open.
- The machine replies `tree {rev, snapshot}`, or a tiny `tree_unchanged {rev}`
  when nothing changed. Concurrent viewers share one in-flight pull per machine.
- Pane output flows only between `subscribe` and `unsubscribe`, which the hub
  sends while a pane page is open. Output is the one stream, and it exists only
  because a viewer asked for it.
- With nobody signed in, a linked machine costs one idle WebSocket (with keepalive).

### D12. Sign-in: mock LDAP, real LDAP, or local
- `Hub:Auth:Mode` picks one of:
  - **MockLdap** (default): an in-memory directory. It seeds admin/admin
    (groups klark-admins and klark-users), alice/alice and bob/bob (klark-users),
    or uses `Hub:Auth:MockUsers` if set.
  - **Ldap**: a simple bind as `UserDnTemplate`, then a group lookup by
    `member`, `uniqueMember` or `memberUid`
    (`System.DirectoryServices.Protocols`).
  - **Local**: `Hub:Users` with PasswordHasher hashes.
- `Hub:Auth:AllowedGroups` restricts who may sign in. If it is empty, anyone
  the directory accepts can sign in.
- Groups become role claims, ready for per-group machine visibility (O3).
- `docker compose --profile ldap` starts a throwaway OpenLDAP with alice and
  bob, for trying Mode=Ldap.

### D13. Binary terminal frames, fleet tokens, load test
- Terminal output goes as a binary frame: `0x01|0x02, id length, pane id,
  output`, where `0x01` means gzip and `0x02` means raw (used for small
  batches). There is no base64 and no JSON. Other messages stay gzip JSON
  (first byte `0x1f`).
- A machine entry with `"Fleet": true` admits many machines with one token,
  named `Name:<X-Klark-Machine header>`.
- `klark-loadtest` (built from `crates/klark-link`) simulates N machines that
  speak the real protocol.
- Measured on 2026-11-06, against a Python stand-in hub (not the real Blazor
  hub), 100 machines × 8 panes, 25 panes viewed, 2 KB/s of output each, 12 s:
  - all 100 connected, 0 failures;
  - 602 tree pulls: 100 full trees, then 502 `tree_unchanged`;
  - output used ~67 KB/s on the wire in total, about the raw size. Before the
    binary frames it was ~106 KB/s, because base64 inside gzip barely
    compresses.

## Capacity target (100 machines)
- 100 idle machines: 100 sockets, plus a tree diff every 2 s each, which is only
  sent when something changed. That is negligible.
- Streaming is bounded by viewers, not machines: only panes someone has open
  stream. Coalescing plus gzip keeps a busy agent pane at a few KB/s.
- Storage is at most 100 × 10 MB = 1 GB on disk.
- Still to do: a load test with 100 simulated machines (T7).

## Status checklist

Done:
- [x] D1: rebrand the package and binary; disable self-update, announcements and the remote catalog.
- [x] D2/D3/D4: `crates/klark-link`, `klark link`, and the lazy protocol, with unit and end-to-end tests (fake hub plus fake klark socket).
- [x] D5: gzip binary frames and per-tick frame coalescing.
- [x] D6: Blazor Server hub, with sign-in, machine tokens, the machine/pane list (blocked panes first), and a live xterm.js pane view with reply, Esc and Ctrl-C.
- [x] D7: SQLite `hub.db` plus per-machine files with the 10 MB cap.
- [x] D8: Dockerfile with a `/data` volume.

To do:
- [ ] T1: build and run the full klark binary and the hub end to end. This needs Zig 0.16 and .NET 9; the cloud session's network blocked both.
- [ ] T2: switch the tree from polling every 2 s to klark's event stream (`events.subscribe`).
- [ ] T3: Windows support for `klark link` (named-pipe socket).
- [ ] T4: transcript view (`agent.read`) in the hub UI, ported from expose's TranscriptView.
- [ ] T5: show stored history from the per-machine DB when a machine is offline.
- [ ] T6: admin page to add and revoke machine tokens and users, stored in hub.db instead of appsettings.
- [x] T7: `klark-loadtest` (100 machines verified against a stand-in hub; still to be run against the real hub).
- [x] D11: lazy pull for trees.
- [x] D12: mock LDAP, real LDAP and local sign-in, with an allowed-groups list.
- [x] D13: binary frames and fleet tokens.
- [ ] T11: CI is dropped by decision. The 10 old workflow files must be deleted in the GitHub UI, because this session cannot change `.github/workflows`.
- [ ] T12: run `klark-loadtest` against the real hub and record CPU/RAM for 100 machines.
- [x] D9: plain HTTP hub, `ws://` link.
- [x] D10: full rename and removal of the website and packaging.
- [ ] T9: HTTP long-poll fallback for `klark link`, in case a proxy blocks WebSockets.
- [ ] T10: remove the now-dead updater code in `src/update.rs`. It is disabled but still compiled.
- [ ] T8: decide what happens to the herdr-expose repo (archive, or keep its local LAN UI as a fallback).

## Open questions
- ~~O1~~: done (D10).
- ~~O2~~: done (D10).
- O3: per-user or per-team machine visibility, or does everyone see everything?
- O4: keep expose's local LAN web UI inside klark as an offline fallback?
